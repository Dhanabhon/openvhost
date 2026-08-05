// SPDX-License-Identifier: GPL-3.0-or-later
//! What state a MariaDB datadir is actually in — read from disk, never a
//! stored boolean. See
//! docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md (D1).
//!
//! **This file carries the data-loss risk of the whole slice.** The verdict
//! [`MariadbDatadirState::NotInitialized`] is the one a caller acts on by
//! running `--initialize`, so every rule below is written to fail towards
//! [`MariadbDatadirState::Foreign`] — refuse and say why — rather than
//! towards "looks empty enough".

use std::io::Read;
use std::path::Path;
use std::{fs, io};

use super::MARIADB_SERIES;

/// Directory entries that mark an already-initialized MariaDB datadir.
///
/// **`mariadb_upgrade_info`, NOT `auto.cnf`.** MariaDB writes no `auto.cnf` at
/// all — measured against a datadir initialized from the real 11.4.9 artifact
/// on 2026-08-04, whose root holds exactly `mysql/`, `mariadb_upgrade_info`,
/// `ib_buffer_pool`, `ibdata1`, `ib_logfile0`, `aria_log.00000001`,
/// `aria_log_control`, `sys`, `test`, `performance_schema` and `undo001..003`.
/// [`crate::mysql`]'s `SENTINEL_FILE` is therefore not merely a different
/// spelling of the same idea, and reusing it would have been a one-word change
/// that silently made every populated MariaDB datadir unrecognizable.
///
/// Both are required, and the half-state they exist to catch is **reachable,
/// not hypothetical**: an init killed 2 s in (process group, so the bootstrap
/// server dies with the script) leaves a datadir holding `mysql/` with all 88
/// system tables and no `mariadb_upgrade_info` — measured 2026-08-04, and a
/// correction to the design's §2 row that predicted an empty directory. Under
/// a rule that accepted either sentinel alone, that directory would read as a
/// finished datadir; under this one it reads [`MariadbDatadirState::Foreign`]
/// and stops.
const SENTINEL_DIR: &str = "mysql";
const SENTINEL_FILE: &str = "mariadb_upgrade_info";

/// The vendor suffix MariaDB appends in [`SENTINEL_FILE`]: the real file is
/// 14 bytes reading exactly `11.4.9-MariaDB`, with no trailing newline.
const VENDOR_SUFFIX_SEPARATOR: char = '-';

/// Cap on how much of [`SENTINEL_FILE`] is read before parsing. The real file
/// is 14 bytes; anything past this cannot be a version string, and a classifier
/// that runs on a directory the user chose must not be steerable into reading
/// an unbounded file.
const MAX_SENTINEL_FILE_BYTES: u64 = 4096;

/// The one directory entry classification ignores outright — nothing broader.
/// macOS Finder writes `.DS_Store` into almost any directory it has ever
/// displayed, including one nobody has intentionally put content into: without
/// this exemption a fresh datadir Finder has merely *looked at* would classify
/// [`MariadbDatadirState::Foreign`] and block a legitimate init. Identical in
/// substance to `crate::mysql`'s exemption, and deliberately identical in
/// scope — one name, not a pattern.
const DS_STORE: &str = ".DS_Store";

/// What a MariaDB datadir directory actually contains, established the one way
/// D1 allows: by reading the filesystem. Never a `state.db` boolean — a
/// restored or hand-copied datadir must classify correctly even though
/// `state.db` has never heard of it.
///
/// Matched **exhaustively** everywhere, never through a wildcard arm: a fourth
/// state must break compilation at every site that decides whether to run
/// `--initialize`, rather than fall into someone's `_ =>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MariadbDatadirState {
    /// Missing, or present but empty: safe to initialize into. **The only
    /// verdict that authorises a destructive next step.**
    NotInitialized,
    /// Both sentinels are present and the recorded series is the one this
    /// build starts: a real, already-initialized datadir.
    Initialized {
        /// The release recorded in `mariadb_upgrade_info`, vendor suffix
        /// stripped — `11.4.9` for a file reading `11.4.9-MariaDB`.
        version: String,
    },
    /// Present, non-empty, and NOT a datadir this build may start: a missing
    /// sentinel, an unreadable version, or a version from another series.
    /// Rendered honestly to the user — never adopted, never deleted, never
    /// initialized into.
    Foreign {
        /// Why, in words a user can act on.
        detail: String,
    },
}

/// Classify `dir` by reading it — see [`MariadbDatadirState`].
///
/// A missing directory is [`MariadbDatadirState::NotInitialized`], not an
/// error: the datadir may simply not have been provisioned yet (the same
/// convention `crate::mysql::classify_datadir` and `home::dir_size_no_follow`
/// use).
///
/// An I/O failure while reading is propagated, never swallowed into a verdict.
/// That is deliberate and it is safe in the direction that matters: `Err` is
/// not [`MariadbDatadirState::NotInitialized`], so a datadir this process
/// cannot read can never be initialized over.
///
/// **The series check is part of the verdict, not a caller's follow-up.**
/// Starting 11.4 against a datadir written by another series is a migration,
/// and this build does not migrate — so a mismatch is
/// [`MariadbDatadirState::Foreign`], which refuses, rather than
/// [`MariadbDatadirState::Initialized`], which would hand a foreign datadir to
/// a server that would try to upgrade it in place.
pub fn classify_mariadb_datadir(dir: &Path) -> io::Result<MariadbDatadirState> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(MariadbDatadirState::NotInitialized);
        }
        Err(e) => return Err(e),
    };

    let mut names: Vec<String> = Vec::new();
    let mut has_sentinel_dir = false;
    let mut has_sentinel_file = false;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == DS_STORE {
            continue; // Finder clutter — never counts toward classification
        }
        let file_type = entry.file_type()?;
        // `file_type()` does not follow symlinks, so a symlink named like a
        // sentinel is not one: it is neither `is_dir` nor `is_file`, and the
        // directory falls through to `Foreign` naming it.
        if name == SENTINEL_DIR && file_type.is_dir() {
            has_sentinel_dir = true;
        } else if name == SENTINEL_FILE && file_type.is_file() {
            has_sentinel_file = true;
        }
        names.push(name);
    }

    if names.is_empty() {
        return Ok(MariadbDatadirState::NotInitialized);
    }
    if !(has_sentinel_dir && has_sentinel_file) {
        return Ok(foreign_missing_sentinels(dir, names));
    }

    let recorded = read_recorded_version(&dir.join(SENTINEL_FILE))?;
    let Some(version) = recorded.as_deref().and_then(parse_version) else {
        return Ok(MariadbDatadirState::Foreign {
            detail: format!(
                "{} holds a {SENTINEL_FILE} this build cannot read a version out of \
                 ({}); refusing to touch it",
                dir.display(),
                describe_unparsable(recorded.as_deref())
            ),
        });
    };
    if version.series != MARIADB_SERIES {
        return Ok(MariadbDatadirState::Foreign {
            detail: format!(
                "{} was initialized by MariaDB {} (series {}), and this build starts \
                 series {MARIADB_SERIES}; upgrading a datadir between series is a \
                 migration OpenVHost does not perform",
                dir.display(),
                version.release,
                version.series
            ),
        });
    }
    Ok(MariadbDatadirState::Initialized {
        version: version.release,
    })
}

/// The `Foreign` verdict for a directory holding neither or only one sentinel,
/// naming what was actually found so the user can tell a wrong folder from a
/// broken one.
fn foreign_missing_sentinels(dir: &Path, mut names: Vec<String>) -> MariadbDatadirState {
    names.sort();
    MariadbDatadirState::Foreign {
        detail: format!(
            "{} does not look like an initialized MariaDB datadir (needs both \
             {SENTINEL_DIR}/ and {SENTINEL_FILE}; found: {})",
            dir.display(),
            names.join(", ")
        ),
    }
}

/// Read at most [`MAX_SENTINEL_FILE_BYTES`] of the sentinel file, as text.
/// `Ok(None)` means the bytes are not UTF-8 — a content judgement, not an I/O
/// failure, so it becomes a `Foreign` verdict rather than an `Err`.
fn read_recorded_version(path: &Path) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    fs::File::open(path)?
        .take(MAX_SENTINEL_FILE_BYTES)
        .read_to_end(&mut buf)?;
    Ok(String::from_utf8(buf).ok())
}

/// The release and its series, as recorded in the sentinel file.
struct RecordedVersion {
    /// `11.4.9` — the numeric release, vendor suffix stripped.
    release: String,
    /// `11.4` — the first two components, which is what a datadir is shared at.
    series: String,
}

/// Parse the sentinel file's FIRST LINE into a release and a series.
///
/// The real file is a single line with no trailing newline (`11.4.9-MariaDB`,
/// 14 bytes). Only the first line is considered so trailing junk cannot smuggle
/// itself into the comparison, and every component must be ASCII digits so a
/// crafted file cannot make the series comparison pass by looking similar.
fn parse_version(content: &str) -> Option<RecordedVersion> {
    let line = content.lines().next().unwrap_or("").trim();
    let release = line
        .split(VENDOR_SUFFIX_SEPARATOR)
        .next()
        .unwrap_or("")
        .trim();
    let mut parts = release.split('.');
    let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
        return None;
    };
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !numeric(major) || !numeric(minor) || !parts.all(numeric) {
        return None;
    }
    Some(RecordedVersion {
        release: release.to_string(),
        series: format!("{major}.{minor}"),
    })
}

/// A short, bounded description of a sentinel file that would not parse — the
/// contents are attacker-influenced in principle, so they are quoted and cut
/// rather than pasted into a message whole.
fn describe_unparsable(content: Option<&str>) -> String {
    match content {
        None => "not valid UTF-8".to_string(),
        Some(text) => {
            let line = text.lines().next().unwrap_or("").trim();
            if line.is_empty() {
                "empty".to_string()
            } else {
                let shown: String = line.chars().take(32).collect();
                format!("starts {shown:?}")
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A synthetic datadir root. `sentinels` says which of the two to create.
    fn datadir_with(sentinel_dir: bool, upgrade_info: Option<&str>) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        if sentinel_dir {
            fs::create_dir(tmp.path().join(SENTINEL_DIR)).unwrap();
        }
        if let Some(body) = upgrade_info {
            fs::write(tmp.path().join(SENTINEL_FILE), body.as_bytes()).unwrap();
        }
        tmp
    }

    fn classify(dir: &Path) -> MariadbDatadirState {
        classify_mariadb_datadir(dir).unwrap()
    }

    fn assert_not_initialized(state: &MariadbDatadirState, why: &str) {
        match state {
            MariadbDatadirState::Initialized { version } => {
                panic!("{why}: classified Initialized (version {version})")
            }
            MariadbDatadirState::NotInitialized | MariadbDatadirState::Foreign { .. } => {}
        }
    }

    // ---- Group 1: the empty / absent end ----

    #[test]
    fn a_missing_directory_is_not_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            classify(&tmp.path().join("never-created")),
            MariadbDatadirState::NotInitialized
        );
    }

    #[test]
    fn an_empty_directory_is_not_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(classify(tmp.path()), MariadbDatadirState::NotInitialized);
    }

    #[test]
    fn a_ds_store_only_directory_is_not_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(DS_STORE), b"binary-plist-ish").unwrap();
        assert_eq!(classify(tmp.path()), MariadbDatadirState::NotInitialized);
    }

    #[test]
    fn ds_store_does_not_mask_a_genuinely_foreign_directory() {
        // Vacuity for "nothing broader": the exemption is one name, and it
        // must not swallow the offender list around it.
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(DS_STORE), b"x").unwrap();
        fs::write(tmp.path().join("holiday-photos.zip"), b"hi").unwrap();
        match classify(tmp.path()) {
            MariadbDatadirState::Foreign { detail } => {
                assert!(detail.contains("holiday-photos.zip"), "{detail}");
                assert!(!detail.contains(DS_STORE), "{detail}");
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    // ---- Group 2: THE SENTINEL RULE. Both, never either alone. ----

    #[test]
    fn only_the_sentinel_dir_is_not_initialized() {
        // This is the REAL crash state, not a hypothetical: an init killed
        // 2 s in leaves exactly this — `mysql/` with all 88 system tables and
        // no `mariadb_upgrade_info` (measured 2026-08-04).
        let tmp = datadir_with(true, None);
        assert_not_initialized(
            &classify(tmp.path()),
            "a datadir holding only mysql/ is a half-written init",
        );
    }

    #[test]
    fn only_the_upgrade_info_file_is_not_initialized() {
        let tmp = datadir_with(false, Some("11.4.9-MariaDB"));
        assert_not_initialized(
            &classify(tmp.path()),
            "a datadir holding only mariadb_upgrade_info has no system schema",
        );
    }

    #[test]
    fn auto_cnf_is_not_a_mariadb_sentinel() {
        // The whole reason this module exists rather than reusing MySQL's
        // rule: `auto.cnf` means nothing here, and a datadir with MySQL's
        // pair must not read as an initialized MariaDB datadir.
        let tmp = datadir_with(true, None);
        fs::write(tmp.path().join("auto.cnf"), b"[auto]\nserver-uuid=x\n").unwrap();
        assert_not_initialized(
            &classify(tmp.path()),
            "auto.cnf must carry no weight for MariaDB",
        );
    }

    #[test]
    fn both_sentinels_with_the_expected_series_are_initialized() {
        // LITERAL names, not the constants. Every other fixture here builds
        // itself from `SENTINEL_DIR`/`SENTINEL_FILE`, which means it moves
        // with them — repointing `SENTINEL_FILE` at `auto.cnf` left this test
        // passing until it was written this way. A fixture that follows the
        // constant under test cannot detect a change to that constant.
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("mysql")).unwrap();
        fs::write(tmp.path().join("mariadb_upgrade_info"), b"11.4.9-MariaDB").unwrap();
        // A real datadir has many more files; classification must not need an
        // exhaustive match, only both sentinels and an agreeing series.
        fs::write(tmp.path().join("ibdata1"), b"").unwrap();
        fs::write(tmp.path().join("aria_log_control"), b"").unwrap();
        assert_eq!(
            classify(tmp.path()),
            MariadbDatadirState::Initialized {
                version: "11.4.9".to_string()
            }
        );
    }

    #[test]
    fn a_sentinel_of_the_wrong_kind_does_not_count() {
        // `mysql` as a FILE and `mariadb_upgrade_info` as a DIRECTORY: the
        // names are right and the shapes are wrong, so neither is evidence.
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(SENTINEL_DIR), b"not a directory").unwrap();
        fs::create_dir(tmp.path().join(SENTINEL_FILE)).unwrap();
        assert_not_initialized(&classify(tmp.path()), "shape matters, not just the name");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_sentinel_is_not_a_sentinel() {
        // `file_type()` does not follow symlinks — the `current`-link lesson.
        // A datadir whose sentinels are links out to somewhere else is not one
        // this build may act on.
        let tmp = tempfile::tempdir().unwrap();
        let real_dir = tmp.path().join("elsewhere");
        fs::create_dir(&real_dir).unwrap();
        let real_file = tmp.path().join("elsewhere.txt");
        fs::write(&real_file, b"11.4.9-MariaDB").unwrap();
        let inner = tmp.path().join("dd");
        fs::create_dir(&inner).unwrap();
        std::os::unix::fs::symlink(&real_dir, inner.join(SENTINEL_DIR)).unwrap();
        std::os::unix::fs::symlink(&real_file, inner.join(SENTINEL_FILE)).unwrap();
        assert_not_initialized(&classify(&inner), "a symlinked sentinel is not a sentinel");
    }

    // ---- Group 3: the series check ----

    #[test]
    fn a_datadir_from_another_series_is_foreign() {
        let tmp = datadir_with(true, Some("11.8.2-MariaDB"));
        match classify(tmp.path()) {
            MariadbDatadirState::Foreign { detail } => {
                assert!(
                    detail.contains("11.8"),
                    "must name the series found: {detail}"
                );
                assert!(
                    detail.contains(MARIADB_SERIES),
                    "must name the series we start: {detail}"
                );
            }
            other => panic!("expected Foreign for a foreign series, got {other:?}"),
        }
    }

    #[test]
    fn an_older_series_is_foreign_too_not_only_a_newer_one() {
        // Both directions are migrations. A rule written as "refuse newer"
        // would pass the test above and silently start 11.4 on a 10.11 datadir.
        let tmp = datadir_with(true, Some("10.11.13-MariaDB"));
        match classify(tmp.path()) {
            MariadbDatadirState::Foreign { detail } => {
                assert!(detail.contains("10.11"), "{detail}")
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn a_patch_release_inside_the_series_is_still_initialized() {
        // Vacuity for the series check: it compares the SERIES, not the whole
        // release, so an older or newer patch of 11.4 is the same datadir.
        for release in ["11.4.0", "11.4.99"] {
            let tmp = datadir_with(true, Some(&format!("{release}-MariaDB")));
            assert_eq!(
                classify(tmp.path()),
                MariadbDatadirState::Initialized {
                    version: release.to_string()
                },
                "release {release}"
            );
        }
    }

    #[test]
    fn an_unreadable_version_is_foreign_never_initialized() {
        // Every one of these has BOTH sentinels present. If the version
        // cannot be established the answer is refusal, not adoption.
        for body in [
            "",
            "\n",
            "   ",
            "-MariaDB",
            "eleven.four.nine",
            "11-MariaDB",
            "11.-MariaDB",
            "11.x.9-MariaDB",
            "../../etc/passwd",
            // The two that make the digits rule load-bearing rather than
            // decorative: BOTH have a series of exactly `11.4`, so a rule that
            // only compared the series would call them Initialized and put a
            // non-numeric string into `version`. Found by mutation — dropping
            // the digit check left every other case in this list passing.
            "11.4.x-MariaDB",
            "11.4.-MariaDB",
        ] {
            let tmp = datadir_with(true, Some(body));
            match classify(tmp.path()) {
                MariadbDatadirState::Foreign { .. } => {}
                other => panic!("body {body:?} classified {other:?}, expected Foreign"),
            }
        }
    }

    #[test]
    fn a_non_utf8_version_file_is_foreign_not_an_error() {
        let tmp = datadir_with(true, None);
        fs::write(tmp.path().join(SENTINEL_DIR).join(".keep"), b"").unwrap();
        fs::write(tmp.path().join(SENTINEL_FILE), [0xff, 0xfe, 0x00]).unwrap();
        match classify(tmp.path()) {
            MariadbDatadirState::Foreign { detail } => {
                assert!(detail.contains("UTF-8"), "{detail}");
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn junk_after_the_first_line_cannot_smuggle_a_series_through() {
        // Only the first line is parsed, and only digits count. A file whose
        // SECOND line says 11.4 must not rescue a first line that does not.
        let tmp = datadir_with(true, Some("11.8.2-MariaDB\n11.4.9-MariaDB\n"));
        match classify(tmp.path()) {
            MariadbDatadirState::Foreign { detail } => assert!(detail.contains("11.8"), "{detail}"),
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn the_sentinel_file_is_never_read_whole() {
        // The read is BOUNDED: classification runs on a directory the user
        // chose, so it must not be steerable into pulling an arbitrary file
        // into memory. Asserted on the read itself rather than on the verdict
        // — found by mutation: deleting `.take(..)` left the verdict identical
        // (a huge file does not parse either way), so a verdict-level test
        // could not fail and was proving nothing.
        let tmp = datadir_with(true, None);
        let path = tmp.path().join(SENTINEL_FILE);
        let huge = "9".repeat(MAX_SENTINEL_FILE_BYTES as usize * 4);
        fs::write(&path, huge.as_bytes()).unwrap();

        let read = read_recorded_version(&path).unwrap().unwrap();
        assert_eq!(
            read.len(),
            MAX_SENTINEL_FILE_BYTES as usize,
            "the read must stop at the cap, not at end of file"
        );
        assert!(huge.len() > read.len(), "the fixture must exceed the cap");

        // And the verdict on such a file is still a refusal.
        match classify(tmp.path()) {
            MariadbDatadirState::Foreign { .. } => {}
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn the_foreign_detail_never_pastes_the_file_contents_whole() {
        let tmp = datadir_with(true, Some(&"A".repeat(500)));
        match classify(tmp.path()) {
            MariadbDatadirState::Foreign { detail } => {
                assert!(detail.len() < 300, "detail is {} bytes", detail.len());
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    // ---- Group 4: classification never mutates ----

    #[test]
    fn classification_leaves_the_datadir_byte_for_byte_alone() {
        // Global constraint: never touch a datadir on ANY path, errors
        // included. Content AND inode — a delete-and-recreate with identical
        // bytes passes a content-only check, and this project has proven that.
        use std::os::unix::fs::MetadataExt;
        let tmp = datadir_with(true, Some("11.8.2-MariaDB"));
        let sentinel = tmp.path().join(SENTINEL_FILE);
        let before = fs::metadata(&sentinel).unwrap();
        let (ino, mtime) = (before.ino(), before.mtime());
        let before_bytes = fs::read(&sentinel).unwrap();
        let mut before_names: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        before_names.sort();

        let _ = classify(tmp.path()); // Foreign — the refusing path
        let _ = classify(&tmp.path().join("missing")); // NotInitialized path

        let after = fs::metadata(&sentinel).unwrap();
        assert_eq!(after.ino(), ino, "the sentinel file was replaced");
        assert_eq!(after.mtime(), mtime, "the sentinel file was rewritten");
        assert_eq!(fs::read(&sentinel).unwrap(), before_bytes);
        let mut after_names: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        after_names.sort();
        assert_eq!(after_names, before_names, "the directory listing changed");
    }

    // ------------------------------------------------------------------
    // Group 5 — the same rules against a datadir MariaDB itself wrote.
    //
    // Ignored by default: it needs the real 125 MB artifact (gitignored build
    // output, not a committed fixture) and takes ~20 s. Checked in rather than
    // run by hand so the proof is repeatable:
    //
    //   OPENVHOST_MARIADB_TARBALL=$PWD/build/out/mariadb-11.4.9-macos-arm64.tar.gz \
    //     cargo test -p openvhost-core --lib -- --ignored --nocapture \
    //     the_real_artifacts_own_datadir
    //
    // Vacuity is built in rather than argued: the test classifies the SAME
    // real datadir three ways — as written, with the version file removed, and
    // with the version rewritten to another series — and only the first may be
    // Initialized. A permissive rule fails on the second, a rule that skips
    // the series check fails on the third.
    //
    // `/tmp`, never `$TMPDIR`: the 103-byte `sun_path` ceiling has bitten this
    // project twice, most recently at 159 bytes.
    // ------------------------------------------------------------------

    #[test]
    #[ignore = "needs the real build artifact; set OPENVHOST_MARIADB_TARBALL"]
    fn the_real_artifacts_own_datadir_classifies_initialized_and_can_be_broken() {
        let tarball = std::env::var("OPENVHOST_MARIADB_TARBALL")
            .expect("set OPENVHOST_MARIADB_TARBALL to build/out/mariadb-11.4.9-macos-arm64.tar.gz");
        let scratch = tempfile::Builder::new()
            .prefix("ovh-mdb-")
            .tempdir_in("/tmp")
            .unwrap();
        let tree = scratch.path().join("tree");
        fs::create_dir(&tree).unwrap();
        let status = std::process::Command::new("tar")
            .arg("xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&tree)
            .status()
            .unwrap_or_else(|e| panic!("extract {tarball}: {e}"));
        assert!(status.success(), "tar exited {status}");

        // The tarball unpacks to a single `mariadb-<version>/` root.
        let basedir = fs::read_dir(&tree)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_dir())
            .expect("the artifact should hold one top-level directory");

        let datadir = scratch.path().join("dd");
        fs::create_dir(&datadir).unwrap();
        let out = std::process::Command::new(basedir.join("scripts/mariadb-install-db"))
            .current_dir(&basedir)
            .arg(format!("--basedir={}", basedir.display()))
            .arg(format!("--datadir={}", datadir.display()))
            .arg("--auth-root-authentication-method=normal")
            .output()
            .unwrap_or_else(|e| panic!("run mariadb-install-db: {e}"));
        assert!(
            out.status.success(),
            "mariadb-install-db exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );

        let mut listing: Vec<_> = fs::read_dir(&datadir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        listing.sort();
        eprintln!("real datadir holds: {listing:?}");
        assert!(
            !listing.iter().any(|n| n == "auto.cnf"),
            "MariaDB is not supposed to write auto.cnf; the D1 premise has changed: {listing:?}"
        );

        match classify(&datadir) {
            MariadbDatadirState::Initialized { version } => {
                assert!(
                    version.starts_with(&format!("{MARIADB_SERIES}.")),
                    "recorded version {version} is not in series {MARIADB_SERIES}"
                );
            }
            other => panic!("the real datadir classified {other:?}, expected Initialized"),
        }

        // Break 1 — remove the version sentinel. A rule that accepted `mysql/`
        // alone would still say Initialized here, and the next step on the
        // opposite verdict is `--initialize` over these 88 system tables.
        let sentinel = datadir.join(SENTINEL_FILE);
        let recorded = fs::read(&sentinel).unwrap();
        fs::remove_file(&sentinel).unwrap();
        assert_not_initialized(
            &classify(&datadir),
            "a real datadir missing mariadb_upgrade_info",
        );

        // Break 2 — same real datadir, another series.
        fs::write(&sentinel, b"11.8.2-MariaDB").unwrap();
        match classify(&datadir) {
            MariadbDatadirState::Foreign { detail } => eprintln!("series mismatch says: {detail}"),
            other => panic!("a foreign series classified {other:?}, expected Foreign"),
        }

        // Restored: back to Initialized, so the two breaks above were the
        // cause and not some side effect of the test's own edits.
        fs::write(&sentinel, &recorded).unwrap();
        assert!(matches!(
            classify(&datadir),
            MariadbDatadirState::Initialized { .. }
        ));
    }

    #[test]
    fn the_sentinel_names_are_the_measured_ones() {
        assert_eq!(SENTINEL_DIR, "mysql");
        assert_eq!(SENTINEL_FILE, "mariadb_upgrade_info");
        assert_ne!(
            SENTINEL_FILE, "auto.cnf",
            "reusing MySQL's sentinel is the one-word change this module exists to prevent"
        );
        // Neither name may reach outside the datadir it is joined onto.
        for name in [SENTINEL_DIR, SENTINEL_FILE] {
            assert_eq!(PathBuf::from(name).components().count(), 1, "{name}");
        }
    }
}
