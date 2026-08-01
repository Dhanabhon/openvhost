// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the MySQL runtimes installed on this machine — in OpenVHost's own
//! package tree first, then in Homebrew.
//!
//! Two install sources coexist here **by design** while the project migrates
//! off Homebrew (MySQL-from-tarball design D3/D7). The owner is running a
//! brew-installed `mysql@8.4` today, so [`discover_mysql`] walks
//! `packages/mysql/` and keeps the Homebrew walk as a fallback; ours wins where
//! both provide the same major, because we know our version exactly and brew's
//! we would have to probe. Nothing here uninstalls, relinks or migrates a keg
//! (D7). The Homebrew walk retires in slice 7 of the programme, not here.
//!
//! Every runtime records [`MysqlRuntimeSource`] — where its binaries came from
//! — because during a migration "which mysqld am I actually running" is a
//! question that gets asked, and the honest answer needs a field on the type
//! rather than a guess at the call site.
//!
//! Never resolves anything through `PATH` — same rule as
//! `crate::php::discover`, for the same reason (a ServBay install shadows
//! binaries there).

use std::path::{Component, Path, PathBuf};

use openvhost_pkg::PackagesRoot;

use super::{MYSQL_PACKAGE_NAME, MysqlMajor};
use crate::discovery::Discovery;

/// A formula directory holds a usable runtime only when all three of these
/// exist under it. `mysqld` is the server this app supervises; `mysql` and
/// `mysqladmin` are spawned directly by later slices (root-password set,
/// connection verify, clean shutdown via `mysqladmin shutdown`) — a formula
/// missing any one of the three cannot support this app's lifecycle, so it
/// is not listed at all rather than listed with a hole (task brief: "all
/// three binaries required or the runtime isn't listed").
const MYSQLD_REL: &str = "bin/mysqld";
const MYSQL_REL: &str = "bin/mysql";
const MYSQLADMIN_REL: &str = "bin/mysqladmin";

/// Directory entries under `<prefix>/opt` that could be a MySQL formula:
/// `mysql` (the alias for the current version) and `mysql@<major>`.
fn is_mysql_formula(name: &str) -> bool {
    name == "mysql" || name.starts_with("mysql@")
}

/// The `major.minor` a candidate formula directory provides — Homebrew's own
/// keg path first, the version probe only as a fallback. The mirror of
/// `crate::php::discover::version_of`; see that function for the measurement
/// (a freshly extracted 55 MB `mysqld` takes ~11.5 s to run for the first time
/// under Gatekeeper, against a 5 s probe bound) that makes the keg path the
/// primary source rather than an optimisation.
fn version_of(dir: &Path, bin: &Path, probe: &dyn Fn(&Path) -> Option<String>) -> Option<String> {
    crate::keg::resolve_keg(dir)
        .and_then(|keg| keg.major_minor())
        .or_else(|| probe(bin))
}

/// The runtime a CATALOGUE major's own **Homebrew** formula directory
/// provides, located by path alone: no process is spawned. The mirror of
/// `crate::php::php_runtime_for_major` — see it for why the code path that has
/// just run `brew install mysql@<major>` itself must not then interrogate the
/// binary it asked for.
///
/// The packaged-tree counterpart is [`packaged_mysql_runtime`]. The two are
/// deliberately separate functions with the source in their names: a caller
/// seeding a rescan after an install knows which install it just ran, and a
/// single "find me a runtime" helper would have to guess.
///
/// All three binaries are required, exactly as [`discover_mysql`] requires
/// them: a formula that cannot support this app's lifecycle is not a runtime,
/// however it was found.
pub fn brew_mysql_runtime_for_major(
    prefixes: &[&Path],
    major: &MysqlMajor,
) -> Option<MysqlRuntime> {
    let formula = super::mysql_brew_formula(major);
    prefixes.iter().find_map(|prefix| {
        let dir = prefix.join("opt").join(&formula);
        runtime_in(&dir, major.clone(), MysqlRuntimeSource::Homebrew)
    })
}

/// Where a discovered runtime's binaries came from.
///
/// A field on [`MysqlRuntime`] rather than something inferred from a path at
/// the call site: two install sources coexist by design during the migration
/// (D3/D7), so "which mysqld am I actually running" is a question a user will
/// ask, and it is only answerable honestly if discovery records the answer at
/// the moment it walks the directory.
///
/// Matched **exhaustively** everywhere — never through a wildcard arm — so a
/// third source (a user-registered runtime, a future Windows package) breaks
/// compilation at every site that has to make a decision about it instead of
/// silently rendering as one of these two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MysqlRuntimeSource {
    /// OpenVHost's own package tree: `packages/mysql/<major>/<version>/`,
    /// fetched from the pinned upstream tarball and SHA-256 verified before
    /// extraction.
    ///
    /// The exact version comes for free (design D4): we asked the catalogue for
    /// it, and the tree records it as a directory name, so nothing has to
    /// execute a 55 MB `mysqld` to find out.
    Packaged {
        /// The exact upstream release, e.g. `"8.4.11"` — the version directory
        /// this major's `current` link selects.
        version: String,
    },
    /// A Homebrew keg this app did not install, found under a `brew --prefix`.
    /// Retired in slice 7 of the off-Homebrew programme, not here (D7).
    Homebrew,
}

impl MysqlRuntimeSource {
    /// The stable, machine-facing spelling. ONE definition, so a DTO tag, a log
    /// field and a UI label cannot drift into three different words for the
    /// same fact.
    pub fn as_str(&self) -> &'static str {
        match self {
            MysqlRuntimeSource::Packaged { .. } => "packaged",
            MysqlRuntimeSource::Homebrew => "homebrew",
        }
    }

    /// The exact version, when the source knows it.
    ///
    /// `None` for Homebrew, and deliberately so: brew's full version would have
    /// to be probed, and probing a freshly extracted `mysqld` under macOS's
    /// first-execution scan is the measurement that put design D4 in the spec.
    /// Reporting the *major* as though it were the full version would be a lie
    /// no caller could detect.
    pub fn version(&self) -> Option<&str> {
        match self {
            MysqlRuntimeSource::Packaged { version } => Some(version),
            MysqlRuntimeSource::Homebrew => None,
        }
    }
}

/// One discovered MySQL installation: a [`MysqlMajor`], the three binaries this
/// app drives directly, and where they came from. All three binaries are
/// guaranteed to exist as files — [`discover_mysql`] never returns a partial
/// runtime.
///
/// `major` can be a value [`MysqlMajor::is_cataloged`] reports `false` for:
/// a discovered installation this build does not offer to INSTALL is still
/// discovered and listed (spec D1 — "a user's 9.x renders as a row without
/// an Install button"). See [`MysqlMajor`]'s doc comment for how its two
/// constructors divide that responsibility.
///
/// **The paths are always concrete** (design D5). For a packaged runtime they
/// name `packages/mysql/<major>/<version>/bin/…`, never
/// `packages/mysql/<major>/current/bin/…`: a supervised child is spawned from
/// whatever is recorded here, and spawning *through* the link would mean a
/// later `current` swap silently changed which engine a restart brings up,
/// with the running process and the one the UI describes diverging and nothing
/// in between to notice. It also makes `mysqld`'s argv[0]-derived basedir
/// ambiguous — the exact class of thing that cost this project a full
/// misdiagnosis in the MySQL lifecycle slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlRuntime {
    pub major: MysqlMajor,
    pub mysqld: PathBuf,
    pub mysql: PathBuf,
    pub mysqladmin: PathBuf,
    /// Which install produced the three paths above — see
    /// [`MysqlRuntimeSource`].
    pub source: MysqlRuntimeSource,
}

/// The runtime rooted at `dir`, when all three binaries are really there.
///
/// The single place a [`MysqlRuntime`] is built, so the all-three rule cannot
/// hold for one source and not the other: a directory that cannot support this
/// app's lifecycle (no `mysqladmin` means no clean shutdown) is not a runtime,
/// however it was found.
fn runtime_in(dir: &Path, major: MysqlMajor, source: MysqlRuntimeSource) -> Option<MysqlRuntime> {
    let mysqld = dir.join(MYSQLD_REL);
    let mysql = dir.join(MYSQL_REL);
    let mysqladmin = dir.join(MYSQLADMIN_REL);
    if !(mysqld.is_file() && mysql.is_file() && mysqladmin.is_file()) {
        return None;
    }
    Some(MysqlRuntime {
        major,
        mysqld,
        mysql,
        mysqladmin,
        source,
    })
}

/// The version a per-major `current` link selects, or `None` when the link is
/// missing, unreadable, or names anything other than a single plain directory
/// name.
///
/// SECURITY: `openvhost-pkg` writes `current` as a RELATIVE symlink whose
/// target is the bare version string (`8.4.11`), so the entire legitimate space
/// of targets is "exactly one [`Component::Normal`]". Requiring precisely that
/// is the containment guarantee, and it is applied to the link's target
/// *before* the string is joined onto anything: `..`, an absolute path, and any
/// multi-component target are all refused, so a tampered link cannot make
/// discovery describe a directory outside the major it was found under.
///
/// This bounds what a link TARGET may name. It is not, and could not be, a
/// defence against someone who can already write inside `<home>/packages` —
/// nothing in this tree is.
fn current_version(link: &Path) -> Option<String> {
    let target = std::fs::read_link(link).ok()?;
    let mut components = target.components();
    // `Component::Normal` alone rejects `..` (ParentDir), `/` (RootDir) and a
    // Windows prefix; the `is_some` check below rejects `a/b`.
    //
    // `./a` is REJECTED, not accepted. std normalises `.` away everywhere
    // EXCEPT at the start of a path, so `Path::new("./a").components()` yields
    // `[CurDir, Normal("a")]` and the pattern below does not match. (A previous
    // version of this comment claimed the opposite; it was wrong, and a wrong
    // premise here is exactly what a future refactor would "correct" in the
    // permissive direction.) Refusing it is right regardless: `openvhost-pkg`
    // writes the bare version string and nothing else, so the accepted set is
    // exactly "one plain directory name" — see
    // `a_current_link_spelled_with_a_leading_dot_segment_is_refused`.
    let Some(Component::Normal(name)) = components.next() else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    name.to_str().map(str::to_string)
}

/// The packaged runtime this major's `current` link selects, or `None` when the
/// major has no usable packaged install.
///
/// The counterpart to [`brew_mysql_runtime_for_major`], and the one place the
/// `current` link is ever resolved. Returns paths inside the concrete version
/// directory (design D5) — see [`MysqlRuntime`]'s doc comment for why that
/// matters more than it looks.
pub fn packaged_mysql_runtime(root: &PackagesRoot, major: &MysqlMajor) -> Option<MysqlRuntime> {
    let major_dir = root.major_dir(MYSQL_PACKAGE_NAME, major.as_str());
    // Through the layout facade, never `major_dir.join("current")` spelled by
    // hand: the installer swings this link through `PackagesRoot`, and a second
    // spelling here is how the writer and the reader end up naming different
    // files.
    let version = current_version(&root.current_link(MYSQL_PACKAGE_NAME, major.as_str()))?;
    let dir = root.package_dir(MYSQL_PACKAGE_NAME, major.as_str(), &version);
    // Belt and braces over `current_version`'s single-component rule, stated
    // structurally so it keeps holding whatever a future `join` does with an
    // unexpected target shape: the directory whose binaries we are about to
    // hand out MUST be a direct child of this major's directory.
    if dir.parent() != Some(major_dir.as_path()) {
        return None;
    }
    runtime_in(
        &dir,
        major.clone(),
        MysqlRuntimeSource::Packaged { version },
    )
}

/// Whether a major directory that yielded no runtime nevertheless holds
/// evidence of an install — i.e. whether "nothing here" would be a lie.
///
/// An entirely empty major directory is NOT evidence: removing the last version
/// of a major legitimately leaves one behind, and reporting that forever would
/// make [`Discovery::is_complete`] a permanent `false` — the boy-who-cried-wolf
/// version of the honesty that type exists to provide.
///
/// A path that exists but cannot be listed (a plain file sitting where a
/// directory belongs, a permission error) DOES count: we genuinely cannot tell,
/// which is exactly what [`Discovery::unidentified`] means.
fn looks_like_a_broken_install(major_dir: &Path) -> bool {
    match std::fs::read_dir(major_dir) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => major_dir.exists(),
    }
}

/// Walk `packages/mysql/` — OpenVHost's own install source.
///
/// Spawns nothing and probes nothing: the version is a directory name we chose
/// at install time (design D4), so this walk is a `read_link` and three
/// `is_file` calls per major.
///
/// A major directory that resolves to no usable runtime but is not empty is
/// reported through [`Discovery::unidentified`] rather than dropped. That is
/// stricter than the Homebrew walk below, which silently skips a partial
/// formula directory, and deliberately so: a broken keg is somebody else's
/// install, a broken `packages/mysql/8.4/` is ours.
fn discover_packaged(root: &PackagesRoot) -> Discovery<MysqlRuntime> {
    let tree = root.as_path().join(MYSQL_PACKAGE_NAME);
    let Ok(entries) = std::fs::read_dir(&tree) else {
        return Discovery::default(); // no package tree yet is not an error
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();

    let mut runtimes = Vec::new();
    let mut unidentified = Vec::new();
    for name in names {
        let major_dir = tree.join(&name);
        // Not `major.minor` shaped: never something this app wrote, so it is
        // not an install we failed to identify. (The shape check is also what
        // keeps `..` and separators out of the major component — see
        // `MysqlMajor`'s doc comment.)
        let Some(major) = MysqlMajor::from_probe(name) else {
            continue;
        };
        match packaged_mysql_runtime(root, &major) {
            Some(rt) => runtimes.push(rt),
            None if looks_like_a_broken_install(&major_dir) => unidentified.push(major_dir),
            None => {}
        }
    }
    Discovery {
        runtimes,
        unidentified,
    }
}

/// Every MySQL runtime on this machine, from BOTH install sources, with
/// OpenVHost's own package tree winning wherever the two provide the same
/// major (design D3).
///
/// Ours wins because we know its version exactly — the tree records it — while
/// brew's would have to be probed. It is one comparison, and it is what buys
/// the migration room to be incremental instead of stranding the user who has
/// a working `brew install mysql@8.4` today.
///
/// The two walks are combined here rather than exposed separately on purpose:
/// a caller that could see only half the machine is the bug this signature
/// prevents. `packages` is minted from a resolved home
/// ([`PackagesRoot::from_home`]), never from user input.
///
/// `unidentified` carries candidates from both sources, so an empty `runtimes`
/// still means "nothing is installed" and never "I could not tell" —
/// see [`Discovery`].
pub fn discover_mysql(
    packages: &PackagesRoot,
    prefixes: &[&Path],
    probe: &dyn Fn(&Path) -> Option<String>,
) -> Discovery<MysqlRuntime> {
    let mut found = discover_packaged(packages);
    let brew = discover_brew(prefixes, probe);
    for rt in brew.runtimes {
        if !found.runtimes.iter().any(|ours| ours.major == rt.major) {
            found.runtimes.push(rt);
        }
    }
    found.unidentified.extend(brew.unidentified);
    found.runtimes.sort_by(|a, b| a.major.cmp(&b.major));
    found
}

/// Merge rules mirror `crate::discover_php_in` exactly, with one addition:
/// a formula directory is only a candidate when ALL THREE of `bin/mysqld`,
/// `bin/mysql` and `bin/mysqladmin` exist as files (see `MYSQLD_REL` and
/// friends). Two preferences apply when merging, and they can disagree:
///
/// 1. **Earlier prefix wins.** `prefixes` is expected in the same order
///    `crate::BREW_PREFIXES` uses (Apple Silicon before Intel) precisely so
///    a native binary is preferred over a Rosetta one. A later prefix must
///    never overwrite an earlier one.
/// 2. **Versioned path beats the `mysql` alias**, within the *same* prefix:
///    `mysql` is an alias that moves the day brew upgrades the current
///    formula, while `mysql@8.4` keeps pointing at 8.4.
///
/// Preference 1 takes precedence over preference 2, for the identical
/// reason `discover_php_in` documents: a stale alias path is cosmetic
/// (discovery reruns on every rescan), but running the wrong architecture is
/// not.
///
/// The probe closure receives the `mysqld` path and returns the version
/// string, exactly like `discover_php_in`'s probe does for `php-fpm`.
/// Production code supplies a bounded `mysqld --version` probe (mirroring
/// `openvhost_conf::probe_php_fpm_version`); tests supply a tempdir-backed
/// fake — this crate spawns no process here. It is consulted only when
/// [`version_of`]'s keg-path read cannot answer. A version that is not
/// `major.minor` shaped (see `MysqlMajor::from_probe`) is treated the same as
/// no answer.
///
/// Returns a [`Discovery`], not a bare `Vec`: a candidate whose version cannot
/// be established is reported as UNIDENTIFIED rather than dropped, so an empty
/// `runtimes` still means "nothing is installed" and never "I could not tell".
///
/// Private: [`discover_mysql`] is the entry point, and it reads both sources.
fn discover_brew(
    prefixes: &[&Path],
    probe: &dyn Fn(&Path) -> Option<String>,
) -> Discovery<MysqlRuntime> {
    // Track which prefix (by index into `prefixes`) produced each entry so
    // the alias override below can check "same prefix" before firing.
    let mut found: Vec<(usize, MysqlRuntime)> = Vec::new();
    let mut unidentified: Vec<PathBuf> = Vec::new();

    for (prefix_idx, prefix) in prefixes.iter().enumerate() {
        let opt = prefix.join("opt");
        let Ok(entries) = std::fs::read_dir(&opt) else {
            continue; // a prefix that is not installed is not an error
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(is_mysql_formula))
            .map(|e| e.path())
            .collect();
        candidates.sort();

        for dir in candidates {
            let mysqld = dir.join(MYSQLD_REL);
            let mysql = dir.join(MYSQL_REL);
            let mysqladmin = dir.join(MYSQLADMIN_REL);
            if !mysqld.is_file() || !mysql.is_file() || !mysqladmin.is_file() {
                continue; // all three or the runtime isn't listed
            }
            let Some(major) = version_of(&dir, &mysqld, probe).and_then(MysqlMajor::from_probe)
            else {
                // Binaries present, version unreadable. NOT the same as
                // "no MySQL here" — see `Discovery`.
                unidentified.push(dir);
                continue;
            };
            match found.iter_mut().find(|(_, r)| r.major == major) {
                // Already known. Only apply the alias→versioned override
                // when this candidate comes from the same prefix as the
                // existing entry — a later prefix must never overwrite an
                // earlier one, aliased or not.
                Some((existing_prefix_idx, existing)) => {
                    if *existing_prefix_idx != prefix_idx {
                        continue;
                    }
                    let existing_is_alias = existing
                        .mysqld
                        .parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.file_name())
                        .is_some_and(|n| n == "mysql");
                    if existing_is_alias {
                        existing.mysqld = mysqld;
                        existing.mysql = mysql;
                        existing.mysqladmin = mysqladmin;
                    }
                }
                None => found.push((
                    prefix_idx,
                    MysqlRuntime {
                        major,
                        mysqld,
                        mysql,
                        mysqladmin,
                        source: MysqlRuntimeSource::Homebrew,
                    },
                )),
            }
        }
    }

    let mut runtimes: Vec<MysqlRuntime> = found.into_iter().map(|(_, runtime)| runtime).collect();
    runtimes.sort_by(|a, b| a.major.cmp(&b.major));
    Discovery {
        runtimes,
        unidentified,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    const ALL_THREE: [&str; 3] = ["mysqld", "mysql", "mysqladmin"];

    /// Build a fake brew prefix: `opt/<formula>/bin/{<binaries>}` for each
    /// entry, mapping the created `mysqld` path (whether or not it was
    /// actually written — see `binaries`) to the version the probe should
    /// report. Omitting a name from `binaries` is how tests prove the
    /// all-three-required rule.
    fn fake_prefix(
        formulae: &[(&str, &str)],
        binaries: &[&str],
    ) -> (tempfile::TempDir, BTreeMap<PathBuf, String>) {
        let dir = tempfile::tempdir().unwrap();
        let mut versions = BTreeMap::new();
        for (formula, version) in formulae {
            let bin_dir = dir.path().join("opt").join(formula).join("bin");
            std::fs::create_dir_all(&bin_dir).unwrap();
            for name in binaries {
                std::fs::write(bin_dir.join(name), b"#!/bin/sh\n").unwrap();
            }
            versions.insert(bin_dir.join("mysqld"), (*version).to_string());
        }
        (dir, versions)
    }

    fn probe_from(map: BTreeMap<PathBuf, String>) -> impl Fn(&Path) -> Option<String> {
        move |p: &Path| map.get(p).cloned()
    }

    /// A packages root on a home that does not exist — for the tests that are
    /// only about the Homebrew walk. A missing tree must read as "no packaged
    /// runtimes", never as an error; `no_package_tree_at_all_is_not_an_error`
    /// below is the test that pins that, and every Homebrew test leans on it.
    fn no_packages() -> PackagesRoot {
        PackagesRoot::from_home(Path::new("/nonexistent/openvhost-home"))
    }

    /// Lay down `packages/mysql/<major>/<version>/bin/{binaries}` with
    /// `mysqld`'s body set to `body`, so a test can tell one version's binary
    /// from another's by CONTENT rather than by the path it was reached
    /// through — which is the whole point of the `current`-swap proof below.
    fn install_fake_package(
        root: &PackagesRoot,
        major: &str,
        version: &str,
        body: &str,
        binaries: &[&str],
    ) {
        let bin = root
            .package_dir(MYSQL_PACKAGE_NAME, major, version)
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in binaries {
            let content = if *name == "mysqld" {
                body
            } else {
                "#!/bin/sh\n"
            };
            std::fs::write(bin.join(name), content.as_bytes()).unwrap();
        }
    }

    /// Point (or re-point) `packages/mysql/<major>/current` at `target`,
    /// exactly as `openvhost-pkg` does: a RELATIVE symlink whose target is the
    /// bare version string.
    #[cfg(unix)]
    fn point_current(root: &PackagesRoot, major: &str, target: &str) {
        let link = root.current_link(MYSQL_PACKAGE_NAME, major);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(PathBuf::from(target), &link).unwrap();
    }

    /// A home with `packages/mysql/8.4/8.4.11/` installed and selected.
    #[cfg(unix)]
    fn packaged_8_4_11() -> (tempfile::TempDir, PackagesRoot) {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.11", "8.4.11 server\n", &ALL_THREE);
        point_current(&root, "8.4", "8.4.11");
        (home, root)
    }

    #[test]
    fn finds_a_versioned_formula() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1);
        assert_eq!(found.runtimes[0].major.as_str(), "8.4");
        assert!(
            found.runtimes[0]
                .mysqld
                .ends_with("opt/mysql@8.4/bin/mysqld")
        );
        assert!(found.runtimes[0].mysql.ends_with("opt/mysql@8.4/bin/mysql"));
        assert!(
            found.runtimes[0]
                .mysqladmin
                .ends_with("opt/mysql@8.4/bin/mysqladmin")
        );
    }

    #[test]
    fn the_unversioned_alias_does_not_double_count_its_own_version() {
        let (dir, versions) = fake_prefix(&[("mysql", "8.4"), ("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major.as_str(), "8.4");
        assert!(
            found.runtimes[0]
                .mysqld
                .to_string_lossy()
                .contains("mysql@8.4"),
            "the versioned path should win: {:?}",
            found.runtimes[0].mysqld
        );
    }

    #[test]
    fn several_versions_come_back_sorted_and_distinct() {
        // The catalogue is ["8.4"] only, but discovery must still merge/sort
        // any shape-valid major (spec D1's out-of-catalogue rows) — two of
        // these three are deliberately out of catalogue.
        let (dir, versions) = fake_prefix(
            &[
                ("mysql@9.7", "9.7"),
                ("mysql@8.4", "8.4"),
                ("mysql@8.0", "8.0"),
            ],
            &ALL_THREE,
        );
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.0", "8.4", "9.7"]);
    }

    #[test]
    fn a_prefix_that_does_not_exist_is_not_an_error() {
        let found = discover_mysql(
            &no_packages(),
            &[Path::new("/nonexistent/openvhost-prefix")],
            &|_| None,
        );
        assert!(found.runtimes.is_empty());
        // Nothing was seen at all, so nothing is outstanding.
        assert!(found.is_complete());
    }

    #[test]
    fn a_formula_whose_version_no_source_can_answer_is_reported_unidentified() {
        // THE R2 collapse, on the MySQL side where it was reproduced: the three
        // binaries are right there, and a killed `mysqld --version` probe used
        // to make the whole install read as "not detected". Excluded from
        // `runtimes` still — nothing may be started on a version we cannot
        // name — but no longer indistinguishable from an empty machine.
        //
        // VACUITY: replacing `unidentified.push(dir)` with a bare `continue`
        // makes the second assertion fail while the first still passes.
        let (dir, _) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&no_packages(), &[dir.path()], &|_| None);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert_eq!(found.unidentified, vec![dir.path().join("opt/mysql@8.4")]);
        assert!(!found.is_complete());
    }

    /// A probe that fails the test if it is ever called — used wherever the
    /// version must come from a path (Homebrew's keg link, or our own version
    /// directory) rather than from executing a binary.
    fn no_probe(_: &Path) -> Option<String> {
        panic!("the version probe must not be consulted when a path states the version");
    }

    /// A real brew layout: `Cellar/<owner>/<version>/bin/{mysqld,mysql,
    /// mysqladmin}` with `opt/<formula>` symlinked at the keg through brew's
    /// own RELATIVE target.
    #[cfg(unix)]
    fn brew_prefix(entries: &[(&str, &str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (formula, owner, version) in entries {
            let keg = dir.path().join("Cellar").join(owner).join(version);
            std::fs::create_dir_all(keg.join("bin")).unwrap();
            for name in ALL_THREE {
                std::fs::write(keg.join("bin").join(name), b"#!/bin/sh\n").unwrap();
            }
            let opt = dir.path().join("opt");
            std::fs::create_dir_all(&opt).unwrap();
            std::os::unix::fs::symlink(
                PathBuf::from("..").join("Cellar").join(owner).join(version),
                opt.join(formula),
            )
            .unwrap();
        }
        dir
    }

    #[cfg(unix)]
    #[test]
    fn a_real_brew_layout_is_identified_without_spawning_the_probe() {
        // The measured failure: the first execution of a freshly extracted
        // 55 MB `mysqld` took 11.53 s under Gatekeeper's scan, against a 5 s
        // probe bound that group-kills the child — so every retry restarted a
        // scan that could never finish inside the bound. The keg path states
        // the version and costs a readlink.
        //
        // VACUITY: replacing `version_of`'s body with a bare `probe(bin)`
        // makes this panic inside `no_probe`.
        let dir = brew_prefix(&[("mysql@8.4", "mysql@8.4", "8.4.11")]);
        let found = discover_mysql(&no_packages(), &[dir.path()], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major.as_str(), "8.4");
        assert!(found.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn a_keg_whose_name_is_not_a_version_falls_back_to_the_probe() {
        let dir = brew_prefix(&[("mysql@8.4", "mysql@8.4", "HEAD-abc1234")]);
        let found = discover_mysql(&no_packages(), &[dir.path()], &|_| Some("8.4".to_string()));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major.as_str(), "8.4");
    }

    #[test]
    fn missing_mysqladmin_means_the_runtime_is_not_listed() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &["mysqld", "mysql"]);
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        assert!(found.runtimes.is_empty(), "got {found:?}");
    }

    #[test]
    fn missing_mysql_client_means_the_runtime_is_not_listed() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &["mysqld", "mysqladmin"]);
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        assert!(found.runtimes.is_empty(), "got {found:?}");
    }

    #[test]
    fn missing_mysqld_means_the_runtime_is_not_listed() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &["mysql", "mysqladmin"]);
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        assert!(found.runtimes.is_empty(), "got {found:?}");
    }

    #[test]
    fn an_earlier_prefix_wins_over_a_later_one() {
        let (a, va) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let (b, vb) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let mut merged = va.clone();
        merged.extend(vb);
        let found = discover_mysql(&no_packages(), &[a.path(), b.path()], &probe_from(merged));
        assert_eq!(found.runtimes.len(), 1);
        assert!(found.runtimes[0].mysqld.starts_with(a.path()));
    }

    #[test]
    fn a_later_prefix_never_replaces_an_earlier_one_even_with_a_versioned_path() {
        let (silicon, v1) = fake_prefix(&[("mysql", "8.4")], &ALL_THREE);
        let (intel, v2) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let mut merged = v1;
        merged.extend(v2);

        let found = discover_mysql(
            &no_packages(),
            &[silicon.path(), intel.path()],
            &probe_from(merged),
        );
        assert_eq!(found.runtimes.len(), 1);
        assert!(
            found.runtimes[0].mysqld.starts_with(silicon.path()),
            "a later prefix replaced an earlier one: {:?}",
            found.runtimes[0].mysqld
        );
    }

    #[test]
    fn within_one_prefix_the_versioned_path_still_beats_the_alias() {
        let (dir, versions) = fake_prefix(&[("mysql", "8.4"), ("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1);
        assert!(
            found.runtimes[0]
                .mysqld
                .to_string_lossy()
                .contains("mysql@8.4"),
            "the versioned path should still win inside one prefix: {:?}",
            found.runtimes[0].mysqld
        );
    }

    #[test]
    fn an_out_of_catalogue_major_is_still_discovered_and_not_cataloged() {
        // Spec D1: a user's 9.x renders as a row without an Install button —
        // discovery must not silently drop it.
        let (dir, versions) = fake_prefix(&[("mysql@9.7", "9.7")], &ALL_THREE);
        let found = discover_mysql(&no_packages(), &[dir.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1);
        assert_eq!(found.runtimes[0].major.as_str(), "9.7");
        assert!(!found.runtimes[0].major.is_cataloged());
    }

    // ---- the install path's path-only resolver ----------------------------

    #[test]
    fn a_major_we_just_installed_is_found_by_path_with_no_probe_at_all() {
        let (dir, _) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let rt = brew_mysql_runtime_for_major(&[dir.path()], &MysqlMajor::parse("8.4").unwrap())
            .expect("the formula directory brew just created must be found");
        assert_eq!(rt.major.as_str(), "8.4");
        assert!(rt.mysqld.ends_with("opt/mysql@8.4/bin/mysqld"));
        assert!(rt.mysql.ends_with("opt/mysql@8.4/bin/mysql"));
        assert!(rt.mysqladmin.ends_with("opt/mysql@8.4/bin/mysqladmin"));
    }

    #[test]
    fn a_major_that_was_not_installed_is_simply_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            brew_mysql_runtime_for_major(&[dir.path()], &MysqlMajor::parse("8.4").unwrap())
                .is_none()
        );
    }

    #[test]
    fn a_partial_formula_directory_is_not_a_runtime_here_either() {
        // The all-three rule is not `discover_mysql`'s alone: an install that
        // left `mysqld` without `mysqladmin` cannot support this app's
        // lifecycle, and reporting it as installed would put a row on the page
        // that can never shut down cleanly.
        let (dir, _) = fake_prefix(&[("mysql@8.4", "8.4")], &["mysqld", "mysql"]);
        assert!(
            brew_mysql_runtime_for_major(&[dir.path()], &MysqlMajor::parse("8.4").unwrap())
                .is_none()
        );
    }

    #[test]
    fn the_unversioned_alias_directory_never_answers_for_a_versioned_major() {
        let (dir, _) = fake_prefix(&[("mysql", "8.4")], &ALL_THREE);
        assert!(
            brew_mysql_runtime_for_major(&[dir.path()], &MysqlMajor::parse("8.4").unwrap())
                .is_none()
        );
    }

    // ------------------------------------------------------------------
    // Group P1 — our own package tree is read, and it wins (design D3).
    //
    // VACUITY, measured by mutation: replacing `discover_packaged(packages)`
    // in `discover_mysql` with `Discovery::default()` — discovery never reads
    // our own tree — fails 11 of this module's 34 tests, including all three
    // packaged-source tests in this group. (`no_package_tree_at_all_...` keeps
    // passing, which is right: it is the brew-only control.)
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_is_found_through_the_current_link() {
        let (_home, root) = packaged_8_4_11();
        let found = discover_mysql(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major.as_str(), "8.4");
        assert!(found.is_complete());
        // No probe was consulted: `no_probe` panics if it is. The version is a
        // directory name we chose at install time (design D4), so nothing here
        // has to execute a 55 MB `mysqld` to find out what it is.
    }

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_beats_a_homebrew_one_for_the_same_major() {
        let (_home, root) = packaged_8_4_11();
        let (brew, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);

        let found = discover_mysql(&root, &[brew.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        let rt = &found.runtimes[0];
        assert_eq!(rt.major.as_str(), "8.4");
        assert!(
            rt.mysqld.starts_with(root.as_path()),
            "the packaged runtime must win: {:?}",
            rt.mysqld
        );
        assert_eq!(
            rt.source,
            MysqlRuntimeSource::Packaged {
                version: "8.4.11".to_string()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_brew_only_major_is_still_found_alongside_a_packaged_one() {
        // D3's whole point: the owner is running a brew MySQL right now, and
        // adopting our own tree must not strand anything they already have.
        let (_home, root) = packaged_8_4_11();
        let (brew, versions) = fake_prefix(&[("mysql@9.7", "9.7")], &ALL_THREE);

        let found = discover_mysql(&root, &[brew.path()], &probe_from(versions));
        let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.4", "9.7"], "got {found:?}");
        assert_eq!(
            found.runtimes[1].source,
            MysqlRuntimeSource::Homebrew,
            "a brew-only major must still be found, and say so"
        );
    }

    #[test]
    fn no_package_tree_at_all_is_not_an_error() {
        // An Intel Mac gets `NoPackageForTarget` from the catalogue by design,
        // so it will never have a packaged runtime. That must read as an honest
        // absence — brew alone — not as a failure.
        let (brew, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&no_packages(), &[brew.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].source, MysqlRuntimeSource::Homebrew);
        assert!(found.is_complete(), "got {:?}", found.unidentified);
    }

    // ------------------------------------------------------------------
    // Group P2 — every runtime reports its source.
    //
    // VACUITY, measured by mutation: making `packaged_mysql_runtime` hand back
    // `MysqlRuntimeSource::Homebrew` — a packaged runtime that lies about
    // where it came from — fails 3 tests here and in P1, plus
    // `stack::tests::startup_discovery_finds_a_packaged_mysql_with_no_homebrew_at_all`
    // in the desktop crate.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_reports_its_exact_version_and_a_homebrew_one_reports_none() {
        let (_home, root) = packaged_8_4_11();
        let (brew, versions) = fake_prefix(&[("mysql@9.7", "9.7")], &ALL_THREE);
        let found = discover_mysql(&root, &[brew.path()], &probe_from(versions));

        let ours = &found.runtimes[0];
        assert_eq!(ours.source.version(), Some("8.4.11"));
        assert_ne!(
            ours.source.version(),
            Some(ours.major.as_str()),
            "the packaged source must report the FULL version, not the major"
        );

        let theirs = &found.runtimes[1];
        assert_eq!(
            theirs.source.version(),
            None,
            "brew's exact version is not known without probing — say so rather than \
             passing the major off as it"
        );
    }

    #[test]
    fn the_two_sources_have_distinct_stable_spellings() {
        // Asserted PAIRWISE, not for non-emptiness: two sources that render
        // identically are worse than no label at all, because the UI would look
        // like it answered.
        let packaged = MysqlRuntimeSource::Packaged {
            version: "8.4.11".to_string(),
        };
        assert_eq!(packaged.as_str(), "packaged");
        assert_eq!(MysqlRuntimeSource::Homebrew.as_str(), "homebrew");
        assert_ne!(packaged.as_str(), MysqlRuntimeSource::Homebrew.as_str());
        assert_ne!(packaged, MysqlRuntimeSource::Homebrew);
    }

    // ------------------------------------------------------------------
    // Group P3 — design D5: a concrete version directory, never `current`.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn the_discovered_paths_are_a_concrete_version_directory_and_never_the_current_link() {
        let (_home, root) = packaged_8_4_11();
        let found = discover_mysql(&root, &[], &no_probe);
        let rt = &found.runtimes[0];
        for p in [&rt.mysqld, &rt.mysql, &rt.mysqladmin] {
            assert!(
                p.starts_with(root.package_dir(MYSQL_PACKAGE_NAME, "8.4", "8.4.11")),
                "{p:?} is not inside the concrete version directory"
            );
            assert!(
                !p.components().any(|c| c.as_os_str() == "current"),
                "{p:?} runs through the current link"
            );
        }
    }

    /// THE assertion that pins D5. A path that merely *looks* concrete would
    /// pass the test above against a `current` link that happens to resolve —
    /// this one swaps the link underneath and demands the already-handed-out
    /// path still reach the binary it named.
    ///
    /// Spawning through the link would mean a `current` swap silently changed
    /// which engine a restart brings up: the running process and the one the UI
    /// describes would diverge with nothing in between to notice.
    ///
    /// VACUITY, proven by mutation and not by inspection: rewriting
    /// `packaged_mysql_runtime` to build its paths from
    /// `root.current_link(...)` instead of `root.package_dir(...)` — the exact
    /// mistake D5 forbids — fails this test, `the_discovered_paths_…` above and
    /// `the_packaged_resolver_answers_…` below, plus the desktop crate's
    /// `a_packaged_mysql_spec_spawns_a_concrete_version_and_survives_a_current_swap`.
    /// Re-running with the shape assertions in those two neutered leaves THIS
    /// one still failing, `left: "8.4.10 server\n"` — so the swap, not the
    /// shape, is what pins D5.
    #[cfg(unix)]
    #[test]
    fn a_current_swap_does_not_change_a_path_discovery_already_handed_out() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.10", "8.4.10 server\n", &ALL_THREE);
        install_fake_package(&root, "8.4", "8.4.11", "8.4.11 server\n", &ALL_THREE);
        point_current(&root, "8.4", "8.4.11");

        let found = discover_mysql(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        let mysqld = found.runtimes[0].mysqld.clone();
        assert_eq!(std::fs::read(&mysqld).unwrap(), b"8.4.11 server\n");

        // A `current` swap is a legitimate operation (a future upgrade flow
        // does exactly this). It must not reach back and change what an
        // already-resolved path names.
        point_current(&root, "8.4", "8.4.10");
        assert_eq!(
            std::fs::read(&mysqld).unwrap(),
            b"8.4.11 server\n",
            "a current swap changed the binary an already-resolved path reaches"
        );

        // ...and a fresh discovery does follow the swap, which is what makes
        // the assertion above a statement about D5 rather than about a broken
        // symlink.
        let after = discover_mysql(&root, &[], &no_probe);
        assert_eq!(
            std::fs::read(&after.runtimes[0].mysqld).unwrap(),
            b"8.4.10 server\n"
        );
    }

    // ------------------------------------------------------------------
    // Group P4 — a broken tree answers honestly instead of panicking or
    // lying. Each of these used to be an unhandled shape.
    //
    // VACUITY, measured by mutation: replacing the
    // `None if looks_like_a_broken_install(..)` arm with a bare `None => {}` —
    // a broken tree silently dropped — fails exactly 4 tests: the two
    // "unidentified" ones here, the missing-binary one, and the escape one.
    // `an_entirely_empty_major_directory_is_reported_as_nothing_at_all` keeps
    // passing, which is the point: it is the non-vacuity twin, and the group
    // pins the whole rule rather than its convenient half.
    // ------------------------------------------------------------------

    #[test]
    fn a_major_directory_with_no_current_link_is_reported_unidentified_not_missing() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.11", "8.4.11 server\n", &ALL_THREE);
        // No `current`: nothing selects a version, and INVENTING a selection
        // here would silently paper over an install whose link swap failed.

        let found = discover_mysql(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert_eq!(
            found.unidentified,
            vec![root.major_dir(MYSQL_PACKAGE_NAME, "8.4")]
        );
        assert!(!found.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn a_current_link_pointing_at_a_vanished_version_is_reported_unidentified() {
        let (home, root) = packaged_8_4_11();
        std::fs::remove_dir_all(root.package_dir(MYSQL_PACKAGE_NAME, "8.4", "8.4.11")).unwrap();

        let found = discover_mysql(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert_eq!(
            found.unidentified,
            vec![root.major_dir(MYSQL_PACKAGE_NAME, "8.4")]
        );
        assert!(!found.is_complete());
        drop(home);
    }

    #[test]
    fn an_entirely_empty_major_directory_is_reported_as_nothing_at_all() {
        // The non-vacuity twin of the two above: removing the last version of a
        // major legitimately leaves an empty directory behind, and flagging
        // that forever would make `is_complete()` a permanent false.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        std::fs::create_dir_all(root.major_dir(MYSQL_PACKAGE_NAME, "8.4")).unwrap();

        let found = discover_mysql(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(found.unidentified.is_empty(), "got {found:?}");
        assert!(found.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn a_tampered_current_link_is_refused_and_reported() {
        // The `current` link is the one value in this walk a hand-edit can
        // point anywhere, and it is joined onto a path. Every shape that is not
        // a plain version directory name must be refused BEFORE that join.
        //
        // Each case plants a REAL tree at the destination — three working
        // binaries — so an unguarded join would genuinely resolve and hand back
        // a runtime filed under 8.4 whose binaries are somebody else's.
        // Pointing `current` at a path that does not exist would pass against
        // no guard at all, because `runtime_in`'s `is_file` checks would refuse
        // it anyway. That includes the `..` case: `packages/mysql/bin/` is
        // planted below precisely so `…/8.4/../bin/mysqld` resolves.
        //
        // VACUITY, re-measured after the audit corrected an over-claim here.
        // The previous note said the two guards — `current_version`'s
        // single-`Component::Normal` rule and `packaged_mysql_runtime`'s
        // structural parent check — were "each INDEPENDENTLY sufficient". That
        // is true only of the first three targets:
        //
        //   * the three multi-component/absolute targets: removing EITHER guard
        //     alone leaves this test green; removing BOTH fails it, reporting an
        //     8.4 runtime whose `mysqld` is `…/mysql/8.4/../8.0/8.0.40/bin/mysqld`
        //     and whose recorded version is the literal `"../8.0/8.0.40"`.
        //   * a bare `..`: only the single-component rule refuses it. Its
        //     LEXICAL parent (`…/mysql/8.4/..`.parent() == `…/mysql/8.4`) IS this
        //     major's directory, so the structural check passes it — remove the
        //     single-component rule and this case alone fails, handing back an
        //     8.4 runtime rooted one level up, at `packages/mysql/`, with the
        //     literal version `".."`.
        //
        // So they are belt and braces, not interchangeable, and the order
        // matters. This is the measurement, not an assumption.
        let outside = tempfile::tempdir().unwrap();
        let decoy = outside.path().join("decoy");
        std::fs::create_dir_all(decoy.join("bin")).unwrap();
        for name in ALL_THREE {
            std::fs::write(decoy.join("bin").join(name), b"decoy\n").unwrap();
        }

        let tampered = [
            // A sibling major's real version directory, reached with `..`.
            "../8.0/8.0.40".to_string(),
            "8.4.11/../../8.0/8.0.40".to_string(),
            // Straight out of the home entirely, absolute.
            decoy.display().to_string(),
            // Not an escape from the tree, but not a version directory either:
            // it names the package root, one level above every version.
            "..".to_string(),
        ];
        for target in tampered {
            let home = tempfile::tempdir().unwrap();
            let root = PackagesRoot::from_home(home.path());
            install_fake_package(&root, "8.4", "8.4.11", "8.4.11 server\n", &ALL_THREE);
            install_fake_package(&root, "8.0", "8.0.40", "8.0.40 server\n", &ALL_THREE);
            // What a bare `..` would reach: `packages/mysql/bin/`. Ignored by
            // the walk itself (`bin` is not `major.minor`-shaped), so planting
            // it creates no runtime of its own.
            //
            // It is not an arbitrary prop: `bin/ lib/ share/` directly under
            // the package root is exactly what an accidental one-level-too-high
            // extraction of the real MySQL tarball leaves behind, because that
            // is the tarball's own root. So the `..` case models a plausible
            // no-attacker state, the way the decoy above does for the absolute
            // case — not a shape only someone who could already plant anything
            // would produce.
            let sibling_bin = root.as_path().join(MYSQL_PACKAGE_NAME).join("bin");
            std::fs::create_dir_all(&sibling_bin).unwrap();
            for name in ALL_THREE {
                std::fs::write(sibling_bin.join(name), b"sibling\n").unwrap();
            }
            point_current(&root, "8.4", &target);

            let found = discover_mysql(&root, &[], &no_probe);
            assert!(
                !found.runtimes.iter().any(|rt| rt.major.as_str() == "8.4"),
                "current -> {target:?} produced an 8.4 runtime: {found:?}"
            );
            // ...and the refusal is REPORTED, not silently swallowed: a
            // tampered link is precisely the state a user must be told about.
            assert!(
                found
                    .unidentified
                    .contains(&root.major_dir(MYSQL_PACKAGE_NAME, "8.4")),
                "current -> {target:?} was refused but not reported: {found:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_current_link_spelled_with_a_leading_dot_segment_is_refused() {
        // `./8.4.11` resolves, on any filesystem, to exactly the directory
        // `8.4.11` next to the link — so this is the one shape that is
        // harmless AND rejected, and it is here because the reason it is
        // rejected was documented wrongly.
        //
        // std normalises `.` away everywhere except at the START of a path, so
        // `Path::new("./8.4.11").components()` is `[CurDir, Normal("8.4.11")]`,
        // not the single `Normal` the old comment claimed. The behaviour was
        // always right; only the explanation was wrong, and an unpinned
        // behaviour explained wrongly is how a refactor "fixes" a guard in the
        // permissive direction.
        //
        // Pinning the strictness is deliberate: `openvhost-pkg` writes the bare
        // version and nothing else, so widening the accepted set buys nothing
        // and costs the single-component rule its meaning.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.11", "8.4.11 server\n", &ALL_THREE);
        point_current(&root, "8.4", "./8.4.11");

        let found = discover_mysql(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(
            found
                .unidentified
                .contains(&root.major_dir(MYSQL_PACKAGE_NAME, "8.4")),
            "got {found:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_packaged_version_missing_one_of_the_three_binaries_is_not_a_runtime() {
        // Same all-three rule the Homebrew walk applies, for the same reason:
        // no `mysqladmin` means no clean shutdown, so the row could never stop
        // properly. Reported unidentified rather than silently skipped — a
        // broken keg is somebody else's install; a broken `packages/mysql/8.4/`
        // is ours.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(
            &root,
            "8.4",
            "8.4.11",
            "8.4.11 server\n",
            &["mysqld", "mysql"],
        );
        point_current(&root, "8.4", "8.4.11");

        let found = discover_mysql(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(!found.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_in_the_package_tree_that_is_not_a_major_is_ignored_entirely() {
        // `.staging` lives under `packages/`, not under `packages/mysql/`, but
        // anything non-`major.minor` here is by definition not something this
        // app wrote — so it is not an install we failed to identify.
        let (home, root) = packaged_8_4_11();
        std::fs::create_dir_all(root.as_path().join(MYSQL_PACKAGE_NAME).join("scratch")).unwrap();

        let found = discover_mysql(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert!(found.is_complete(), "got {:?}", found.unidentified);
        drop(home);
    }

    // ------------------------------------------------------------------
    // Group P5 — the single-major packaged resolver (the seed a post-install
    // rescan uses, mirroring `brew_mysql_runtime_for_major`).
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn the_packaged_resolver_answers_for_the_major_it_was_asked_about() {
        let (_home, root) = packaged_8_4_11();
        let rt = packaged_mysql_runtime(&root, &MysqlMajor::parse("8.4").unwrap())
            .expect("the version directory `current` selects is right there");
        assert_eq!(
            rt.source,
            MysqlRuntimeSource::Packaged {
                version: "8.4.11".to_string()
            }
        );
        assert!(rt.mysqld.ends_with("packages/mysql/8.4/8.4.11/bin/mysqld"));
    }

    #[test]
    fn the_packaged_resolver_is_absent_for_a_major_that_was_never_installed() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        assert!(packaged_mysql_runtime(&root, &MysqlMajor::parse("8.4").unwrap()).is_none());
    }
}
