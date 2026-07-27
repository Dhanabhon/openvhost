// SPDX-License-Identifier: GPL-3.0-or-later
//! Homebrew as a PHP source: which versions we offer, where brew lives, and
//! the exact command that installs one.
//!
//! SECURITY: this module composes the argv. A caller supplies a version, never
//! a formula and never a flag. Arguments are passed as a vector rather than
//! through a shell, which stops command injection — but not flag injection, so
//! `PhpMajor::parse` enforces the shape AND membership of [`CATALOGUE`].
//!
//! SECURITY: [`brew_install_spec`] also requires `brew` to be an absolute
//! path. An empty or relative leading component in `PATH` is resolved by
//! `exec` as the current working directory, and brew shells out to `git` and
//! `curl` — so a relative `brew` path would turn the composed `PATH` into a
//! PATH-hijack primitive for anyone who controls a file in the CWD.

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
///
/// INVARIANT: `brew` must be an absolute path with a real parent directory.
/// `Path::new("brew").parent()` is `Some("")`, not `None`, so a naive
/// `unwrap_or_default()` fallback never fires for a relative or bare-filename
/// path — the composed `PATH` would then start with an empty (or `.`)
/// leading component. `exec` resolves an empty/`.` leading `PATH` component
/// as the current working directory, and brew shells out to `git` and
/// `curl`, so that would hand execution of `git`/`curl` to whoever controls
/// a file in the process's CWD. Rejecting non-absolute input here — rather
/// than trusting callers — is what keeps that primitive from ever reaching
/// argv.
pub fn brew_install_spec(brew: &Path, major: &PhpMajor) -> Result<SpawnSpec, CoreError> {
    if !brew.is_absolute() {
        return Err(CoreError::Validation {
            field: "brew_path",
            reason: format!("{} is not an absolute path", brew.display()),
        });
    }
    let brew_bin = match brew.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => {
            return Err(CoreError::Validation {
                field: "brew_path",
                reason: format!("{} has no parent directory", brew.display()),
            });
        }
    };

    // brew shells out to git, curl and friends, resolved through THIS PATH —
    // so it inherits the same rule `discover.rs` documents for php-fpm and
    // nginx: never resolve anything through the process's ambient PATH,
    // because a ServBay install shadows binaries there. Composed from a
    // fixed baseline (brew's own bin, then the standard system dirs) rather
    // than appending the parent's inherited PATH: that inherited value is
    // attacker-influenced environment the app does not control, and brew's
    // own prefix does not ship git/curl/tar — prepending it would not have
    // closed the gap, only hidden it behind "PATH looks populated".
    let mut path = OsString::from(brew_bin);
    path.push(":/usr/bin:/bin:/usr/sbin:/sbin");

    Ok(SpawnSpec {
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
    })
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
