// SPDX-License-Identifier: GPL-3.0-or-later
//! Homebrew as a MySQL source: which major we offer, and the exact command
//! that installs it.
//!
//! SECURITY: mirrors `crate::php::brew` exactly — see its module doc comment
//! for the full PATH-hijack rationale. The short version: this module
//! composes the argv, a caller supplies only a version (never a formula,
//! never a flag), arguments go through a `Vec`, never a shell, and
//! [`mysql_brew_install_spec`] requires `brew` to be an absolute path for
//! the identical reason `crate::php::brew_install_spec` does (a relative
//! leading `PATH` component resolves to the CWD, and brew shells out to
//! `git`/`curl`).

use std::ffi::OsString;
use std::path::Path;

use openvhost_proc::SpawnSpec;

use crate::error::CoreError;

/// The MySQL majors this build offers to INSTALL. Deliberately a single
/// entry today (spec D1): the unversioned `mysql` formula tracks a rolling
/// release that EOLs quarterly and would let `brew upgrade` silently move a
/// datadir across majors, so only the pinned `mysql@8.4` formula is offered.
/// Named `MYSQL_CATALOGUE` rather than reusing [`crate::php::CATALOGUE`]'s
/// bare `CATALOGUE` name — both get flattened into this crate's public root
/// via `pub use`, and a second `CATALOGUE` there would collide.
///
/// Hand-maintained for the same reason PHP's catalogue is: asking `brew`
/// would mean spawning a process on a path that has to stay cheap, and a
/// stale entry fails loudly at install time rather than silently.
pub const MYSQL_CATALOGUE: [&str; 1] = ["8.4"];

/// Shape check shared by [`MysqlMajor::parse`] (untrusted input: shape AND
/// catalogue membership) and [`MysqlMajor::from_probe`] (discovery: shape
/// only) — digits, one dot, digits, nothing else. No `regex` dependency:
/// this crate adds none for this module, mirroring `PhpMajor::parse`'s own
/// hand-rolled layer-1 check.
fn is_major_minor_shape(s: &str) -> bool {
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

/// A MySQL `major.minor` version string.
///
/// Two constructors enforce different things — this is the one place this
/// module's design deliberately departs from mirroring `PhpMajor` exactly
/// (PHP sidesteps the question entirely: `PhpRuntime.major` is a raw,
/// unvalidated `String`, because a discovered PHP runtime is used as-is
/// regardless of catalogue membership). [`crate::mysql::MysqlRuntime`]
/// instead holds a typed [`MysqlMajor`] (this task's brief specifies that
/// shape), which means discovery — required by spec D1 to report a major
/// this build does not offer to install — needs its own, more permissive
/// path into the type:
///
/// - [`MysqlMajor::parse`] is the untrusted-input gate: shape AND
///   [`MYSQL_CATALOGUE`] membership. This is the ONLY constructor reachable
///   from outside this crate (IPC ingress, [`mysql_brew_install_spec`]) — it
///   is what stops a caller from asking this build to install or initialize
///   a formula this build has never tested.
/// - `MysqlMajor::from_probe` is `pub(crate)`: shape only, no catalogue
///   check. Its only caller is [`crate::mysql::discover_mysql`], which
///   sources the string from this crate's own bounded `mysqld --version`
///   probe — never from external input. This is what lets a discovered
///   9.x install still render as a row (spec D1: "honest display, no
///   support burden").
///
/// Both constructors apply the IDENTICAL shape check, so path derivation
/// (`mysql_paths`) is safe to call with a value from EITHER constructor: a
/// `major.minor` string containing only ASCII digits and a single `.` can
/// never contain a path separator or `..`, so it can never steer a joined
/// path outside `<home>/...` regardless of catalogue membership. Catalogue
/// membership is a separate, orthogonal POLICY question ("can we
/// install/initialize this major"), never a confinement one — exactly like
/// `Domain`/`Docroot` are charset guards, not policy, for the site pipeline.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MysqlMajor(String);

impl MysqlMajor {
    /// The untrusted-input gate: shape AND catalogue membership. The only
    /// constructor a caller outside this crate can reach.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        // Layer 1: shape. Digits, one dot, digits — nothing else.
        if !is_major_minor_shape(s) {
            return Err(CoreError::Validation {
                field: "mysql_version",
                reason: format!("{s:?} is not a major.minor version"),
            });
        }
        // Layer 2: policy. Shape alone would still let a well-formed but
        // untested (or intentionally unsupported, e.g. "8.0") version reach
        // `brew install`.
        if !MYSQL_CATALOGUE.contains(&s) {
            return Err(CoreError::Validation {
                field: "mysql_version",
                reason: format!(
                    "MySQL {s} is not offered by this build (offered: {})",
                    MYSQL_CATALOGUE.join(", ")
                ),
            });
        }
        Ok(Self(s.to_string()))
    }

    /// Discovery-only: shape check, no catalogue check. `pub(crate)` — see
    /// this type's doc comment for why it exists and who may call it.
    pub(crate) fn from_probe(s: String) -> Option<Self> {
        is_major_minor_shape(&s).then_some(Self(s))
    }

    /// The bare `major.minor` string, e.g. `"8.4"`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this major is one [`MYSQL_CATALOGUE`] offers to install.
    /// `false` is an expected, valid outcome for a discovered runtime (spec
    /// D1) — callers use this to decide whether to render an Install
    /// affordance, never to decide whether a path derived from this value
    /// is safe to use (it always is; see this type's doc comment).
    pub fn is_cataloged(&self) -> bool {
        MYSQL_CATALOGUE.contains(&self.0.as_str())
    }
}

/// The command that installs `major`. Composed here — the formula name is
/// never accepted from a caller. Mirrors [`crate::php::brew_install_spec`]
/// byte-for-byte apart from the formula prefix (`mysql@` vs `php@`) and the
/// major type; see its doc comment for the full PATH-hijack rationale. `brew`
/// itself is located via the EXISTING [`crate::find_brew`] — Homebrew's own
/// location is not PHP-specific, so this module does not duplicate that
/// lookup for MySQL.
pub fn mysql_brew_install_spec(brew: &Path, major: &MysqlMajor) -> Result<SpawnSpec, CoreError> {
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

    // Composed from a fixed baseline, never the process's own ambient PATH —
    // see `crate::php::brew_install_spec`'s doc comment for why (a ServBay
    // install shadows binaries there, and brew's children — git, curl, tar —
    // would inherit the same shadowing if the parent's PATH were appended).
    let mut path = OsString::from(brew_bin);
    path.push(":/usr/bin:/bin:/usr/sbin:/sbin");

    Ok(SpawnSpec {
        program: brew.to_path_buf(),
        args: vec![
            OsString::from("install"),
            OsString::from(format!("mysql@{}", major.as_str())),
        ],
        cwd: None,
        env: vec![
            // Without this, pressing Install can spend several minutes
            // updating Homebrew itself before starting the work the user
            // asked for.
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
        assert_eq!(MysqlMajor::parse("8.4").unwrap().as_str(), "8.4");
    }

    #[test]
    fn rejects_anything_that_is_not_major_dot_minor() {
        for bad in [
            "",
            "8",
            "8.",
            ".4",
            "8.4.1",
            "eight.four",
            " 8.4",
            "8.4 ",
            "8_4",
        ] {
            assert!(MysqlMajor::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_a_flag_even_though_argv_prevents_command_injection() {
        for bad in ["--build-from-source", "--HEAD", "-f", "--cask", "nginx"] {
            assert!(MysqlMajor::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_a_well_formed_version_this_build_does_not_offer() {
        // "8.0" is named explicitly in the task brief: well-formed shape,
        // just not a major this build offers to install.
        assert!(MysqlMajor::parse("8.0").is_err());
        assert!(MysqlMajor::parse("9.7").is_err());
    }

    #[test]
    fn the_catalogue_is_exactly_8_4_today() {
        assert_eq!(MYSQL_CATALOGUE, ["8.4"]);
    }

    #[test]
    fn from_probe_accepts_shape_regardless_of_catalogue_membership() {
        // Discovery must be able to represent an out-of-catalogue install
        // (spec D1) — from_probe is shape-only.
        assert_eq!(
            MysqlMajor::from_probe("8.4".to_string()).unwrap().as_str(),
            "8.4"
        );
        assert_eq!(
            MysqlMajor::from_probe("9.7".to_string()).unwrap().as_str(),
            "9.7"
        );
    }

    #[test]
    fn from_probe_still_rejects_a_malformed_shape() {
        for bad in ["", "8", "8.", "eight.four", "8.4.1"] {
            assert!(
                MysqlMajor::from_probe(bad.to_string()).is_none(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn is_cataloged_distinguishes_offered_from_discovered_only() {
        let offered = MysqlMajor::parse("8.4").unwrap();
        assert!(offered.is_cataloged());
        let discovered_only = MysqlMajor::from_probe("9.7".to_string()).unwrap();
        assert!(!discovered_only.is_cataloged());
    }

    #[test]
    fn the_install_command_is_exactly_install_and_the_formula() {
        let spec = mysql_brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &MysqlMajor::parse("8.4").unwrap(),
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
        assert_eq!(args, vec!["install".to_string(), "mysql@8.4".to_string()]);
    }

    #[test]
    fn the_install_command_disables_homebrews_own_auto_update() {
        let spec = mysql_brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &MysqlMajor::parse("8.4").unwrap(),
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
        let spec = mysql_brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &MysqlMajor::parse("8.4").unwrap(),
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
        for bad in ["brew", "./brew", "bin/brew", ""] {
            let err = mysql_brew_install_spec(
                std::path::Path::new(bad),
                &MysqlMajor::parse("8.4").unwrap(),
            );
            assert!(err.is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn the_inherited_ambient_path_never_appears_in_the_composed_value() {
        // Mirrors php/brew.rs's identical test. The function under test never
        // reads $PATH — it composes one purely from `brew`'s own parent plus a
        // fixed suffix — so this assertion cannot be affected by a concurrently
        // running sibling test mutating the same process-global env var; only
        // the restore-previous bookkeeping shares that (harmless, since nothing
        // else in this crate reads PATH from env) narrow window.
        const MARKER: &str = "/tmp/openvhost-hostile-shadow-dir-marker-mysql";
        let previous = std::env::var_os("PATH");
        // SAFETY: no other thread in this test binary reads/writes PATH
        // concurrently with this single-threaded set/restore pair.
        unsafe {
            std::env::set_var("PATH", format!("{MARKER}:/usr/bin"));
        }

        let spec = mysql_brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &MysqlMajor::parse("8.4").unwrap(),
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
        let spec = mysql_brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &MysqlMajor::parse("8.4").unwrap(),
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
