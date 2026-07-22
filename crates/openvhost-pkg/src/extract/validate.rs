// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure path/entry validation — the extractor's trusted core. No I/O.

use unicode_normalization::UnicodeNormalization;

use crate::error::PkgError;
use crate::request::RESERVED;

pub(crate) const MAX_DEPTH: usize = 32;
pub(crate) const MAX_REL_BYTES: usize = 240;
/// Per-archive entry-count cap (S17), enforced by both format walks
/// (`extract::targz`, `extract::zip`).
pub(crate) const MAX_ENTRIES: usize = 100_000;
/// Decompressed-bytes cap (S17), enforced by both format walks.
pub(crate) const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct RawEntry {
    pub rel: String,
    pub is_dir: bool,
}

fn reject(reason: &str) -> PkgError {
    tracing::warn!(reason = %reason, "archive rejected");
    PkgError::UnsafeArchive(reason.to_string())
}

/// Validate a raw archive entry name and return a normalized relative path
/// using '/' separators. Rejects (S11): non-relative, `..`/`.`/empty
/// components, backslashes, drive/UNC prefixes, `:` (ADS), NUL, reserved
/// device basenames, trailing dot/space, depth > MAX_DEPTH, byte length >
/// MAX_REL_BYTES. Callers pass valid UTF-8 (zip callers convert `name_raw`
/// and reject non-UTF8 before calling).
pub(crate) fn validate_entry_name(raw: &str) -> Result<String, PkgError> {
    if raw.is_empty() {
        return Err(reject("empty entry name"));
    }
    if raw.len() > MAX_REL_BYTES {
        return Err(reject("entry path too long"));
    }
    if raw.contains('\0') {
        return Err(reject("entry name contains NUL"));
    }
    if raw.contains('\\') {
        return Err(reject("entry name contains backslash"));
    }
    if raw.starts_with('/') {
        return Err(reject("absolute entry path"));
    }
    // Drive prefix like "C:" anywhere is caught by the ':' rule below.
    let comps: Vec<&str> = raw
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    // Note: filtering empty and "." would hide "a//b" and "a/./b"; detect them explicitly.
    if raw.split('/').any(|c| c.is_empty()) {
        // leading handled above; this catches "a//b" and trailing "a/"
        // Trailing slash is legitimate for dir entries; strip exactly one trailing '/'.
        let trimmed = raw.strip_suffix('/').unwrap_or(raw);
        if trimmed.split('/').any(|c| c.is_empty()) {
            return Err(reject("empty path component"));
        }
    }
    if raw.split('/').any(|c| c == ".") {
        return Err(reject("'.' path component"));
    }
    if comps.len() > MAX_DEPTH {
        return Err(reject("entry nesting too deep"));
    }
    for c in raw.split('/') {
        if c.is_empty() {
            continue;
        }
        if c == ".." {
            return Err(reject("'..' path component"));
        }
        if c.contains(':') {
            return Err(reject("':' in path (drive/ADS)"));
        }
        if c.ends_with('.') || c.ends_with(' ') {
            return Err(reject("component ends with dot or space"));
        }
        let stem = c.split('.').next().unwrap_or(c).to_ascii_lowercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err(reject("reserved device name component"));
        }
    }
    Ok(raw.strip_suffix('/').unwrap_or(raw).to_string())
}

/// Case-folded + NFC-normalized key for cross-filesystem collision detection
/// (S12). APFS/NTFS are case-insensitive by default and APFS folds NFC/NFD.
pub(crate) fn collision_key(rel: &str) -> String {
    rel.nfc().collect::<String>().to_lowercase()
}

/// Apply the single-top-dir strip rule (S18): if EVERY entry shares one
/// top-level component AND that component appears as an explicit directory
/// entry, remove that leading component from all entries. Returns the
/// decision as [`StripInfo`] — computing `root` here, ONCE, rather than
/// leaving each format walk (`targz`, `zip`) recompute it independently
/// from `entries[0]` (two copies of the same logic that could silently
/// drift apart). Entries are already name-validated.
pub(crate) fn strip_single_root(entries: &mut [RawEntry]) -> StripInfo {
    let root = entries
        .first()
        .map(|e| top(&e.rel).to_string())
        .unwrap_or_default();
    let stripped = strip_single_root_vec(entries, &root);
    StripInfo { stripped, root }
}

fn top(rel: &str) -> &str {
    rel.split('/').next().unwrap_or(rel)
}

fn strip_single_root_vec(entries: &mut [RawEntry], root: &str) -> bool {
    if entries.is_empty() || root.is_empty() {
        return false;
    }
    let all_share = entries.iter().all(|e| top(&e.rel) == root);
    let root_is_dir_entry = entries
        .iter()
        .any(|e| e.is_dir && e.rel.trim_end_matches('/') == root);
    // Reject the degenerate "single top-level file" case: not a dir → no strip.
    if !all_share || !root_is_dir_entry {
        return false;
    }
    let prefix = format!("{root}/");
    for e in entries.iter_mut() {
        e.rel = e.rel.strip_prefix(&prefix).unwrap_or("").to_string();
    }
    true
}

/// The single-root-strip (S18) decision, captured as data so a second walk
/// over the same archive — pass 2 for tar.gz, or the same metadata pass for
/// zip's random-access central directory — can compute every entry's final
/// rel via the SAME deterministic transform ([`stripped_rel`]) instead of
/// re-deriving it by matching raw names against a plan (e.g. a blind
/// `split_once('/')` chop). Shared by every format walk: the extractor's
/// core guarantee is that materialization writes EXACTLY what validation
/// accepted, never an approximation reconstructed by fuzzy string matching.
pub(crate) struct StripInfo {
    pub stripped: bool,
    pub root: String,
}

/// Apply the single-root-strip decision to an already-validated raw rel,
/// deterministically. This is the ONE place any format walk computes an
/// entry's final rel, so two computations of the same input can never
/// disagree:
/// - not stripped: the rel is unchanged.
/// - stripped, and `validated_raw` IS the root itself: `None` — this is the
///   root entry the strip drops.
/// - stripped, and `validated_raw` starts with `root/`: the rel with that
///   prefix removed.
/// - stripped, but `validated_raw` shares no relationship with `root`
///   (only reachable for a tar hardlink TARGET string, which is an
///   independent field never covered by `strip_single_root`'s
///   all-entries-share-root check): `None` — callers fail closed on this
///   (hardlink materialization rejects a target that doesn't resolve to an
///   extracted file; a file-write loop keyed on the plan skips a name
///   absent from it).
pub(crate) fn stripped_rel(validated_raw: &str, strip: &StripInfo) -> Option<String> {
    if !strip.stripped {
        return Some(validated_raw.to_string());
    }
    if validated_raw == strip.root {
        return None;
    }
    validated_raw
        .strip_prefix(&format!("{}/", strip.root))
        .map(|s| s.to_string())
}

/// Validate a symlink target (S14): relative, valid UTF-8, no `.`/`..`
/// components (sibling/descendant only).
pub(crate) fn validate_symlink_target(target: &str) -> Result<(), PkgError> {
    if target.is_empty() {
        return Err(reject("empty symlink target"));
    }
    if target.starts_with('/') || target.contains('\\') || target.contains(':') {
        return Err(reject("absolute or non-relative symlink target"));
    }
    for c in target.split('/') {
        if c == "." || c == ".." || c.is_empty() {
            return Err(reject("symlink target has '.'/'..'/empty component"));
        }
    }
    // No component above can ever be '.'/'..'  (checked in the loop just
    // above), so the target can never lexically ascend out of wherever the
    // link lives, regardless of nesting depth — nothing further to check.
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for ok in ["php-8.4.23/main.c", "a/b/c.txt", "bin/php"] {
            assert!(validate_entry_name(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn rejects_traversal_and_absolute_and_ads() {
        for bad in [
            "../evil",
            "a/../../evil",
            "/abs/path",
            "C:/x",
            "c:\\x",
            "a/b:stream",
            "",
            ".",
            "a/./b",
            "a//b",
            "a/..",
        ] {
            assert!(validate_entry_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_reserved_and_trailing() {
        for bad in ["con", "a/nul.txt", "com1", "a/b./c", "a/b /c", "lpt9.log"] {
            assert!(validate_entry_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_nul_and_bad_utf8_is_caller_job() {
        assert!(validate_entry_name("a\0b").is_err());
    }

    #[test]
    fn rejects_depth_and_length_overflow() {
        let deep = vec!["a"; MAX_DEPTH + 1].join("/");
        assert!(validate_entry_name(&deep).is_err());
        let long = "a/".repeat(200) + "b";
        assert!(validate_entry_name(&long).is_err());
    }

    #[test]
    fn collision_key_folds_case_and_normalization() {
        assert_eq!(collision_key("File.TXT"), collision_key("file.txt"));
        // NFC 'é' vs NFD 'e\u{301}'
        assert_eq!(
            collision_key("caf\u{e9}.txt"),
            collision_key("cafe\u{301}.txt")
        );
        assert_ne!(collision_key("a.txt"), collision_key("b.txt"));
    }

    #[test]
    fn strip_single_root_applies_only_for_one_top_dir() {
        let mut one = vec![
            RawEntry {
                rel: "php-8.4/".into(),
                is_dir: true,
            },
            RawEntry {
                rel: "php-8.4/main.c".into(),
                is_dir: false,
            },
        ];
        let strip = strip_single_root(&mut one);
        assert!(strip.stripped);
        assert_eq!(strip.root, "php-8.4");
        assert_eq!(one[0].rel, "");
        assert_eq!(one[1].rel, "main.c");

        let mut flat = vec![
            RawEntry {
                rel: "php.exe".into(),
                is_dir: false,
            },
            RawEntry {
                rel: "ext/".into(),
                is_dir: true,
            },
        ];
        assert!(!strip_single_root(&mut flat).stripped);
        assert_eq!(flat[0].rel, "php.exe");

        // single top-level entry that is a FILE, not a dir -> no strip
        let mut single_file = vec![RawEntry {
            rel: "only.txt".into(),
            is_dir: false,
        }];
        assert!(!strip_single_root(&mut single_file).stripped);
    }

    #[test]
    fn stripped_rel_unchanged_when_not_stripped() {
        let strip = StripInfo {
            stripped: false,
            root: "anything".into(),
        };
        assert_eq!(stripped_rel("a/b", &strip).as_deref(), Some("a/b"));
    }

    #[test]
    fn stripped_rel_drops_the_root_entry_itself() {
        let strip = StripInfo {
            stripped: true,
            root: "php-8.4".into(),
        };
        assert_eq!(stripped_rel("php-8.4", &strip), None);
    }

    #[test]
    fn stripped_rel_removes_the_shared_prefix() {
        let strip = StripInfo {
            stripped: true,
            root: "php-8.4".into(),
        };
        assert_eq!(
            stripped_rel("php-8.4/main.c", &strip).as_deref(),
            Some("main.c")
        );
    }

    #[test]
    fn stripped_rel_none_when_unrelated_to_root() {
        // Only reachable in practice for a tar hardlink target string (an
        // independent field `strip_single_root` never validates against the
        // shared-root invariant) — callers fail closed on `None` here.
        let strip = StripInfo {
            stripped: true,
            root: "php-8.4".into(),
        };
        assert_eq!(stripped_rel("other/thing", &strip), None);
    }

    #[test]
    fn symlink_targets() {
        // sibling / descendant relative targets ok
        assert!(validate_symlink_target("libfoo.so.1").is_ok());
        assert!(validate_symlink_target("c/d").is_ok());
        // absolute, parent-escaping, or dot components rejected
        for tgt in ["/abs", "../../etc/passwd", "./x", "b/../x"] {
            assert!(validate_symlink_target(tgt).is_err(), "reject {tgt}");
        }
    }
}
