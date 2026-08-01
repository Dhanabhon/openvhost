// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened tar.gz extraction: two passes over a seekable handle. Pass 1
//! validates EVERY entry and rejects the whole archive on any violation;
//! only then does pass 2 write. Never uses tar-rs `unpack` (RUSTSEC-2021-0080
//! link traversal) — a manual walk applying the `validate` primitives.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;

use super::common::{clamp_mode, copy_capped, reject, set_dir_mode, set_file_mode};
use super::validate::{
    Admission, EntryClass, MAX_ENTRIES, MAX_TOTAL_BYTES, RawEntry, SeenPaths, StripInfo,
    strip_single_root, stripped_rel, validate_entry_name, validate_symlink,
};
use crate::error::PkgError;

/// The extraction-plan contract this format walk's pass 1 builds and pass 2
/// materializes. tar.gz-specific: `zip.rs` builds its own lighter-weight
/// `Staged`/`PlannedFile` locals instead, since zip's random-access central
/// directory needs no separate plan/materialize split.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlannedKind {
    Dir,
    File { mode: u32 },
    Symlink { target: String },
    Hardlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedEntry {
    rel: String,
    kind: PlannedKind,
}

/// A `Read` adapter that errors once cumulative bytes read through it exceed
/// a fixed cap. Wraps the gzip decompression stream in BOTH extraction
/// passes (DoS hardening B2) so tar-rs's OWN internal buffering — e.g.
/// `read_all()`, used for GNU longname/longlink and PAX extended-header
/// payloads — is bounded by the same decompressed-bytes cap `copy_capped`
/// enforces for regular file content (S17). Without this, a crafted
/// long-name/PAX header declaring a multi-GiB payload could be decompressed
/// and buffered into memory by tar-rs internals before any of our own
/// per-entry checks ever run.
struct LimitedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> LimitedReader<R> {
    fn new(inner: R, cap: u64) -> Self {
        Self {
            inner,
            remaining: cap,
        }
    }
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if (n as u64) > self.remaining {
            return Err(io::Error::other(
                "decompressed stream exceeds the total-bytes cap",
            ));
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Extract `archive` (a verified, open handle positioned anywhere) into the
/// already-created empty directory `dest`. Pass 1 validates the whole archive
/// and rejects it on any violation; only then does pass 2 write.
pub(crate) fn extract_targz(archive: &mut fs::File, dest: &Path) -> Result<(), PkgError> {
    let (plan, strip) = plan_targz(archive)?;
    materialize(archive, &plan, &strip, dest)?;
    Ok(())
}

/// Pass 1 — read all headers, validate, build a strip-adjusted plan.
fn plan_targz(archive: &mut fs::File) -> Result<(Vec<PlannedEntry>, StripInfo), PkgError> {
    archive
        .seek(SeekFrom::Start(0))
        .map_err(|e| PkgError::io("seek", Path::new("<archive>"), e))?;
    let mut ar = tar::Archive::new(LimitedReader::new(
        GzDecoder::new(&mut *archive),
        MAX_TOTAL_BYTES,
    ));

    // First collect raw (rel, class) for the strip decision + kind metadata.
    struct Staged {
        rel: String,
        kind: PlannedKind,
        class: EntryClass,
    }
    let mut staged: Vec<Staged> = Vec::new();
    let mut count = 0usize;
    let mut declared_total: u64 = 0;

    for entry in ar.entries().map_err(|e| reject(format!("tar read: {e}")))? {
        let entry = entry.map_err(|e| reject(format!("tar entry: {e}")))?;
        // Count EVERY entry tar-rs yields us, BEFORE any metadata-type skip
        // below (DoS hardening B1): `XGlobalHeader`/longname/longlink
        // entries are otherwise iterated-and-discarded uncounted, so a
        // crafted archive could stream millions of them past `MAX_ENTRIES`
        // — a decompress-and-discard DoS that never trips the cap.
        count += 1;
        if count > MAX_ENTRIES {
            return Err(reject("too many entries"));
        }
        let et = entry.header().entry_type();
        // Skip metadata headers tar-rs may surface; reject dangerous types.
        use tar::EntryType as T;
        if matches!(
            et,
            T::XHeader | T::XGlobalHeader | T::GNULongName | T::GNULongLink
        ) {
            continue;
        }
        let path = entry.path().map_err(|e| reject(format!("bad path: {e}")))?;
        let rel = path
            .to_str()
            .ok_or_else(|| reject("entry path not utf-8"))?
            .replace('\\', "/");
        let rel = validate_entry_name(&rel)?;

        let kind = match et {
            T::Regular | T::Continuous => {
                // Fail fast on the RUNNING declared total (S17): tar-rs
                // decompresses a skipped entry's data to advance to the next
                // header, so checking only after the whole enumeration loop
                // would force decompressing every prior entry's (honestly
                // declared) huge payload before rejecting — a CPU/wall-clock
                // DoS in the reject path. Same fail-fast shape as the
                // MAX_ENTRIES check above.
                declared_total = declared_total.saturating_add(entry.size());
                if declared_total > MAX_TOTAL_BYTES {
                    return Err(reject("declared size exceeds cap"));
                }
                PlannedKind::File {
                    mode: entry.header().mode().unwrap_or(0o644),
                }
            }
            T::Directory => PlannedKind::Dir,
            // Deliberately NOT validated here. The symlink containment rule
            // (S14) needs the link's FINAL post-strip rel, which does not
            // exist until `strip_single_root` has run further down; judged
            // against the PRE-strip name it over-permits by exactly the
            // components the strip removes. `pkg/bin/x -> ../../etc/passwd`
            // passes at pre-strip depth 2 and then materializes at `bin/x`,
            // where `../..` is the PARENT of the extraction root. The one
            // call to `validate_symlink` lives in the post-strip loop below;
            // see `rejects_a_symlink_that_only_escapes_after_the_root_strip`.
            T::Symlink => PlannedKind::Symlink {
                target: link_target(&entry)?,
            },
            T::Link => {
                let tgt = link_target(&entry)?;
                let tgt = validate_entry_name(&tgt)?;
                PlannedKind::Hardlink { target: tgt }
            }
            _ => return Err(reject("disallowed entry type (device/fifo/sparse)")),
        };
        let class = classify(&kind);
        staged.push(Staged { rel, kind, class });
    }

    // The single-root-strip decision (S18) — including `root` itself — is
    // computed by `strip_single_root`; this walk and `zip.rs`'s never
    // recompute `root` independently from two copies of the same
    // `entries.first()` logic.
    let mut raws: Vec<RawEntry> = staged
        .iter()
        .map(|s| RawEntry {
            rel: s.rel.clone(),
            is_dir: s.class.is_dir(),
        })
        .collect();
    let strip = strip_single_root(&mut raws);

    // Collision check on final paths, computed via the shared deterministic
    // transform — never a fuzzy re-match, so pass 2 can reproduce the exact
    // same rel from the raw archive name alone. `SeenPaths` owns the whole
    // collide-or-not policy (see its docs for the one accepted repeat);
    // this walk only decides what to do with each outcome.
    let mut seen = SeenPaths::new();
    let mut plan: Vec<PlannedEntry> = Vec::with_capacity(staged.len());
    for s in staged {
        let Some(rel) = stripped_rel(&s.rel, &strip) else {
            continue; // the stripped root dir itself
        };
        match seen.admit(&rel, s.class)? {
            Admission::Fresh => {}
            // Drop the duplicate: the first occurrence already plans this
            // directory, and the plan is meant to be 1:1 with the
            // destination tree.
            Admission::RepeatedDirHeader => continue,
        }
        // Hardlink targets are an independent field (not covered by
        // `strip_single_root`'s all-entries-share-root check), so recompute
        // via the SAME deterministic transform rather than a blind
        // split_once('/') chop; if it doesn't share the stripped root, leave
        // it unchanged — pass 2's "hardlink target missing" check then fails
        // closed on it rather than silently misresolving.
        let kind = match s.kind {
            PlannedKind::Hardlink { target } if strip.stripped => {
                let t = stripped_rel(&target, &strip).unwrap_or(target);
                PlannedKind::Hardlink { target: t }
            }
            // The symlink containment rule (S14) is evaluated HERE, and only
            // here, because `rel` is the FINAL path the link will be created
            // at — the only depth against which "how far may this target
            // ascend" has an answer (see `validate_symlink`).
            //
            // Every symlink in the archive reaches this point. `stripped_rel`
            // returns `None` only for the shared root entry itself, and a
            // symlink NAMING the root makes `root_shape` report
            // `NotADirectory`, so no strip happens at all; and
            // `Admission::RepeatedDirHeader` requires `EntryClass::Directory`
            // on both sides, which `classify` never gives a symlink.
            PlannedKind::Symlink { target } => {
                validate_symlink(&rel, &target)?;
                PlannedKind::Symlink { target }
            }
            other => other,
        };
        plan.push(PlannedEntry { rel, kind });
    }
    Ok((plan, strip))
}

/// Classify a planned entry for [`SeenPaths::admit`] and the single-root
/// strip. EXHAUSTIVE over [`PlannedKind`] with no wildcard arm, deliberately:
/// the collision set's one exemption is keyed on "every occurrence is a
/// DIRECTORY entry", so a symlink or hardlink must be provably a
/// non-directory here rather than incidentally rejected later by whatever
/// `symlink(2)`/`fs::copy` happens to do about an existing path. A kind
/// added in future fails to compile until it is classified.
fn classify(kind: &PlannedKind) -> EntryClass {
    match kind {
        PlannedKind::Dir => EntryClass::Directory,
        PlannedKind::File { .. } | PlannedKind::Symlink { .. } | PlannedKind::Hardlink { .. } => {
            EntryClass::NonDirectory
        }
    }
}

fn link_target(entry: &tar::Entry<'_, impl Read>) -> Result<String, PkgError> {
    let l = entry
        .link_name()
        .map_err(|e| reject(format!("bad link name: {e}")))?
        .ok_or_else(|| reject("link entry without target"))?;
    l.to_str()
        .ok_or_else(|| reject("link target not utf-8"))
        .map(|s| s.replace('\\', "/"))
}

/// Pass 2 — create dirs, stream regular files from the SAME handle (re-seek +
/// fresh decoder, real-bytes cap), then deferred hardlinks (copy) and finally
/// symlinks (S14: last, after the tree exists, so no ancestor is a symlink at
/// creation time), then strip macOS quarantine xattrs (S19).
///
/// **That order is security-relevant, not incidental.** Directories →
/// files → hardlinks → symlinks. Symlinks going last is what lets
/// `verify_real_ancestors` assert that no ancestor is a link, AND what keeps
/// the hardlink loop's `src.is_file()` + `fs::copy` (both of which FOLLOW
/// symlinks) from ever reading through an archive-supplied one. Two tests
/// pin the two halves:
/// `rejects_symlink_ancestor_traversal` and
/// `refuses_a_hardlink_whose_target_is_an_archive_supplied_symlink`.
fn materialize(
    archive: &mut fs::File,
    plan: &[PlannedEntry],
    strip: &StripInfo,
    dest: &Path,
) -> Result<(), PkgError> {
    // Directories first, shallow→deep.
    let mut dirs: Vec<&PlannedEntry> = plan
        .iter()
        .filter(|e| matches!(e.kind, PlannedKind::Dir))
        .collect();
    dirs.sort_by_key(|e| e.rel.split('/').count());
    for d in dirs {
        let p = dest.join(&d.rel);
        fs::create_dir_all(&p).map_err(|e| PkgError::io("create_dir", &p, e))?;
        set_dir_mode(&p)?;
    }

    // Regular files: clamped-mode lookup keyed by validated rel.
    let file_modes: std::collections::HashMap<&str, u32> = plan
        .iter()
        .filter_map(|e| match &e.kind {
            PlannedKind::File { mode } => Some((e.rel.as_str(), clamp_mode(*mode))),
            _ => None,
        })
        .collect();

    // Re-seek to 0 and re-derive each Regular entry's rel via the SAME
    // deterministic transform pass 1 used — never a fuzzy match against the
    // plan. (A fuzzy match is how a nested segment that happens to reuse
    // the stripped root's own name could previously mismap, or silently
    // drop, a validated file — see the pass2_rel_matches_pass1_* tests.)
    archive
        .seek(SeekFrom::Start(0))
        .map_err(|e| PkgError::io("seek", Path::new("<archive>"), e))?;
    let mut ar = tar::Archive::new(LimitedReader::new(
        GzDecoder::new(&mut *archive),
        MAX_TOTAL_BYTES,
    ));
    let mut written: u64 = 0;
    for entry in ar
        .entries()
        .map_err(|e| reject(format!("tar reread: {e}")))?
    {
        let mut entry = entry.map_err(|e| reject(format!("tar entry: {e}")))?;
        use tar::EntryType as T;
        if !matches!(entry.header().entry_type(), T::Regular | T::Continuous) {
            continue;
        }
        let raw = entry
            .path()
            .map_err(|e| reject(format!("bad path: {e}")))?
            .to_str()
            .ok_or_else(|| reject("path not utf-8"))?
            .replace('\\', "/");
        let cleaned = validate_entry_name(&raw)?;
        let Some(rel) = stripped_rel(&cleaned, strip) else {
            continue; // the stripped root dir itself
        };
        let Some(&mode) = file_modes.get(rel.as_str()) else {
            continue; // not a plan File entry — shouldn't happen for a
            // pass-1-accepted archive; fail closed by skipping
        };
        let out_path = dest.join(&rel);
        ensure_parent(&out_path)?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
            .map_err(|e| PkgError::io("create_new", &out_path, e))?;
        written = copy_capped(&mut entry, &mut out, written)?;
        set_file_mode(&out_path, mode)?;
    }

    // Hardlinks (materialized as copies) — target is an already-extracted
    // file. `fs::copy`'s own return (bytes copied) is folded into the SAME
    // running `written` total `copy_capped` maintains above, and rejected
    // past the cap (DoS hardening B3): otherwise, ~100k hardlinks each
    // copying one already-in-cap file could amplify into a multi-TB disk
    // write, entirely bypassing the real-bytes cap that governs regular
    // file content.
    //
    // THIS LOOP MUST STAY AHEAD OF THE SYMLINK LOOP. Both `src.is_file()`
    // and `fs::copy` follow symlinks, so a link created earlier in this same
    // pass would silently become a legal hardlink source and the copy would
    // read through it. Running first means no archive-supplied symlink
    // exists yet, and a hardlink naming one fails closed on "target
    // missing" — pinned by
    // `refuses_a_hardlink_whose_target_is_an_archive_supplied_symlink`.
    for e in plan {
        if let PlannedKind::Hardlink { target } = &e.kind {
            let src = dest.join(target);
            if !src.is_file() {
                return Err(reject(format!("hardlink target missing: {target}")));
            }
            let dst = dest.join(&e.rel);
            ensure_parent(&dst)?;
            let copied =
                fs::copy(&src, &dst).map_err(|e2| PkgError::io("hardlink copy", &dst, e2))?;
            written = written.saturating_add(copied);
            if written > MAX_TOTAL_BYTES {
                return Err(reject("decompressed size exceeds cap"));
            }
        }
    }

    // Symlinks last (S14: after the tree exists, so no ancestor is a
    // symlink at creation time). Verify every ancestor is a REAL directory
    // before creating each link: `create_dir_all` treats a PRE-EXISTING
    // symlink as an acceptable stand-in directory and silently walks
    // through it, which would let a later symlink in this same archive
    // land somewhere it was never validated to reach.
    for e in plan {
        if let PlannedKind::Symlink { target } = &e.kind {
            let link = dest.join(&e.rel);
            verify_real_ancestors(dest, &link)?;
            ensure_parent(&link)?;
            create_symlink(target, &link)?;
        }
    }

    strip_quarantine(dest);
    Ok(())
}

/// Strip `com.apple.quarantine` (and other non-essential `com.apple.*`) xattrs
/// from the extracted tree before install (S19) — quarantine can ride through
/// archive xattrs (macOS specialist, empirically confirmed). Best-effort;
/// no-op off macOS. Uses the `xattr` crate.
#[cfg(target_os = "macos")]
fn strip_quarantine(dest: &Path) {
    fn walk(p: &Path) {
        if let Ok(names) = xattr::list(p) {
            for n in names {
                if n.to_string_lossy().starts_with("com.apple.") {
                    let _ = xattr::remove(p, &n);
                }
            }
        }
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                let cp = e.path();
                if fs::symlink_metadata(&cp)
                    .map(|m| !m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    walk(&cp);
                }
            }
        }
    }
    walk(dest);
}
#[cfg(not(target_os = "macos"))]
fn strip_quarantine(_dest: &Path) {}

fn ensure_parent(p: &Path) -> Result<(), PkgError> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| PkgError::io("create_dir", parent, e))?;
    }
    Ok(())
}

/// Verify every ancestor directory between `dest` and `path`'s parent is
/// EITHER not yet created (fine — the caller creates it for real via
/// `ensure_parent`) or an actual directory, never a symlink.
///
/// Symlinks are materialized last (S14). If an ancestor here is already a
/// symlink, some EARLIER symlink entry in this same archive put it there.
/// `fs::create_dir_all` treats an existing symlink-to-a-directory as an
/// acceptable stand-in and silently walks through it — which would let
/// THIS symlink land somewhere the archive was never validated to reach
/// (e.g. `a -> b` then `a/c -> ...` would actually create `b/c`, not
/// `a/c`). Reject the whole archive instead of walking through it.
fn verify_real_ancestors(dest: &Path, path: &Path) -> Result<(), PkgError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    for ancestor in parent.ancestors() {
        if ancestor == dest {
            break;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(reject(format!(
                    "symlink ancestor in path: {}",
                    ancestor.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(PkgError::io("stat", ancestor, e)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &str, link: &Path) -> Result<(), PkgError> {
    std::os::unix::fs::symlink(target, link).map_err(|e| PkgError::io("symlink", link, e))
}
#[cfg(windows)]
fn create_symlink(_target: &str, _link: &Path) -> Result<(), PkgError> {
    // Symlink creation needs privilege on Windows; internal package symlinks
    // are rare and out of scope for the v0 Windows runtime (deferred with the
    // matrix). Reject so behavior is explicit, never silently skipped.
    Err(reject("symlink entries not supported on windows v0"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::{TarSpec, targz_bytes, temp_file_with};
    use std::io::{Seek, SeekFrom};

    fn extract(bytes: &[u8]) -> Result<tempfile::TempDir, PkgError> {
        let mut tf = temp_file_with(bytes);
        tf.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        let dest = tempfile::tempdir().unwrap();
        extract_targz(tf.as_file_mut(), dest.path())?;
        Ok(dest)
    }

    #[test]
    fn limited_reader_errors_once_cumulative_bytes_exceed_the_cap() {
        // Direct, isolated proof of the new adapter (DoS hardening B2) — no
        // archive needed to exercise the `Read` impl itself.
        let data = b"hello world, this is more than ten bytes".to_vec();
        let mut r = LimitedReader::new(&data[..], 10);
        let mut buf = [0u8; 4];
        // First two reads (4 + 4 = 8 bytes) stay under the 10-byte cap.
        assert_eq!(r.read(&mut buf).unwrap(), 4);
        assert_eq!(r.read(&mut buf).unwrap(), 4);
        // Third read would bring the cumulative total to 12, over the cap.
        assert!(r.read(&mut buf).is_err());
    }

    #[test]
    fn limited_reader_allows_reads_up_to_exactly_the_cap() {
        let data = b"0123456789".to_vec(); // exactly 10 bytes
        let mut r = LimitedReader::new(&data[..], 10);
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, data);
    }

    #[test]
    fn counts_metadata_entries_toward_the_max_entries_cap() {
        // DoS hardening (B1): tar-rs yields `XGlobalHeader` entries to our
        // pass-1 loop, and they must count against `MAX_ENTRIES` even
        // though they're skipped as metadata — otherwise a crafted archive
        // could stream millions of them past the cap uncounted (a
        // decompress-and-discard DoS). Build `MAX_ENTRIES + 1` global-header
        // entries (zero-byte, skipped) and confirm the whole archive is
        // rejected as "too many entries" rather than silently accepted.
        use flate2::{Compression, write::GzEncoder};
        let gz = GzEncoder::new(Vec::new(), Compression::fast());
        let mut ar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(0);
        h.set_entry_type(tar::EntryType::XGlobalHeader);
        h.set_cksum();
        for _ in 0..=MAX_ENTRIES {
            ar.append(&h, io::empty()).unwrap();
        }
        let bytes = ar.into_inner().unwrap().finish().unwrap();
        match extract(&bytes) {
            Err(PkgError::UnsafeArchive(msg)) => assert_eq!(msg, "too many entries"),
            other => panic!("expected UnsafeArchive(\"too many entries\"), got {other:?}"),
        }
    }

    #[test]
    fn extracts_clean_archive_and_strips_root() {
        let bytes = targz_bytes(&[
            TarSpec::Dir {
                path: "php-8.4.23/",
            },
            TarSpec::File {
                path: "php-8.4.23/main.c",
                data: b"int main;",
                mode: 0o644,
            },
            TarSpec::File {
                path: "php-8.4.23/bin/php",
                data: b"#!/bin/sh",
                mode: 0o755,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("main.c").is_file());
        assert!(dest.path().join("bin/php").is_file());
    }

    #[test]
    fn rejects_zip_slip() {
        let bytes = targz_bytes(&[TarSpec::File {
            path: "../evil",
            data: b"x",
            mode: 0o644,
        }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_absolute_path() {
        let bytes = targz_bytes(&[TarSpec::File {
            path: "/etc/evil",
            data: b"x",
            mode: 0o644,
        }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_device_and_fifo() {
        let bytes = targz_bytes(&[TarSpec::Fifo { path: "a/pipe" }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_symlink_escape() {
        // `a/` is the archive's single implicit root, so the link lands at
        // `<dest>/link` (d = 0) and a two-step ascent has nowhere legal to
        // go. Assert the specific clause, not just `is_err()`.
        let bytes = targz_bytes(&[TarSpec::Symlink {
            path: "a/link",
            target: "../../etc",
        }]);
        match extract(&bytes) {
            Err(PkgError::UnsafeArchive(msg)) => {
                assert_eq!(msg, "symlink target ascends past the package root")
            }
            other => panic!("expected the containment rejection, got {other:?}"),
        }
    }

    #[test]
    fn rejects_symlink_chain_escape() {
        // S14: `d -> .` then `l -> d/../x` escapes at runtime, because the
        // kernel resolves `d` before applying `..`. Since the rule now
        // ADMITS `..` (as a leading run), the two links are refused by two
        // different clauses: `d`'s target is a bare `.` component, and `l`'s
        // puts `..` after a named component — the exact primitive the
        // laundering pair is built from. `d` comes first, so its reason is
        // the one that surfaces.
        let bytes = targz_bytes(&[
            TarSpec::Symlink {
                path: "d",
                target: ".",
            },
            TarSpec::Symlink {
                path: "l",
                target: "d/../x",
            },
        ]);
        match extract(&bytes) {
            Err(PkgError::UnsafeArchive(msg)) => {
                assert_eq!(msg, "'.' component in symlink target")
            }
            other => panic!("expected the containment rejection, got {other:?}"),
        }
    }

    #[test]
    fn accepts_internal_relative_symlink() {
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::File {
                path: "p/libfoo.so.1",
                data: b"x",
                mode: 0o755,
            },
            TarSpec::Symlink {
                path: "p/libfoo.so",
                target: "libfoo.so.1",
            },
        ]);
        let dest = extract(&bytes).unwrap();
        let meta = std::fs::symlink_metadata(dest.path().join("libfoo.so")).unwrap();
        assert!(meta.file_type().is_symlink());
    }

    #[test]
    fn rejects_hardlink_escape_and_materializes_internal_as_copy() {
        let bad = targz_bytes(&[TarSpec::Hardlink {
            path: "p/l",
            target: "../outside",
        }]);
        assert!(extract(&bad).is_err());
        let good = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::File {
                path: "p/real",
                data: b"data",
                mode: 0o644,
            },
            TarSpec::Hardlink {
                path: "p/copy",
                target: "p/real",
            },
        ]);
        let dest = extract(&good).unwrap();
        assert_eq!(std::fs::read(dest.path().join("copy")).unwrap(), b"data");
    }

    #[test]
    fn rejects_case_collision() {
        let bytes = targz_bytes(&[
            TarSpec::File {
                path: "a/File.txt",
                data: b"1",
                mode: 0o644,
            },
            TarSpec::File {
                path: "a/file.txt",
                data: b"2",
                mode: 0o644,
            },
        ]);
        assert!(extract(&bytes).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn clamps_modes() {
        use std::os::unix::fs::PermissionsExt;
        // NOTE: includes the "s/" dir entry so the single-root strip (S18)
        // applies, matching every other fixture in this suite — the plan
        // doc's version of this fixture omits it, which leaves the files at
        // dest/s/{setuid,data} instead of the dest/{setuid,data} the
        // assertions below check; confirmed via a debug tree dump before
        // fixing (see task-3-report.md).
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "s/" },
            TarSpec::File {
                path: "s/setuid",
                data: b"x",
                mode: 0o4755,
            },
            TarSpec::File {
                path: "s/data",
                data: b"x",
                mode: 0o666,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        let ex = std::fs::metadata(dest.path().join("setuid"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        let da = std::fs::metadata(dest.path().join("data"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(ex, 0o755, "exec bit kept, setuid stripped");
        assert_eq!(da, 0o644, "no exec bit");
    }

    #[test]
    fn pass2_rel_matches_pass1_when_nested_dir_reuses_root_name() {
        // Regression for a pass-2 mismap: a nested directory happens to be
        // named the same as the archive's stripped root ("root/root/target/"
        // strips to "root/target"), which collides TEXTUALLY with another
        // entry's own (pre-strip) raw name ("root/target", the file). The
        // old fuzzy raw-vs-plan string match resolved the file's raw name
        // "root/target" against that directory's (coincidentally identical)
        // plan rel instead of stripping it to "target", missed
        // `file_modes`, and silently dropped the file: `extract()` returned
        // `Ok` but `dest/target` was never written. The shared deterministic
        // `stripped_rel` transform can't make this mistake — it recomputes
        // from the raw name alone, never by matching against the plan.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "root/" },
            TarSpec::Dir {
                path: "root/root/target/",
            },
            TarSpec::File {
                path: "root/target",
                data: b"AAA",
                mode: 0o644,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("root/target").is_dir());
        assert_eq!(std::fs::read(dest.path().join("target")).unwrap(), b"AAA");
    }

    #[test]
    fn pass2_rel_matches_pass1_for_two_files_sharing_a_reused_segment_name() {
        // Same ambiguity as above, but with two Regular entries: nested file
        // "root/root/x" strips to "root/x", which is also the OTHER file's
        // own pre-strip raw name. Pass 1 validates both as distinct,
        // non-colliding plan entries ("root/x" and "x"). The old fuzzy match
        // resolved entry2's raw "root/x" against entry1's (already-used)
        // plan rel "root/x" instead of stripping it to "x", so the second
        // `create_new` collided with the first write — a pass-1-accepted,
        // non-colliding archive that pass 2 nonetheless failed to
        // materialize. Confirms no partial/misdirected write: each file
        // lands at its own distinct, correct path with its own content.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "root/" },
            TarSpec::File {
                path: "root/root/x",
                data: b"NESTED",
                mode: 0o644,
            },
            TarSpec::File {
                path: "root/x",
                data: b"TOPLEVEL",
                mode: 0o644,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert_eq!(
            std::fs::read(dest.path().join("root/x")).unwrap(),
            b"NESTED"
        );
        assert_eq!(std::fs::read(dest.path().join("x")).unwrap(), b"TOPLEVEL");
    }

    #[test]
    fn rejects_symlink_ancestor_traversal() {
        // S14: `fs::create_dir_all` treats a PRE-EXISTING symlink as an
        // acceptable stand-in directory and silently walks through it.
        // Without an explicit real-ancestor check, "a/c" would actually be
        // created at "b/c" (through the "a -> b" symlink created moments
        // earlier in this same materialize pass) instead of failing closed.
        // Both symlinks pass `validate_symlink` independently (each target,
        // "b", is a plain sibling-relative name with no ascent at all) —
        // only the ancestor-is-a-real-directory check at creation time
        // catches this.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "b/" },
            TarSpec::Symlink {
                path: "a",
                target: "b",
            },
            TarSpec::Symlink {
                path: "a/c",
                target: "b",
            },
        ]);
        assert!(extract(&bytes).is_err());
    }

    // ---------------------------------------------------------------
    // Real-payload shapes. Fixtures below replay the entry shapes Slice 0
    // measured in the REAL upstream `mysql-8.4.11-macos15-arm64.tar.gz`,
    // offline: its top-level component is never declared by a directory
    // entry of its own, and `<top>/bin/` appears five separate times (raw
    // tar lines 1, 24, 26, 92, 279), `<top>/lib/` four times.
    // ---------------------------------------------------------------

    #[test]
    fn accepts_repeated_directory_headers() {
        // Repeating a directory header is idempotent and benign — tar
        // producers do it routinely. Before this fix the case-folded
        // duplicate check read it as `path collision: bin` and rejected the
        // whole archive (Slice 0, variant B).
        let bytes = targz_bytes(&[
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/",
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/bin/",
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/bin/mysqld",
                data: b"ELF",
                mode: 0o755,
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/bin/",
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/lib/",
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/bin/",
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/lib/libmysqlclient.dylib",
                data: b"MACH",
                mode: 0o755,
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/lib/",
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("bin/mysqld").is_file());
        assert!(dest.path().join("lib/libmysqlclient.dylib").is_file());
    }

    #[test]
    fn strips_a_single_root_with_no_directory_header_of_its_own() {
        // Slice 0's control pair: variant C (no explicit root dir entry)
        // and variant D (C plus that one header) BOTH returned `Ok` — C
        // just put every file one level too deep, so discovery found no
        // `bin/mysqld`. Assert the resulting TREE, never `is_ok()`: `Ok`
        // cannot tell the two apart.
        let bytes = targz_bytes(&[
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/bin/",
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/bin/mysqld",
                data: b"ELF",
                mode: 0o755,
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/LICENSE",
                data: b"GPL",
                mode: 0o644,
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(
            dest.path().join("bin/mysqld").is_file(),
            "payload must land at the package root"
        );
        assert!(
            !dest.path().join("mysql-8.4.11-macos15-arm64").exists(),
            "payload must not land one level too deep"
        );
    }

    #[test]
    fn upstream_shape_repeated_dir_headers_and_an_implicit_root() {
        // Both fixes at once, in the order the real archive presents them:
        // the first entry is `<top>/bin/`, `<top>/` itself is never
        // declared, and `<top>/bin/` recurs.
        let bytes = targz_bytes(&[
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/bin/",
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/bin/mysqld",
                data: b"ELF",
                mode: 0o755,
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/lib/",
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/bin/",
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/lib/libssl.3.dylib",
                data: b"MACH",
                mode: 0o755,
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/lib/",
            },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("bin/mysqld").is_file());
        assert!(dest.path().join("lib/libssl.3.dylib").is_file());
        assert!(!dest.path().join("mysql-8.4.11-macos15-arm64").exists());
    }

    #[test]
    fn keeps_a_lone_top_level_file_instead_of_stripping_it_away() {
        // The strip's third state: an entry NAMING the shared top-level
        // component that is not a directory is payload, not a wrapper.
        // Stripping it would silently delete it and still return `Ok`, so
        // assert the tree.
        let bytes = targz_bytes(&[TarSpec::File {
            path: "only.txt",
            data: b"payload",
            mode: 0o644,
        }]);
        let dest = extract(&bytes).unwrap();
        assert_eq!(
            std::fs::read(dest.path().join("only.txt")).unwrap(),
            b"payload"
        );
    }

    /// Assert the SPECIFIC collision rejection, never merely `is_err()`.
    /// A bare `is_err()` here would also be satisfied by the extractor
    /// blundering into an `EEXIST` from `create_new`/`symlink(2)` while
    /// materializing a duplicate it should have refused to plan — which is
    /// a coincidence of the filesystem, not a check.
    fn assert_path_collision(bytes: &[u8], rel: &str) {
        match extract(bytes) {
            Err(PkgError::UnsafeArchive(msg)) => {
                assert_eq!(msg, format!("path collision: {rel}"));
            }
            other => panic!("expected UnsafeArchive(\"path collision: {rel}\"), got {other:?}"),
        }
    }

    #[test]
    fn rejects_two_files_with_the_same_name() {
        // tar (unlike zip, whose reader collapses same-named central
        // directory records into one) can genuinely carry two identically
        // named file entries. Still a collision.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::File {
                path: "p/a",
                data: b"first",
                mode: 0o644,
            },
            TarSpec::File {
                path: "p/a",
                data: b"second",
                mode: 0o644,
            },
        ]);
        assert_path_collision(&bytes, "a");
    }

    #[test]
    fn rejects_a_file_colliding_with_a_directory_in_either_order() {
        let dir_then_file = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::Dir { path: "p/a/" },
            TarSpec::File {
                path: "p/a",
                data: b"x",
                mode: 0o644,
            },
        ]);
        assert_path_collision(&dir_then_file, "a");

        let file_then_dir = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::File {
                path: "p/a",
                data: b"x",
                mode: 0o644,
            },
            TarSpec::Dir { path: "p/a/" },
        ]);
        assert_path_collision(&file_then_dir, "a");
    }

    #[test]
    fn rejects_a_symlink_or_hardlink_colliding_with_a_directory() {
        // The auditor's case. A directory header followed by a SYMLINK of
        // the same name must be rejected by the collision check itself,
        // keyed on the entry kind — not left to `symlink(2)` returning
        // EEXIST, and not silently dropped as if it were a benign repeat
        // (which would yield a clean `Ok` and a package missing a link its
        // binaries need to load).
        let symlink = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::Dir { path: "p/a/" },
            TarSpec::Symlink {
                path: "p/a",
                target: "real",
            },
        ]);
        assert_path_collision(&symlink, "a");

        let hardlink = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::File {
                path: "p/real",
                data: b"x",
                mode: 0o644,
            },
            TarSpec::Dir { path: "p/a/" },
            TarSpec::Hardlink {
                path: "p/a",
                target: "p/real",
            },
        ]);
        assert_path_collision(&hardlink, "a");
    }

    #[test]
    fn rejects_case_folded_collision_between_two_different_directory_names() {
        // The nearest neighbour to the repeated-directory-header
        // relaxation: two directory entries, both directories, but
        // GENUINELY DIFFERENT names that fold together on APFS/NTFS.
        // Accepting a repeat must not accept this.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::Dir { path: "p/Bin/" },
            TarSpec::Dir { path: "p/bin/" },
        ]);
        assert_path_collision(&bytes, "bin");
    }

    #[test]
    fn a_repeated_directory_header_cannot_launder_a_later_file() {
        // The smuggling property, end to end: no number of benign repeats
        // turns the claimed directory into something a file may take over.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::Dir { path: "p/bin/" },
            TarSpec::Dir { path: "p/bin/" },
            TarSpec::Dir { path: "p/bin/" },
            TarSpec::File {
                path: "p/bin",
                data: b"smuggled",
                mode: 0o755,
            },
        ]);
        assert_path_collision(&bytes, "bin");
    }

    #[test]
    fn a_repeated_directory_header_is_dropped_from_the_plan_not_planned_twice() {
        // The duplicate must be DROPPED, not materialized twice: pass 2
        // creates directories from the plan, and the plan is meant to be
        // 1:1 with the destination tree. Inspect the plan directly — a
        // doubly-planned directory is invisible in the resulting tree,
        // because `create_dir_all` is idempotent.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::Dir { path: "p/bin/" },
            TarSpec::Dir { path: "p/bin/" },
            TarSpec::Dir { path: "p/bin/" },
            TarSpec::File {
                path: "p/bin/mysqld",
                data: b"ELF",
                mode: 0o755,
            },
        ]);
        let mut tf = temp_file_with(&bytes);
        tf.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        let (plan, _strip) = plan_targz(tf.as_file_mut()).unwrap();
        assert_eq!(
            plan.iter().filter(|e| e.rel == "bin").count(),
            1,
            "three `bin/` headers must yield exactly one planned directory, got plan {plan:?}"
        );
        assert_eq!(
            plan.iter().filter(|e| e.rel == "bin/mysqld").count(),
            1,
            "the file must survive"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn strip_quarantine_removes_apple_xattrs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("quarantined");
        std::fs::write(&file_path, b"x").unwrap();
        xattr::set(&file_path, "com.apple.quarantine", b"0083;00000000;Test;").unwrap();
        assert!(
            xattr::list(&file_path)
                .unwrap()
                .any(|n| n.to_string_lossy() == "com.apple.quarantine"),
            "test setup: xattr should be present before strip_quarantine runs"
        );

        strip_quarantine(dir.path());

        assert!(
            !xattr::list(&file_path)
                .unwrap()
                .any(|n| n.to_string_lossy() == "com.apple.quarantine"),
            "com.apple.quarantine should be removed after strip_quarantine"
        );
    }

    // ---------------------------------------------------------------
    // The symlink containment rule (S14), end to end. `validate.rs` owns
    // the clause-by-clause unit tests; these exercise the rule through a
    // real archive, at the ONE call site that has the link's final
    // post-strip path, and — for the filesystem test — through the kernel's
    // own resolver rather than our lexical model of it.
    // ---------------------------------------------------------------

    #[test]
    fn rejects_a_symlink_that_only_escapes_after_the_root_strip() {
        // The call-site trap. This archive has one implicit top-level
        // component and no directory header of its own, so the single-root
        // strip fires and every entry moves up one level. Judged against the
        // PRE-strip name `pkg/bin/x`, `../../etc/passwd` looks contained
        // (d = 2, k = 2) — and it is not: the link is created at
        // `<dest>/bin/x`, where `../..` is the PARENT of `<dest>`. Only the
        // post-strip rel (`bin/x`, d = 1) sees k = 2 > d.
        //
        // Move the `validate_symlink` call back into the staging loop above
        // (where `rel` is still pre-strip) and this archive extracts
        // cleanly, with `bin/x` pointing outside the package.
        let bytes = targz_bytes(&[
            TarSpec::File {
                path: "pkg/bin/mysqld",
                data: b"ELF",
                mode: 0o755,
            },
            TarSpec::Symlink {
                path: "pkg/bin/x",
                target: "../../etc/passwd",
            },
        ]);
        match extract(&bytes) {
            Err(PkgError::UnsafeArchive(msg)) => {
                assert_eq!(msg, "symlink target ascends past the package root")
            }
            other => panic!("expected the containment rejection, got {other:?}"),
        }
    }

    #[test]
    fn rejects_the_two_link_laundering_pair() {
        // Verbatim, the escape a lexical containment rule accepts:
        //
        //   root/a/b/up -> ../..                 normalizes to "."
        //   root/pwn    -> a/b/up/../../secret   normalizes to "a/secret"
        //
        // Both look contained. On disk the kernel resolves `up` first, so
        // reading `root/pwn` reads two levels ABOVE the extraction root.
        // Rejected at the primitive, by two independent clauses: `up`
        // resolves to the root itself, and `pwn` puts `..` after named
        // components. `up` comes first in the archive, so its reason is the
        // one that surfaces here.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "root/" },
            TarSpec::Dir { path: "root/a/" },
            TarSpec::Dir { path: "root/a/b/" },
            TarSpec::Symlink {
                path: "root/a/b/up",
                target: "../..",
            },
            TarSpec::Symlink {
                path: "root/pwn",
                target: "a/b/up/../../secret",
            },
        ]);
        match extract(&bytes) {
            Err(PkgError::UnsafeArchive(msg)) => {
                assert_eq!(msg, "symlink target resolves to the package root itself")
            }
            other => panic!("expected the containment rejection, got {other:?}"),
        }

        // ...and the `pwn` half on its own, so a weakening of the
        // `..`-after-a-named-component clause cannot hide behind the other
        // clause rejecting `up` first.
        let pwn_only = targz_bytes(&[
            TarSpec::Dir { path: "root/" },
            TarSpec::Symlink {
                path: "root/pwn",
                target: "a/b/up/../../secret",
            },
        ]);
        match extract(&pwn_only) {
            Err(PkgError::UnsafeArchive(msg)) => assert_eq!(msg, "'..' after a named component"),
            other => panic!("expected the containment rejection, got {other:?}"),
        }
    }

    /// Every symlink under `dir`, collected without ever descending THROUGH
    /// one (`symlink_metadata`, never `metadata`) — walking through a link
    /// would recurse into whatever it points at and could loop forever.
    #[cfg(unix)]
    fn collect_symlinks(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let ft = fs::symlink_metadata(&p).unwrap().file_type();
            if ft.is_symlink() {
                out.push(p);
            } else if ft.is_dir() {
                collect_symlinks(&p, out);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn every_materialized_symlink_resolves_inside_the_destination() {
        // A purely lexical unit test cannot catch a bug in a lexical rule —
        // the rule and the test would share the same wrong model of how
        // paths resolve. This one materializes the real payload's four link
        // shapes (k = 0, 1, 2, 3 against d = 1, 1, 2, 3), including two
        // links that point AT ANOTHER LINK — `lib/plugin/libfido2.1.dylib ->
        // ../../lib/libfido2.1.dylib -> libfido2.1.15.0.dylib`, straight out
        // of upstream — and then asks the KERNEL, via `canonicalize`, where
        // each one actually lands.
        let bytes = targz_bytes(&[
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/bin/",
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/bin/mysqld",
                data: b"ELF",
                mode: 0o755,
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/lib/",
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/lib/libprotobuf-lite.24.4.0.dylib",
                data: b"MACH",
                mode: 0o755,
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/lib/libcrypto.3.dylib",
                data: b"MACH",
                mode: 0o755,
            },
            TarSpec::File {
                path: "mysql-8.4.11-macos15-arm64/lib/libfido2.1.15.0.dylib",
                data: b"MACH",
                mode: 0o755,
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/lib/plugin/",
            },
            TarSpec::Dir {
                path: "mysql-8.4.11-macos15-arm64/lib/plugin/debug/",
            },
            // k = 0: plain sibling.
            TarSpec::Symlink {
                path: "mysql-8.4.11-macos15-arm64/lib/libcrypto.dylib",
                target: "libcrypto.3.dylib",
            },
            TarSpec::Symlink {
                path: "mysql-8.4.11-macos15-arm64/lib/libfido2.1.dylib",
                target: "libfido2.1.15.0.dylib",
            },
            // k = 1 = d: the shape `@loader_path` relocation depends on.
            TarSpec::Symlink {
                path: "mysql-8.4.11-macos15-arm64/bin/libprotobuf-lite.24.4.0.dylib",
                target: "../lib/libprotobuf-lite.24.4.0.dylib",
            },
            // k = 2 = d, and the target is itself a symlink.
            TarSpec::Symlink {
                path: "mysql-8.4.11-macos15-arm64/lib/plugin/libfido2.1.dylib",
                target: "../../lib/libfido2.1.dylib",
            },
            // k = 3 = d.
            TarSpec::Symlink {
                path: "mysql-8.4.11-macos15-arm64/lib/plugin/debug/libcrypto.3.dylib",
                target: "../../../lib/libcrypto.3.dylib",
            },
            TarSpec::Symlink {
                path: "mysql-8.4.11-macos15-arm64/lib/plugin/debug/libfido2.1.dylib",
                target: "../../../lib/libfido2.1.dylib",
            },
        ]);
        let dest = extract(&bytes).unwrap();
        // Canonicalize BOTH sides: on macOS a temp dir sits under
        // `/var/folders/...`, itself reached through the `/var -> private/var`
        // symlink, so a raw `dest.path()` prefix would never match.
        let root = fs::canonicalize(dest.path()).unwrap();
        let mut links = Vec::new();
        collect_symlinks(dest.path(), &mut links);
        assert_eq!(
            links.len(),
            6,
            "the fixture must actually materialize all six links it declares — \
             a short or empty walk would make the containment assertion vacuous"
        );
        for l in &links {
            let real = fs::canonicalize(l)
                .unwrap_or_else(|e| panic!("dangling symlink {}: {e}", l.display()));
            assert!(
                real.starts_with(&root),
                "{} resolves to {}, outside {}",
                l.display(),
                real.display(),
                root.display()
            );
        }
    }

    #[test]
    fn refuses_a_hardlink_whose_target_is_an_archive_supplied_symlink() {
        // Pins the materialization ORDER, which is security-relevant and was
        // otherwise untested: files, then hardlinks, then symlinks. The
        // hardlink loop's `src.is_file()` and `fs::copy` both FOLLOW
        // symlinks, so `h -> s` is refused only because `s` does not exist
        // yet when that loop runs.
        //
        // Move symlink creation ahead of the hardlink loop and `dest/s` is
        // already a live link to `real`: `is_file()` follows it, the copy
        // reads through it, and this archive extracts cleanly instead of
        // being rejected.
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "pkg/" },
            TarSpec::File {
                path: "pkg/real",
                data: b"payload",
                mode: 0o644,
            },
            TarSpec::Symlink {
                path: "pkg/s",
                target: "real",
            },
            TarSpec::Hardlink {
                path: "pkg/h",
                target: "pkg/s",
            },
        ]);
        match extract(&bytes) {
            Err(PkgError::UnsafeArchive(msg)) => assert_eq!(msg, "hardlink target missing: s"),
            other => panic!("expected the hardlink rejection, got {other:?}"),
        }
    }
}
