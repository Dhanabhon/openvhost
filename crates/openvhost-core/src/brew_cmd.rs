// SPDX-License-Identifier: GPL-3.0-or-later
//! The ONE place a Homebrew command line is composed.
//!
//! Every `brew` invocation this app makes — `install` and `uninstall`, PHP and
//! MySQL — is built here, so the security properties below are stated, tested
//! and audited once instead of four times. `php::brew` and `mysql::brew` are
//! now thin wrappers that decide only the *formula name* (from their own
//! catalogue-gated major type) and the *verb* (from [`BrewVerb`], a closed set
//! defined here).
//!
//! SECURITY: this module composes the argv. A caller supplies a formula built
//! from a validated version, never a flag, and never the verb as a string.
//! Arguments are passed as a vector rather than through a shell, which stops
//! command injection — but not flag injection, which is why the major types
//! (`PhpMajor::parse` / `MysqlMajor::parse`) enforce shape AND catalogue
//! membership before a formula name can exist at all.
//!
//! SECURITY: [`brew_spec`] requires `brew` to be an absolute path. An empty or
//! relative leading component in `PATH` is resolved by `exec` as the current
//! working directory, and brew shells out to `git` and `curl` — so a relative
//! `brew` path would turn the composed `PATH` into a PATH-hijack primitive for
//! anyone who controls a file in the CWD.

use std::ffi::OsString;
use std::path::Path;

use openvhost_proc::SpawnSpec;

use crate::error::CoreError;

/// The Homebrew subcommands this build is allowed to run.
///
/// An enum rather than a `&str` parameter, and exhaustively matched with no
/// wildcard: the verb reaches argv directly, so "which verbs exist" is a
/// decision this crate makes at compile time, never something a caller can
/// influence. Adding a variant must fail to compile in [`BrewVerb::as_arg`]
/// rather than silently passing an unreviewed subcommand to `brew`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrewVerb {
    Install,
    Uninstall,
}

impl BrewVerb {
    fn as_arg(self) -> &'static str {
        match self {
            BrewVerb::Install => "install",
            BrewVerb::Uninstall => "uninstall",
        }
    }
}

/// `brew <verb> <formula>` — exactly two arguments, no flags, ever.
///
/// Deliberately flag-free in both directions. `install` never gets
/// `--build-from-source`; `uninstall` never gets `--ignore-dependencies` or
/// `--force` (package-uninstall design D1: if brew refuses because another
/// formula depends on this one, that refusal is surfaced verbatim — brew knows
/// things about the user's machine that we do not, and overriding it is how a
/// package manager breaks someone's system). The pinned-argv tests in
/// `php::brew` and `mysql::brew` fail the moment anyone adds one, which is
/// both a security property and a no-surprises property.
///
/// INVARIANT: `brew` must be an absolute path with a real parent directory.
/// `Path::new("brew").parent()` is `Some("")`, not `None`, so a naive
/// `unwrap_or_default()` fallback never fires for a relative or bare-filename
/// path — the composed `PATH` would then start with an empty (or `.`) leading
/// component. `exec` resolves an empty/`.` leading `PATH` component as the
/// current working directory, and brew shells out to `git` and `curl`, so that
/// would hand execution of `git`/`curl` to whoever controls a file in the
/// process's CWD. Rejecting non-absolute input here — rather than trusting
/// callers — is what keeps that primitive from ever reaching argv.
pub(crate) fn brew_spec(
    brew: &Path,
    verb: BrewVerb,
    formula: &str,
) -> Result<SpawnSpec, CoreError> {
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
        args: vec![OsString::from(verb.as_arg()), OsString::from(formula)],
        cwd: None,
        env: vec![
            // Without this, pressing Install can spend five minutes updating
            // Homebrew itself before starting the work the user asked for.
            // Harmless and equally correct for `uninstall`, which has nothing
            // to fetch: one env for both verbs means one thing to audit.
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

    fn args_of(spec: &SpawnSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn every_verb_maps_to_exactly_its_own_subcommand() {
        // Pinned per variant rather than "some non-empty string": a verb that
        // silently mapped onto the wrong subcommand would turn an uninstall
        // into an install (or worse) with nothing else in this crate noticing.
        assert_eq!(BrewVerb::Install.as_arg(), "install");
        assert_eq!(BrewVerb::Uninstall.as_arg(), "uninstall");
    }

    #[test]
    fn the_command_is_exactly_the_verb_and_the_formula() {
        let spec = brew_spec(
            Path::new("/opt/homebrew/bin/brew"),
            BrewVerb::Uninstall,
            "php@8.3",
        )
        .unwrap();
        assert_eq!(
            spec.program,
            std::path::PathBuf::from("/opt/homebrew/bin/brew")
        );
        assert_eq!(
            args_of(&spec),
            vec!["uninstall".to_string(), "php@8.3".into()]
        );
    }

    #[test]
    fn an_uninstall_never_carries_ignore_dependencies_or_force() {
        // Design D1: brew's refusal is surfaced verbatim, never overridden.
        let spec = brew_spec(
            Path::new("/opt/homebrew/bin/brew"),
            BrewVerb::Uninstall,
            "mysql@8.4",
        )
        .unwrap();
        let args = args_of(&spec);
        assert_eq!(args.len(), 2, "got {args:?}");
        assert!(
            !args.iter().any(|a| a.starts_with('-')),
            "no flags may reach brew: {args:?}"
        );
    }

    #[test]
    fn a_relative_brew_path_is_refused_for_every_verb() {
        for verb in [BrewVerb::Install, BrewVerb::Uninstall] {
            for bad in ["brew", "./brew", "bin/brew", ""] {
                assert!(
                    brew_spec(Path::new(bad), verb, "php@8.3").is_err(),
                    "accepted {bad:?} for {verb:?}"
                );
            }
        }
    }
}
