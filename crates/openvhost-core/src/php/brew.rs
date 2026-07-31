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

/// The versions this build offers. Hand-maintained: asking `brew` would mean
/// spawning a process on a path that has to stay cheap, and a stale entry
/// fails loudly at install time rather than silently.
pub const CATALOGUE: [&str; 5] = ["8.1", "8.2", "8.3", "8.4", "8.5"];

/// A PHP `major.minor` this build offers. Parsing enforces the shape;
/// membership of [`CATALOGUE`] enforces the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpMajor(String);

impl PhpMajor {
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        // Layer 1: shape. Digits, one dot, digits — nothing else.
        let mut parts = s.split('.');
        let ok = match (parts.next(), parts.next(), parts.next()) {
            (Some(a), Some(b), None) => {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && b.bytes().all(|c| c.is_ascii_digit())
            }
            _ => false,
        };
        if !ok {
            return Err(CoreError::Validation {
                field: "php_version",
                reason: format!("{s:?} is not a major.minor version"),
            });
        }
        // Layer 2: policy. Shape alone would still let a flag-shaped-but-numeric
        // value, or a version we have never tested, reach `brew install`.
        if !CATALOGUE.contains(&s) {
            return Err(CoreError::Validation {
                field: "php_version",
                reason: format!(
                    "PHP {s} is not offered by this build (offered: {})",
                    CATALOGUE.join(", ")
                ),
            });
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
pub fn brew_uninstall_spec(brew: &Path, major: &PhpMajor) -> Result<SpawnSpec, CoreError> {
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
    fn the_inherited_ambient_path_never_appears_in_the_composed_value() {
        // The composed PATH must come from a fixed baseline, never the
        // process's own ambient PATH: a ServBay install shadows `php-fpm`/
        // `nginx` on that ambient value (see `discover.rs`), and brew's
        // children (git, curl, tar) would inherit the same shadowing if the
        // parent's PATH were appended. Set a PATH with an unmistakable
        // marker directory and assert it never reaches the child's env —
        // restoring the previous value afterwards so this test cannot leak
        // into any other test in the same process.
        const MARKER: &str = "/tmp/openvhost-hostile-shadow-dir-marker";
        let previous = std::env::var_os("PATH");
        // SAFETY: no other thread in this test binary reads/writes PATH
        // concurrently with this single-threaded set/restore pair.
        unsafe {
            std::env::set_var("PATH", format!("{MARKER}:/usr/bin"));
        }

        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        )
        .unwrap();

        // SAFETY: restoring the pre-test value before any assertion can panic
        // and skip it.
        unsafe {
            match &previous {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }

        let path = spec
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .expect("PATH must be set explicitly");
        assert!(
            !path.contains(MARKER),
            "the inherited ambient PATH leaked into the composed value: {path}"
        );
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
