// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened zip extraction: iterate the central directory only, validate
//! every entry (reject-the-archive on any violation), then write. Symlink
//! entries (S_IFLNK in external attrs) are skipped entirely — never
//! honored, never materialized as a plain file. Encrypted entries are
//! rejected. Never uses zip-rs's own `extract`/`extract_unwrapped_root_dir`
//! helpers (historic `../`/dup handling) — this is a manual walk.

use std::collections::HashSet;
use std::fs;
use std::io::{Seek, SeekFrom};
use std::path::Path;

use super::common::{clamp_mode, copy_capped, reject, set_dir_mode, set_file_mode};
use super::validate::{
    MAX_ENTRIES, MAX_TOTAL_BYTES, RawEntry, collision_key, strip_single_root, stripped_rel,
    validate_entry_name,
};
use crate::error::PkgError;

/// `S_IFLNK` (unix "is a symlink" file-type bits), as stored in a zip
/// entry's external file attributes (upper 16 bits = unix mode).
const S_IFLNK: u32 = 0o120000;

/// A validated, post-strip regular-file entry ready for pass-2 writing.
/// `rel` is already the FINAL destination-relative path — resolved exactly
/// once (via [`stripped_rel`]) from the entry's validated name, during the
/// same central-directory walk that decided the strip. Nothing re-derives
/// it later: unlike the tar.gz walk (which must re-open the archive as a
/// fresh sequential stream for pass 2 and so recomputes each rel from the
/// raw name it re-reads), zip's central directory gives random access by
/// index, so the one rel computed here is carried straight through to the
/// write step with no second computation to possibly disagree with the
/// first.
struct PlannedFile {
    index: usize,
    rel: String,
    mode: u32,
}

/// Extract `archive` (a verified, open handle) into the already-created
/// empty directory `dest`. Validates every entry from the central
/// directory's metadata (no entry's compressed payload is read during
/// validation) and rejects the WHOLE archive on any violation before any
/// I/O; only then does it write.
pub(crate) fn extract_zip(archive: &mut fs::File, dest: &Path) -> Result<(), PkgError> {
    archive
        .seek(SeekFrom::Start(0))
        .map_err(|e| PkgError::io("seek", Path::new("<archive>"), e))?;
    let mut zip =
        zip::ZipArchive::new(&mut *archive).map_err(|e| reject(format!("zip open: {e}")))?;

    // `len()` is exact and free here (the whole central directory was
    // already parsed by `ZipArchive::new`) — unlike tar's streaming format,
    // there is no need for a running per-entry counter to fail fast.
    if zip.len() > MAX_ENTRIES {
        return Err(reject("too many entries"));
    }

    // Pass 1 (metadata-only): validate every entry, skip symlinks entirely,
    // and stage the rest for the strip decision. `by_index_raw` (NOT
    // `by_index`) is used deliberately: `by_index`'s password gate fails
    // closed on an encrypted entry BEFORE handing back a `ZipFile` at all,
    // which would make an `entry.encrypted()` check reached through
    // `by_index` unreachable dead code — the archive would still get
    // rejected (via `by_index`'s own generic error), but nothing in this
    // file's own logic would ever exercise that rejection, and a future
    // crate change that defers the password check to decompression time
    // would silently stop rejecting at all. `by_index_raw` reads the SAME
    // cached central-directory metadata (`name_raw`, `encrypted`,
    // `unix_mode`, `size`) WITHOUT that password gate and WITHOUT
    // decompressing or decrypting anything (confirmed empirically: it
    // reports `encrypted() == true` for an encrypted entry instead of
    // erroring), so the `encrypted()` check below is a real, reachable,
    // directly-tested rejection.
    struct Staged {
        rel: String,
        is_dir: bool,
        file: Option<(usize, u32)>,
    }
    let mut staged: Vec<Staged> = Vec::new();
    let mut declared_total: u64 = 0;

    for i in 0..zip.len() {
        let entry = zip
            .by_index_raw(i)
            .map_err(|e| reject(format!("zip entry {i}: {e}")))?;
        // Encrypted entries are rejected explicitly, before any name/mode
        // inspection: see the pass-1 comment above for why `by_index_raw`
        // makes this a real, reachable check. Covered by
        // `rejects_encrypted_entries`, which asserts this exact
        // `UnsafeArchive("encrypted zip entry")` rejection, not just
        // "some error occurred".
        if entry.encrypted() {
            return Err(reject("encrypted zip entry"));
        }
        // Name safety (S11): validate the RAW bytes, never zip's lossy
        // `name()`/`mangled_name()`/`is_dir()` (all derived, at parse time,
        // from a `String::from_utf8_lossy` decode of the raw name — which
        // silently replaces invalid UTF-8 with U+FFFD and could disguise
        // the true byte content of a hostile name).
        let raw = entry.name_raw();
        let name = std::str::from_utf8(raw).map_err(|_| reject("zip entry name not utf-8"))?;
        // `is_dir` is derived from OUR validated `name`, not `entry.is_dir()`.
        let is_dir = name.ends_with('/');
        let mode = entry
            .unix_mode()
            .unwrap_or(if is_dir { 0o755 } else { 0o644 });
        // Symlink entries (S14): skip entirely — never honored, never
        // materialized as a plain file, whether or not the target escapes.
        if !is_dir && (mode & S_IFLNK) == S_IFLNK {
            continue;
        }
        let rel = validate_entry_name(&name.replace('\\', "/"))?;
        if is_dir {
            staged.push(Staged {
                rel,
                is_dir: true,
                file: None,
            });
        } else {
            declared_total = declared_total.saturating_add(entry.size());
            staged.push(Staged {
                rel: rel.clone(),
                is_dir: false,
                file: Some((i, mode)),
            });
        }
    }
    if declared_total > MAX_TOTAL_BYTES {
        return Err(reject("declared size exceeds cap"));
    }

    // Single-root strip (S18): `strip_single_root` computes both the
    // decision and `root` itself, so every staged entry's final rel is
    // derived through the ONE shared deterministic transform
    // (`stripped_rel`) — never a blind `split_once('/')` chop, which can
    // silently mis-map or drop a validated entry.
    let mut raws: Vec<RawEntry> = staged
        .iter()
        .map(|s| RawEntry {
            rel: s.rel.clone(),
            is_dir: s.is_dir,
        })
        .collect();
    let strip = strip_single_root(&mut raws);

    // Resolve every staged entry's FINAL rel exactly once, via the shared
    // transform, and collision-check directories and files together (same
    // namespace: a file must not collide with a directory either).
    let mut seen: HashSet<String> = HashSet::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<PlannedFile> = Vec::new();
    for s in staged {
        let Some(rel) = stripped_rel(&s.rel, &strip) else {
            continue; // the stripped root entry itself
        };
        if !seen.insert(collision_key(&rel)) {
            return Err(reject(format!("path collision: {rel}")));
        }
        match s.file {
            None => dirs.push(rel),
            Some((index, mode)) => files.push(PlannedFile { index, rel, mode }),
        }
    }

    // Pass 2a: directories, shallow -> deep, mode-clamped (S16) exactly
    // like targz.rs's `set_dir_mode`. `create_dir_all` alone leaves a
    // directory's mode dependent on the process umask (e.g. 0o777 under a
    // permissive one); these directories go on to hold binaries and config
    // later executed/read (php-fpm/nginx), so a world-writable directory
    // would let a co-resident local user swap a verified binary/config
    // before first use.
    dirs.sort_by_key(|r| r.split('/').count());
    for rel in &dirs {
        let p = dest.join(rel);
        fs::create_dir_all(&p).map_err(|e| PkgError::io("create_dir", &p, e))?;
        set_dir_mode(&p)?;
    }

    // Pass 2b: files, streamed from the SAME archive handle by index (never
    // re-opened by path), with a running real-decompressed-bytes cap (S17).
    let mut written: u64 = 0;
    for f in &files {
        let out_path = dest.join(&f.rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| PkgError::io("create_dir", parent, e))?;
        }
        let mut entry = zip
            .by_index(f.index)
            .map_err(|e| reject(format!("zip reopen: {e}")))?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
            .map_err(|e| PkgError::io("create_new", &out_path, e))?;
        written = copy_capped(&mut entry, &mut out, written)?;
        set_file_mode(&out_path, clamp_mode(f.mode))?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::{ZipSpec, mark_first_entry_encrypted, temp_file_with, zip_bytes};
    use std::io::{Seek, SeekFrom};

    fn extract(bytes: &[u8]) -> Result<tempfile::TempDir, PkgError> {
        let mut tf = temp_file_with(bytes);
        tf.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        let dest = tempfile::tempdir().unwrap();
        extract_zip(tf.as_file_mut(), dest.path())?;
        Ok(dest)
    }

    #[test]
    fn extracts_flat_zip_without_stripping() {
        let bytes = zip_bytes(&[
            ZipSpec::File {
                path: "php.exe",
                data: b"MZ",
                mode: 0o755,
            },
            ZipSpec::Dir { path: "ext/" },
            ZipSpec::File {
                path: "ext/gd.dll",
                data: b"dll",
                mode: 0o644,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("php.exe").is_file());
        assert!(dest.path().join("ext/gd.dll").is_file());
    }

    #[test]
    fn rejects_zip_slip() {
        let bytes = zip_bytes(&[ZipSpec::File {
            path: "../evil",
            data: b"x",
            mode: 0o644,
        }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_duplicate_names() {
        // A literal "two zip entries named exactly `a.txt`" fixture is not
        // constructible against the pinned `zip` 2.4.2 crate at all:
        // - The safe writer refuses it outright: `ZipWriter::start_file`
        //   routes through `insert_file_data`, which errors with
        //   `InvalidArchive("Duplicate filename")` before a second entry of
        //   the same name can even be written (confirmed empirically: this
        //   fixture panics if attempted).
        // - Even a hand-spliced archive with two identically-named raw
        //   central-directory records doesn't reach our code as "two
        //   entries" either: `ZipArchive::new` collapses same-named central
        //   directory records into ONE logical entry, because the reader
        //   stores them in an `IndexMap<Box<str>, _>` keyed by exact name —
        //   inserting a second record under an existing key overwrites the
        //   value without adding a slot. Confirmed empirically against a
        //   hand-crafted two-"a.txt"-records archive: `zip.len()` reports
        //   `1`, not `2`. So the underlying format library already forecloses
        //   this exact attack shape before our validation ever runs.
        //
        // The equivalent, ACTUALLY constructible zip-specific hazard is a
        // directory entry and a file entry that are DISTINCT raw zip names
        // (so the writer/reader treat them as two independent, valid
        // entries) but collide on their DESTINATION path once our own
        // trailing-slash normalization is applied: `"pkg/a/"` (a directory)
        // and `"pkg/a"` (a file) both validate to `rel == "a"` after the
        // shared `pkg/` root is stripped. This is exactly the case our
        // `collision_key`/`seen` rejection defends against.
        let bytes = zip_bytes(&[
            ZipSpec::Dir { path: "pkg/" },
            ZipSpec::Dir { path: "pkg/a/" },
            ZipSpec::File {
                path: "pkg/a",
                data: b"x",
                mode: 0o644,
            },
        ]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_case_collision() {
        let bytes = zip_bytes(&[
            ZipSpec::File {
                path: "Read.md",
                data: b"1",
                mode: 0o644,
            },
            ZipSpec::File {
                path: "read.md",
                data: b"2",
                mode: 0o644,
            },
        ]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn skips_symlink_entries_entirely() {
        // zip symlinks are NOT honored and NOT materialized as files (S14).
        let bytes = zip_bytes(&[
            ZipSpec::File {
                path: "real",
                data: b"x",
                mode: 0o644,
            },
            ZipSpec::Symlink {
                path: "link",
                target: "real",
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("real").is_file());
        assert!(
            !dest.path().join("link").exists(),
            "symlink entry must be skipped"
        );
    }

    #[cfg(unix)]
    #[test]
    fn clamps_modes() {
        use std::os::unix::fs::PermissionsExt;
        let bytes = zip_bytes(&[ZipSpec::File {
            path: "s",
            data: b"x",
            mode: 0o4777,
        }]);
        let dest = extract(&bytes).unwrap();
        let m = std::fs::metadata(dest.path().join("s"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(m, 0o755);
    }

    #[test]
    fn rejects_encrypted_entries() {
        let mut bytes = zip_bytes(&[ZipSpec::File {
            path: "secret",
            data: b"x",
            mode: 0o644,
        }]);
        mark_first_entry_encrypted(&mut bytes);
        // Assert the SPECIFIC rejection our own `entry.encrypted()` check
        // (via `by_index_raw`) produces, not just "some error happened" —
        // this is the regression test for that check actually firing.
        match extract(&bytes) {
            Err(PkgError::UnsafeArchive(msg)) => assert_eq!(msg, "encrypted zip entry"),
            other => panic!("expected UnsafeArchive(\"encrypted zip entry\"), got {other:?}"),
        }
    }

    #[cfg(unix)]
    /// `umask(2)` is process-wide, not per-thread. Scoping this test to set
    /// it (via a Drop-restoring guard) is safe in THIS suite specifically
    /// because every other test here checks either mere existence
    /// (`is_file`/`is_dir`/`exists`) or a mode this crate's own code
    /// explicitly `chmod`s after creation (files: `set_file_mode`; dirs, as
    /// of this test: `set_dir_mode`) — never an AMBIENT, umask-derived
    /// mode. A future test that asserts an un-clamped, umask-derived mode
    /// would need its own isolation.
    struct UmaskGuard(libc::mode_t);

    #[cfg(unix)]
    impl UmaskGuard {
        fn set(new_mask: libc::mode_t) -> Self {
            // SAFETY: `umask` has no preconditions beyond a valid process
            // (always true) and atomically returns the previous mask, which
            // we capture here and restore on drop.
            let previous = unsafe { libc::umask(new_mask) };
            Self(previous)
        }
    }

    #[cfg(unix)]
    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            // SAFETY: restores the mask `set` captured, same call shape.
            unsafe {
                libc::umask(self.0);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn clamps_dir_modes_regardless_of_umask() {
        use std::os::unix::fs::PermissionsExt;
        // A permissive (zero) umask would let a bare `create_dir_all` land
        // at 0o777. Under a common 0o022 default umask, an unconditional
        // `assert_eq!(mode, 0o755)` would pass EVEN WITHOUT an explicit
        // chmod (0o777 & !0o022 == 0o755), which would make the assertion
        // vacuous as a regression guard for `set_dir_mode`. Forcing umask
        // to 0 here means this test can only pass if the directory's mode
        // is explicitly clamped, regardless of the environment's ambient
        // umask.
        let _umask_guard = UmaskGuard::set(0);
        // A lone top-level `Dir` entry would itself be treated as the
        // single-root-strip (S18) target and dropped entirely (see
        // `rejects_duplicate_names`'s comment for the same gotcha) — add a
        // sibling file so there's no single shared top-level component and
        // `sub/` survives as a real, created directory to assert on.
        let bytes = zip_bytes(&[
            ZipSpec::Dir { path: "sub/" },
            ZipSpec::File {
                path: "other.txt",
                data: b"x",
                mode: 0o644,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        let m = std::fs::metadata(dest.path().join("sub"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(m, 0o755);
    }

    #[test]
    fn extracts_and_strips_single_root_and_rejects_stripped_root_reuse_ambiguity() {
        // Regression guard mirroring targz.rs's pass2_rel_matches_pass1_*
        // tests: a nested entry name that reuses the stripped root's own
        // name ("root/root/target" strips to "root/target") must not be
        // confused with another entry's pre-strip raw name. Because zip.rs
        // derives every entry's final rel exactly once via `stripped_rel`
        // (never a `split_once('/')` re-match), this is correct by
        // construction; the test locks that in for this format's walk too.
        let bytes = zip_bytes(&[
            ZipSpec::Dir { path: "root/" },
            ZipSpec::Dir {
                path: "root/root/target/",
            },
            ZipSpec::File {
                path: "root/target",
                data: b"AAA",
                mode: 0o644,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("root/target").is_dir());
        assert_eq!(std::fs::read(dest.path().join("target")).unwrap(), b"AAA");
    }
}
