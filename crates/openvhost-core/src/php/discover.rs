// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the PHP runtimes installed on this machine.
//!
//! Never resolves anything through `PATH`: a ServBay install shadows
//! `php-fpm` there, which is why the existing probe code walks known prefixes
//! instead. The same rule applies here.

use std::path::{Path, PathBuf};

use crate::site::apply::PhpRuntime;

/// Homebrew prefixes, most-likely first: Apple Silicon, then Intel.
pub const BREW_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];

/// A formula directory holds a runtime when this file exists under it.
const FPM_REL: &str = "sbin/php-fpm";

/// Directory entries under `<prefix>/opt` that could be a PHP formula:
/// `php` (the alias for the current version) and `php@<major>`.
fn is_php_formula(name: &str) -> bool {
    name == "php" || name.starts_with("php@")
}

/// Two preferences apply when merging discovered runtimes, and they can
/// disagree:
///
/// 1. **Earlier prefix wins.** `BREW_PREFIXES` is ordered Apple Silicon
///    before Intel precisely so a native binary is preferred over a Rosetta
///    one. A later prefix must never overwrite an earlier one.
/// 2. **Versioned path beats the `php` alias**, within the *same* prefix:
///    `php` is an alias that moves the day brew upgrades the current
///    formula, while `php@8.5` keeps pointing at 8.5.
///
/// Preference 1 takes precedence over preference 2: the alias-vs-versioned
/// override only applies when the incoming candidate comes from the same
/// prefix as the existing entry. A stale alias path is cosmetic (discovery
/// reruns on every rescan), but running the wrong architecture is not.
pub fn discover_php_in(
    prefixes: &[&Path],
    probe: &dyn Fn(&Path) -> Option<String>,
) -> Vec<PhpRuntime> {
    // Track which prefix (by index into `prefixes`) produced each entry so
    // the alias override below can check "same prefix" before firing.
    let mut found: Vec<(usize, PhpRuntime)> = Vec::new();

    for (prefix_idx, prefix) in prefixes.iter().enumerate() {
        let opt = prefix.join("opt");
        let Ok(entries) = std::fs::read_dir(&opt) else {
            continue; // a prefix that is not installed is not an error
        };
        // Sorted so a machine with both `php` and `php@8.5` is deterministic:
        // `php@8.5` sorts after `php`, and the versioned path is preferred
        // below, so ordering here only has to be stable.
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(is_php_formula))
            .map(|e| e.path())
            .collect();
        candidates.sort();

        for dir in candidates {
            let bin = dir.join(FPM_REL);
            if !bin.is_file() {
                continue;
            }
            let Some(major) = probe(&bin) else {
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
                        .fpm_bin
                        .parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.file_name())
                        .is_some_and(|n| n == "php");
                    if existing_is_alias {
                        existing.fpm_bin = bin;
                    }
                }
                None => found.push((
                    prefix_idx,
                    PhpRuntime {
                        major,
                        fpm_bin: bin,
                    },
                )),
            }
        }
    }

    let mut found: Vec<PhpRuntime> = found.into_iter().map(|(_, runtime)| runtime).collect();
    found.sort_by(|a, b| a.major.cmp(&b.major));
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// Build a fake brew prefix: `opt/<formula>/sbin/php-fpm` for each entry,
    /// mapping the created binary path to the version the probe should report.
    fn fake_prefix(formulae: &[(&str, &str)]) -> (tempfile::TempDir, BTreeMap<PathBuf, String>) {
        let dir = tempfile::tempdir().unwrap();
        let mut versions = BTreeMap::new();
        for (formula, version) in formulae {
            let bin = dir.path().join("opt").join(formula).join("sbin/php-fpm");
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
            versions.insert(bin, (*version).to_string());
        }
        (dir, versions)
    }

    fn probe_from(map: BTreeMap<PathBuf, String>) -> impl Fn(&Path) -> Option<String> {
        move |p: &Path| map.get(p).cloned()
    }

    #[test]
    fn finds_a_versioned_formula() {
        let (dir, versions) = fake_prefix(&[("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].major, "8.3");
        assert!(found[0].fpm_bin.ends_with("opt/php@8.3/sbin/php-fpm"));
    }

    #[test]
    fn the_unversioned_alias_does_not_double_count_its_own_version() {
        // On a real machine /opt/homebrew/opt/php and /opt/homebrew/opt/php@8.5
        // both resolve to the same Cellar directory — the unversioned formula
        // is an alias for the current one. Two entries would mean two service
        // rows and two pools listening on two sockets for one binary.
        let (dir, versions) = fake_prefix(&[("php", "8.5"), ("php@8.5", "8.5")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].major, "8.5");
        // The versioned path is the stable one: `php` moves when brew upgrades it.
        assert!(
            found[0].fpm_bin.to_string_lossy().contains("php@8.5"),
            "the versioned path should win: {:?}",
            found[0].fpm_bin
        );
    }

    #[test]
    fn several_versions_come_back_sorted_and_distinct() {
        let (dir, versions) =
            fake_prefix(&[("php@8.4", "8.4"), ("php@8.1", "8.1"), ("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        let majors: Vec<&str> = found.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.1", "8.3", "8.4"]);
    }

    #[test]
    fn a_prefix_that_does_not_exist_is_not_an_error() {
        let found = discover_php_in(&[Path::new("/nonexistent/openvhost-prefix")], &|_| None);
        assert!(found.is_empty());
    }

    #[test]
    fn a_formula_whose_binary_is_not_php_fpm_is_skipped() {
        // The probe is what decides. A directory that looks right but holds
        // something else must not become a runtime.
        let (dir, _) = fake_prefix(&[("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &|_| None);
        assert!(found.is_empty(), "got {found:?}");
    }

    #[test]
    fn an_earlier_prefix_wins_over_a_later_one() {
        // Apple Silicon before Intel: a machine with both must not report the
        // same major twice.
        let (a, va) = fake_prefix(&[("php@8.3", "8.3")]);
        let (b, vb) = fake_prefix(&[("php@8.3", "8.3")]);
        let mut merged = va.clone();
        merged.extend(vb);
        let found = discover_php_in(&[a.path(), b.path()], &probe_from(merged));
        assert_eq!(found.len(), 1);
        assert!(found[0].fpm_bin.starts_with(a.path()));
    }

    #[test]
    fn a_later_prefix_never_replaces_an_earlier_one_even_with_a_versioned_path() {
        // Apple Silicon has only the `php` alias for 8.3; Intel has php@8.3.
        // Preferring the versioned path here would run a Rosetta binary while a
        // native one is installed — the exact thing the prefix order exists to
        // prevent. Path staleness is cosmetic; the wrong architecture is not.
        let (silicon, v1) = fake_prefix(&[("php", "8.3")]);
        let (intel, v2) = fake_prefix(&[("php@8.3", "8.3")]);
        let mut merged = v1;
        merged.extend(v2);

        let found = discover_php_in(&[silicon.path(), intel.path()], &probe_from(merged));
        assert_eq!(found.len(), 1);
        assert!(
            found[0].fpm_bin.starts_with(silicon.path()),
            "a later prefix replaced an earlier one: {:?}",
            found[0].fpm_bin
        );
    }

    #[test]
    fn within_one_prefix_the_versioned_path_still_beats_the_alias() {
        // The other preference must survive the fix: inside a single prefix,
        // `php@8.5` is the stable path and `php` is the alias that moves.
        let (dir, versions) = fake_prefix(&[("php", "8.5"), ("php@8.5", "8.5")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1);
        assert!(
            found[0].fpm_bin.to_string_lossy().contains("php@8.5"),
            "the versioned path should still win inside one prefix: {:?}",
            found[0].fpm_bin
        );
    }
}
