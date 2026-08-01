// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the PHP runtimes installed on this machine.
//!
//! Never resolves anything through `PATH`: a ServBay install shadows
//! `php-fpm` there, which is why the existing probe code walks known prefixes
//! instead. The same rule applies here.

use std::path::{Path, PathBuf};

use super::PhpMajor;
use crate::discovery::Discovery;
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

/// The `major.minor` a candidate formula directory provides.
///
/// **Homebrew's own keg path first, the version probe only as a fallback.**
/// `<prefix>/opt/php@8.4` is a symlink into `<prefix>/Cellar/php@8.4/8.4.13`,
/// and that path states the version — reading it costs a `readlink`, where the
/// probe costs a process launch that macOS can stall for ~11.5 s on a freshly
/// installed binary while Gatekeeper scans it. `openvhost_conf::PROBE_TIMEOUT`
/// kills the probe at 5 s, so on that path the probe answers `None` for a
/// version that is plainly installed; see [`crate::keg`] for the measurement.
///
/// The probe is what remains for anything the keg path cannot answer: a prefix
/// that is not a brew layout at all, a keg directory whose name is not a
/// version (`HEAD`), or a formula reached some other way. When BOTH decline,
/// the candidate is [`Discovery::unidentified`] — never silently absent.
fn version_of(dir: &Path, bin: &Path, probe: &dyn Fn(&Path) -> Option<String>) -> Option<String> {
    crate::keg::resolve_keg(dir)
        .and_then(|keg| keg.major_minor())
        .or_else(|| probe(bin))
}

/// The runtime a CATALOGUE major's own formula directory provides, located by
/// path alone: no process is spawned, and no version is parsed out of anything.
///
/// This exists for exactly one caller — the code path that has just run
/// `brew install php@<major>` ITSELF. We asked brew for that formula, so
/// `<prefix>/opt/php@<major>/sbin/php-fpm` is our own request echoed back by
/// brew, not an unknown binary whose claims have to be checked. Interrogating
/// it afterwards is what made a successful install report "not detected".
///
/// `None` means the formula directory is not there — a genuine "brew did not
/// leave this behind", with no third state hiding inside it, which is what lets
/// the install command keep answering with a plain boolean.
pub fn php_runtime_for_major(prefixes: &[&Path], major: &PhpMajor) -> Option<PhpRuntime> {
    let formula = super::brew_formula(major);
    prefixes
        .iter()
        .map(|prefix| prefix.join("opt").join(&formula).join(FPM_REL))
        .find(|bin| bin.is_file())
        .map(|fpm_bin| PhpRuntime {
            major: major.as_str().to_string(),
            fpm_bin,
        })
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
///
/// Returns a [`Discovery`], not a bare `Vec`: a candidate whose version cannot
/// be established is reported as UNIDENTIFIED rather than dropped, so an empty
/// `runtimes` still means "nothing is installed" and never "I could not tell".
pub fn discover_php_in(
    prefixes: &[&Path],
    probe: &dyn Fn(&Path) -> Option<String>,
) -> Discovery<PhpRuntime> {
    // Track which prefix (by index into `prefixes`) produced each entry so
    // the alias override below can check "same prefix" before firing.
    let mut found: Vec<(usize, PhpRuntime)> = Vec::new();
    let mut unidentified: Vec<PathBuf> = Vec::new();

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
            let Some(major) = version_of(&dir, &bin, probe) else {
                // Binaries present, version unreadable. NOT the same as
                // "no PHP here" — see `Discovery`.
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

    let mut runtimes: Vec<PhpRuntime> = found.into_iter().map(|(_, runtime)| runtime).collect();
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

    /// A probe that fails the test if it is ever called — the instrument for
    /// "this answer came from the keg path, not from a process launch".
    fn no_probe(_: &Path) -> Option<String> {
        panic!("the version probe must not be consulted when the keg path answers");
    }

    /// A real brew layout: `Cellar/<owner>/<version>/sbin/php-fpm` with
    /// `opt/<formula>` symlinked at the keg through a RELATIVE target, exactly
    /// as brew writes it.
    #[cfg(unix)]
    fn brew_prefix(entries: &[(&str, &str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (formula, owner, version) in entries {
            let keg = dir.path().join("Cellar").join(owner).join(version);
            std::fs::create_dir_all(keg.join("sbin")).unwrap();
            std::fs::write(keg.join("sbin/php-fpm"), b"#!/bin/sh\n").unwrap();
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

    #[test]
    fn finds_a_versioned_formula() {
        let (dir, versions) = fake_prefix(&[("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1);
        assert_eq!(found.runtimes[0].major, "8.3");
        assert!(
            found.runtimes[0]
                .fpm_bin
                .ends_with("opt/php@8.3/sbin/php-fpm")
        );
        assert!(found.is_complete());
    }

    // ---- the version comes from brew's keg path, not from a process --------
    //
    // VACUITY (neuter-and-watch-it-fail): replacing `version_of`'s body with a
    // bare `probe(bin)` makes both tests below panic inside `no_probe`, which
    // is the instrument firing. Re-adding the keg lookup makes them pass.

    #[cfg(unix)]
    #[test]
    fn a_real_brew_layout_is_identified_without_spawning_the_probe() {
        // THE R2 fix. `mysqld`/`php-fpm` freshly extracted by brew carry
        // `com.apple.provenance`, and their FIRST execution stalls ~11.5 s
        // under Gatekeeper's scan — past the probe's 5 s bound, forever, since
        // every retry restarts a scan that is killed before it finishes. The
        // keg path already states the version.
        let dir = brew_prefix(&[("php@8.4", "php@8.4", "8.4.13")]);
        let found = discover_php_in(&[dir.path()], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major, "8.4");
    }

    #[cfg(unix)]
    #[test]
    fn an_aliased_versioned_formula_still_reports_its_own_major() {
        // `opt/php@8.5 -> ../Cellar/php/8.5.9` — this machine's actual shape.
        // The keg directory name carries the version even though the OWNER is
        // the unversioned formula, so discovery is right about the version.
        // (Uninstalling it is a separate question, refused by
        // `keg_provenance`.)
        let dir = brew_prefix(&[("php@8.5", "php", "8.5.9")]);
        let found = discover_php_in(&[dir.path()], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major, "8.5");
    }

    #[cfg(unix)]
    #[test]
    fn a_keg_whose_name_is_not_a_version_falls_back_to_the_probe() {
        // `--HEAD` builds land in `Cellar/php/HEAD-abc1234`. The keg path
        // cannot answer, so the probe still has a job.
        let dir = brew_prefix(&[("php@8.4", "php@8.4", "HEAD-abc1234")]);
        let found = discover_php_in(&[dir.path()], &|_| Some("8.4".to_string()));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major, "8.4");
    }

    #[test]
    fn the_unversioned_alias_does_not_double_count_its_own_version() {
        // On a real machine /opt/homebrew/opt/php and /opt/homebrew/opt/php@8.5
        // both resolve to the same Cellar directory — the unversioned formula
        // is an alias for the current one. Two entries would mean two service
        // rows and two pools listening on two sockets for one binary.
        let (dir, versions) = fake_prefix(&[("php", "8.5"), ("php@8.5", "8.5")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major, "8.5");
        // The versioned path is the stable one: `php` moves when brew upgrades it.
        assert!(
            found.runtimes[0]
                .fpm_bin
                .to_string_lossy()
                .contains("php@8.5"),
            "the versioned path should win: {:?}",
            found.runtimes[0].fpm_bin
        );
    }

    #[test]
    fn several_versions_come_back_sorted_and_distinct() {
        let (dir, versions) =
            fake_prefix(&[("php@8.4", "8.4"), ("php@8.1", "8.1"), ("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.1", "8.3", "8.4"]);
    }

    #[test]
    fn a_prefix_that_does_not_exist_is_not_an_error() {
        let found = discover_php_in(&[Path::new("/nonexistent/openvhost-prefix")], &|_| None);
        assert!(found.runtimes.is_empty());
        // Nothing was seen at all, so nothing is outstanding: this is a
        // genuine "nothing installed", distinct from the case below.
        assert!(found.is_complete());
    }

    #[test]
    fn a_formula_whose_version_no_source_can_answer_is_reported_unidentified() {
        // Was `..._is_skipped`, and the rename is the point. Silently dropping
        // this candidate is what let a killed version probe read as "nothing
        // is installed": the binaries are RIGHT THERE. It is still excluded
        // from `runtimes` — nothing may be started on a version we cannot
        // name — but the caller can now tell the two apart.
        //
        // VACUITY: replacing the `unidentified.push(dir)` with a bare
        // `continue` makes the second assertion fail while the first still
        // passes, which is exactly the collapse this test exists to catch.
        let (dir, _) = fake_prefix(&[("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &|_| None);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert_eq!(found.unidentified, vec![dir.path().join("opt/php@8.3")]);
        assert!(!found.is_complete());
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
        assert_eq!(found.runtimes.len(), 1);
        assert!(found.runtimes[0].fpm_bin.starts_with(a.path()));
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
        assert_eq!(found.runtimes.len(), 1);
        assert!(
            found.runtimes[0].fpm_bin.starts_with(silicon.path()),
            "a later prefix replaced an earlier one: {:?}",
            found.runtimes[0].fpm_bin
        );
    }

    #[test]
    fn within_one_prefix_the_versioned_path_still_beats_the_alias() {
        // The other preference must survive the fix: inside a single prefix,
        // `php@8.5` is the stable path and `php` is the alias that moves.
        let (dir, versions) = fake_prefix(&[("php", "8.5"), ("php@8.5", "8.5")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1);
        assert!(
            found.runtimes[0]
                .fpm_bin
                .to_string_lossy()
                .contains("php@8.5"),
            "the versioned path should still win inside one prefix: {:?}",
            found.runtimes[0].fpm_bin
        );
    }

    // ---- the install path's path-only resolver ----------------------------

    #[test]
    fn a_major_we_just_installed_is_found_by_path_with_no_probe_at_all() {
        // VACUITY: this asserts a POSITIVE result, so it cannot pass against a
        // stub returning `None`; the sibling test below is the negative side.
        let (dir, _) = fake_prefix(&[("php@8.3", "8.3")]);
        let rt = php_runtime_for_major(&[dir.path()], &PhpMajor::parse("8.3").unwrap())
            .expect("the formula directory brew just created must be found");
        assert_eq!(rt.major, "8.3");
        assert!(rt.fpm_bin.ends_with("opt/php@8.3/sbin/php-fpm"));
    }

    #[test]
    fn a_major_that_was_not_installed_is_simply_absent() {
        let (dir, _) = fake_prefix(&[("php@8.3", "8.3")]);
        assert!(
            php_runtime_for_major(&[dir.path()], &PhpMajor::parse("8.4").unwrap()).is_none(),
            "a formula directory that is not there must not be reported"
        );
    }

    #[test]
    fn the_unversioned_alias_directory_never_answers_for_a_versioned_major() {
        // `opt/php` may well BE 8.4, but this resolver answers only for the
        // formula the install command actually asked brew for. Answering from
        // the alias would report "installed" for a major whose own formula
        // brew never created.
        let (dir, _) = fake_prefix(&[("php", "8.4")]);
        assert!(php_runtime_for_major(&[dir.path()], &PhpMajor::parse("8.4").unwrap()).is_none());
    }

    #[test]
    fn an_earlier_prefix_wins_for_the_installed_major_too() {
        let (a, _) = fake_prefix(&[("php@8.3", "8.3")]);
        let (b, _) = fake_prefix(&[("php@8.3", "8.3")]);
        let rt =
            php_runtime_for_major(&[a.path(), b.path()], &PhpMajor::parse("8.3").unwrap()).unwrap();
        assert!(rt.fpm_bin.starts_with(a.path()));
    }
}
