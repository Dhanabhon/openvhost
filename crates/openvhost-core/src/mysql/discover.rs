// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the MySQL runtimes installed on this machine.
//!
//! Never resolves anything through `PATH` — same rule as
//! `crate::php::discover`, for the same reason (a ServBay install shadows
//! binaries there).

use std::path::{Path, PathBuf};

use super::MysqlMajor;

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

/// One discovered MySQL installation: a [`MysqlMajor`] plus the three
/// binaries this app drives directly. All three are guaranteed to exist as
/// files — [`discover_mysql`] never returns a partial runtime.
///
/// `major` can be a value [`MysqlMajor::is_cataloged`] reports `false` for:
/// a discovered installation this build does not offer to INSTALL is still
/// discovered and listed (spec D1 — "a user's 9.x renders as a row without
/// an Install button"). See [`MysqlMajor`]'s doc comment for how its two
/// constructors divide that responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlRuntime {
    pub major: MysqlMajor,
    pub mysqld: PathBuf,
    pub mysql: PathBuf,
    pub mysqladmin: PathBuf,
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
/// fake — this crate spawns no process here. A probed string that is not
/// `major.minor` shaped (see `MysqlMajor::from_probe`) is treated the same
/// as no match: skipped.
pub fn discover_mysql(
    prefixes: &[&Path],
    probe: &dyn Fn(&Path) -> Option<String>,
) -> Vec<MysqlRuntime> {
    // Track which prefix (by index into `prefixes`) produced each entry so
    // the alias override below can check "same prefix" before firing.
    let mut found: Vec<(usize, MysqlRuntime)> = Vec::new();

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
            let Some(major) = probe(&mysqld).and_then(MysqlMajor::from_probe) else {
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
                    },
                )),
            }
        }
    }

    let mut found: Vec<MysqlRuntime> = found.into_iter().map(|(_, runtime)| runtime).collect();
    found.sort_by(|a, b| a.major.cmp(&b.major));
    found
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

    #[test]
    fn finds_a_versioned_formula() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].major.as_str(), "8.4");
        assert!(found[0].mysqld.ends_with("opt/mysql@8.4/bin/mysqld"));
        assert!(found[0].mysql.ends_with("opt/mysql@8.4/bin/mysql"));
        assert!(
            found[0]
                .mysqladmin
                .ends_with("opt/mysql@8.4/bin/mysqladmin")
        );
    }

    #[test]
    fn the_unversioned_alias_does_not_double_count_its_own_version() {
        let (dir, versions) = fake_prefix(&[("mysql", "8.4"), ("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].major.as_str(), "8.4");
        assert!(
            found[0].mysqld.to_string_lossy().contains("mysql@8.4"),
            "the versioned path should win: {:?}",
            found[0].mysqld
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
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        let majors: Vec<&str> = found.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.0", "8.4", "9.7"]);
    }

    #[test]
    fn a_prefix_that_does_not_exist_is_not_an_error() {
        let found = discover_mysql(&[Path::new("/nonexistent/openvhost-prefix")], &|_| None);
        assert!(found.is_empty());
    }

    #[test]
    fn a_formula_whose_probe_does_not_recognize_it_is_skipped() {
        let (dir, _) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&[dir.path()], &|_| None);
        assert!(found.is_empty(), "got {found:?}");
    }

    #[test]
    fn missing_mysqladmin_means_the_runtime_is_not_listed() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &["mysqld", "mysql"]);
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        assert!(found.is_empty(), "got {found:?}");
    }

    #[test]
    fn missing_mysql_client_means_the_runtime_is_not_listed() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &["mysqld", "mysqladmin"]);
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        assert!(found.is_empty(), "got {found:?}");
    }

    #[test]
    fn missing_mysqld_means_the_runtime_is_not_listed() {
        let (dir, versions) = fake_prefix(&[("mysql@8.4", "8.4")], &["mysql", "mysqladmin"]);
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        assert!(found.is_empty(), "got {found:?}");
    }

    #[test]
    fn an_earlier_prefix_wins_over_a_later_one() {
        let (a, va) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let (b, vb) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let mut merged = va.clone();
        merged.extend(vb);
        let found = discover_mysql(&[a.path(), b.path()], &probe_from(merged));
        assert_eq!(found.len(), 1);
        assert!(found[0].mysqld.starts_with(a.path()));
    }

    #[test]
    fn a_later_prefix_never_replaces_an_earlier_one_even_with_a_versioned_path() {
        let (silicon, v1) = fake_prefix(&[("mysql", "8.4")], &ALL_THREE);
        let (intel, v2) = fake_prefix(&[("mysql@8.4", "8.4")], &ALL_THREE);
        let mut merged = v1;
        merged.extend(v2);

        let found = discover_mysql(&[silicon.path(), intel.path()], &probe_from(merged));
        assert_eq!(found.len(), 1);
        assert!(
            found[0].mysqld.starts_with(silicon.path()),
            "a later prefix replaced an earlier one: {:?}",
            found[0].mysqld
        );
    }

    #[test]
    fn within_one_prefix_the_versioned_path_still_beats_the_alias() {
        let (dir, versions) = fake_prefix(&[("mysql", "8.4"), ("mysql@8.4", "8.4")], &ALL_THREE);
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1);
        assert!(
            found[0].mysqld.to_string_lossy().contains("mysql@8.4"),
            "the versioned path should still win inside one prefix: {:?}",
            found[0].mysqld
        );
    }

    #[test]
    fn an_out_of_catalogue_major_is_still_discovered_and_not_cataloged() {
        // Spec D1: a user's 9.x renders as a row without an Install button —
        // discovery must not silently drop it.
        let (dir, versions) = fake_prefix(&[("mysql@9.7", "9.7")], &ALL_THREE);
        let found = discover_mysql(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].major.as_str(), "9.7");
        assert!(!found[0].major.is_cataloged());
    }
}
