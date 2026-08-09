// SPDX-License-Identifier: GPL-3.0-or-later
//! MySQL admin-CLI spawns (`mysqladmin`/`mysql` — ping, `ALTER USER`,
//! `shutdown`): bounded, contained, one-shot invocations of the running
//! server's own command-line tools.
//!
//! Plus one MariaDB argv builder, [`mariadb_admin_ping_argv`], which spawns
//! nothing itself — it is here rather than in `stack.rs` (its only caller)
//! because what a reader most needs to see is how it DIFFERS from
//! [`mysqladmin_ping_argv`], and a divergence documented one file away from
//! the thing it diverges from is a divergence that gets "tidied up".
//! MariaDB's own init-time CLI work lives in `openvhost-core`'s
//! `mariadb::init`, which owns its whole staged sequence.
//!
//! Review fix wave finding 4: these do NOT belong in `openvhost-conf`.
//! That crate's `inspect` module documents a "golden-rule-4 reading"
//! (CONFIRMED by security-auditor, 2026-07-26) that lets a one-shot tool
//! invocation spawn outside `openvhost-proc` — but it was confirmed for
//! `openvhost-conf`'s OWN read-only version/config probes
//! (`nginx -v`/`-t`, `php-fpm -v`, `mysqld --validate-config`), never
//! materializing anything, never touching a live instance. The functions
//! here are a different trust class: they reach a RUNNING, Supervisor-
//! registered `mysqld` and mutate its root credential (`ALTER USER`) or
//! shut it down — orchestration behavior, not config generation — so they
//! live in the command/orchestration layer instead, alongside
//! `commands.rs`'s `run_mysql_init` (the ONE caller of the credential-
//! mutating pair) and `stack.rs`'s `mysql_spec` (the ONE other caller of
//! [`mysqladmin_ping_argv`]).
//!
//! `MysqlValidator` (read-only `mysqld --validate-config`) correctly STAYS
//! in `openvhost-conf` — it never reaches a running instance and fits the
//! confirmed carve-out exactly, the same as `NginxAdapter`/
//! `PhpRuntimeAdapter`'s validators.
//!
//! Every function below reuses `openvhost_conf::run_bounded` (that crate's
//! own six-condition-confirmed containment, `pub`-re-exported for exactly
//! this reuse) rather than a second implementation of its subtle drop-
//! ordering logic: (1) bounded by that crate's `PROBE_TIMEOUT`; (2) its own
//! process-group kill on timeout; (3) every argument built below is either
//! a literal flag or a path/string THIS crate's caller derived from managed
//! state — never client-supplied; (4) unprivileged, no privileged helper,
//! no system-state mutation (`mysqladmin shutdown` stops OUR OWN
//! unelevated `mysqld` child, the same way `nginx -s quit` would); (5)/(6)
//! inherited from `run_bounded` (output captured, environment assembled).

use std::ffi::OsString;
use std::path::Path;

use openvhost_conf::{ConfError, run_bounded};

/// One bounded, contained `mysqladmin`/`mysql` CLI invocation's result (the
/// staged-init sequence's `mysqladmin ping`/`ALTER USER`/`mysqladmin
/// shutdown` steps, plus `reset_mysql_root_password`/
/// `verify_mysql_connection` — spec D2/D3/D7). Same shape as
/// `openvhost_conf::ValidationReport`, kept as its own type rather than
/// reused: these calls also want `stdout` (the `ALTER`/`SELECT` response),
/// which a config-validation report has no reason to carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlCliOutcome {
    /// True iff the CLI exited 0. Never derived from stderr emptiness — the
    /// same discipline `openvhost_conf::MysqlValidator` and every validator
    /// in that crate apply, for the identical reason (a clean run can still
    /// write to stderr).
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

async fn run_mysql_cli(
    cmd: &mut tokio::process::Command,
    stdin: Option<&[u8]>,
) -> Result<MysqlCliOutcome, ConfError> {
    let out = run_bounded(cmd, stdin).await?;
    Ok(MysqlCliOutcome {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
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
/// `stack.rs`'s `mysql_spec` (the SUPERVISOR's ongoing
/// `ReadinessProbe::Command` against the final, running server) — ONE
/// function producing the argv for BOTH call sites is what stops them
/// drifting apart into two subtly different pings; do not fork it.
/// `Vec<String>`, not `Vec<OsString>`:
/// `openvhost_proc::ReadinessProbe::Command.argv` is typed `Vec<String>`,
/// so this shape is dictated by that caller; the lossy `.display()`
/// conversion below is unavoidable for that one, though every OTHER
/// function in this module builds its argv as `OsString` instead.
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

/// The `mariadb-admin ping` argv for the Supervisor's readiness probe on
/// `stack.rs`'s `mariadb_spec` — MariaDB's counterpart to
/// [`mysqladmin_ping_argv`] above, deliberately kept beside it so the ONE
/// place the two engines' pings differ is visible in a single screen.
///
/// **It asserts that the server ANSWERS, not that any credential works.**
/// `ping` exits 0 even when the connection is refused with access denied —
/// the same property [`mysqladmin_ping`] documents for MySQL, verified for
/// MariaDB while building the staged init. That is the right assertion for a
/// readiness probe: `Running` should mean "the socket is up and the server is
/// answering on it", which is exactly what a client needs to know before
/// connecting. It is also the ONLY assertion available here, because spec
/// D4's credential rule (never argv, never env) forbids putting the root
/// password in a `ReadinessProbe::Command`'s `argv` — a `Vec<String>` held
/// for the service's whole lifetime and re-spawned on every attempt, visible
/// in `ps` each time. Nothing downstream may read a passing probe as
/// "the stored password is good"; proving that needs a real connection, and
/// belongs to a verify step that can use an ephemeral 0600 defaults file.
///
/// **The flag list is NOT MySQL's with the names swapped.** MariaDB 11.4.9's
/// `mariadb-admin` rejects `--no-login-paths` outright — `unknown option`,
/// exit 2 — so copying [`mysqladmin_ping_argv`] verbatim would have made
/// every probe attempt fail for a reason having nothing to do with the
/// server, leaving a perfectly healthy MariaDB stuck `Starting` until its
/// deadline and then `Failed`. Nothing is lost by dropping it: that flag
/// exists because MySQL's `--no-defaults` still reads `.mylogin.cnf`, and
/// MariaDB has no login-path file at all (`--help` documents `--no-defaults`
/// as "Don't read default options from any option file", and ships no
/// `mysql_config_editor`), so `--no-defaults` alone is the complete
/// containment here that the pair is for MySQL. Same shape as `--mysqlx=OFF`,
/// which the spec already flagged as not existing on this engine: verify the
/// option against the real binary rather than assuming a shared ancestry.
pub fn mariadb_admin_ping_argv(mariadb_admin: &Path, socket: &Path) -> Vec<String> {
    vec![
        mariadb_admin.display().to_string(),
        "--no-defaults".to_string(),
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
/// `openvhost_conf::probe_nginx_version`: a single failed attempt during a
/// retry loop is an EXPECTED outcome, not an exceptional one — the caller
/// decides how many attempts fit inside its own deadline (spec D2's 10s
/// cap).
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
/// `openvhost_core::mysql::alter_user_sql`'s output, i.e. the freshly
/// generated root password itself, spec D3).
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
    run_mysql_cli(&mut cmd, Some(sql.as_bytes())).await
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
    run_mysql_cli(&mut cmd, Some(sql.as_bytes())).await
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
    run_mysql_cli(&mut cmd, None).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Warmed at creation, outside the `run_bounded` calls these tests then
    /// time — see [`crate::tests_support`] for what that costs and why every
    /// fixture helper in this workspace does it.
    fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        crate::tests_support::write_exec_fixture(&p, body);
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

    /// The MariaDB probe, pinned as an exact list for the same reason the
    /// MySQL one above is — and pinned to be DIFFERENT, which is the part
    /// worth a test.
    ///
    /// `--no-login-paths` is absent because MariaDB 11.4.9's `mariadb-admin`
    /// exits 2 with `unknown option '--no-login-paths'`. That failure mode is
    /// invisible from the Rust side: the Supervisor only ever sees "this
    /// probe did not exit 0", so a healthy server would sit `Starting` for
    /// the full deadline and then report `Failed`, with the real cause an
    /// argument the reader would have to think to suspect. Anyone tidying
    /// these two functions into agreement re-introduces exactly that, so the
    /// assertion below is deliberately spelled out rather than derived from
    /// [`mysqladmin_ping_argv`].
    #[test]
    fn the_mariadb_ping_argv_omits_the_mysql_only_login_path_flag() {
        let argv = mariadb_admin_ping_argv(
            Path::new("/home/packages/mariadb/11.4/11.4.9/bin/mariadb-admin"),
            Path::new("/tmp/ovh/run/mariadb-11.4.sock"),
        );
        assert_eq!(
            argv,
            vec![
                "/home/packages/mariadb/11.4/11.4.9/bin/mariadb-admin".to_string(),
                "--no-defaults".to_string(),
                "--protocol=SOCKET".to_string(),
                "--socket=/tmp/ovh/run/mariadb-11.4.sock".to_string(),
                "--user=root".to_string(),
                "--connect-timeout=1".to_string(),
                "--silent".to_string(),
                "ping".to_string(),
            ]
        );
        // Stated twice on purpose: the list above pins the whole argv, this
        // pins the one element whose PRESENCE would be the regression, so a
        // future reordering of the list cannot quietly reintroduce it.
        assert!(
            !argv.iter().any(|a| a == "--no-login-paths"),
            "mariadb-admin rejects this flag outright: {argv:?}"
        );
        // …and it really is a MySQL-only flag, not one this test forgot to
        // add to both — otherwise the assertion above says nothing.
        assert!(
            mysqladmin_ping_argv(Path::new("/x"), Path::new("/y"))
                .iter()
                .any(|a| a == "--no-login-paths")
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
