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

/// Validate one symlink entry (S14): the link's FINAL destination-relative
/// path and its target, together, in ONE call.
///
/// # Why one function taking both
///
/// Containment is not expressible on the target alone — how far a target may
/// ascend depends on how deep the link itself sits. A target-only entry
/// point is callable without depth context, and calling one against a
/// PRE-strip name over-permits by exactly the number of components
/// [`strip_single_root`] removes: raw `pkg/bin/x -> ../../etc/passwd` looks
/// contained at depth 2, then materializes at `bin/x`, where `../..` is the
/// PARENT of the extraction root. Demanding the final rel in the signature
/// makes that call impossible to write.
///
/// # The rule
///
/// Let `k` be the length of the target's LEADING contiguous run of `..`
/// components, and `d` the number of components in the link's own directory.
///
/// 1. The target is non-empty and relative: no leading `/`, no `\`, no `:`,
///    no empty component, no `.` component.
/// 2. `..` appears ONLY in that leading run. Any `..` after a named
///    component is rejected. **Load-bearing — see below.**
/// 3. `k <= d`.
/// 4. The resolved path is non-empty: a link resolving to the extraction
///    root itself is refused. **Also load-bearing — see below.**
/// 5. The resolved path satisfies [`MAX_DEPTH`] and [`MAX_REL_BYTES`], so a
///    symlink cannot name a path [`validate_entry_name`] would have refused.
///
/// Measured against upstream `mysql-8.4.11-macos15-arm64.tar.gz`: all 34
/// symlinks pass, no exceptions, and each of the 22 whose target contains
/// `..` saturates `k == d` exactly — none of them even wants to reach the
/// package root, let alone leave it.
///
/// # Why lexical normalization is NOT enough
///
/// The obvious rule — normalize the target against the link's own directory
/// and require the result to stay under the extraction root — is UNSOUND.
/// Lexical `..` cancellation assumes every preceding component is a real
/// directory; a symlink component breaks that, so two INDIVIDUALLY CONTAINED
/// links compose into an escape:
///
/// ```text
/// root/a/b/up -> ../..                 normalizes to "."        (would pass)
/// root/pwn    -> a/b/up/../../secret   normalizes to "a/secret" (would pass)
/// ```
///
/// The kernel resolves `up` FIRST, so the following `../..` pops two levels
/// above the root and reading `root/pwn` reads a file outside it — built on
/// disk and read through during the audit of this rule. Materializing
/// symlinks last does not close it either: that protects against
/// write-through DURING extraction, not against traversal afterwards.
///
/// # Why this rule IS sound
///
/// It rejects the PRIMITIVE rather than the compositions. Under
/// [`validate_entry_name`] every entry path is relative and `..`-free, so at
/// the end of extraction every directory component of every materialized
/// path is a REAL directory — directories via `create_dir_all`, files via
/// `create_new`, symlinks last with `verify_real_ancestors` rejecting a
/// symlinked ancestor. So `(../)^k` is only ever applied to a path made
/// entirely of real directories, and `k <= d` keeps that ancestor at or
/// below the root. The tail contains no `..`, so traversing it can only
/// descend; an intermediate symlink in the tail is bound by this same rule,
/// so by induction every hop lands at or below the root. **`..` is never
/// applied to a path whose last component was a symlink** — that is the
/// invariant. Chain length is irrelevant.
///
/// Clauses 2 and 4 each break the laundering pair above on their OWN — 2
/// refuses `pwn` (its `..` follows named components), 4 refuses `up` (it
/// resolves to the root itself). That redundancy is deliberate: **keep
/// both.** Weakening either leaves a rule that still passes every acceptance
/// test in this module and fails only in production.
pub(crate) fn validate_symlink(link_rel: &str, target: &str) -> Result<(), PkgError> {
    // The link's own final rel comes from `validate_entry_name` plus the
    // single-root strip, so it is relative, `..`-free and non-empty. Fail
    // closed rather than assume it.
    if link_rel.is_empty() {
        return Err(reject("empty symlink path"));
    }

    // Clause 1 — the shape of the target itself.
    if target.is_empty() {
        return Err(reject("empty symlink target"));
    }
    if target.starts_with('/') || target.contains('\\') || target.contains(':') {
        return Err(reject("absolute or non-relative symlink target"));
    }
    let comps: Vec<&str> = target.split('/').collect();
    for c in &comps {
        if c.is_empty() {
            return Err(reject("empty component in symlink target"));
        }
        if *c == "." {
            return Err(reject("'.' component in symlink target"));
        }
    }

    // Clause 2 — `..` ONLY as a leading contiguous run. This is the clause
    // that makes the whole rule sound (see "Why this rule IS sound" above);
    // it is not a stylistic tidy-up of clause 3 and must not be folded into
    // it.
    let k = comps.iter().take_while(|c| **c == "..").count();
    if comps[k..].contains(&"..") {
        return Err(reject("'..' after a named component"));
    }

    // Clause 3 — that run may not out-ascend the link's own depth. `d` is
    // measured on the FINAL rel; see the signature note above for what
    // happens when a caller passes a pre-strip one.
    let mut link_dir: Vec<&str> = link_rel.split('/').collect();
    link_dir.pop(); // the link's own basename
    let d = link_dir.len();
    if k > d {
        return Err(reject("symlink target ascends past the package root"));
    }

    // Clauses 4 and 5 — what the target actually names, once resolved
    // against the link's directory. `k <= d` was just checked and
    // `k <= comps.len()` holds by construction, so neither slice can panic.
    let mut resolved: Vec<&str> = link_dir[..d - k].to_vec();
    resolved.extend_from_slice(&comps[k..]);
    if resolved.is_empty() {
        return Err(reject("symlink target resolves to the package root itself"));
    }
    // Only the two size limits, deliberately: the reserved-device-name and
    // trailing-dot rules govern entry NAMES (paths this extractor creates),
    // whereas a symlink target is a path it merely points at.
    if resolved.join("/").len() > MAX_REL_BYTES {
        return Err(reject("symlink target path too long"));
    }
    if resolved.len() > MAX_DEPTH {
        return Err(reject("symlink target nesting too deep"));
    }
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

    // -----------------------------------------------------------------
    // The symlink containment rule (S14), clause by clause. `..` is allowed
    // ONLY as a leading contiguous run, bounded by the link's own depth —
    // see `validate_symlink`'s docs for the soundness argument, and for why
    // clauses 2 and 4 are deliberately redundant.
    // -----------------------------------------------------------------

    /// The rejection message `validate_symlink` produced, so a test can
    /// assert WHICH clause fired rather than merely that something failed.
    /// Under a bare `is_err()` a rule with two independent clauses is
    /// indistinguishable from a rule with one, which is exactly the
    /// weakening these tests exist to notice.
    #[track_caller]
    fn reason(link_rel: &str, target: &str) -> String {
        match validate_symlink(link_rel, target) {
            Err(PkgError::UnsafeArchive(m)) => m,
            other => panic!("expected UnsafeArchive for {link_rel} -> {target}, got {other:?}"),
        }
    }

    /// Every symlink upstream `mysql-8.4.11-macos15-arm64.tar.gz` ships, in
    /// the `link -> target` form `tar -tv` prints it, read straight off the
    /// real 167,977,240-byte tarball. Link paths are shown POST-STRIP — the
    /// paths this extractor actually creates, which is the only depth the
    /// containment rule is meaningful against.
    ///
    /// 34 entries, of which 22 contain `..`. Those 22 are the mechanism that
    /// makes a relocatable macOS payload work: drop them and the install
    /// still returns a clean `Ok`, but `mysqld` dies at exec on a missing
    /// `@loader_path` library.
    const REAL_MYSQL_LINKS: [&str; 34] = [
        "bin/libprotobuf-lite.24.4.0.dylib -> ../lib/libprotobuf-lite.24.4.0.dylib",
        "bin/libprotobuf.24.4.0.dylib -> ../lib/libprotobuf.24.4.0.dylib",
        "lib/libcom_err.dylib -> libcom_err.3.0.dylib",
        "lib/libcrypto.dylib -> libcrypto.3.dylib",
        "lib/libfido2.1.dylib -> libfido2.1.15.0.dylib",
        "lib/libfido2.dylib -> libfido2.1.dylib",
        "lib/libgssapi_krb5.dylib -> libgssapi_krb5.2.2.dylib",
        "lib/libk5crypto.dylib -> libk5crypto.3.1.dylib",
        "lib/libkrb5.dylib -> libkrb5.3.3.dylib",
        "lib/libkrb5support.dylib -> libkrb5support.1.1.dylib",
        "lib/libmysqlclient.dylib -> libmysqlclient.24.dylib",
        "lib/libprotobuf-lite.dylib -> libprotobuf-lite.24.4.0.dylib",
        "lib/libprotobuf.dylib -> libprotobuf.24.4.0.dylib",
        "lib/libssl.dylib -> libssl.3.dylib",
        "lib/plugin/debug/libcom_err.3.0.dylib -> ../../../lib/libcom_err.3.0.dylib",
        "lib/plugin/debug/libcrypto.3.dylib -> ../../../lib/libcrypto.3.dylib",
        "lib/plugin/debug/libfido2.1.dylib -> ../../../lib/libfido2.1.dylib",
        "lib/plugin/debug/libgssapi_krb5.2.2.dylib -> ../../../lib/libgssapi_krb5.2.2.dylib",
        "lib/plugin/debug/libk5crypto.3.1.dylib -> ../../../lib/libk5crypto.3.1.dylib",
        "lib/plugin/debug/libkrb5.3.3.dylib -> ../../../lib/libkrb5.3.3.dylib",
        "lib/plugin/debug/libkrb5support.1.1.dylib -> ../../../lib/libkrb5support.1.1.dylib",
        "lib/plugin/debug/libprotobuf-lite.24.4.0.dylib -> ../../../lib/libprotobuf-lite.24.4.0.dylib",
        "lib/plugin/debug/libprotobuf.24.4.0.dylib -> ../../../lib/libprotobuf.24.4.0.dylib",
        "lib/plugin/debug/libssl.3.dylib -> ../../../lib/libssl.3.dylib",
        "lib/plugin/libcom_err.3.0.dylib -> ../../lib/libcom_err.3.0.dylib",
        "lib/plugin/libcrypto.3.dylib -> ../../lib/libcrypto.3.dylib",
        "lib/plugin/libfido2.1.dylib -> ../../lib/libfido2.1.dylib",
        "lib/plugin/libgssapi_krb5.2.2.dylib -> ../../lib/libgssapi_krb5.2.2.dylib",
        "lib/plugin/libk5crypto.3.1.dylib -> ../../lib/libk5crypto.3.1.dylib",
        "lib/plugin/libkrb5.3.3.dylib -> ../../lib/libkrb5.3.3.dylib",
        "lib/plugin/libkrb5support.1.1.dylib -> ../../lib/libkrb5support.1.1.dylib",
        "lib/plugin/libprotobuf-lite.24.4.0.dylib -> ../../lib/libprotobuf-lite.24.4.0.dylib",
        "lib/plugin/libprotobuf.24.4.0.dylib -> ../../lib/libprotobuf.24.4.0.dylib",
        "lib/plugin/libssl.3.dylib -> ../../lib/libssl.3.dylib",
    ];

    /// Split one [`REAL_MYSQL_LINKS`] row into its (link rel, target) pair.
    #[track_caller]
    fn real_link(row: &str) -> (&str, &str) {
        match row.split_once(" -> ") {
            Some(pair) => pair,
            None => panic!("malformed fixture row {row:?}"),
        }
    }

    #[test]
    fn accepts_every_symlink_in_the_real_mysql_payload() {
        for row in REAL_MYSQL_LINKS {
            let (link, target) = real_link(row);
            assert!(
                validate_symlink(link, target).is_ok(),
                "must accept {link} -> {target}"
            );
        }
    }

    #[test]
    fn every_real_dotdot_target_saturates_the_links_own_depth() {
        // Black-box proof that the rule is TIGHT against the real payload
        // rather than merely permissive enough for it: each of the 22 real
        // `..` targets uses exactly as much ascent as the link's own
        // directory allows, so prepending ONE more `..` must trip clause 3.
        // A rule that dropped `d` and simply allowed leading `..` would
        // accept these too, and pass the acceptance test above unchanged.
        let mut checked = 0usize;
        for row in REAL_MYSQL_LINKS {
            let (link, target) = real_link(row);
            if !target.starts_with("../") {
                continue;
            }
            checked += 1;
            let one_more = format!("../{target}");
            assert_eq!(
                reason(link, &one_more),
                "symlink target ascends past the package root",
                "{link} -> {one_more} must not be accepted"
            );
        }
        assert_eq!(checked, 22, "the real payload has 22 '..' targets");
    }

    #[test]
    fn accepts_ascent_that_stays_inside_the_package() {
        for (link, target) in [
            ("lib/libfoo.dylib", "libfoo.1.dylib"),      // k = 0
            ("bin/libfoo.dylib", "../lib/libfoo.dylib"), // k = 1 = d
            ("lib/plugin/x.dylib", "../../lib/x.dylib"), // k = 2 = d
            ("lib/plugin/debug/x", "../../../lib/x"),    // k = 3 = d
            ("a/b/c/l", "../d"),                         // k = 1 < d = 3
            ("a/b/l", ".."),                             // k = 1 < d = 2, empty tail
        ] {
            assert!(
                validate_symlink(link, target).is_ok(),
                "must accept {link} -> {target}"
            );
        }
    }

    #[test]
    fn rejects_absolute_dot_and_empty_target_components() {
        // Clause 1 — retained verbatim from the pre-`..` rule.
        assert_eq!(reason("a/l", ""), "empty symlink target");
        for bad in ["/abs/path", "c:\\x", "b\\c", "x:stream"] {
            assert_eq!(
                reason("a/l", bad),
                "absolute or non-relative symlink target",
                "{bad}"
            );
        }
        for bad in ["./x", "x/./y", "."] {
            assert_eq!(
                reason("a/l", bad),
                "'.' component in symlink target",
                "{bad}"
            );
        }
        for bad in ["x//y", "x/"] {
            assert_eq!(
                reason("a/l", bad),
                "empty component in symlink target",
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_dotdot_after_a_named_component() {
        // Clause 2, the load-bearing one: `..` is legal only in the leading
        // run, so no amount of in-root prefix can launder an ascent.
        for (link, target) in [
            ("a/b/l", "c/../d"),
            ("a/b/l", "../c/../d"),
            ("a/b/l", "c/.."),
            ("a/b/l", "../../c/d/../../.."),
            ("pwn", "a/b/up/../../secret"),
        ] {
            assert_eq!(
                reason(link, target),
                "'..' after a named component",
                "{link} -> {target}"
            );
        }
    }

    #[test]
    fn rejects_ascent_past_the_package_root() {
        // Clause 3, measured against the link's own directory depth.
        for (link, target) in [
            ("l", "../x"),                 // d = 0, k = 1
            ("bin/x", "../../etc/passwd"), // d = 1, k = 2
            ("a/b/c", "../../../x"),       // d = 2, k = 3
        ] {
            assert_eq!(
                reason(link, target),
                "symlink target ascends past the package root",
                "{link} -> {target}"
            );
        }
    }

    #[test]
    fn rejects_a_target_that_resolves_to_the_package_root_itself() {
        // Clause 4. `k == d` with an empty tail names the extraction root,
        // which is the rung the laundering pair stands on.
        for (link, target) in [("a/x", ".."), ("a/b/up", "../.."), ("a/b/c/x", "../../..")] {
            assert_eq!(
                reason(link, target),
                "symlink target resolves to the package root itself",
                "{link} -> {target}"
            );
        }
    }

    #[test]
    fn rejects_a_resolved_path_the_entry_name_rules_would_have_refused() {
        // Clause 5 — a symlink must not be able to name a path
        // `validate_entry_name` would have refused for an entry.
        let deep = vec!["b"; MAX_DEPTH].join("/");
        assert_eq!(reason("a/l", &deep), "symlink target nesting too deep");
        let long = "b".repeat(MAX_REL_BYTES);
        assert_eq!(reason("a/l", &long), "symlink target path too long");
    }

    #[test]
    fn the_two_link_laundering_pair_is_rejected_at_the_primitive() {
        // The escape a purely lexical containment rule accepts. Normalized
        // against its own directory `up` reduces to "." and `pwn` reduces to
        // "a/secret", so both look contained; on disk the kernel resolves
        // `up` FIRST and `pwn`'s trailing `../..` pops two levels ABOVE the
        // extraction root.
        //
        // Clause 4 refuses `up` and clause 2 refuses `pwn`, INDEPENDENTLY,
        // and each is asserted here by its own exact message. Delete either
        // clause and the corresponding call returns `Ok`, failing this test
        // — which is precisely what a bare `is_err()` on the pair would not
        // notice, because the surviving clause would still reject the other
        // link.
        assert_eq!(
            reason("a/b/up", "../.."),
            "symlink target resolves to the package root itself"
        );
        assert_eq!(
            reason("pwn", "a/b/up/../../secret"),
            "'..' after a named component"
        );
    }
}
