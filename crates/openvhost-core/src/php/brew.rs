// SPDX-License-Identifier: GPL-3.0-or-later
//! Homebrew as a PHP source: which versions we offer, where brew lives, and
//! the exact command that installs one.
//!
//! SECURITY: a caller supplies a version, never a formula and never a flag —
//! `PhpMajor::parse` enforces the shape AND membership of [`CATALOGUE`], which
//! is what defeats flag injection (argv alone stops command injection but not
//! `--build-from-source`). The argv/env itself is composed by
//! [`crate::brew_cmd::brew_spec`], the one chokepoint for every `brew`
//! invocation in this crate — see that module for the absolute-`brew`-path and
//! composed-`PATH` rationale that used to be duplicated here.

use std::path::{Path, PathBuf};

use openvhost_proc::SpawnSpec;

use super::BREW_PREFIXES;
use crate::brew_cmd::{BrewVerb, brew_spec};
use crate::error::CoreError;
use crate::site::model::PHP_VERSION_MAX_LEN;

/// The versions this build offers. Hand-maintained: asking `brew` would mean
/// spawning a process on a path that has to stay cheap, and a stale entry
/// fails loudly at install time rather than silently.
pub const CATALOGUE: [&str; 5] = ["8.1", "8.2", "8.3", "8.4", "8.5"];

/// The shared "well-formed but not offered by this build" error — used by both
/// [`PhpMajor::parse`]'s layer-2 check and [`cataloged`]'s guard, so the two
/// call sites can never drift to different wording for the identical condition.
/// Mirrors `mysql::brew::not_cataloged_error` exactly.
fn not_cataloged_error(version: &str) -> CoreError {
    CoreError::Validation {
        field: "php_version",
        reason: format!(
            "PHP {version} is not offered by this build (offered: {})",
            CATALOGUE.join(", ")
        ),
    }
}

/// Digits, one dot, digits — nothing else, and at most
/// [`PHP_VERSION_MAX_LEN`] bytes. [`PhpMajor::parse`]'s layer-1 check, lifted
/// out of it so the packaged-tree walk in [`crate::php::discover`] can apply
/// the identical rule to a directory name without a second hand-rolled copy
/// drifting from this one.
///
/// **The length bound, and why it is the same constant.** Digits-and-a-dot
/// alone accepts `8.` followed by 120 more digits, and the walk feeds this
/// predicate a name read off the disk. That name becomes `PhpRuntime.major`,
/// which becomes a service id and a php-fpm socket filename — so the walk's
/// own comment about "keeping a surprising directory name out of the major
/// component" is only fully true with a bound on it.
/// [`crate::site::model::PhpVersion::parse`] already applies exactly this
/// bound to the value arriving from the UI; reusing the constant rather than
/// picking a second number is what keeps the two ingress points from
/// disagreeing about the same field.
///
/// Every downstream consumer was checked to degrade safely without it, so this
/// is hygiene rather than a hole being closed — but an unbounded component in
/// a path we later join and spawn from is not a property worth leaving to the
/// goodwill of every future consumer.
///
/// **A predicate, deliberately not a constructor.** `MysqlMajor` grew a
/// discovery-only `from_probe`, and this module's own [`cataloged`] guard
/// exists because that opened a path to `brew`'s argv that never passed
/// `parse`. Exposing the shape TEST widens nothing: it mints no [`PhpMajor`],
/// so there is still exactly one production constructor and it is still
/// catalogue-gated.
pub(super) fn is_major_minor_shape(s: &str) -> bool {
    if s.len() > PHP_VERSION_MAX_LEN {
        return false;
    }
    let mut parts = s.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(a), Some(b), None) => {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| c.is_ascii_digit())
                && b.bytes().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// A PHP `major.minor` this build offers. Parsing enforces the shape;
/// membership of [`CATALOGUE`] enforces the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpMajor(String);

impl PhpMajor {
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        // Layer 1: shape. Digits, one dot, digits — nothing else.
        if !is_major_minor_shape(s) {
            return Err(CoreError::Validation {
                field: "php_version",
                reason: format!("{s:?} is not a major.minor version"),
            });
        }
        // Layer 2: policy. Shape alone would still let a flag-shaped-but-numeric
        // value, or a version we have never tested, reach `brew install`.
        if !CATALOGUE.contains(&s) {
            return Err(not_cataloged_error(s));
        }
        Ok(Self(s.to_string()))
    }

    /// The ONLY way to obtain an out-of-catalogue `PhpMajor`, and it exists
    /// solely so [`cataloged`]'s guard can be proven to fire.
    ///
    /// `#[cfg(test)]` deliberately: production has exactly one constructor
    /// today, and this must never become a second, more permissive path into a
    /// child process's argv (which is precisely the hole
    /// `MysqlMajor::from_probe` opened on the MySQL side — see that type's doc
    /// comment). Widening this to `pub(crate)` for a discovery path is exactly
    /// the refactor the guard is defending against; if that day comes, the
    /// guard is already there and this test hook can go.
    #[cfg(test)]
    fn out_of_catalogue(s: &str) -> Self {
        Self(s.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this major is one [`CATALOGUE`] offers. Mirrors
    /// `MysqlMajor::is_cataloged`.
    pub fn is_cataloged(&self) -> bool {
        CATALOGUE.contains(&self.0.as_str())
    }
}

/// The catalogue gate both spec builders share.
///
/// Re-checks membership rather than trusting the caller to have obtained
/// `major` via [`PhpMajor::parse`]. Today `parse` IS the only production
/// constructor, so this cannot fire — and that is exactly the argument the
/// MySQL side made before it turned out to be false: `MysqlMajor` grew a
/// discovery-only `from_probe`, `MysqlRuntime.major` is a `pub` field, and an
/// out-of-catalogue major could reach `brew`'s argv without ever passing
/// `parse`. `PhpRuntime.major` is an untyped `String` today, so the symmetry
/// refactor that types it — and adds `PhpMajor::from_probe` alongside — would
/// reopen the identical hole here. Provenance is never assumed; the guarantee
/// is enforced at the boundary that needs it.
fn cataloged(major: &PhpMajor) -> Result<(), CoreError> {
    if major.is_cataloged() {
        Ok(())
    } else {
        Err(not_cataloged_error(major.as_str()))
    }
}

/// Locate `brew` by absolute path. NEVER via `PATH` — the same rule the
/// php-fpm and nginx probes follow, for the same reason.
pub fn find_brew() -> Option<PathBuf> {
    BREW_PREFIXES
        .iter()
        .map(|p| Path::new(p).join("bin/brew"))
        .find(|p| p.is_file())
}

/// The Homebrew formula that provides `major` — THE definition. A formula name
/// is never accepted from a caller, only derived from a catalogue-gated
/// [`PhpMajor`], and the install spec, the uninstall spec and any UI that
/// names the formula to a user all read it from here, so the string shown and
/// the string executed are one expression rather than two that can drift.
pub fn brew_formula(major: &PhpMajor) -> String {
    format!("php@{}", major.as_str())
}

/// The command that installs `major`. Composed via
/// [`crate::brew_cmd::brew_spec`] — see that module for the absolute-`brew`
/// invariant and the composed `PATH` this used to spell out inline.
pub fn brew_install_spec(brew: &Path, major: &PhpMajor) -> Result<SpawnSpec, CoreError> {
    cataloged(major)?;
    brew_spec(brew, BrewVerb::Install, &brew_formula(major))
}

/// The command that REMOVES `major` (package-uninstall design D1: uninstall is
/// `brew uninstall`, mirroring install through the same composer, so the two
/// cannot drift in how they reach `brew`).
///
/// No `--ignore-dependencies` and no `--force`: if brew refuses because another
/// formula depends on this one, that refusal is the caller's to surface
/// verbatim. Nothing here checks that the formula is currently installed —
/// `brew uninstall` on a keg that is already gone fails loudly by itself, which
/// is more honest than this crate second-guessing brew's own state.
///
/// Catalogue-gated by [`cataloged`], like the install spec — see that
/// function for why the "`PhpMajor` has exactly one constructor" argument is
/// not load-bearing enough to rest on.
pub fn brew_uninstall_spec(brew: &Path, major: &PhpMajor) -> Result<SpawnSpec, CoreError> {
    cataloged(major)?;
    brew_spec(brew, BrewVerb::Uninstall, &brew_formula(major))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_version_this_build_offers() {
        assert_eq!(PhpMajor::parse("8.3").unwrap().as_str(), "8.3");
    }

    #[test]
    fn rejects_anything_that_is_not_major_dot_minor() {
        // Shape guard. Every one of these would otherwise become an argv entry.
        for bad in [
            "",
            "8",
            "8.",
            ".3",
            "8.3.1",
            "eight.three",
            " 8.3",
            "8.3 ",
            "8_3",
        ] {
            assert!(PhpMajor::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn the_shape_predicate_bounds_the_length_it_will_accept() {
        // Audit LOW-2. Without the bound, `8.` followed by 120 digits is a
        // "major.minor" as far as this predicate is concerned — and the
        // packaged walk hands it directory names read off the disk, which then
        // become a service id and a socket filename.
        //
        // The boundary is pinned on BOTH sides, because a bound tested only
        // from the rejecting side passes just as well when it is off by one.
        let at_limit = format!("8.{}", "9".repeat(PHP_VERSION_MAX_LEN - 2));
        assert_eq!(at_limit.len(), PHP_VERSION_MAX_LEN);
        assert!(
            is_major_minor_shape(&at_limit),
            "the bound must not reject a value AT the limit: {at_limit:?}"
        );

        let one_over = format!("8.{}", "9".repeat(PHP_VERSION_MAX_LEN - 1));
        assert_eq!(one_over.len(), PHP_VERSION_MAX_LEN + 1);
        assert!(
            !is_major_minor_shape(&one_over),
            "accepted a value one byte over the limit: {one_over:?}"
        );

        // The auditor's own example, well clear of the boundary.
        assert!(!is_major_minor_shape(&format!("8.{}", "1".repeat(120))));
    }

    #[test]
    fn the_length_bound_is_the_same_one_the_ui_ingress_applies() {
        // ONE constant, two entry points. A value this long is refused whether
        // it arrives as a `PhpVersion` from the UI or as a directory name from
        // the packaged walk — if these ever disagree, one of them is admitting
        // a major the other would have refused for the identical field.
        let one_over = format!("8.{}", "9".repeat(PHP_VERSION_MAX_LEN - 1));
        assert!(!is_major_minor_shape(&one_over));
        assert!(crate::site::model::PhpVersion::parse(&one_over).is_err());
    }

    #[test]
    fn rejects_a_flag_even_though_argv_prevents_command_injection() {
        // argv stops `; rm -rf` but NOT `--build-from-source`, which brew would
        // happily honour. None of these contain a `.` separating two digit
        // runs, so the shape check (layer 1) rejects every one of them before
        // `CATALOGUE.contains` (layer 2) is ever consulted — this test would
        // still pass if the catalogue check were deleted. It is the shape
        // check, not the allowlist, that defeats flag injection: a real brew
        // flag starts with `-` and can never match `^\d+\.\d+$`.
        for bad in ["--build-from-source", "--HEAD", "-f", "--cask", "nginx"] {
            assert!(PhpMajor::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_a_well_formed_version_this_build_does_not_offer() {
        // Shape alone is not enough: "9.9" and "7.4" both pass the shape
        // check (digits, one dot, digits), so only the catalogue membership
        // check (layer 2) can reject them. This is the layer the shape check
        // cannot provide — it covers well-formed-but-unsupported versions.
        assert!(PhpMajor::parse("9.9").is_err());
        assert!(PhpMajor::parse("7.4").is_err());
    }

    #[test]
    fn the_install_command_is_exactly_install_and_the_formula() {
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        )
        .unwrap();
        assert_eq!(
            spec.program,
            std::path::PathBuf::from("/opt/homebrew/bin/brew")
        );
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // Pinned exactly. This test fails the moment anyone adds a flag —
        // which is both a security property and a no-surprises property.
        assert_eq!(args, vec!["install".to_string(), "php@8.3".to_string()]);
    }

    #[test]
    fn the_uninstall_command_is_exactly_uninstall_and_the_formula() {
        let spec = brew_uninstall_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        )
        .unwrap();
        assert_eq!(
            spec.program,
            std::path::PathBuf::from("/opt/homebrew/bin/brew")
        );
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // Pinned exactly, like the install spec above. Design D1: NO
        // `--ignore-dependencies`, NO `--force` — brew's refusal is surfaced
        // verbatim, never overridden. This test fails the moment anyone adds
        // one.
        assert_eq!(args, vec!["uninstall".to_string(), "php@8.3".to_string()]);
    }

    #[test]
    fn install_and_uninstall_name_the_same_formula() {
        // The two specs must agree on the target: an uninstall that removed a
        // different formula than the install created would be catastrophic and
        // silent. Both derive it from `formula`, and this pins that they do.
        let major = PhpMajor::parse("8.4").unwrap();
        let brew = std::path::Path::new("/opt/homebrew/bin/brew");
        let installed = brew_install_spec(brew, &major).unwrap();
        let removed = brew_uninstall_spec(brew, &major).unwrap();
        assert_eq!(installed.args[1], removed.args[1]);
        assert_eq!(installed.args[1].to_string_lossy(), "php@8.4");
    }

    #[test]
    fn a_relative_brew_path_is_refused_for_an_uninstall_too() {
        for bad in ["brew", "./brew", "bin/brew", ""] {
            assert!(
                brew_uninstall_spec(std::path::Path::new(bad), &PhpMajor::parse("8.3").unwrap())
                    .is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn is_cataloged_distinguishes_offered_from_merely_shape_valid() {
        assert!(PhpMajor::parse("8.4").unwrap().is_cataloged());
        assert!(!PhpMajor::out_of_catalogue("7.4").is_cataloged());
    }

    #[test]
    fn refuses_to_compose_an_install_spec_for_an_out_of_catalogue_major() {
        // The guard `mysql::brew::cataloged` already carries, carried across.
        // `PhpMajor::parse` is the only PRODUCTION constructor today, so this
        // value can only be built by the `#[cfg(test)]` hook — which is the
        // point: the day a symmetry refactor adds `PhpMajor::from_probe` (as
        // the MySQL side did), an out-of-catalogue formula would otherwise
        // reach `brew`'s argv with nothing in between.
        //
        // VACUITY: deleting `cataloged(major)?;` from `brew_install_spec` makes
        // this test fail with a composed `["install", "php@7.4"]` spec.
        let err = brew_install_spec(
            Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::out_of_catalogue("7.4"),
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Validation {
                    field: "php_version",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn refuses_to_compose_an_uninstall_spec_for_an_out_of_catalogue_major() {
        // The half that matters most: an uninstall names a formula that is
        // about to be DELETED. `php@7.4` here would be a `brew uninstall` of a
        // formula this build never installed and has never tested removing.
        //
        // VACUITY: deleting `cataloged(major)?;` from `brew_uninstall_spec`
        // makes this test fail with a composed `["uninstall", "php@7.4"]`.
        let err = brew_uninstall_spec(
            Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::out_of_catalogue("7.4"),
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Validation {
                    field: "php_version",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn the_catalogue_refusal_is_worded_identically_wherever_it_is_raised() {
        // One `not_cataloged_error`, three call sites (parse's layer 2 and the
        // two spec guards). Pinned so a future edit cannot leave a user reading
        // two different sentences for one condition.
        let from_parse = PhpMajor::parse("7.4").unwrap_err().to_string();
        let from_guard = brew_uninstall_spec(
            Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::out_of_catalogue("7.4"),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(from_parse, from_guard);
    }

    #[test]
    fn the_install_command_disables_homebrews_own_auto_update() {
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        )
        .unwrap();
        let env: Vec<(String, String)> = spec
            .env
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert!(
            env.contains(&("HOMEBREW_NO_AUTO_UPDATE".to_string(), "1".to_string())),
            "got {env:?}"
        );
    }

    #[test]
    fn the_install_command_puts_brews_own_bin_on_path() {
        // The app launched from Finder has a minimal PATH. brew shells out to
        // git and curl, so its own prefix has to be reachable or the install
        // fails only in a bundled build and not in `tauri dev`.
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        )
        .unwrap();
        let path = spec
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .expect("PATH must be set explicitly");
        assert!(path.starts_with("/opt/homebrew/bin"), "got {path}");
    }

    #[test]
    fn a_relative_brew_path_is_refused_rather_than_putting_the_cwd_on_path() {
        // `Path::new("brew").parent()` is Some(""), so composing PATH from it
        // yields ":/usr/bin:/bin" — an empty leading component, which exec
        // resolves as the working directory. brew shells out to git and curl,
        // so that is an execution primitive for anyone who can write a file there.
        for bad in ["brew", "./brew", "bin/brew", ""] {
            let err =
                brew_install_spec(std::path::Path::new(bad), &PhpMajor::parse("8.3").unwrap());
            assert!(err.is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn the_composed_path_is_a_fixed_baseline_and_not_the_processs_ambient_one() {
        // The composed PATH must come from a fixed baseline, never the
        // process's own ambient PATH: a ServBay install shadows `php-fpm`/
        // `nginx` on that ambient value (see `discover.rs`), and brew's
        // children (git, curl, tar) would inherit the same shadowing if the
        // parent's PATH were appended.
        //
        // **Pinned by the whole string, not by planting a marker in the
        // process's own PATH.** `brew_cmd::brew_spec` reads no environment
        // variable at all — the baseline is a literal in that function — so
        // there is nothing an in-process mutation could influence, and the
        // equality below already refuses every way an ambient value could get
        // in: prepended, appended or substituted. A marker check is strictly
        // weaker than this and cost process-global state to run.
        //
        // WHAT WAS HERE BEFORE, because the reason matters more than the
        // diff: this test called `std::env::set_var("PATH", …)` under a
        // SAFETY comment asserting "no other thread in this test binary
        // reads/writes PATH concurrently". **That was false.** The default
        // harness runs tests on many threads; `mysql::brew` carried a
        // byte-identical copy of this test in the same binary; and `setenv(3)`
        // is not thread-safe against *any* concurrent `getenv`, including the
        // ones libc and `std::process::Command` make on behalf of unrelated
        // tests in this crate that spawn children. The exchange was
        // undefined behaviour for a check the assertion below subsumes.
        //
        // Residual, stated rather than papered over: an append made
        // *conditional* on a non-empty ambient PATH would satisfy this
        // equality in a process whose PATH happened to be empty. Closing that
        // would mean varying the environment, which is the hazard just
        // removed — and it cannot arise without an env read appearing in
        // `brew_cmd`, where there is none and the module doc says so.
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        )
        .unwrap();

        let path = spec
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .expect("PATH must be set explicitly");
        assert_eq!(path, "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin");
    }

    #[test]
    fn the_composed_path_never_starts_with_an_empty_or_relative_component() {
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        )
        .unwrap();
        let path = spec
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .expect("PATH must be set explicitly");
        assert!(
            path.starts_with('/'),
            "PATH starts with a non-absolute component: {path}"
        );
        assert!(
            !path.starts_with(':'),
            "PATH has an empty leading component: {path}"
        );
    }
}
