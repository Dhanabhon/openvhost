// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened tar.gz extraction: two passes over a seekable handle. Pass 1
//! validates EVERY entry and rejects the whole archive on any violation;
//! only then does pass 2 write. Never uses tar-rs `unpack` (RUSTSEC-2021-0080
//! link traversal) — a manual walk applying the `validate` primitives.
//!
//! Every function here is exercised by this file's own test suite; the
//! first NON-test caller (wiring this into the install pipeline) lands in a
//! later task, so the plain (non-test) library build has no live entry
//! point yet. `cfg_attr(not(test), ...)` defers *that* warning without
//! silencing dead-code checks in the test build — where this module's
//! correctness actually gets proven.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;

use super::validate::{
    MAX_ENTRIES, MAX_TOTAL_BYTES, RawEntry, StripInfo, collision_key, strip_single_root,
    stripped_rel, validate_entry_name, validate_symlink_target,
};
use super::{PlannedEntry, PlannedKind};
use crate::error::PkgError;

fn reject(msg: impl Into<String>) -> PkgError {
    PkgError::UnsafeArchive(msg.into())
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
        .map_err(|e| io_err("seek", Path::new("<archive>"), e))?;
    let mut ar = tar::Archive::new(GzDecoder::new(&mut *archive));

    // First collect raw (rel, is_dir) for the strip decision + kind metadata.
    struct Staged {
        rel: String,
        kind: PlannedKind,
        is_dir: bool,
    }
    let mut staged: Vec<Staged> = Vec::new();
    let mut count = 0usize;
    let mut declared_total: u64 = 0;

    for entry in ar.entries().map_err(|e| reject(format!("tar read: {e}")))? {
        let entry = entry.map_err(|e| reject(format!("tar entry: {e}")))?;
        let et = entry.header().entry_type();
        // Skip metadata headers tar-rs may surface; reject dangerous types.
        use tar::EntryType as T;
        if matches!(
            et,
            T::XHeader | T::XGlobalHeader | T::GNULongName | T::GNULongLink
        ) {
            continue;
        }
        count += 1;
        if count > MAX_ENTRIES {
            return Err(reject("too many entries"));
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
            T::Symlink => {
                let tgt = link_target(&entry)?;
                validate_symlink_target(&rel, &tgt)?;
                PlannedKind::Symlink { target: tgt }
            }
            T::Link => {
                let tgt = link_target(&entry)?;
                let tgt = validate_entry_name(&tgt)?;
                PlannedKind::Hardlink { target: tgt }
            }
            _ => return Err(reject("disallowed entry type (device/fifo/sparse)")),
        };
        let is_dir = matches!(kind, PlannedKind::Dir);
        staged.push(Staged { rel, kind, is_dir });
    }

    // Capture the strip decision as data BEFORE `strip_single_root` mutates
    // `raws` in place. `root` is only ever consulted through `stripped_rel`,
    // which pass 2 applies identically to raw archive names — the mutated
    // `raws` themselves are discarded once we have the boolean decision.
    let root = staged
        .first()
        .map(|s| s.rel.split('/').next().unwrap_or(&s.rel).to_string())
        .unwrap_or_default();
    let mut raws: Vec<RawEntry> = staged
        .iter()
        .map(|s| RawEntry {
            rel: s.rel.clone(),
            is_dir: s.is_dir,
        })
        .collect();
    let stripped = strip_single_root(&mut raws);
    let strip = StripInfo { stripped, root };

    // Collision check on final paths, computed via the shared deterministic
    // transform — never a fuzzy re-match, so pass 2 can reproduce the exact
    // same rel from the raw archive name alone.
    let mut seen: HashSet<String> = HashSet::new();
    let mut plan: Vec<PlannedEntry> = Vec::with_capacity(staged.len());
    for s in staged {
        let Some(rel) = stripped_rel(&s.rel, &strip) else {
            continue; // the stripped root dir itself
        };
        if !seen.insert(collision_key(&rel)) {
            return Err(reject(format!("path collision: {rel}")));
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
            other => other,
        };
        plan.push(PlannedEntry { rel, kind });
    }
    Ok((plan, strip))
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
        fs::create_dir_all(&p).map_err(|e| io_err("create_dir", &p, e))?;
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
        .map_err(|e| io_err("seek", Path::new("<archive>"), e))?;
    let mut ar = tar::Archive::new(GzDecoder::new(&mut *archive));
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
            .map_err(|e| io_err("create_new", &out_path, e))?;
        written = copy_capped(&mut entry, &mut out, written)?;
        set_file_mode(&out_path, mode)?;
    }

    // Hardlinks (materialized as copies) — target is an already-extracted file.
    for e in plan {
        if let PlannedKind::Hardlink { target } = &e.kind {
            let src = dest.join(target);
            if !src.is_file() {
                return Err(reject(format!("hardlink target missing: {target}")));
            }
            let dst = dest.join(&e.rel);
            ensure_parent(&dst)?;
            fs::copy(&src, &dst).map_err(|e2| io_err("hardlink copy", &dst, e2))?;
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

/// Copy with a running total cap over REAL decompressed bytes (S17).
fn copy_capped(
    reader: &mut impl Read,
    writer: &mut impl io::Write,
    mut total: u64,
) -> Result<u64, PkgError> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| reject(format!("read: {e}")))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_TOTAL_BYTES {
            return Err(reject("decompressed size exceeds cap"));
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| reject(format!("write: {e}")))?;
    }
    Ok(total)
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

fn set_file_mode(p: &Path, mode: u32) -> Result<(), PkgError> {
    set_file_mode_impl(p, mode)
}

#[cfg(unix)]
fn set_file_mode_impl(p: &Path, mode: u32) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).map_err(|e| io_err("chmod", p, e))
}
#[cfg(not(unix))]
fn set_file_mode_impl(_p: &Path, _mode: u32) -> Result<(), PkgError> {
    Ok(())
}

fn clamp_mode(mode: u32) -> u32 {
    if mode & 0o111 != 0 { 0o755 } else { 0o644 }
}

fn ensure_parent(p: &Path) -> Result<(), PkgError> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err("create_dir", parent, e))?;
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
            Err(e) => return Err(io_err("stat", ancestor, e)),
        }
    }
    Ok(())
}

fn io_err(op: &'static str, path: &Path, source: io::Error) -> PkgError {
    PkgError::Io {
        op,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(unix)]
fn set_dir_mode(p: &Path) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).map_err(|e| io_err("chmod", p, e))
}
#[cfg(not(unix))]
fn set_dir_mode(_p: &Path) -> Result<(), PkgError> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &str, link: &Path) -> Result<(), PkgError> {
    std::os::unix::fs::symlink(target, link).map_err(|e| io_err("symlink", link, e))
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
        let bytes = targz_bytes(&[TarSpec::Symlink {
            path: "a/link",
            target: "../../etc",
        }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_symlink_chain_escape() {
        // S14: d -> . then L -> d/../x escapes at runtime; lexical rules reject
        // any '..'/'.' in a target, so both are refused.
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
        assert!(extract(&bytes).is_err());
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
        // Both symlinks pass `validate_symlink_target` independently (each
        // target, "b", is a plain sibling-relative name) — only the
        // ancestor-is-a-real-directory check at creation time catches this.
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
}
