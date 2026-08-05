// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal `my.cnf` generation plus a pre-flight `mysqld --validate-config`
//! check. See spec D5:
//! docs/superpowers/specs/2026-07-29-p1-db-mysql-design.md.
//!
//! Deliberately no trait here (spec D5: "no one-implementation DB trait").
//! MySQL and MariaDB `my.cnf` have already diverged (CLAUDE.md golden
//! rules: "keep separate template trees, do not share includes between
//! them beyond truly common fragments"), so forcing a shared adapter trait
//! today would abstract over a difference that does not exist in this
//! codebase yet. A second implementation, when it arrives, gets its own
//! template tree and its own concrete functions; a trait gets introduced
//! then, if the two truly share enough to justify one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::ctx::to_config_path;
use crate::engine::render;
use crate::error::ConfError;
use crate::{GeneratedFile, ValidationReport};

/// Every path `my.cnf` needs for one MySQL major, as plain values.
///
/// Deliberately NOT `openvhost_core::mysql::MysqlPaths` itself:
/// `openvhost-conf` does not depend on `openvhost-core` (the reverse is
/// true — `openvhost-core`'s `Cargo.toml` depends on `openvhost-conf`), so
/// importing that type here would invert the workspace's dependency graph.
/// `crate::validate::find_brew_binaries`'s doc comment already makes the
/// identical call for the Homebrew prefix list, for the identical reason.
/// The command layer (plan Task 5) computes `MysqlPaths` in `openvhost-core`
/// and copies its fields into this struct one by one — the field names below
/// mirror `MysqlPaths` deliberately, field for field, so that copy is a
/// straight line-up, not a renaming exercise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlCtx {
    /// Where the rendered file is written. Mirrors `MysqlPaths::my_cnf`
    /// (`<home>/config/generated/mysql/<major>/my.cnf`).
    pub my_cnf: PathBuf,
    /// Mirrors `MysqlPaths::datadir` (`<home>/data/mysql/<major>/`).
    pub datadir: PathBuf,
    /// Mirrors `MysqlPaths::socket` (`<home>/run/mysql-<major>.sock`) — used
    /// for both the `[mysqld]` listen socket and the `[client]` default.
    pub socket: PathBuf,
    /// `<home>/run/mysql-<major>.pid` — NOT one of `MysqlPaths`'s fields:
    /// nothing in Task 2's datadir lifecycle needs a pid-file path, only
    /// this template's `[mysqld]` block does, so the command layer derives
    /// it separately (the same `<home>/run/` directory nginx's own
    /// `pid_path` already uses).
    pub pid_file: PathBuf,
    /// Mirrors `MysqlPaths::custom_confd`
    /// (`<home>/config/custom/mysql/<major>/conf.d`) — the user's own
    /// `!includedir` target, never written by this app.
    pub custom_confd: PathBuf,
    /// The four fields below are THE RUNTIME DIRECTORIES, and they are not
    /// paths under the OpenVHost home at all: every one of them names a
    /// directory INSIDE the installed server's own package tree, supplied by
    /// the caller from the runtime discovery actually resolved (never guessed
    /// here, and never probed here — `openvhost-conf` discovers nothing, the
    /// same rule [`MysqlValidator`] follows for the `mysqld` path).
    ///
    /// WHY THEY EXIST (spec D3 of
    /// docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md): a
    /// server that is not TOLD these resolves them out of its **compiled-in
    /// prefix**. Measured on 2026-08-04 against the real 11.4.9 artifact —
    /// `SHOW VARIABLES` on a running server started with `--no-defaults`
    /// reported `basedir`, `character_sets_dir` and `plugin_dir` all under
    /// `/opt/openvhost-build/mariadb-11.4.9/`, a directory that does not
    /// exist on any user's machine. That is exactly how the first MariaDB
    /// artifact came to resolve `plugin_dir` out of a mode-1777 tree
    /// (CWE-426/427). The build-time fix removed the *reachable* attack;
    /// pinning here removes the *dependence on the prefix* altogether, which
    /// is a different guarantee — a future reader will be tempted to drop one
    /// as redundant, and neither replaces the other.
    ///
    /// MySQL carries them for the same reason and not as symmetry: without
    /// them mysqld relies on a property of Homebrew's prefix that nobody has
    /// checked.
    ///
    /// **The layouts genuinely differ between engines**, which is why these
    /// are four explicit values rather than one `basedir` this crate expands:
    /// Homebrew's `mysql@8.4` keeps charsets at `share/mysql/charsets` and
    /// messages at `share/mysql/`, while the MariaDB package keeps them at
    /// `share/charsets` and `share/` (both measured, same day). A `basedir` +
    /// hardcoded suffixes would silently pick the wrong tree for one of them.
    ///
    /// `<basedir>` — the server's installation root.
    pub basedir: PathBuf,
    /// `<basedir>/lib/plugin` — where the server loads plugins from.
    pub plugin_dir: PathBuf,
    /// The directory holding `Index.xml` and the character-set definitions.
    pub character_sets_dir: PathBuf,
    /// The PARENT of the per-language `errmsg.sys` directories (the server
    /// appends `lc_messages`, e.g. `english/`), not that directory itself.
    pub lc_messages_dir: PathBuf,
}

/// Render `my.cnf` for one MySQL major.
///
/// Keys EXACTLY per spec D5 — nothing else, no tuning — plus the four
/// runtime directories the 2026-08-04 MariaDB-service spec D3 added to BOTH
/// engines (see [`MysqlCtx::basedir`] for why): `[mysqld]` `basedir`,
/// `plugin_dir`, `character-sets-dir`, `lc_messages_dir`, `datadir`,
/// `socket`, `pid-file`, `port=3306`, `bind-address=127.0.0.1`,
/// `skip-name-resolve`, `mysqlx=OFF`, `log-error-verbosity=2`,
/// `!includedir <custom conf.d>`; `[client]` `socket`, `port`. `port` is a
/// bare literal `3306` in the template, not a Tera variable: spec D7 fixes
/// it for this slice ("No port newtype (3306 fixed this slice)"), so a
/// variable here would be a knob that does not actually turn. `log-error`
/// (a file path) is deliberately ABSENT — spec D4 leaves it unset so mysqld's
/// stderr lands in the supervisor's ring buffer instead of a file this app
/// would then have to tail separately.
///
/// Pure function of `ctx`: same input, byte-identical output (workspace hard
/// rule). Every path value passes through [`to_config_path`] — the crate's
/// single chokepoint for embedding a path into a config template — which
/// already documents feeding "the php-fpm pool template's unquoted INI
/// `listen =`, `error_log =`, and `include=` lines"; `my.cnf` is the same
/// unquoted-INI family (MySQL's own option-file parser takes the rest of the
/// line, verbatim including spaces, as the value — no quoting is needed or
/// used here, matching `php-fpm/pool.conf.tera`'s style rather than
/// `nginx/site.conf.tera`'s double-quoted style).
pub fn generate_my_cnf(ctx: &MysqlCtx) -> Result<GeneratedFile, ConfError> {
    let datadir = to_config_path(&ctx.datadir)?;
    let socket = to_config_path(&ctx.socket)?;
    let pid_file = to_config_path(&ctx.pid_file)?;
    let custom_confd = to_config_path(&ctx.custom_confd)?;
    let basedir = to_config_path(&ctx.basedir)?;
    let plugin_dir = to_config_path(&ctx.plugin_dir)?;
    let character_sets_dir = to_config_path(&ctx.character_sets_dir)?;
    let lc_messages_dir = to_config_path(&ctx.lc_messages_dir)?;

    let mut tc = tera::Context::new();
    tc.insert("datadir", &datadir);
    tc.insert("socket", &socket);
    tc.insert("pid_file", &pid_file);
    tc.insert("custom_confd", &custom_confd);
    tc.insert("basedir", &basedir);
    tc.insert("plugin_dir", &plugin_dir);
    tc.insert("character_sets_dir", &character_sets_dir);
    tc.insert("lc_messages_dir", &lc_messages_dir);
    let contents = render("mysql/my.cnf", &tc)?;
    Ok(GeneratedFile {
        path: ctx.my_cnf.clone(),
        contents,
    })
}

/// Pre-flight config check via an already-resolved `mysqld` binary — never
/// discovered here (spec D5: "the validator takes the mysqld path as an
/// input, no discovery inside conf"; discovery is `openvhost-core::mysql`'s
/// job, mirroring how `WebServerAdapter::validate`/`PhpRuntimeAdapter::validate`
/// take their binary path as a caller-supplied argument rather than locating
/// it themselves).
///
/// Two caveats, recorded per spec D5 rather than silently assumed:
///
/// 1. UNVERIFIED as of this task: whether `--validate-config` touches or
///    locks the datadir it is pointed at. That verification is owed to the
///    plan's Task-7 live gate (real `mysqld` on a real machine) — if it
///    turns out to touch/lock the datadir, this validator gets DROPPED
///    outright. A bad config then fails visibly at start, which the
///    supervisor's new `ReadinessProbe::Command` (spec D4) surfaces on its
///    own, so dropping this pre-check would not leave a silent gap.
/// 2. MySQL's own documentation describes `--validate-config` as an
///    INCOMPLETE check (it does not catch everything a real startup does),
///    so start+readiness remains the DEFINITIVE validation. This is a
///    pre-flight convenience — a faster, friendlier error for the common
///    case — never the last word.
pub struct MysqlValidator {
    pub mysqld: PathBuf,
}

impl MysqlValidator {
    /// Validate a `my.cnf` that already exists on disk at `candidate`.
    /// Writes nothing — mirrors `inspect::validate_live`'s "the config file
    /// already exists, in place" contract, and `NginxAdapter`/`PhpFpmRuntime`'s
    /// `validate` in reporting `ok` from the exit code alone (never derived
    /// from stderr emptiness).
    ///
    /// `--defaults-file=<candidate>` MUST be the first argument (spec D5):
    /// mysqld's startup option parsing special-cases
    /// `--defaults-file`/`--defaults-extra-file`/`--no-defaults` as
    /// recognized ONLY when given as the very first command-line argument,
    /// before its normal option parsing begins — putting it anywhere else
    /// silently fails to select the candidate file at all.
    pub async fn validate(&self, candidate: &Path) -> Result<ValidationReport, ConfError> {
        // Built via `OsString`, not `format!("--defaults-file={}", candidate.display())`:
        // `.display()` is a lossy UTF-8 conversion, and this argument goes
        // straight into `Command::arg` (never a shell), so there is no reason
        // to risk mangling a non-UTF-8 path when appending onto an `OsString`
        // works for every path this program can construct.
        let mut defaults_file_arg = OsString::from("--defaults-file=");
        defaults_file_arg.push(candidate.as_os_str());

        let out = tokio::process::Command::new(&self.mysqld)
            .arg(defaults_file_arg)
            .arg("--validate-config")
            .output()
            .await
            .map_err(|e| ConfError::ValidatorSpawn {
                bin: self.mysqld.display().to_string(),
                source: e,
            })?;
        Ok(ValidationReport {
            ok: out.status.success(), // exit code ONLY
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ctx::directive;

    /// The package tree the fixture's server was "discovered" in — Homebrew's
    /// real `mysql@8.4` layout, which is NOT the MariaDB one (`share/mysql/…`
    /// vs `share/…`; both measured 2026-08-04).
    const FIXTURE_BASEDIR: &str = "/opt/homebrew/opt/mysql@8.4";

    fn fixture_ctx() -> MysqlCtx {
        MysqlCtx {
            my_cnf: PathBuf::from("/tmp/ovh/config/generated/mysql/8.4/my.cnf"),
            datadir: PathBuf::from("/tmp/ovh/data/mysql/8.4"),
            socket: PathBuf::from("/tmp/ovh/run/mysql-8.4.sock"),
            pid_file: PathBuf::from("/tmp/ovh/run/mysql-8.4.pid"),
            custom_confd: PathBuf::from("/tmp/ovh/config/custom/mysql/8.4/conf.d"),
            basedir: PathBuf::from(FIXTURE_BASEDIR),
            plugin_dir: PathBuf::from("/opt/homebrew/opt/mysql@8.4/lib/plugin"),
            character_sets_dir: PathBuf::from("/opt/homebrew/opt/mysql@8.4/share/mysql/charsets"),
            lc_messages_dir: PathBuf::from("/opt/homebrew/opt/mysql@8.4/share/mysql"),
        }
    }

    /// The exact expected file, byte for byte, for [`fixture_ctx`]. This is
    /// the golden-file test the task brief asks for: not "contains", but a
    /// full `assert_eq!` against a fixed home/major.
    const EXPECTED_MY_CNF: &str = "\
# ---------------------------------------------------------------------------
# GENERATED by OpenVHost — DO NOT EDIT. Regenerated idempotently; your edits
# will be lost. To customize, add files under:
#   /tmp/ovh/config/custom/mysql/8.4/conf.d
# ---------------------------------------------------------------------------
[mysqld]
basedir=/opt/homebrew/opt/mysql@8.4
plugin_dir=/opt/homebrew/opt/mysql@8.4/lib/plugin
character-sets-dir=/opt/homebrew/opt/mysql@8.4/share/mysql/charsets
lc_messages_dir=/opt/homebrew/opt/mysql@8.4/share/mysql
datadir=/tmp/ovh/data/mysql/8.4
socket=/tmp/ovh/run/mysql-8.4.sock
pid-file=/tmp/ovh/run/mysql-8.4.pid
port=3306
bind-address=127.0.0.1
skip-name-resolve
mysqlx=OFF
log-error-verbosity=2
!includedir /tmp/ovh/config/custom/mysql/8.4/conf.d

[client]
socket=/tmp/ovh/run/mysql-8.4.sock
port=3306
";

    #[test]
    fn my_cnf_matches_the_golden_file_exactly() {
        let f = generate_my_cnf(&fixture_ctx()).unwrap();
        assert_eq!(
            f.path,
            PathBuf::from("/tmp/ovh/config/generated/mysql/8.4/my.cnf")
        );
        assert_eq!(
            f.contents, EXPECTED_MY_CNF,
            "rendered my.cnf did not match the golden file, got:\n{}",
            f.contents
        );
    }

    /// Spec D3: the four runtime directories must be present AND must name
    /// somewhere inside the package tree the server was discovered in — not
    /// the compiled-in prefix, and not an OpenVHost home path.
    ///
    /// VACUITY: proven by editing `FIXTURE_BASEDIR` to `/opt/somewhere-else`
    /// (a prefix none of the other three share) and re-running — the
    /// `starts_with` arm fails for all three, naming the offending value.
    /// Separately, deleting any one of the four lines from the template fails
    /// the `expect` on that key rather than passing vacuously.
    #[test]
    fn my_cnf_pins_all_four_runtime_directories_inside_the_package_tree() {
        let c = generate_my_cnf(&fixture_ctx()).unwrap();
        let c = c.contents;

        let basedir = directive(&c, "basedir").expect("basedir must be pinned");
        assert_eq!(basedir, FIXTURE_BASEDIR);

        for key in ["plugin_dir", "character-sets-dir", "lc_messages_dir"] {
            let value = directive(&c, key).unwrap_or_else(|| panic!("{key} must be pinned"));
            assert!(
                value.starts_with(&format!("{FIXTURE_BASEDIR}/")),
                "{key}={value} must live inside the package tree {FIXTURE_BASEDIR}"
            );
        }
    }

    /// The four must NOT be derived from a single `basedir` by this crate:
    /// Homebrew's mysql@8.4 keeps charsets at `share/mysql/charsets` while the
    /// MariaDB package keeps them at `share/charsets` (both measured
    /// 2026-08-04), so a hardcoded suffix here would silently point one engine
    /// at the other's layout. Pins that the caller's value is what is rendered.
    #[test]
    fn my_cnf_renders_the_callers_layout_verbatim_not_a_guessed_suffix() {
        let mut ctx = fixture_ctx();
        ctx.character_sets_dir = PathBuf::from("/opt/homebrew/opt/mysql@8.4/elsewhere/charsets");
        let c = generate_my_cnf(&ctx).unwrap().contents;
        assert_eq!(
            directive(&c, "character-sets-dir"),
            Some("/opt/homebrew/opt/mysql@8.4/elsewhere/charsets")
        );
    }

    #[test]
    fn generation_is_deterministic() {
        // Workspace hard rule: same input, byte-identical output.
        let a = generate_my_cnf(&fixture_ctx()).unwrap();
        let b = generate_my_cnf(&fixture_ctx()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn every_rendered_line_has_no_stray_trailing_whitespace() {
        // A trailing space after e.g. `datadir=/x/y ` is invisible in a diff
        // but would become part of the option-file VALUE (my.cnf's parser
        // takes the rest of the line verbatim) — catch it here rather than
        // let it surface as a mysterious "no such file or directory".
        let c = generate_my_cnf(&fixture_ctx()).unwrap().contents;
        for line in c.lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in: {line:?}");
        }
    }

    // ---- MysqlValidator ----

    /// A fake "mysqld": a shell script that writes fixed text and exits with
    /// a fixed code. Lets these tests assert real spawn behaviour without a
    /// real MySQL install, which the plan's Global Constraints forbid for
    /// every task before the Task-7 live gate.
    fn fake_mysqld(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("mysqld");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[tokio::test]
    async fn validator_reports_ok_on_a_clean_exit() {
        let d = tempfile::tempdir().unwrap();
        let v = MysqlValidator {
            mysqld: fake_mysqld(d.path(), "exit 0"),
        };
        let r = v
            .validate(Path::new("/tmp/ovh/config/generated/mysql/8.4/my.cnf"))
            .await
            .unwrap();
        assert!(r.ok);
    }

    #[tokio::test]
    async fn validator_reports_failure_and_keeps_stderr_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let v = MysqlValidator {
            mysqld: fake_mysqld(
                d.path(),
                "echo 'ERROR: unknown variable bogus' 1>&2; exit 1",
            ),
        };
        let r = v
            .validate(Path::new("/tmp/ovh/config/generated/mysql/8.4/my.cnf"))
            .await
            .unwrap();
        assert!(!r.ok, "exit 1 must report ok = false");
        assert!(r.stderr.contains("unknown variable bogus"));
    }

    #[tokio::test]
    async fn validator_reports_ok_purely_from_exit_code_even_with_stderr_output() {
        // Mirrors the nginx/php-fpm validators: `ok` must never be derived
        // from stderr emptiness. mysqld's `--validate-config` legitimately
        // writes informational lines to stderr even on a clean pass.
        let d = tempfile::tempdir().unwrap();
        let v = MysqlValidator {
            mysqld: fake_mysqld(
                d.path(),
                "echo 'note: validating configuration' 1>&2; exit 0",
            ),
        };
        let r = v
            .validate(Path::new("/tmp/ovh/config/generated/mysql/8.4/my.cnf"))
            .await
            .unwrap();
        assert!(
            r.ok,
            "a non-empty stderr must not flip a clean exit to failed"
        );
    }

    #[tokio::test]
    async fn validator_puts_defaults_file_first_and_nothing_else_in_argv() {
        // Pins BOTH the order (defaults-file first, spec D5) and that the
        // argv is EXACTLY these two tokens — nothing extra a real mysqld
        // would misinterpret as a third positional argument.
        let d = tempfile::tempdir().unwrap();
        let v = MysqlValidator {
            mysqld: fake_mysqld(d.path(), r#"echo "$@" 1>&2"#),
        };
        let candidate = PathBuf::from("/tmp/ovh/config/generated/mysql/8.4/my.cnf");
        let r = v.validate(&candidate).await.unwrap();
        assert_eq!(
            r.stderr.trim_end(),
            "--defaults-file=/tmp/ovh/config/generated/mysql/8.4/my.cnf --validate-config"
        );
    }

    #[tokio::test]
    async fn validator_errors_when_the_binary_cannot_be_launched() {
        let v = MysqlValidator {
            mysqld: PathBuf::from("/nonexistent/mysqld"),
        };
        let e = v
            .validate(Path::new("/tmp/ovh/config/generated/mysql/8.4/my.cnf"))
            .await
            .unwrap_err();
        assert!(matches!(e, ConfError::ValidatorSpawn { .. }), "got {e:?}");
    }
}
