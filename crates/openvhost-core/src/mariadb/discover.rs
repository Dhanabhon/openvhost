// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the MariaDB runtime installed in OpenVHost's own package tree.
//!
//! Its own copy of [`crate::mysql::discover`]'s walk rather than a shared one,
//! because what differs is the *names* — `mariadbd`/`mariadb`/`mariadb-admin`
//! against `mysqld`/`mysql`/`mysqladmin` — while the shape is identical, and a
//! generic walk parameterised by three binary names would be an abstraction
//! over two cases (spec D5).
//!
//! **There is no Homebrew half here, by design.** MariaDB arrives only from
//! the package tree this project builds and pins (the off-Homebrew decision,
//! 2026-08-01); nothing below looks at `/opt/homebrew` and nothing resolves
//! anything through `PATH`, for the same reason `crate::php::discover` does
//! not (a ServBay install shadows binaries there).

use std::path::{Path, PathBuf};

use openvhost_pkg::PackagesRoot;

use super::{MARIADB_PACKAGE_NAME, MARIADB_SERIES};
use crate::discovery::Discovery;

/// A version directory holds a usable runtime only when all three of these
/// exist under it. `mariadbd` is the server this app supervises; `mariadb` and
/// `mariadb-admin` are spawned directly by later steps (root-password set,
/// connection verify, readiness probe, clean shutdown) — a tree missing any
/// one of the three cannot support this app's lifecycle, so it is not listed
/// at all rather than listed with a hole.
///
/// The tarball also ships `mysqld`/`mysql`/`mysqladmin` compatibility aliases.
/// They are deliberately NOT what discovery looks for: naming the MariaDB
/// binaries is what keeps "which engine did I just start" answerable from a
/// process listing alone, and it is what stops a MySQL tree from ever
/// satisfying this check by accident.
const MARIADBD_REL: &str = "bin/mariadbd";
const MARIADB_REL: &str = "bin/mariadb";
const MARIADB_ADMIN_REL: &str = "bin/mariadb-admin";

/// One discovered MariaDB installation: the three binaries this app drives
/// directly, and the exact version they came from. All three are guaranteed to
/// exist as files — a partial runtime is never returned.
///
/// **The paths are always concrete** (spec D5). They name
/// `packages/mariadb/11.4/11.4.9/bin/…`, never
/// `packages/mariadb/11.4/current/bin/…`: a supervised child is spawned from
/// whatever is recorded here, so spawning *through* the link would mean a
/// later `current` swap silently changed which engine a restart brings up,
/// with the running process and the one the UI describes diverging and nothing
/// in between to notice. It also makes the server's argv[0]-derived basedir
/// ambiguous — the exact class of thing that cost a full misdiagnosis in the
/// MySQL lifecycle slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MariadbRuntime {
    /// The series this build ships — always [`MARIADB_SERIES`]. Carried on the
    /// runtime rather than assumed at every call site, so the day a second
    /// series exists the callers already read it from here.
    pub series: &'static str,
    /// The exact release, e.g. `11.4.9`, taken from the version directory's
    /// own name. Recorded at install time, never probed.
    pub version: String,
    pub mariadbd: PathBuf,
    pub mariadb: PathBuf,
    pub mariadb_admin: PathBuf,
}

/// The runtime rooted at `dir`, when all three binaries are really there.
///
/// The single place a [`MariadbRuntime`] is built, so the all-three rule
/// cannot hold at one call site and not another: a directory that cannot
/// support this app's lifecycle (no `mariadb-admin` means no readiness probe
/// and no clean shutdown) is not a runtime, however it was found.
fn runtime_in(dir: &Path, version: String) -> Option<MariadbRuntime> {
    let mariadbd = dir.join(MARIADBD_REL);
    let mariadb = dir.join(MARIADB_REL);
    let mariadb_admin = dir.join(MARIADB_ADMIN_REL);
    if !(mariadbd.is_file() && mariadb.is_file() && mariadb_admin.is_file()) {
        return None;
    }
    Some(MariadbRuntime {
        series: MARIADB_SERIES,
        version,
        mariadbd,
        mariadb,
        mariadb_admin,
    })
}

/// The packaged runtime this series' `current` link selects, or `None` when
/// there is no usable packaged install.
///
/// The one place the `current` link is ever resolved for MariaDB. It reuses
/// `crate::mysql::discover::current_version` where that already lives rather
/// than growing a second copy: the rule it enforces — a link target must be
/// exactly one [`std::path::Component::Normal`], so `..`, an absolute path and
/// any multi-component target are refused — is a property of how
/// `openvhost-pkg` writes the link, not of MySQL, and a security predicate
/// duplicated is a security predicate that will drift. Moving it somewhere
/// neutral is the mechanical follow-up spec D5 defers.
pub fn packaged_mariadb_runtime(root: &PackagesRoot) -> Option<MariadbRuntime> {
    let major_dir = root.major_dir(MARIADB_PACKAGE_NAME, MARIADB_SERIES);
    // Through the layout facade, never `major_dir.join("current")` spelled by
    // hand: the installer swings this link through `PackagesRoot`, and a second
    // spelling here is how the writer and the reader end up naming different
    // files.
    let version =
        crate::mysql::current_version(&root.current_link(MARIADB_PACKAGE_NAME, MARIADB_SERIES))?;
    let dir = root.package_dir(MARIADB_PACKAGE_NAME, MARIADB_SERIES, &version);
    // Belt and braces over `current_version`'s single-component rule, stated
    // structurally so it keeps holding whatever a future `join` does with an
    // unexpected target shape: the directory whose binaries we are about to
    // hand out MUST be a direct child of this series' directory.
    if dir.parent() != Some(major_dir.as_path()) {
        return None;
    }
    runtime_in(&dir, version)
}

/// Every MariaDB runtime on this machine — at most one, since this build ships
/// exactly one series (spec §13.3).
///
/// Returns a [`Discovery`], not an `Option`, for the honesty that type exists
/// to provide: a series directory that holds *something* but yields no runtime
/// is reported through [`Discovery::unidentified`] rather than dropped, so an
/// empty `runtimes` genuinely means "nothing is installed" and never "I could
/// not tell". A broken `packages/mariadb/11.4/` is ours, and saying nothing
/// about it would be a lie the user cannot detect.
///
/// Spawns nothing and probes nothing: the version is a directory name we chose
/// at install time, so this walk is a `read_link` and three `is_file` calls.
pub fn discover_mariadb(root: &PackagesRoot) -> Discovery<MariadbRuntime> {
    let major_dir = root.major_dir(MARIADB_PACKAGE_NAME, MARIADB_SERIES);
    match packaged_mariadb_runtime(root) {
        Some(rt) => Discovery {
            runtimes: vec![rt],
            unidentified: Vec::new(),
        },
        None if crate::mysql::looks_like_a_broken_install(&major_dir) => Discovery {
            runtimes: Vec::new(),
            unidentified: vec![major_dir],
        },
        None => Discovery::default(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ALL_THREE: [&str; 3] = ["mariadbd", "mariadb", "mariadb-admin"];

    /// Lay down `packages/mariadb/11.4/<version>/bin/<names>`. The `mariadbd`
    /// stub carries `body` so a test can tell one version's binary from
    /// another's by reading the path it was handed.
    fn install_fake(root: &PackagesRoot, version: &str, body: &str, names: &[&str]) {
        let bin = root
            .package_dir(MARIADB_PACKAGE_NAME, MARIADB_SERIES, version)
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in names {
            let content = if *name == "mariadbd" {
                body
            } else {
                "#!/bin/sh\n"
            };
            std::fs::write(bin.join(name), content.as_bytes()).unwrap();
        }
    }

    /// Point (or re-point) `packages/mariadb/11.4/current` at `target`,
    /// exactly as `openvhost-pkg` does: a RELATIVE symlink whose target is the
    /// bare version string.
    #[cfg(unix)]
    fn point_current(root: &PackagesRoot, target: &str) {
        let link = root.current_link(MARIADB_PACKAGE_NAME, MARIADB_SERIES);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(PathBuf::from(target), &link).unwrap();
    }

    #[cfg(unix)]
    fn installed(version: &str, body: &str) -> (tempfile::TempDir, PackagesRoot) {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake(&root, version, body, &ALL_THREE);
        point_current(&root, version);
        (home, root)
    }

    // ---- Group 1: all three or nothing ----

    #[cfg(unix)]
    #[test]
    fn all_three_binaries_resolve_to_a_runtime() {
        let (_home, root) = installed("11.4.9", "11.4.9\n");
        let rt = packaged_mariadb_runtime(&root).expect("all three present");
        assert_eq!(rt.series, MARIADB_SERIES);
        assert_eq!(rt.version, "11.4.9");
        assert!(rt.mariadbd.ends_with("11.4.9/bin/mariadbd"), "{rt:?}");
        assert!(rt.mariadb.ends_with("11.4.9/bin/mariadb"), "{rt:?}");
        assert!(
            rt.mariadb_admin.ends_with("11.4.9/bin/mariadb-admin"),
            "{rt:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn each_missing_binary_on_its_own_disqualifies_the_runtime() {
        // Each of the three, individually — a rule that required only
        // `mariadbd` would pass a single-missing-binary test for the other two.
        for missing in ALL_THREE {
            let present: Vec<&str> = ALL_THREE.into_iter().filter(|n| *n != missing).collect();
            let home = tempfile::tempdir().unwrap();
            let root = PackagesRoot::from_home(home.path());
            install_fake(&root, "11.4.9", "11.4.9\n", &present);
            point_current(&root, "11.4.9");
            assert!(
                packaged_mariadb_runtime(&root).is_none(),
                "a tree missing {missing} must not be a runtime"
            );
            // And it is reported rather than silently dropped: the directory
            // exists and is non-empty, so "nothing installed" would be a lie.
            let found = discover_mariadb(&root);
            assert!(found.runtimes.is_empty());
            assert!(
                !found.is_complete(),
                "a broken install of ours must surface as unidentified"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_mysql_compatibility_aliases_do_not_satisfy_the_check() {
        // The tarball ships `mysqld`/`mysql`/`mysqladmin` aliases too. Finding
        // those must not be enough — discovery names the MariaDB binaries.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake(
            &root,
            "11.4.9",
            "11.4.9\n",
            &["mysqld", "mysql", "mysqladmin"],
        );
        point_current(&root, "11.4.9");
        assert!(packaged_mariadb_runtime(&root).is_none());
    }

    #[test]
    fn an_empty_package_tree_is_complete_and_empty() {
        // Vacuity for the unidentified reporting above: "nothing installed"
        // must still be sayable, or `is_complete` would be permanently false.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        let found = discover_mariadb(&root);
        assert!(found.runtimes.is_empty());
        assert!(found.is_complete());
    }

    // ---- Group 2: the resolved path is concrete, never `current` ----

    #[cfg(unix)]
    #[test]
    fn the_resolved_paths_name_a_concrete_version_directory() {
        let (_home, root) = installed("11.4.9", "11.4.9\n");
        let rt = packaged_mariadb_runtime(&root).unwrap();
        for p in [&rt.mariadbd, &rt.mariadb, &rt.mariadb_admin] {
            assert!(
                !p.components().any(|c| c.as_os_str() == "current"),
                "{} still goes through the current link",
                p.display()
            );
            assert!(
                p.components().any(|c| c.as_os_str() == "11.4.9"),
                "{} does not name the version",
                p.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_current_swap_does_not_move_an_already_resolved_path() {
        // The whole point of recording a concrete path. Resolve against
        // 11.4.9, then swing `current` to 11.4.10 underneath — the recorded
        // path must still lead to the ORIGINAL binary, byte for byte. Reading
        // the file through the recorded path is what makes this non-vacuous:
        // had discovery handed out `…/current/bin/mariadbd`, the same read
        // would come back with the new version's bytes.
        let (_home, root) = installed("11.4.9", "the 11.4.9 server\n");
        let rt = packaged_mariadb_runtime(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(&rt.mariadbd).unwrap(),
            "the 11.4.9 server\n"
        );

        install_fake(&root, "11.4.10", "the 11.4.10 server\n", &ALL_THREE);
        point_current(&root, "11.4.10");

        assert_eq!(
            std::fs::read_to_string(&rt.mariadbd).unwrap(),
            "the 11.4.9 server\n",
            "the recorded path followed the current link"
        );
        assert_eq!(rt.version, "11.4.9", "the recorded version moved");

        // And a FRESH resolve does see the swap — otherwise the assertion
        // above could be passing because the swap never took effect.
        let after = packaged_mariadb_runtime(&root).unwrap();
        assert_eq!(after.version, "11.4.10");
        assert_eq!(
            std::fs::read_to_string(&after.mariadbd).unwrap(),
            "the 11.4.10 server\n"
        );
    }

    // ---- Group 3: what the `current` link may name ----

    #[cfg(unix)]
    #[test]
    fn a_missing_current_link_yields_no_runtime() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake(&root, "11.4.9", "11.4.9\n", &ALL_THREE);
        // Installed but never selected.
        assert!(packaged_mariadb_runtime(&root).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_current_link_escaping_the_series_directory_is_refused() {
        // Containment, reused from `crate::mysql::current_version` rather than
        // re-implemented: a target that is anything other than exactly one
        // plain directory name cannot make discovery describe a tree outside
        // the series it was found under.
        let (_home, root) = installed("11.4.9", "11.4.9\n");
        assert!(packaged_mariadb_runtime(&root).is_some(), "baseline");
        for target in ["../../elsewhere", "/etc", "a/b", "..", "./11.4.9"] {
            point_current(&root, target);
            assert!(
                packaged_mariadb_runtime(&root).is_none(),
                "current -> {target:?} must be refused"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_current_link_naming_a_version_that_is_not_installed_yields_nothing() {
        let (_home, root) = installed("11.4.9", "11.4.9\n");
        point_current(&root, "11.4.99");
        assert!(packaged_mariadb_runtime(&root).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_reports_the_one_installed_runtime() {
        let (_home, root) = installed("11.4.9", "11.4.9\n");
        let found = discover_mariadb(&root);
        assert!(found.is_complete());
        assert_eq!(found.runtimes.len(), 1);
        assert_eq!(found.runtimes[0].version, "11.4.9");
    }
}
