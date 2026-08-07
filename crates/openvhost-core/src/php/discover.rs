// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the PHP runtimes installed on this machine — in OpenVHost's own
//! package tree first, then in Homebrew.
//!
//! Two install sources coexist here **by design** while the project migrates
//! off Homebrew (PHP-discovery design D2, mirroring MySQL's D3/D7 and nginx's
//! D2). [`discover_php`] walks `packages/php/` and keeps the Homebrew walk as
//! a fallback; ours wins per major, because we know our version exactly and
//! brew's we would have to probe. Nothing here uninstalls, relinks or migrates
//! a keg. The Homebrew walk retires in slice 7 of the programme, not here.
//!
//! Every runtime records [`PhpRuntimeSource`] — where its binaries came from —
//! for the same reason [`crate::nginx::NginxRuntimeSource`] and
//! [`crate::mysql::MysqlRuntimeSource`] exist: during a migration "which
//! php-fpm am I actually running" is a question that gets asked, and the
//! honest answer needs a field on the type rather than a guess at the call
//! site.
//!
//! Never resolves anything through `PATH`: a ServBay install shadows
//! `php-fpm` there, which is why the existing probe code walks known prefixes
//! instead. The same rule applies here.

use std::path::{Path, PathBuf};

use openvhost_pkg::PackagesRoot;

use super::{PHP_PACKAGE_NAME, PhpMajor};
use crate::discovery::Discovery;
use crate::site::apply::PhpRuntime;

/// Homebrew prefixes, most-likely first: Apple Silicon, then Intel.
pub const BREW_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];

/// A **Homebrew formula** directory holds a runtime when this file exists
/// under it.
///
/// `sbin`, and only for brew: OpenVHost's own tree puts the identical binary
/// at [`PACKAGED_FPM_REL`] instead. The two layouts are genuinely different
/// and assuming one for the other is a silent "nothing installed" — see that
/// constant.
const FPM_REL: &str = "sbin/php-fpm";

/// The same binary in **OpenVHost's own package tree**, at a different path.
///
/// `bin/php-fpm`, NOT `sbin/php-fpm`. `build/recipes/php.sh` declares
/// `RECIPE_SERVER_BIN="bin/php-fpm"` and `RECIPE_REQUIRED_LAYOUT=(bin
/// modules)` — the artifact contract's own checks execute `$tree/bin/php-fpm`
/// — and [`crate::php::PHP_WARMUP_BINARY`] is `bin/php-fpm` for the same
/// reason. Reusing brew's `sbin` spelling here would make every packaged
/// install read as absent, with no error anywhere to explain it.
const PACKAGED_FPM_REL: &str = "bin/php-fpm";

/// Where a discovered PHP runtime's binaries came from.
///
/// A field on [`PhpRuntime`] rather than something inferred from a path at the
/// call site — the mirror of [`crate::nginx::NginxRuntimeSource`] and
/// [`crate::mysql::MysqlRuntimeSource`], for the identical reason: two install
/// sources coexist by design during the migration, so "which php-fpm am I
/// actually running" is a question a user will ask, and it is only answerable
/// honestly if discovery records the answer at the moment it walks the
/// directory.
///
/// Matched **exhaustively** everywhere — never through a wildcard arm — so a
/// third source (a user-registered runtime, a future Windows package) breaks
/// compilation at every site that has to decide about it instead of silently
/// rendering as one of these two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhpRuntimeSource {
    /// OpenVHost's own package tree: `packages/php/<major>/<version>/`,
    /// fetched from the pinned upstream artifact and SHA-256 verified before
    /// extraction.
    ///
    /// The exact version comes for free (design D1): we asked the catalogue
    /// for it, and the tree records it as a directory name, so nothing has to
    /// execute `php-fpm` to find out.
    ///
    /// The first-execution cost that avoids is real, but **bounded, and much
    /// smaller than this comment first claimed**. Measured on this project's
    /// own 5A artifact (`/opt/openvhost-build/php-8.4.24`, copied to a fresh
    /// inode): first `bin/php-fpm -v` **0.79 s**, every one after **0.01 s** —
    /// comfortably inside `openvhost_conf::PROBE_TIMEOUT`'s 5 s bound, so a
    /// probe here would answer slowly rather than never. The ~11.5 s figure
    /// quoted elsewhere in this file and in [`crate::keg`] is **Homebrew's**
    /// cost, correctly measured there, and it does not transfer to a binary we
    /// extracted ourselves;
    /// `docs/superpowers/plans/2026-08-01-p1-pkg-extractor.md` is where that
    /// was first corrected (~1.9 s once per machine for the notarization
    /// lookup, plus ~750 ms per fresh inode).
    ///
    /// The decision stands on the cheaper argument: the version is already
    /// written down, so executing anything to re-learn it is work with a
    /// failure mode and no answer we did not already have.
    Packaged {
        /// The exact upstream release, e.g. `"8.4.24"` — the version directory
        /// this major's `current` link selects.
        version: String,
    },
    /// A Homebrew keg this app did not install, found under a `brew --prefix`.
    /// Retired once slice 7 of the off-Homebrew programme removes the Homebrew
    /// fallback entirely — not here.
    Homebrew,
}

impl PhpRuntimeSource {
    /// The stable, machine-facing spelling. ONE definition, so a DTO tag, a
    /// log field and a UI label cannot drift into three different words for
    /// the same fact.
    pub fn as_str(&self) -> &'static str {
        match self {
            PhpRuntimeSource::Packaged { .. } => "packaged",
            PhpRuntimeSource::Homebrew => "homebrew",
        }
    }

    /// The exact version, when the source knows it.
    ///
    /// `None` for Homebrew, and deliberately so: brew's full version would
    /// have to be probed, and probing a freshly extracted `php-fpm` under
    /// macOS's first-execution scan is the measurement [`crate::keg`] records.
    /// Reporting the *major* as though it were the full version would be a lie
    /// no caller could detect.
    pub fn version(&self) -> Option<&str> {
        match self {
            PhpRuntimeSource::Packaged { version } => Some(version),
            PhpRuntimeSource::Homebrew => None,
        }
    }
}

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
            // Homebrew by construction: this resolver looks under a brew
            // prefix's `opt/` and nowhere else. The packaged counterpart is
            // `packaged_php_runtime`, and the two are deliberately separate
            // functions with the source in their names — a caller seeding a
            // rescan after an install knows which install it just ran, and a
            // single "find me a runtime" helper would have to guess.
            source: PhpRuntimeSource::Homebrew,
        })
}

/// The packaged runtime this major's `current` link selects, or `None` when
/// the major has no usable packaged install.
///
/// Copies [`crate::mysql::packaged_mysql_runtime`]'s discipline exactly rather
/// than re-deriving it:
///
/// * `current` is resolved through [`PackagesRoot`]'s own facade, never
///   `major_dir.join("current")` spelled by hand — the installer swings this
///   link through that same facade, and a second spelling here is how the
///   writer and the reader end up naming different files;
/// * the link's target is validated by [`crate::mysql::current_version`] (a
///   single plain directory-name component, containing neither `..` nor an
///   absolute path — see that function for the security reasoning);
/// * the resolved version directory is checked to be a DIRECT CHILD of the
///   major directory before its binary is ever handed out, belt-and-braces
///   over the rule above and stated structurally so it keeps holding whatever
///   a future `join` does with an unexpected target shape;
/// * the returned path names the **concrete version directory**, never
///   `current` (design D3) — see [`PhpRuntime`]'s own doc comment.
///
/// Private, and `major` is a `&str` rather than a [`PhpMajor`] on purpose:
/// [`PhpMajor::parse`] is catalogue-gated, and a packaged 8.1 that a later
/// build stopped offering must still be DISCOVERED (it is running the user's
/// sites). The shape rule that keeps a directory name safe to join is applied
/// here as well as in the walk, so the containment holds however this is
/// reached.
fn packaged_php_runtime(root: &PackagesRoot, major: &str) -> Option<PhpRuntime> {
    if !super::brew::is_major_minor_shape(major) {
        return None;
    }
    let major_dir = root.major_dir(PHP_PACKAGE_NAME, major);
    let version = crate::mysql::current_version(&root.current_link(PHP_PACKAGE_NAME, major))?;
    let dir = root.package_dir(PHP_PACKAGE_NAME, major, &version);
    if dir.parent() != Some(major_dir.as_path()) {
        return None;
    }
    let fpm_bin = dir.join(PACKAGED_FPM_REL);
    if !fpm_bin.is_file() {
        return None;
    }
    Some(PhpRuntime {
        major: major.to_string(),
        fpm_bin,
        source: PhpRuntimeSource::Packaged { version },
    })
}

/// Walk `packages/php/` — OpenVHost's own install source.
///
/// Spawns nothing and probes nothing: the version is a directory name we chose
/// at install time (design D1), so this walk is a `read_link` and one
/// `is_file` call per major.
///
/// **Every series, not one hardcoded one** (design D3). `packaged_nginx_runtime`
/// resolves a single series because nginx ships one; multiple PHP majors side
/// by side is this app's headline feature, so this walks the tree.
///
/// A major directory that resolves to no usable runtime but is not empty is
/// reported through [`Discovery::unidentified`] rather than dropped (design
/// D4). That is stricter than the Homebrew walk, which silently skips a
/// partial formula directory, and deliberately so: a broken keg is somebody
/// else's install, a broken `packages/php/8.4/` is ours — a state our own
/// installer can produce.
fn discover_packaged(root: &PackagesRoot) -> Discovery<PhpRuntime> {
    let tree = root.as_path().join(PHP_PACKAGE_NAME);
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
        // Not `major.minor` shaped: never something this app wrote, so it is
        // not an install we failed to identify — skipped entirely rather than
        // reported. (The shape check is also what keeps a surprising directory
        // name out of the major component that `PhpRuntime.major` carries into
        // a service id and a socket filename.)
        if !super::brew::is_major_minor_shape(&name) {
            continue;
        }
        let major_dir = tree.join(&name);
        match packaged_php_runtime(root, &name) {
            Some(rt) => runtimes.push(rt),
            None if crate::mysql::looks_like_a_broken_install(&major_dir) => {
                unidentified.push(major_dir)
            }
            None => {}
        }
    }
    Discovery {
        runtimes,
        unidentified,
    }
}

/// Every PHP runtime on this machine, from BOTH install sources, with
/// OpenVHost's own package tree winning wherever the two provide the same
/// major (design D2).
///
/// Ours wins because we know its version exactly — the tree records it — while
/// brew's would have to be probed. It is one comparison, and it is what buys
/// the migration room to be incremental instead of stranding the user who has
/// a working `brew install php@8.4` today.
///
/// **The packaged pass sits in FRONT of `discover_php_in`, and changes
/// nothing inside it.** Brew's two documented preferences — earlier prefix
/// wins, versioned path beats the `php` alias within one prefix — still govern
/// the brew pass exactly as before; this function only refuses to append a
/// brew runtime for a major we already have.
///
/// The two walks are combined here rather than exposed separately, exactly as
/// [`crate::mysql::discover_mysql`] combines its own: a caller that could see
/// only half the machine is the bug this signature prevents, and since the
/// desktop app's startup and rescan seams moved across, the Homebrew half is
/// private and there is no longer any way to ask for it.
///
/// **Ordering is part of the contract** (design D5), and it is worth stating
/// exactly: the result is sorted by major as a **byte-lexicographic `String`
/// compare**, not a numeric one. While every component is a single digit the
/// two orders coincide and "the first entry is the catch-all's runtime" does
/// name the lowest major; they diverge the moment one is not. With `8.9`,
/// `8.10` and `10.0` installed the order is `["10.0", "8.10", "8.9"]` and the
/// catch-all gets `10.0`. That is not hypothetical housekeeping: the packaged
/// walk deliberately does NOT catalogue-gate what it discovers (a packaged 8.1
/// a later build stopped offering must still be found), so the set of majors
/// reaching this sort is open-ended in a way [`CATALOGUE`](super::CATALOGUE)
/// is not.
///
/// The ordering is left exactly as it is on purpose. Changing it would move
/// both the display order and the catch-all selection, and *which* runtime the
/// catch-all should serve is a separate, pre-existing product question already
/// recorded for an owner decision (design doc §10) — this walk applies the
/// existing rule to a larger set rather than redefining it.
///
/// What packaged-first DOES change is which entry occupies a given major's
/// slot: on a machine that has both sources for a major, that entry is now the
/// packaged one, so the catch-all serves from a runtime we can name. That is
/// intended, and it is the one user-visible behaviour change this walk makes.
///
/// `packages` is minted from a resolved home ([`PackagesRoot::from_home`]),
/// never from user input. `unidentified` carries candidates from both sources,
/// so an empty `runtimes` still means "nothing is installed" and never "I
/// could not tell" — see [`Discovery`].
pub fn discover_php(
    packages: &PackagesRoot,
    prefixes: &[&Path],
    probe: &dyn Fn(&Path) -> Option<String>,
) -> Discovery<PhpRuntime> {
    let mut found = discover_packaged(packages);
    let brew = discover_php_in(prefixes, probe);
    for rt in brew.runtimes {
        if !found.runtimes.iter().any(|ours| ours.major == rt.major) {
            found.runtimes.push(rt);
        }
    }
    found.unidentified.extend(brew.unidentified);
    found.runtimes.sort_by(|a, b| a.major.cmp(&b.major));
    found
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
///
/// **This is the Homebrew HALF of the machine, and it is private.**
/// [`discover_php`] is the entry point that reads both install sources; a
/// caller that used this directly could not see OpenVHost's own package tree.
/// It was `pub` for exactly as long as the desktop app's startup and rescan
/// seams called it (`stack.rs`, `commands.rs`); both moved across to
/// [`discover_php`], so the half-blind walk is now unreachable from outside
/// this module — a compiler guarantee rather than a convention, matching
/// `crate::mysql::discover_brew` and `crate::nginx::brew_nginx_runtime`.
fn discover_php_in(
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
                        source: PhpRuntimeSource::Homebrew,
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

    // ==================================================================
    // OpenVHost's own package tree — fixtures.
    // ==================================================================

    /// A packages root on a home that does not exist — for the tests that are
    /// only about the Homebrew walk. A missing tree must read as "no packaged
    /// PHP", never as an error.
    fn no_packages() -> PackagesRoot {
        PackagesRoot::from_home(Path::new("/nonexistent/openvhost-home"))
    }

    /// Lay down a package tree at `dir` with the php-fpm binary at `rel` and
    /// its body set to `body`, so a test can tell one version's binary from
    /// another's by CONTENT rather than by the path it was reached through —
    /// which is the whole point of the `current`-swap proof below.
    fn lay_down_tree(dir: &Path, rel: &str, body: &str) {
        let bin = dir.join(rel);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, body.as_bytes()).unwrap();
    }

    /// Lay down `packages/php/<major>/<version>/bin/php-fpm` — `bin`, which is
    /// what `build/recipes/php.sh` produces, and deliberately NOT brew's
    /// `sbin`.
    fn install_fake_package(root: &PackagesRoot, major: &str, version: &str, body: &str) {
        lay_down_tree(
            &root.package_dir(PHP_PACKAGE_NAME, major, version),
            PACKAGED_FPM_REL,
            body,
        );
    }

    /// Point (or re-point) `packages/php/<major>/current` at `target`, exactly
    /// as `openvhost-pkg` does: a RELATIVE symlink whose target is the bare
    /// version string.
    #[cfg(unix)]
    fn point_current(root: &PackagesRoot, major: &str, target: &str) {
        let link = root.current_link(PHP_PACKAGE_NAME, major);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(PathBuf::from(target), &link).unwrap();
    }

    /// A home with `packages/php/8.4/8.4.24/` installed and selected.
    #[cfg(unix)]
    fn packaged_8_4_24() -> (tempfile::TempDir, PackagesRoot) {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.24", "8.4.24 fpm\n");
        point_current(&root, "8.4", "8.4.24");
        (home, root)
    }

    // ------------------------------------------------------------------
    // Group P1 — our own package tree is read, and it wins (design D2).
    //
    // VACUITY, measured by mutation: making `discover_packaged` return
    // `Discovery::default()` before it reads anything — discovery never looks
    // at our own tree — fails 19 of this module's 38 tests, including every
    // packaged test in every group below. `no_package_tree_at_all_...`,
    // `with_no_package_tree_the_merged_walk_answers_exactly_as_the_brew_walk_does`
    // and all nine pre-existing Homebrew tests keep passing, which is right:
    // they are the brew-only controls, and spec §8.6 is the claim that they
    // must not move.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_is_found_through_the_current_link() {
        let (_home, root) = packaged_8_4_24();
        let found = discover_php(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major, "8.4");
        assert!(found.is_complete());
        // No probe was consulted: `no_probe` panics if it is. The version is a
        // directory name we chose at install time (design D1), so nothing here
        // has to execute php-fpm to find out what it is.
    }

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_beats_a_homebrew_one_for_the_same_major() {
        // Spec §8.3: packaged wins and brew's entry is DROPPED — not
        // duplicated, not appended. The length assertion is the half that
        // catches an append; the path assertion is the half that catches a
        // merge that kept the wrong one.
        //
        // VACUITY, measured: dropping the `!found.runtimes.iter().any(…)`
        // guard in `discover_php` so every brew runtime is appended fails
        // exactly this test and `the_first_entry_is_the_lowest_major_…` (P5).
        // Both fail on the LENGTH, which is why asserting the contents alone
        // would not have covered the merge at all.
        let (_home, root) = packaged_8_4_24();
        let (brew, versions) = fake_prefix(&[("php@8.4", "8.4")]);

        let found = discover_php(&root, &[brew.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        let rt = &found.runtimes[0];
        assert_eq!(rt.major, "8.4");
        assert!(
            rt.fpm_bin.starts_with(root.as_path()),
            "the packaged runtime must win: {:?}",
            rt.fpm_bin
        );
        assert!(
            !found
                .runtimes
                .iter()
                .any(|r| r.fpm_bin.starts_with(brew.path())),
            "brew's entry for an already-packaged major must be dropped: {found:?}"
        );
        assert_eq!(
            rt.source,
            PhpRuntimeSource::Packaged {
                version: "8.4.24".to_string()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_brew_only_major_is_still_found_alongside_a_packaged_one() {
        // Spec §8.4, and D2's whole point: the owner is running brew PHPs
        // right now, and adopting our own tree must not strand anything they
        // already have.
        let (_home, root) = packaged_8_4_24();
        let (brew, versions) = fake_prefix(&[("php@8.1", "8.1"), ("php@8.5", "8.5")]);

        let found = discover_php(&root, &[brew.path()], &probe_from(versions));
        let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.1", "8.4", "8.5"], "got {found:?}");
        assert_eq!(found.runtimes[0].source, PhpRuntimeSource::Homebrew);
        assert_eq!(found.runtimes[2].source, PhpRuntimeSource::Homebrew);
    }

    #[test]
    fn no_package_tree_at_all_is_not_an_error() {
        // TODAY'S MACHINE, and spec §8.6: the PHP release is still deferred,
        // so every real install falls straight through to the Homebrew walk.
        // That must read as an honest absence — brew alone — not as a failure.
        let (brew, versions) = fake_prefix(&[("php@8.3", "8.3")]);
        let found = discover_php(&no_packages(), &[brew.path()], &probe_from(versions));
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].source, PhpRuntimeSource::Homebrew);
        assert!(found.is_complete(), "got {:?}", found.unidentified);
    }

    #[test]
    fn with_no_package_tree_the_merged_walk_answers_exactly_as_the_brew_walk_does() {
        // Spec §8.6 stated as an EQUALITY rather than as a spot check: on a
        // machine with no package tree, the new entry point and the old one
        // are the same function. A machine shape this walk mishandles cannot
        // hide behind "well, the fields I asserted matched".
        let (brew, versions) = fake_prefix(&[
            ("php", "8.5"),
            ("php@8.5", "8.5"),
            ("php@8.1", "8.1"),
            ("php@8.3", "8.3"),
        ]);
        let probe = probe_from(versions);
        assert_eq!(
            discover_php(&no_packages(), &[brew.path()], &probe),
            discover_php_in(&[brew.path()], &probe)
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_earlier_prefix_still_wins_among_the_brew_entries_of_the_merged_walk() {
        // Spec §8.4: brew's two preferences keep applying WITHIN the brew
        // pass. Apple Silicon has only the `php` alias for 8.3; Intel has
        // php@8.3. Preferring the versioned path here would run a Rosetta
        // binary while a native one is installed. The packaged 8.4 sits
        // alongside and must not perturb that.
        let (_home, root) = packaged_8_4_24();
        let (silicon, v1) = fake_prefix(&[("php", "8.3")]);
        let (intel, v2) = fake_prefix(&[("php@8.3", "8.3")]);
        let mut merged = v1;
        merged.extend(v2);

        let found = discover_php(&root, &[silicon.path(), intel.path()], &probe_from(merged));
        let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.3", "8.4"], "got {found:?}");
        assert!(
            found.runtimes[0].fpm_bin.starts_with(silicon.path()),
            "a later prefix replaced an earlier one: {:?}",
            found.runtimes[0].fpm_bin
        );
    }

    #[cfg(unix)]
    #[test]
    fn within_one_prefix_the_versioned_path_still_beats_the_alias_in_the_merged_walk() {
        // The other brew preference, likewise unperturbed by a packaged major
        // sitting next to it.
        let (_home, root) = packaged_8_4_24();
        let (brew, versions) = fake_prefix(&[("php", "8.5"), ("php@8.5", "8.5")]);

        let found = discover_php(&root, &[brew.path()], &probe_from(versions));
        let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.4", "8.5"], "got {found:?}");
        assert!(
            found.runtimes[1]
                .fpm_bin
                .to_string_lossy()
                .contains("php@8.5"),
            "the versioned path should still win inside one prefix: {:?}",
            found.runtimes[1].fpm_bin
        );
    }

    // ------------------------------------------------------------------
    // Group P2 — every runtime reports its source.
    //
    // VACUITY, measured by mutation: making `packaged_php_runtime` hand back
    // `PhpRuntimeSource::Homebrew` — a packaged runtime that lies about where
    // it came from — fails 4 tests: the first one here, plus
    // `a_packaged_runtime_beats_a_homebrew_one_for_the_same_major` (P1),
    // `resolving_a_packaged_runtime_never_executes_its_php_fpm` (P3) and
    // `the_first_entry_is_the_lowest_major_…` (P5). The second test here keeps
    // passing, and must: it is about the SPELLINGS, which the mutation leaves
    // alone.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_reports_its_exact_version_and_a_homebrew_one_reports_none() {
        let (_home, root) = packaged_8_4_24();
        let (brew, versions) = fake_prefix(&[("php@8.5", "8.5")]);
        let found = discover_php(&root, &[brew.path()], &probe_from(versions));

        let ours = &found.runtimes[0];
        assert_eq!(ours.source.version(), Some("8.4.24"));
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
        // identically are worse than no label at all, because the UI would
        // look like it answered.
        let packaged = PhpRuntimeSource::Packaged {
            version: "8.4.24".to_string(),
        };
        assert_eq!(packaged.as_str(), "packaged");
        assert_eq!(PhpRuntimeSource::Homebrew.as_str(), "homebrew");
        assert_ne!(packaged.as_str(), PhpRuntimeSource::Homebrew.as_str());
        assert_ne!(packaged, PhpRuntimeSource::Homebrew);
    }

    // ------------------------------------------------------------------
    // Group P3 — design D3: a concrete version directory, never `current`;
    // and the packaged arm executes nothing at all.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn the_discovered_path_is_a_concrete_version_directory_and_never_the_current_link() {
        let (_home, root) = packaged_8_4_24();
        let found = discover_php(&root, &[], &no_probe);
        let bin = &found.runtimes[0].fpm_bin;
        assert!(
            bin.starts_with(root.package_dir(PHP_PACKAGE_NAME, "8.4", "8.4.24")),
            "{bin:?} is not inside the concrete version directory"
        );
        assert!(
            !bin.components().any(|c| c.as_os_str() == "current"),
            "{bin:?} runs through the current link"
        );
    }

    /// THE assertion that pins D3. A path that merely *looks* concrete would
    /// pass the test above against a `current` link that happens to resolve —
    /// this one swaps the link underneath and demands the already-handed-out
    /// path still reach the binary it named.
    ///
    /// Spawning through the link would mean a `current` swap silently changed
    /// which php-fpm a restart brings up: the running process and the one the
    /// UI describes would diverge with nothing in between to notice. It is the
    /// class of thing that cost this project a full misdiagnosis in the MySQL
    /// lifecycle slice.
    ///
    /// VACUITY, proven by mutation and not by inspection: rewriting
    /// `packaged_php_runtime` to build its path from `root.current_link(...)`
    /// instead of `root.package_dir(...)` — the exact mistake D3 forbids —
    /// fails exactly two tests, this one and `the_discovered_path_…` above.
    /// The failure here is the assertion below the swap, comparing the bytes
    /// of `8.4.23 fpm\n` (what `current` was swapped TO) against `8.4.24
    /// fpm\n`. That is the point: the other test only inspects the path's
    /// SHAPE, which a `current` link that happens to resolve would satisfy —
    /// this one reads through the path afterwards, so only the swap can pin
    /// D3.
    #[cfg(unix)]
    #[test]
    fn a_current_swap_does_not_change_a_path_discovery_already_handed_out() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.23", "8.4.23 fpm\n");
        install_fake_package(&root, "8.4", "8.4.24", "8.4.24 fpm\n");
        point_current(&root, "8.4", "8.4.24");

        let found = discover_php(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        let fpm = found.runtimes[0].fpm_bin.clone();
        assert_eq!(std::fs::read(&fpm).unwrap(), b"8.4.24 fpm\n");

        // A `current` swap is a legitimate operation (a future upgrade flow
        // does exactly this). It must not reach back and change what an
        // already-resolved path names.
        point_current(&root, "8.4", "8.4.23");
        assert_eq!(
            std::fs::read(&fpm).unwrap(),
            b"8.4.24 fpm\n",
            "a current swap changed the binary an already-resolved path reaches"
        );

        // ...and a fresh discovery — the rescan path, which runs the identical
        // walk again — DOES follow the swap, which is what makes the assertion
        // above a statement about D3 rather than about a broken symlink.
        let after = discover_php(&root, &[], &no_probe);
        assert_eq!(
            std::fs::read(&after.runtimes[0].fpm_bin).unwrap(),
            b"8.4.23 fpm\n"
        );
    }

    /// Spec §8.2 made OBSERVABLE rather than argued from the code: the
    /// packaged php-fpm is a real executable that records having been run, and
    /// the test fails if that record appears.
    ///
    /// Two instruments, because each misses what the other catches:
    ///
    /// * `no_probe` panics if the version-probe closure is consulted at all —
    ///   which is the only seam this crate has for launching a process here;
    /// * the tripwire catches an execution reaching the binary by ANY other
    ///   route, and its own firing is demonstrated at the end of the test, so
    ///   it is not an instrument that could never go off.
    #[cfg(unix)]
    #[test]
    fn resolving_a_packaged_runtime_never_executes_its_php_fpm() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        let sentinel = home.path().join("php-fpm-was-executed");
        install_fake_package(&root, "8.4", "8.4.24", "placeholder\n");
        let script = root
            .package_dir(PHP_PACKAGE_NAME, "8.4", "8.4.24")
            .join(PACKAGED_FPM_REL);
        std::fs::write(
            &script,
            format!("#!/bin/sh\n: > '{}'\n", sentinel.display()).as_bytes(),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        point_current(&root, "8.4", "8.4.24");

        let found = discover_php(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(
            found.runtimes[0].source,
            PhpRuntimeSource::Packaged {
                version: "8.4.24".to_string()
            },
            "the version must have come from the tree"
        );
        assert!(
            !sentinel.exists(),
            "discovery executed the packaged php-fpm; the version must come from \
             the directory name, because a freshly extracted binary's first run \
             stalls past the probe's 5 s bound under Gatekeeper"
        );

        // The instrument is not vacuous: run the same script deliberately and
        // watch the sentinel appear. Without this, a tripwire that could never
        // fire would read exactly like a passing test.
        let status = std::process::Command::new(&script).status().unwrap();
        assert!(status.success());
        assert!(
            sentinel.exists(),
            "the tripwire cannot fire, so the assertion above proved nothing"
        );
    }

    // ------------------------------------------------------------------
    // Group P4 — design D4: a broken tree of OURS answers honestly instead of
    // vanishing. Stricter than the Homebrew walk on purpose: a broken keg is
    // somebody else's install, a broken `packages/php/8.4/` is one our own
    // installer can produce.
    //
    // VACUITY, measured by mutation: replacing the
    // `None if looks_like_a_broken_install(..)` arm with a bare `None => {}`
    // fails exactly 6 tests — every test in this group except one.
    // `an_entirely_empty_major_directory_is_reported_as_nothing_at_all` keeps
    // passing, which is the point: it is the non-vacuity twin, and the group
    // pins the whole rule rather than its convenient half.
    // ------------------------------------------------------------------

    #[test]
    fn a_major_directory_with_no_current_link_is_reported_unidentified_not_missing() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.24", "8.4.24 fpm\n");
        // No `current`: nothing selects a version, and INVENTING a selection
        // here would silently paper over an install whose link swap failed.

        let found = discover_php(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert_eq!(
            found.unidentified,
            vec![root.major_dir(PHP_PACKAGE_NAME, "8.4")]
        );
        assert!(!found.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn a_current_link_pointing_at_a_vanished_version_is_reported_unidentified() {
        let (home, root) = packaged_8_4_24();
        std::fs::remove_dir_all(root.package_dir(PHP_PACKAGE_NAME, "8.4", "8.4.24")).unwrap();

        let found = discover_php(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert_eq!(
            found.unidentified,
            vec![root.major_dir(PHP_PACKAGE_NAME, "8.4")]
        );
        assert!(!found.is_complete());
        drop(home);
    }

    #[test]
    fn an_entirely_empty_major_directory_is_reported_as_nothing_at_all() {
        // The non-vacuity twin of the two above: removing the last version of
        // a major legitimately leaves an empty directory behind, and flagging
        // that forever would make `is_complete()` a permanent false.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        std::fs::create_dir_all(root.major_dir(PHP_PACKAGE_NAME, "8.4")).unwrap();

        let found = discover_php(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(found.unidentified.is_empty(), "got {found:?}");
        assert!(found.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn a_packaged_version_missing_php_fpm_is_reported_unidentified() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        std::fs::create_dir_all(
            root.package_dir(PHP_PACKAGE_NAME, "8.4", "8.4.24")
                .join("modules"),
        )
        .unwrap();
        point_current(&root, "8.4", "8.4.24");

        let found = discover_php(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(!found.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn a_packaged_tree_laid_out_the_brew_way_is_not_a_runtime() {
        // The one-character mistake this whole constant pair exists to catch.
        // Our recipe emits `bin/php-fpm`; brew's kegs carry `sbin/php-fpm`.
        // Reusing brew's spelling for the packaged walk would make every real
        // install read as absent, and — because the tree IS there — the
        // honest answer is "unidentified", not silence.
        //
        // VACUITY, measured: pointing `PACKAGED_FPM_REL` at `sbin/php-fpm`
        // fails exactly this test and nothing else in the crate. That is the
        // whole point of it — every other packaged test would keep passing
        // against the wrong constant, because they all build their fixtures
        // from that same constant.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        lay_down_tree(
            &root.package_dir(PHP_PACKAGE_NAME, "8.4", "8.4.24"),
            FPM_REL,
            "8.4.24 fpm\n",
        );
        point_current(&root, "8.4", "8.4.24");

        let found = discover_php(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert_eq!(
            found.unidentified,
            vec![root.major_dir(PHP_PACKAGE_NAME, "8.4")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_tampered_current_link_is_refused_and_reported() {
        // The `current` link is the one value in this walk a hand-edit can
        // point anywhere, and it is joined onto a path. Every shape that is
        // not a plain version directory name must be refused BEFORE that join.
        //
        // Each case plants a REAL tree at the destination — a working
        // php-fpm — so an unguarded join would genuinely resolve and hand back
        // a runtime filed under 8.4 whose binary is somebody else's. Pointing
        // `current` at a path that does not exist would pass against no guard
        // at all, because the `is_file` check would refuse it anyway. That
        // includes the `..` case: `packages/php/bin/` is planted below
        // precisely so `…/8.4/../bin/php-fpm` resolves.
        //
        // VACUITY, and the two guards are NOT interchangeable — measured,
        // one mutation at a time, against the whole `openvhost-core` lib:
        //
        //   * `packaged_php_runtime`'s structural parent check removed on its
        //     own: 0 failures anywhere. It is belt and braces and nothing
        //     more, BY CONSTRUCTION — while `current_version` returns a single
        //     plain component, `package_dir` can only ever produce a direct
        //     child, so no input reaches this check first. It is here for the
        //     day one of those two functions changes, which is exactly when a
        //     guard nobody can make fire is worth having.
        //   * `current_version`'s single-`Component::Normal` rule removed on
        //     its own: this test fails, and only on the bare `..` target. Its
        //     LEXICAL parent (`…/php/8.4/..`.parent() == `…/php/8.4`) IS this
        //     major's directory, so the structural check waves it through,
        //     handing back an 8.4 runtime rooted one level up at
        //     `packages/php/bin/php-fpm` with the literal version `".."`. The
        //     three multi-component/absolute targets are still refused, by the
        //     structural check.
        //   * BOTH removed: this test fails on the FIRST target instead,
        //     reporting an 8.4 runtime whose php-fpm is
        //     `…/php/8.4/../8.3/8.3.99/bin/php-fpm` and whose recorded version
        //     is the literal `"../8.3/8.3.99"`.
        let outside = tempfile::tempdir().unwrap();
        let decoy = outside.path().join("decoy");
        lay_down_tree(&decoy, PACKAGED_FPM_REL, "decoy\n");

        let tampered = [
            // A sibling major's real version directory, reached with `..`.
            "../8.3/8.3.99".to_string(),
            "8.4.24/../../8.3/8.3.99".to_string(),
            // Straight out of the home entirely, absolute.
            decoy.display().to_string(),
            // Not an escape from the tree, but not a version directory either:
            // it names the package root, one level above every version.
            "..".to_string(),
        ];
        for target in tampered {
            let home = tempfile::tempdir().unwrap();
            let root = PackagesRoot::from_home(home.path());
            install_fake_package(&root, "8.4", "8.4.24", "8.4.24 fpm\n");
            install_fake_package(&root, "8.3", "8.3.99", "8.3.99 fpm\n");
            // What a bare `..` would reach: `packages/php/bin/`. Ignored by
            // the walk itself (`bin` is not `major.minor`-shaped), so planting
            // it creates no runtime of its own. It is not an arbitrary prop:
            // `bin/ lib/ share/` directly under the package root is exactly
            // what a one-level-too-high extraction of the real artifact leaves
            // behind, because that is the tarball's own root.
            lay_down_tree(
                &root.as_path().join(PHP_PACKAGE_NAME),
                PACKAGED_FPM_REL,
                "sibling\n",
            );
            point_current(&root, "8.4", &target);

            let found = discover_php(&root, &[], &no_probe);
            assert!(
                !found.runtimes.iter().any(|rt| rt.major == "8.4"),
                "current -> {target:?} produced an 8.4 runtime: {found:?}"
            );
            // ...and the refusal is REPORTED, not silently swallowed: a
            // tampered link is precisely the state a user must be told about.
            assert!(
                found
                    .unidentified
                    .contains(&root.major_dir(PHP_PACKAGE_NAME, "8.4")),
                "current -> {target:?} was refused but not reported: {found:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_current_link_spelled_with_a_leading_dot_segment_is_refused() {
        // `./8.4.24` resolves, on any filesystem, to exactly the directory
        // `8.4.24` next to the link — so this is the one shape that is
        // harmless AND rejected. std normalises `.` away everywhere except at
        // the START of a path, so `Path::new("./8.4.24").components()` is
        // `[CurDir, Normal("8.4.24")]`, not a single `Normal`.
        //
        // Pinning the strictness is deliberate: `openvhost-pkg` writes the
        // bare version and nothing else, so widening the accepted set buys
        // nothing and costs the single-component rule its meaning.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.4", "8.4.24", "8.4.24 fpm\n");
        point_current(&root, "8.4", "./8.4.24");

        let found = discover_php(&root, &[], &no_probe);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(
            found
                .unidentified
                .contains(&root.major_dir(PHP_PACKAGE_NAME, "8.4")),
            "got {found:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_in_the_package_tree_that_is_not_a_major_is_ignored_entirely() {
        // Anything non-`major.minor` here is by definition not something this
        // app wrote — so it is not an install we failed to identify, and
        // reporting it would make `is_complete()` cry wolf. The same check is
        // what keeps a surprising directory name out of the major component
        // that later becomes a service id (`php-fpm-<major>`) and a socket
        // filename.
        //
        // VACUITY, and the first draft of this test FAILED it: the junk
        // directories were empty, so removing the shape check changed nothing
        // — an empty directory yields no runtime and is not "a broken install"
        // either, so the test passed against no guard at all. Each junk name
        // now carries a COMPLETE, resolvable package, so with the check
        // removed the walk hands back live runtimes whose majors are
        // `php@8.4`, `scratch` and `8.4.24`, and the length assertion fires.
        // Measured: removing the walk's check fails exactly this test.
        // Removing `packaged_php_runtime`'s copy as WELL fails the same one
        // and nothing more — that copy is belt and braces, unreachable while
        // this check stands, and there for whoever gives that private
        // function a second caller.
        let (home, root) = packaged_8_4_24();
        let junk = [
            "scratch", "8", "8.4.24", "php@8.4", "8.x", "..8.4", "8.4.1_1",
        ];
        for name in junk {
            let dir = root.as_path().join(PHP_PACKAGE_NAME).join(name);
            lay_down_tree(&dir.join("9.9.9"), PACKAGED_FPM_REL, "junk\n");
            std::os::unix::fs::symlink(PathBuf::from("9.9.9"), dir.join("current")).unwrap();
        }

        let found = discover_php(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(found.runtimes[0].major, "8.4");
        assert!(found.is_complete(), "got {:?}", found.unidentified);
        drop(home);
    }

    // ------------------------------------------------------------------
    // Group P5 — design D5: ordering is part of the contract.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn the_first_entry_is_the_lowest_major_and_is_the_packaged_one_when_both_have_it() {
        // "The first entry is the catch-all's runtime" is a live property of
        // the returned `Vec`. Packaged-first does not REORDER it — the result
        // is still sorted by major — but on a machine carrying both sources
        // for the lowest major, the entry in that slot is now ours. That is
        // the intended, user-visible change, and it is stated here rather than
        // discovered.
        //
        // "Lowest major" in this test's NAME is true of `8.3` vs `8.5` and of
        // every catalogued pair, because the sort is a byte-lexicographic
        // `String` compare and those agree while the components are single
        // digits. They do not agree in general — `["10.0", "8.10", "8.9"]` is
        // the sorted order — and `discover_php`'s own doc comment states the
        // rule precisely. Deliberately not "fixed" here: the ordering is
        // pre-existing and out of this slice's scope (design doc §10).
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "8.3", "8.3.99", "8.3.99 fpm\n");
        point_current(&root, "8.3", "8.3.99");
        let (brew, versions) = fake_prefix(&[("php@8.3", "8.3"), ("php@8.5", "8.5")]);

        let found = discover_php(&root, &[brew.path()], &probe_from(versions));
        let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.3", "8.5"], "got {found:?}");
        assert_eq!(
            found.runtimes[0].source,
            PhpRuntimeSource::Packaged {
                version: "8.3.99".to_string()
            },
            "the catch-all must serve from the runtime we can name"
        );
    }

    // ------------------------------------------------------------------
    // Group P6 — filesystem semantics.
    // ------------------------------------------------------------------

    /// Whether the volume `dir` lives on folds case, established by probing it
    /// rather than assumed from the target OS: macOS ships case-INSENSITIVE
    /// APFS by default but case-sensitive volumes are a supported choice, and
    /// CI runners differ.
    fn volume_folds_case(dir: &Path) -> bool {
        let probe = dir.join("openvhost-case-probe");
        std::fs::create_dir_all(&probe).unwrap();
        let folded = dir.join("OPENVHOST-CASE-PROBE").is_dir();
        std::fs::remove_dir_all(&probe).unwrap();
        folded
    }

    #[cfg(unix)]
    #[test]
    fn only_the_package_name_component_can_vary_by_case_and_it_follows_the_volume() {
        // The major (`8.4`) and version (`8.4.24`) components are ASCII digits
        // and dots by construction — `is_major_minor_shape` enforces the
        // first, `current_version`'s single-component rule plus the
        // direct-child check bound the second — so case folding CANNOT change
        // which of them is selected. The one case-bearing component in the
        // whole path is the fixed package name we write ourselves, and this
        // pins that it behaves exactly as the volume does: found on a folding
        // volume, absent on a case-sensitive one, with no third behaviour.
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        let miscased = root.as_path().join("PHP").join("8.4");
        lay_down_tree(&miscased.join("8.4.24"), PACKAGED_FPM_REL, "8.4.24 fpm\n");
        std::os::unix::fs::symlink(PathBuf::from("8.4.24"), miscased.join("current")).unwrap();

        let folds = volume_folds_case(home.path());
        let found = discover_php(&root, &[], &no_probe);
        assert_eq!(
            found.runtimes.len(),
            usize::from(folds),
            "case folding is {folds}, got {found:?}"
        );
        if folds {
            // And the RECORDED path spells the package name the way we spell
            // it, not the way the disk does. Benign — it resolves on the very
            // volume that produced it — but worth pinning, because that path
            // is what a supervised child is later spawned from.
            assert!(
                found.runtimes[0]
                    .fpm_bin
                    .starts_with(root.as_path().join(PHP_PACKAGE_NAME)),
                "got {:?}",
                found.runtimes[0].fpm_bin
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_version_directory_defeats_the_direct_child_check() {
        // KNOWN OPEN GAP, shared verbatim with nginx, MySQL and MariaDB, and
        // pinned here rather than fixed (this slice's plan says so
        // explicitly): the direct-child check is LEXICAL — `dir.parent()` —
        // while `is_file()` follows symlinks. A version directory that is
        // itself a symlink therefore passes every guard and hands back a path
        // that looks in-tree and resolves out of it.
        //
        // **The gap is one level wider than this test exercises, and the fix
        // must be specified against the wider case.** The SERIES directory
        // works too, and it defeats the obvious repair. Measured:
        //
        //     packages/php/8.4 -> /elsewhere     (the series dir is the link)
        //     /elsewhere/current -> 9.9.9
        //     => resolves to /elsewhere/9.9.9/bin/php-fpm
        //
        // Canonicalising the version directory and re-checking `parent()`
        // closes the case below and NOT that one: both sides canonicalise into
        // `/elsewhere`, so `canon(dir).parent() == canon(major_dir)` is `true`
        // and the escape survives. The fix that closes both is to **confine
        // the canonicalised path under the canonicalised packages root**
        // (`canon(dir).starts_with(canon(root))`), which answers `false` here.
        //
        // Severity is bounded by who can produce the state: planting either
        // symlink requires write access to `<home>/packages`, which is already
        // the user's own account. Closing it belongs in one place for all four
        // engines rather than four.
        //
        // This test is deliberately worded as an assertion about TODAY'S
        // behaviour. When the gap is closed, it must be rewritten, and that is
        // the intent: a silent change here is what this pins against.
        let outside = tempfile::tempdir().unwrap();
        let real = outside.path().join("elsewhere");
        lay_down_tree(&real, PACKAGED_FPM_REL, "out of tree\n");

        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        let major_dir = root.major_dir(PHP_PACKAGE_NAME, "8.4");
        std::fs::create_dir_all(&major_dir).unwrap();
        std::os::unix::fs::symlink(&real, major_dir.join("8.4.24")).unwrap();
        point_current(&root, "8.4", "8.4.24");

        let found = discover_php(&root, &[], &no_probe);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(
            std::fs::read(&found.runtimes[0].fpm_bin).unwrap(),
            b"out of tree\n",
            "the recorded path resolves outside the package tree — the gap"
        );
        assert!(
            found.runtimes[0].fpm_bin.starts_with(&major_dir),
            "...while looking lexically in-tree, which is why the check misses it"
        );
    }
}
