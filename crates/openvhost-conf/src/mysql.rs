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
use crate::inspect::{ProbeFailure, run_bounded};
use crate::{GeneratedFile, PROBE_TIMEOUT, ValidationReport};

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
}

/// Render `my.cnf` for one MySQL major.
///
/// Keys EXACTLY per spec D5 — nothing else, no tuning: `[mysqld]` `datadir`,
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

    let mut tc = tera::Context::new();
    tc.insert("datadir", &datadir);
    tc.insert("socket", &socket);
    tc.insert("pid_file", &pid_file);
    tc.insert("custom_confd", &custom_confd);
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

/// One bounded, contained `mysqladmin`/`mysql` CLI invocation's result (Task
/// 5: the staged-init sequence's `mysqladmin ping`/`ALTER USER`/`mysqladmin
/// shutdown` steps, plus `reset_mysql_root_password`/`verify_mysql_connection`
/// — spec D2/D3/D7). Same shape as [`ValidationReport`], kept as its own type
/// rather than reused: these calls also want `stdout` (the `ALTER`/`SELECT`
/// response), which a config-validation report has no reason to carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlCliOutcome {
    /// True iff the CLI exited 0. Never derived from stderr emptiness — the
    /// same discipline [`MysqlValidator`] and every other validator in this
    /// crate apply, for the identical reason (a clean run can still write to
    /// stderr).
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// # Golden-rule-4 reading applies here too
///
/// See `crate::inspect`'s module-level "golden-rule-4 reading" doc comment
/// (CONFIRMED by security-auditor, 2026-07-26) for the full six-condition
/// test a one-shot tool invocation must meet to spawn outside
/// `openvhost-proc`. Every function below reuses [`run_bounded`] itself
/// (Task 5 widened it to `pub(crate)` and to accept optional stdin, rather
/// than duplicating its containment logic a third time — see that widening's
/// own doc comment) so all six conditions are inherited verbatim: (1)
/// [`PROBE_TIMEOUT`]; (2) `run_bounded`'s own process-group kill; (3) every
/// argument below is either a literal flag or a path/string this crate's
/// caller derived from managed state — never client-supplied; (4)
/// unprivileged, no privileged helper, no system-state mutation (`mysqladmin
/// shutdown` stops OUR OWN unelevated `mysqld` child, the same way `nginx -s
/// quit` would); (5)/(6) inherited from `run_bounded`.
///
/// Maps [`ProbeFailure`] onto [`ConfError`] exactly like [`validate_live`]
/// does, so a caller sees the identical two failure shapes
/// (`ValidatorSpawn`/`ValidatorTimeout`) regardless of which mysql tool it
/// asked this crate to run.
async fn run_mysql_cli(
    cmd: &mut tokio::process::Command,
    bin: &Path,
    stdin: Option<&[u8]>,
) -> Result<MysqlCliOutcome, ConfError> {
    match run_bounded(cmd, stdin).await {
        Ok(out) => Ok(MysqlCliOutcome {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
        Err(ProbeFailure::Io(source)) => Err(ConfError::ValidatorSpawn {
            bin: bin.display().to_string(),
            source,
        }),
        Err(ProbeFailure::TimedOut) => Err(ConfError::ValidatorTimeout {
            bin: bin.display().to_string(),
            secs: PROBE_TIMEOUT.as_secs(),
        }),
    }
}

fn socket_arg(socket: &Path) -> OsString {
    let mut a = OsString::from("--socket=");
    a.push(socket.as_os_str());
    a
}

fn defaults_file_arg(path: &Path) -> OsString {
    let mut a = OsString::from("--defaults-file=");
    a.push(path.as_os_str());
    a
}

/// The exact `mysqladmin ping` argv (spec D4), as plain strings: shared by
/// [`mysqladmin_ping`] below (the staged-init sequence's OWN one-off poll
/// against the network-less temp server, spec D2 step 3) and
/// `apps/desktop/src-tauri/src/stack.rs`'s `mysql_spec` (the SUPERVISOR's
/// ongoing `ReadinessProbe::Command` against the final, running server) —
/// ONE function producing the argv for BOTH call sites is what stops them
/// drifting apart into two subtly different pings. `Vec<String>`, not
/// `Vec<OsString>`: `openvhost_proc::ReadinessProbe::Command.argv` is typed
/// `Vec<String>`, so this shape is dictated by that caller; the lossy
/// `.display()` conversion below is unavoidable for that one, though every
/// OTHER function in this module builds its argv as `OsString` instead.
pub fn mysqladmin_ping_argv(mysqladmin: &Path, socket: &Path) -> Vec<String> {
    vec![
        mysqladmin.display().to_string(),
        "--no-defaults".to_string(),
        "--no-login-paths".to_string(),
        "--protocol=SOCKET".to_string(),
        format!("--socket={}", socket.display()),
        "--user=root".to_string(),
        "--connect-timeout=1".to_string(),
        "--silent".to_string(),
        "ping".to_string(),
    ]
}

/// ONE `mysqladmin ping` attempt against `socket` (spec D2 step 3). Needs no
/// credential — `mysqladmin ping` succeeds even when authentication would be
/// denied, so this proves only that the server is LISTENING, never that any
/// particular credential works. Deliberately not a `Result`, mirroring
/// `probe_nginx_version`: a single failed attempt during a retry loop is an
/// EXPECTED outcome, not an exceptional one — the caller decides how many
/// attempts fit inside its own deadline (spec D2's 10s cap).
pub async fn mysqladmin_ping(mysqladmin: &Path, socket: &Path) -> bool {
    let argv = mysqladmin_ping_argv(mysqladmin, socket);
    let Some((program, rest)) = argv.split_first() else {
        return false;
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(rest);
    run_bounded(&mut cmd, None)
        .await
        .is_ok_and(|out| out.status.success())
}

/// Run `sql` on `mysql_bin` against `socket`, authenticating as `root` with
/// NO password (spec D2 step 4: right after `--initialize-insecure`,
/// `root@localhost` has an empty password — this is the ONE call in the
/// staged-init sequence that runs before any credential exists at all). `sql`
/// crosses via STDIN, never argv/env (the caller is
/// `crate::mysql::alter_user_sql`'s output, i.e. the freshly generated root
/// password itself, spec D3).
pub async fn mysql_alter_password_unauthenticated(
    mysql_bin: &Path,
    socket: &Path,
    sql: &str,
) -> Result<MysqlCliOutcome, ConfError> {
    let mut cmd = tokio::process::Command::new(mysql_bin);
    cmd.arg("--no-defaults")
        .arg("--protocol=SOCKET")
        .arg(socket_arg(socket))
        .arg("--user=root");
    run_mysql_cli(&mut cmd, mysql_bin, Some(sql.as_bytes())).await
}

/// Run `sql` on `mysql_bin`, authenticating via an ALREADY-WRITTEN
/// `--defaults-file` (spec D3: an ephemeral, 0600, RAII-deleted file the
/// command layer writes — this function only ever reads its path, never its
/// contents). `sql` crosses via STDIN. Used by BOTH
/// `reset_mysql_root_password` (an `ALTER USER` script, which produces no
/// output either way) and `verify_mysql_connection` (a
/// `SELECT VERSION(), @@port` query) — the two share nothing but
/// "authenticate with a stored/known credential and run a script", so one
/// function serves both rather than forking near-duplicates.
///
/// `--batch --skip-column-names` makes the OUTPUT SHAPE deterministic for
/// the query caller rather than relying on `mysql`'s own "is stdin a tty"
/// auto-detection (it already batches non-interactive input, but naming the
/// flags explicitly means this function's contract does not quietly depend
/// on that heuristic): a query with rows prints them tab-separated with NO
/// header line, so `verify_mysql_connection` can split the first line on
/// `\t` rather than skipping a header first. Harmless for `ALTER USER`,
/// which produces no rows to format either way.
pub async fn mysql_exec_with_defaults_file(
    mysql_bin: &Path,
    defaults_file: &Path,
    sql: &str,
) -> Result<MysqlCliOutcome, ConfError> {
    let mut cmd = tokio::process::Command::new(mysql_bin);
    cmd.arg(defaults_file_arg(defaults_file))
        .arg("--batch")
        .arg("--skip-column-names");
    run_mysql_cli(&mut cmd, mysql_bin, Some(sql.as_bytes())).await
}

/// `mysqladmin shutdown` against the temp server, authenticating via an
/// ephemeral `--defaults-file` (spec D2 step 5) — the credential was JUST set
/// by the preceding `ALTER USER`, so an unauthenticated shutdown (like the
/// ping above) would now be refused.
pub async fn mysqladmin_shutdown(
    mysqladmin: &Path,
    defaults_file: &Path,
) -> Result<MysqlCliOutcome, ConfError> {
    let mut cmd = tokio::process::Command::new(mysqladmin);
    cmd.arg(defaults_file_arg(defaults_file)).arg("shutdown");
    run_mysql_cli(&mut cmd, mysqladmin, None).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn fixture_ctx() -> MysqlCtx {
        MysqlCtx {
            my_cnf: PathBuf::from("/tmp/ovh/config/generated/mysql/8.4/my.cnf"),
            datadir: PathBuf::from("/tmp/ovh/data/mysql/8.4"),
            socket: PathBuf::from("/tmp/ovh/run/mysql-8.4.sock"),
            pid_file: PathBuf::from("/tmp/ovh/run/mysql-8.4.pid"),
            custom_confd: PathBuf::from("/tmp/ovh/config/custom/mysql/8.4/conf.d"),
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

    // ---- mysqladmin_ping / mysql_alter_password_unauthenticated /
    // mysql_exec_with_defaults_file / mysqladmin_shutdown ----

    /// A fake CLI tool: any name, any body. Generalizes `fake_mysqld` (kept
    /// separate above so its existing callers stay untouched) for the admin
    /// CLI functions below, which fake `mysqladmin`/`mysql` rather than
    /// `mysqld`.
    fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn ping_argv_matches_spec_d4_exactly() {
        let argv = mysqladmin_ping_argv(
            Path::new("/opt/homebrew/opt/mysql@8.4/bin/mysqladmin"),
            Path::new("/tmp/ovh/run/mysql-8.4.sock"),
        );
        assert_eq!(
            argv,
            vec![
                "/opt/homebrew/opt/mysql@8.4/bin/mysqladmin".to_string(),
                "--no-defaults".to_string(),
                "--no-login-paths".to_string(),
                "--protocol=SOCKET".to_string(),
                "--socket=/tmp/ovh/run/mysql-8.4.sock".to_string(),
                "--user=root".to_string(),
                "--connect-timeout=1".to_string(),
                "--silent".to_string(),
                "ping".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn ping_reports_true_on_a_clean_exit() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "mysqladmin", "exit 0");
        assert!(mysqladmin_ping(&bin, Path::new("/tmp/x.sock")).await);
    }

    #[tokio::test]
    async fn ping_reports_false_on_a_nonzero_exit() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "mysqladmin", "exit 1");
        assert!(!mysqladmin_ping(&bin, Path::new("/tmp/x.sock")).await);
    }

    #[tokio::test]
    async fn ping_reports_false_when_the_binary_does_not_exist() {
        assert!(
            !mysqladmin_ping(
                Path::new("/nonexistent/mysqladmin"),
                Path::new("/tmp/x.sock")
            )
            .await
        );
    }

    #[tokio::test]
    async fn unauthenticated_alter_feeds_sql_on_stdin_not_argv() {
        // The fake echoes argv to stderr and stdin to stdout, so this test
        // can assert BOTH: the SQL text appears on stdin, never on argv.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "mysql", "echo \"$@\" 1>&2; cat");
        let r = mysql_alter_password_unauthenticated(
            &bin,
            Path::new("/tmp/init.sock"),
            "ALTER USER 'root'@'localhost' IDENTIFIED BY 'secretpw';",
        )
        .await
        .unwrap();
        assert!(r.ok);
        assert!(
            r.stdout.contains("secretpw"),
            "the SQL must reach the child via stdin: {r:?}"
        );
        assert!(
            !r.stderr.contains("secretpw"),
            "the SQL must NEVER appear on argv: {r:?}"
        );
        assert!(r.stderr.contains("--protocol=SOCKET"));
        assert!(r.stderr.contains("--socket=/tmp/init.sock"));
        assert!(r.stderr.contains("--user=root"));
        assert!(
            !r.stderr.contains("--password"),
            "no password flag at all — root has none yet: {r:?}"
        );
    }

    #[tokio::test]
    async fn exec_with_defaults_file_puts_the_defaults_file_and_batch_flags_on_argv() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "mysql", "echo \"$@\" 1>&2; cat");
        let defaults = d.path().join("defaults.cnf");
        std::fs::write(&defaults, "[client]\nuser=root\n").unwrap();
        let r = mysql_exec_with_defaults_file(&bin, &defaults, "SELECT VERSION();")
            .await
            .unwrap();
        assert!(r.ok);
        assert_eq!(
            r.stderr.trim_end(),
            format!(
                "--defaults-file={} --batch --skip-column-names",
                defaults.display()
            )
        );
        assert!(r.stdout.contains("SELECT VERSION();"));
    }

    #[tokio::test]
    async fn shutdown_puts_only_the_defaults_file_and_shutdown_on_argv() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "mysqladmin", r#"echo "$@" 1>&2"#);
        let defaults = d.path().join("defaults.cnf");
        std::fs::write(&defaults, "[client]\nuser=root\n").unwrap();
        let r = mysqladmin_shutdown(&bin, &defaults).await.unwrap();
        assert!(r.ok);
        assert_eq!(
            r.stderr.trim_end(),
            format!("--defaults-file={} shutdown", defaults.display())
        );
    }

    #[tokio::test]
    async fn admin_cli_errors_map_to_conferror_like_the_validator_does() {
        let e = mysqladmin_shutdown(
            Path::new("/nonexistent/mysqladmin"),
            Path::new("/tmp/defaults.cnf"),
        )
        .await
        .unwrap_err();
        assert!(matches!(e, ConfError::ValidatorSpawn { .. }), "got {e:?}");
    }
}
