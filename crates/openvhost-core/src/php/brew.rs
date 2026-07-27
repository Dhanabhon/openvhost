// SPDX-License-Identifier: GPL-3.0-or-later
//! Homebrew as a PHP source: which versions we offer, where brew lives, and
//! the exact command that installs one.
//!
//! SECURITY: this module composes the argv. A caller supplies a version, never
//! a formula and never a flag. Arguments are passed as a vector rather than
//! through a shell, which stops command injection — but not flag injection, so
//! `PhpMajor::parse` enforces the shape AND membership of [`CATALOGUE`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use openvhost_proc::SpawnSpec;

use super::BREW_PREFIXES;
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

/// The command that installs `major`. Composed here — the formula name is
/// never accepted from a caller.
pub fn brew_install_spec(brew: &Path, major: &PhpMajor) -> SpawnSpec {
    // brew shells out to git, curl and friends. The supervisor's env
    // allow-list forwards the parent's PATH, which for an app launched from
    // Finder is the bare system one — so brew's own prefix is prepended
    // explicitly rather than hoping the launch context had it.
    let brew_bin = brew.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut path = OsString::from(brew_bin);
    if let Some(inherited) = std::env::var_os("PATH") {
        path.push(":");
        path.push(inherited);
    }

    SpawnSpec {
        program: brew.to_path_buf(),
        args: vec![
            OsString::from("install"),
            OsString::from(format!("php@{}", major.as_str())),
        ],
        cwd: None,
        env: vec![
            // Without this, pressing Install can spend five minutes updating
            // Homebrew itself before starting the work the user asked for.
            (
                OsString::from("HOMEBREW_NO_AUTO_UPDATE"),
                OsString::from("1"),
            ),
            (OsString::from("PATH"), path),
        ],
    }
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
        // happily honour. This is the reason the allowlist exists.
        for bad in ["--build-from-source", "--HEAD", "-f", "--cask", "nginx"] {
            assert!(PhpMajor::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_a_well_formed_version_this_build_does_not_offer() {
        // Shape alone is not enough: policy is the second layer.
        assert!(PhpMajor::parse("9.9").is_err());
        assert!(PhpMajor::parse("7.4").is_err());
    }

    #[test]
    fn the_install_command_is_exactly_install_and_the_formula() {
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        );
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
    fn the_install_command_disables_homebrews_own_auto_update() {
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        );
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
        );
        let path = spec
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .expect("PATH must be set explicitly");
        assert!(path.starts_with("/opt/homebrew/bin"), "got {path}");
    }
}
