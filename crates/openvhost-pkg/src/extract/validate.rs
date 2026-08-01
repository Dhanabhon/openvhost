// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure path/entry validation — the extractor's trusted core. No I/O.

use std::collections::HashMap;

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

/// Whether an archive entry is a directory entry or anything else. A
/// two-variant enum rather than a `bool` on purpose: callers must classify
/// their own entry kinds through an EXHAUSTIVE match (see `targz`'s
/// `classify`), so a kind that is not a directory — a regular file, a
/// symlink, a hardlink, or any kind added later — cannot reach
/// [`SeenPaths::admit`] wearing a `true` someone forgot to update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryClass {
    /// A directory entry, and nothing else.
    Directory,
    /// Every other entry kind: regular file, symlink, hardlink.
    NonDirectory,
}

impl EntryClass {
    /// Directories are the only entries the single-root-strip rule may treat
    /// as a wrapper. Kept here so the strip's notion of "directory" and the
    /// collision set's cannot drift apart.
    pub(crate) fn is_dir(self) -> bool {
        matches!(self, EntryClass::Directory)
    }
}

/// One destination path already claimed by an accepted entry, kept whole
/// (not just its folded [`collision_key`]) so a repeat can be compared
/// against the ORIGINAL spelling rather than merely against its fold.
#[derive(Debug)]
struct ClaimedPath {
    /// The exact validated rel that claimed this key.
    rel: String,
    class: EntryClass,
}

/// What offering an entry to [`SeenPaths`] resolved to. Deliberately an enum
/// with no catch-all: a caller must decide what to do with EVERY outcome, and
/// a future outcome must not compile until every walk handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Nothing had claimed this destination path; it is now claimed.
    Fresh,
    /// A repeated DIRECTORY header for a path already claimed by a
    /// byte-identical DIRECTORY header. Idempotent — the caller must DROP
    /// the duplicate rather than materialize it twice; the first occurrence
    /// already plans that directory.
    RepeatedDirHeader,
}

/// The set of destination paths an archive has already claimed (S12), and
/// the place that decides whether a second entry naming the same path is a
/// collision or a benign repeat.
///
/// Collision detection is case-folded and NFC-normalized ([`collision_key`])
/// because APFS/NTFS are case-insensitive by default and APFS folds NFC/NFD:
/// two entries that merely LOOK distinct in the archive can still be the
/// same file on disk.
///
/// **The one accepted repeat.** [`SeenPaths::admit`] returns
/// [`Admission::RepeatedDirHeader`] only when the incoming entry and the
/// claim it lands on are BOTH [`EntryClass::Directory`] AND their validated
/// rels are byte-identical. Real tar producers emit a directory header more
/// than once as a matter of course — upstream
/// `mysql-8.4.11-macos15-arm64.tar.gz` declares `<top>/bin/` five times and
/// `<top>/lib/` four — and repeating an idempotent "make this directory"
/// instruction claims nothing new.
///
/// **Everything else still rejects**, and the match in `admit` spells the
/// alternatives out one by one rather than leaving them to a guard: any
/// [`EntryClass::NonDirectory`] on EITHER side (file/file, file/dir and
/// dir/file, symlink/dir, hardlink/dir), and two directories whose names
/// differ but fold together (`Bin` vs `bin`) — genuinely different names
/// that would silently become one entry on a case-folding volume.
///
/// **Why nothing can be smuggled through the relaxation.** The exemption is
/// keyed on the ENTRY KIND of every occurrence, never on "this key has been
/// seen before": the only arm that can return anything but an error requires
/// `Directory` on both sides, so a non-directory meets exactly the rejection
/// it met before this relaxation existed, whatever preceded it. That arm
/// also leaves the claim untouched, so no sequence of repeats can downgrade
/// a claimed directory into something a later entry may take over.
#[derive(Debug, Default)]
pub(crate) struct SeenPaths {
    claimed: HashMap<String, ClaimedPath>,
}

impl SeenPaths {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Offer a validated, post-strip destination-relative path. `Err` means
    /// the whole archive is rejected.
    pub(crate) fn admit(&mut self, rel: &str, class: EntryClass) -> Result<Admission, PkgError> {
        let key = collision_key(rel);
        let Some(prior) = self.claimed.get(&key) else {
            self.claimed.insert(
                key,
                ClaimedPath {
                    rel: rel.to_string(),
                    class,
                },
            );
            return Ok(Admission::Fresh);
        };
        match (prior.class, class) {
            // The ONLY exemption: a directory entry repeating a directory
            // entry of the exact same name. Note this arm does not touch the
            // claim.
            (EntryClass::Directory, EntryClass::Directory) if prior.rel == rel => {
                Ok(Admission::RepeatedDirHeader)
            }
            // Two directories whose names merely FOLD together are two
            // different names, not a repeat.
            (EntryClass::Directory, EntryClass::Directory) => {
                Err(reject(&format!("path collision: {rel}")))
            }
            // Anything that is not a directory, on either side.
            (EntryClass::Directory, EntryClass::NonDirectory)
            | (EntryClass::NonDirectory, EntryClass::Directory)
            | (EntryClass::NonDirectory, EntryClass::NonDirectory) => {
                Err(reject(&format!("path collision: {rel}")))
            }
        }
    }
}

/// What an archive's entries say about the single top-level component they
/// all share — the three-state answer the single-top-dir strip rule (S18)
/// actually needs. Collapsing it to the two-state "is there an explicit
/// directory header?" is what let a real upstream tarball install one level
/// too deep and still report success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootShape {
    /// A directory entry names the shared root.
    ExplicitDir,
    /// NO entry names the shared root, but every entry nests beneath it, so
    /// it is a directory by implication. Producers routinely omit the
    /// header: upstream `mysql-8.4.11-macos15-arm64.tar.gz` never declares
    /// its own top-level directory, and Slice 0's control run confirmed that
    /// one missing header was the entire difference between a correct tree
    /// and a payload one level too deep.
    ImpliedByChildren,
    /// An entry names the shared root and it is NOT a directory — a lone
    /// top-level file, or a file that other entries claim to nest under.
    /// That entry is payload, not a wrapper; stripping would delete it
    /// (`stripped_rel` maps the root itself to `None`) and still return
    /// `Ok`.
    NotADirectory,
}

/// Apply the single-top-dir strip rule (S18): if EVERY entry shares one
/// top-level component and that component is a directory — declared or
/// implied ([`RootShape`]) — remove that leading component from all entries.
/// Returns the decision as [`StripInfo`] — computing `root` here, ONCE,
/// rather than leaving each format walk (`targz`, `zip`) recompute it
/// independently from `entries[0]` (two copies of the same logic that could
/// silently drift apart). Entries are already name-validated.
///
/// The in-place rewrite of `entries` goes through [`stripped_rel`], the same
/// transform both format walks use to compute an entry's final rel, so the
/// mutation here and the paths actually materialized cannot disagree.
pub(crate) fn strip_single_root(entries: &mut [RawEntry]) -> StripInfo {
    let root = entries
        .first()
        .map(|e| top(&e.rel).to_string())
        .unwrap_or_default();
    let info = StripInfo {
        stripped: should_strip(entries, &root),
        root,
    };
    if info.stripped {
        for e in entries.iter_mut() {
            // `None` here is the root entry itself, which the strip drops.
            e.rel = stripped_rel(&e.rel, &info).unwrap_or_default();
        }
    }
    info
}

fn top(rel: &str) -> &str {
    rel.split('/').next().unwrap_or(rel)
}

fn should_strip(entries: &[RawEntry], root: &str) -> bool {
    if entries.is_empty() || root.is_empty() {
        return false;
    }
    if !entries.iter().all(|e| top(&e.rel) == root) {
        return false;
    }
    match root_shape(entries, root) {
        RootShape::ExplicitDir | RootShape::ImpliedByChildren => true,
        RootShape::NotADirectory => false,
    }
}

/// Classify the shared top-level component from the entries that name it.
/// A non-directory naming the root is decisive and outranks any directory
/// entry for the same path — such an archive is self-contradictory, and
/// declining to strip makes the contradiction surface as a collision
/// rejection instead of two silently dropped entries.
fn root_shape(entries: &[RawEntry], root: &str) -> RootShape {
    let mut shape = RootShape::ImpliedByChildren;
    for e in entries {
        if e.rel.trim_end_matches('/') != root {
            continue;
        }
        if !e.is_dir {
            return RootShape::NotADirectory;
        }
        shape = RootShape::ExplicitDir;
    }
    shape
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

    // ----------------------------------------------------------------
    // SeenPaths: the collision policy and its ONE exemption.
    // ----------------------------------------------------------------

    fn dir(seen: &mut SeenPaths, rel: &str) -> Result<Admission, PkgError> {
        seen.admit(rel, EntryClass::Directory)
    }
    fn non_dir(seen: &mut SeenPaths, rel: &str) -> Result<Admission, PkgError> {
        seen.admit(rel, EntryClass::NonDirectory)
    }

    #[test]
    fn a_repeated_directory_header_is_admitted_as_a_repeat() {
        // Upstream mysql-8.4.11-macos15-arm64.tar.gz declares `<top>/bin/`
        // five separate times (raw tar lines 1, 24, 26, 92, 279).
        let mut seen = SeenPaths::new();
        assert_eq!(dir(&mut seen, "bin").unwrap(), Admission::Fresh);
        for _ in 0..4 {
            assert_eq!(
                dir(&mut seen, "bin").unwrap(),
                Admission::RepeatedDirHeader,
                "a repeated directory header is idempotent"
            );
        }
    }

    #[test]
    fn a_file_colliding_with_a_file_is_rejected() {
        let mut seen = SeenPaths::new();
        assert_eq!(non_dir(&mut seen, "a").unwrap(), Admission::Fresh);
        match non_dir(&mut seen, "a") {
            Err(PkgError::UnsafeArchive(m)) => assert_eq!(m, "path collision: a"),
            other => panic!("expected a collision, got {other:?}"),
        }
    }

    #[test]
    fn a_non_directory_colliding_with_a_directory_is_rejected_in_either_order() {
        // The auditor's case: whatever the NON-directory is — regular file,
        // symlink, hardlink — it is `EntryClass::NonDirectory` and takes the
        // same rejection. Nothing here relies on `symlink(2)` returning
        // EEXIST later.
        let mut dir_first = SeenPaths::new();
        assert_eq!(dir(&mut dir_first, "a").unwrap(), Admission::Fresh);
        assert!(non_dir(&mut dir_first, "a").is_err(), "dir then non-dir");

        let mut file_first = SeenPaths::new();
        assert_eq!(non_dir(&mut file_first, "a").unwrap(), Admission::Fresh);
        assert!(dir(&mut file_first, "a").is_err(), "non-dir then dir");
    }

    #[test]
    fn two_directories_whose_names_only_fold_together_are_rejected() {
        // Both sides are directories, so only the byte-identical-name
        // requirement stands between this and the exemption. `Bin` and `bin`
        // are genuinely different names that become one entry on APFS/NTFS.
        let mut case = SeenPaths::new();
        assert_eq!(dir(&mut case, "Bin").unwrap(), Admission::Fresh);
        match dir(&mut case, "bin") {
            Err(PkgError::UnsafeArchive(m)) => assert_eq!(m, "path collision: bin"),
            other => panic!("expected a collision, got {other:?}"),
        }

        // Same for NFC vs NFD spellings of the same-looking name.
        let mut norm = SeenPaths::new();
        assert_eq!(dir(&mut norm, "caf\u{e9}").unwrap(), Admission::Fresh);
        assert!(dir(&mut norm, "cafe\u{301}").is_err());
    }

    #[test]
    fn repeated_directory_headers_cannot_launder_a_later_non_directory() {
        // The smuggling property. The exemption arm never mutates the claim,
        // so no number of benign repeats turns the claimed directory into
        // something a file may take over.
        let mut seen = SeenPaths::new();
        assert_eq!(dir(&mut seen, "bin").unwrap(), Admission::Fresh);
        for _ in 0..50 {
            assert_eq!(dir(&mut seen, "bin").unwrap(), Admission::RepeatedDirHeader);
        }
        match non_dir(&mut seen, "bin") {
            Err(PkgError::UnsafeArchive(m)) => assert_eq!(m, "path collision: bin"),
            other => panic!("a file must still collide after any number of repeats, got {other:?}"),
        }
    }

    #[test]
    fn distinct_paths_are_all_fresh() {
        // Guards the opposite failure: a collision set that rejected
        // everything would pass every test above.
        let mut seen = SeenPaths::new();
        for rel in ["bin", "bin/mysqld", "lib", "lib/plugin", "share/doc"] {
            assert_eq!(dir(&mut seen, rel).unwrap(), Admission::Fresh, "{rel}");
        }
    }

    #[test]
    fn entry_class_reports_only_directory_as_a_directory() {
        assert!(EntryClass::Directory.is_dir());
        assert!(!EntryClass::NonDirectory.is_dir());
    }

    // ----------------------------------------------------------------
    // strip_single_root: the three-state root shape.
    // ----------------------------------------------------------------

    #[test]
    fn strips_a_shared_root_that_has_no_directory_entry_of_its_own() {
        // The real upstream MySQL shape: one top-level component, never
        // declared. Before this fix the strip was skipped, every file landed
        // one level too deep, and the install still returned `Ok`.
        let mut entries = vec![
            RawEntry {
                rel: "mysql-8.4.11-macos15-arm64/bin".into(),
                is_dir: true,
            },
            RawEntry {
                rel: "mysql-8.4.11-macos15-arm64/bin/mysqld".into(),
                is_dir: false,
            },
            RawEntry {
                rel: "mysql-8.4.11-macos15-arm64/LICENSE".into(),
                is_dir: false,
            },
        ];
        let strip = strip_single_root(&mut entries);
        assert!(strip.stripped);
        assert_eq!(strip.root, "mysql-8.4.11-macos15-arm64");
        assert_eq!(entries[1].rel, "bin/mysqld");
    }

    #[test]
    fn does_not_strip_when_an_entry_names_the_root_and_is_not_a_directory() {
        // Third state. Stripping here would map the root entry to `None`
        // (see `stripped_rel`) and DELETE the payload while still reporting
        // success — the same silent shape as the bug above, inverted.
        let mut lone_file = vec![RawEntry {
            rel: "only.txt".into(),
            is_dir: false,
        }];
        assert!(!strip_single_root(&mut lone_file).stripped);
        assert_eq!(lone_file[0].rel, "only.txt", "must not be rewritten away");

        // A file that other entries claim to nest under: self-contradictory.
        // Declining to strip lets the contradiction surface as a collision
        // instead of two silently dropped entries.
        let mut file_with_children = vec![
            RawEntry {
                rel: "x".into(),
                is_dir: false,
            },
            RawEntry {
                rel: "x/y".into(),
                is_dir: false,
            },
        ];
        assert!(!strip_single_root(&mut file_with_children).stripped);

        // A non-directory naming the root outranks a directory entry for the
        // same path, whichever order they appear in.
        for order in [[true, false], [false, true]] {
            let mut both = vec![
                RawEntry {
                    rel: "p".into(),
                    is_dir: order[0],
                },
                RawEntry {
                    rel: "p".into(),
                    is_dir: order[1],
                },
                RawEntry {
                    rel: "p/child".into(),
                    is_dir: false,
                },
            ];
            assert!(!strip_single_root(&mut both).stripped, "order {order:?}");
        }
    }

    #[test]
    fn does_not_strip_when_entries_do_not_share_one_top_level_component() {
        let mut entries = vec![
            RawEntry {
                rel: "bin/php".into(),
                is_dir: false,
            },
            RawEntry {
                rel: "lib/x.so".into(),
                is_dir: false,
            },
        ];
        assert!(!strip_single_root(&mut entries).stripped);
        assert_eq!(entries[0].rel, "bin/php");
    }

    #[test]
    fn the_in_place_rewrite_agrees_with_stripped_rel() {
        // The rewrite goes through `stripped_rel`, so the mutated entries and
        // the paths each format walk materializes cannot disagree.
        let raw = ["p", "p/a", "p/a/b"];
        let mut entries: Vec<RawEntry> = raw
            .iter()
            .map(|r| RawEntry {
                rel: (*r).to_string(),
                is_dir: !r.contains('.'),
            })
            .collect();
        let strip = strip_single_root(&mut entries);
        assert!(strip.stripped);
        for (i, r) in raw.iter().enumerate() {
            assert_eq!(
                entries[i].rel,
                stripped_rel(r, &strip).unwrap_or_default(),
                "{r}"
            );
        }
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
