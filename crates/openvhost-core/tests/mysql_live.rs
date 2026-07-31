// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in, real-binary end-to-end proof for the MySQL lifecycle slice (P1
//! MySQL lifecycle design: docs/superpowers/specs/2026-07-29-p1-db-mysql-design.md;
//! plan Phase C, Task 7). Drives spec D2's staged-init sequence and spec
//! D4's supervised readiness/grace against a REAL `mysql@8.4` install, then
//! proves clean teardown and zero leaked processes.
//!
//! This mirrors this crate's OTHER live-gated proofs —
//! `crates/openvhost-conf/tests/validate_live.rs` (real-binary validation,
//! `find_brew_binaries()` skip) and `crates/openvhost-pkg/tests/live_net.rs`
//! (`OPENVHOST_NET_TESTS=1` opt-in gate) — rather than
//! `crates/openvhost-core/tests/macos_stack.rs`: that file's own P0-4
//! live-gate (binary-presence-only, no env var, per git ba59762) was retired
//! along with the demo stack it proved (see its current doc comment), so
//! THIS test restores an explicit opt-in env var for the reason
//! `live_net.rs` already established — a real `mysqld` lifecycle (init +
//! start + stop) is heavier than a version probe or a `-t`/`-T` check, so it
//! must not run on every `cargo test --workspace`.
//!
//! Two independent skip gates, checked in order:
//! 1. `OPENVHOST_MYSQL_LIVE_TESTS=1` — the opt-in itself.
//! 2. `mysql@8.4` must already be installed. This test NEVER runs
//!    `brew install` — installing packages is the controller's manual step
//!    (plan Phase C), never something a test performs as a side effect.
//!
//! Hermetic: every path (datadir, my.cnf, sockets, run dir) lives under a
//! freshly created `/tmp` tempdir, never `~/.openvhost` or Homebrew's own
//! `$(brew --prefix)/var/mysql` (spec Owner Caveat 1). This test constructs
//! its "home" directly and passes it explicitly to every function that needs
//! one — exactly like `validate_live.rs`'s `temp_home_ctx` and the
//! historical `macos_stack.rs` (git ba59762) — rather than mutating the
//! process-global `OPENVHOST_HOME` env var (this crate's own convention:
//! `home.rs`'s doc comment already notes "never mutating process env in
//! tests"). `provision_home` is called on that tempdir so the environment
//! matches what a real launch already has in place (`<home>/run` etc.)
//! before any MySQL step runs.
//!
//! The staged-init SEQUENCE (spec D2) is reproduced here directly against
//! real child processes — `mysqld --initialize-insecure`, a network-less
//! temp server, `mysqladmin ping`, `ALTER USER` over stdin, `mysqladmin
//! shutdown` via an ephemeral 0600 defaults-file, then `finalize_staging` —
//! mirroring `apps/desktop/src-tauri/src/commands.rs::run_mysql_init` and
//! `mysql_admin.rs` in argv shape and ordering, WITHOUT depending on the
//! tauri crate (a core-crate test cannot pull in `apps/desktop/src-tauri`).
//! Likewise the `ServiceSpec` this test hands the supervisor is rebuilt
//! inline from spec D4's exact argv rather than importing
//! `stack.rs::mysql_spec` — THAT function is the production twin, and any
//! drift between it and the copy below is exactly what the whole-branch
//! review is expected to catch.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use openvhost_conf::{MysqlCtx, MysqlValidator, run_bounded};
use openvhost_core::mysql::{
    DatadirState, MysqlInitOutcome, MysqlInstanceRepo, MysqlMajor, MysqlPaths, MysqlRuntime,
    RootPassword, alter_user_sql, classify_datadir, discover_mysql, finalize_staging,
    generate_root_password, mysql_paths, staging_dir_path, write_generated_config,
};
use openvhost_core::{BREW_PREFIXES, Db};
use openvhost_proc::{
    OutputStream, ProcessDriver, ReadinessProbe, ServiceSpec, ServiceState, SpawnSpec,
    SpawnedChild, Supervisor, SupervisorEvent, TaskEvent, default_driver, run_task,
};
use tokio::sync::broadcast;

/// The opt-in gate (see module doc). Mirrors
/// `openvhost-pkg/tests/live_net.rs`'s `OPENVHOST_NET_TESTS` naming
/// convention.
const LIVE_ENV_VAR: &str = "OPENVHOST_MYSQL_LIVE_TESTS";

/// The one major this slice supports (spec D1) — the only version this test
/// looks for.
const TARGET_MAJOR: &str = "8.4";

/// Generous but bounded: real `mysqld --initialize-insecure` normally takes
/// 1-3s on modern hardware; this is a safety net, never the expected
/// duration.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
/// Spec D2 step 3's own cap on the temp server's readiness poll.
const TEMP_SERVER_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Spec D4's readiness deadline for the supervised, real server.
const SUPERVISOR_READY_DEADLINE: Duration = Duration::from_secs(15);
/// Spec D4's stop grace (SIGTERM must succeed well inside this).
const SUPERVISOR_GRACE: Duration = Duration::from_secs(15);
/// How many of a captured child's most recent output lines a failure message
/// embeds (the failure-reporting fix): enough to show a real mysqld's own
/// fatal-error banner without dumping an unbounded startup log into a panic
/// message.
const OUTPUT_TAIL_LINES: usize = 20;

// ---------------------------------------------------------------------------
// RAII guards — never leak a real child on an early return or a panic.
// ---------------------------------------------------------------------------

/// An ephemeral, 0600, RAII-deleted MySQL `--defaults-file` carrying a
/// credential (spec D2 step 5 / D3). Mirrors
/// `apps/desktop/src-tauri/src/commands.rs::EphemeralDefaultsFile` exactly:
/// mode 0600 from the FIRST byte on disk (`create_new` + `mode` together,
/// never a separate `chmod` after an unprotected write), RAII-deleted by
/// `Drop`. Rebuilt here because that type is private to the tauri crate and
/// this is a core-crate test.
struct EphemeralDefaultsFile {
    path: PathBuf,
}

impl EphemeralDefaultsFile {
    fn write(socket: &Path, password: &RootPassword) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let run_dir = socket.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(run_dir)?;
        let name = format!(".mysql-defaults-{}", uuid::Uuid::new_v4().simple());
        let path = run_dir.join(name);
        let contents = format!(
            "[client]\nuser=root\npassword={}\nsocket={}\nprotocol=SOCKET\n",
            password.expose(),
            socket.display()
        );
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        use std::io::Write as _;
        if let Err(e) = f.write_all(contents.as_bytes()) {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        Ok(Self { path })
    }
}

impl Drop for EphemeralDefaultsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Kills the network-less temp server if this test panics/returns before it
/// was deliberately shut down. Mirrors
/// `apps/desktop/src-tauri/src/commands.rs::TempServerGuard` exactly.
struct TempServerGuard {
    driver: Arc<dyn ProcessDriver>,
    child: SpawnedChild,
    finished: bool,
}

impl Drop for TempServerGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.driver.kill(&mut self.child);
        }
    }
}

/// Force-stops the supervised service on drop so a panicking assertion
/// never leaks the real mysqld — mirrors `openvhost-proc/tests/e2e.rs` and
/// `readiness.rs`'s identical `StopGuard`.
struct StopGuard<'a> {
    sup: &'a Supervisor,
    id: String,
}

impl Drop for StopGuard<'_> {
    fn drop(&mut self) {
        let _ = self.sup.stop(&self.id);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let all_terminal = self
                .sup
                .snapshot()
                .iter()
                .all(|s| !matches!(s.state, ServiceState::Starting | ServiceState::Running));
            if all_terminal {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

// ---------------------------------------------------------------------------
// Small pure / sync helpers.
// ---------------------------------------------------------------------------

/// `/tmp`, not `TMPDIR` (`/var/folders/...`): short base-path headroom for
/// the unix socket `sun_path` guard (`MysqlPaths::check_socket_lengths`) —
/// mirrors `validate_live.rs` and the historical `macos_stack.rs` (git
/// ba59762) exactly.
fn hermetic_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ovhmysql")
        .tempdir_in("/tmp")
        .expect("failed to create a hermetic /tmp home")
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

/// Mirrors `apps/desktop/src-tauri/src/mysql_admin.rs::mysqladmin_ping_argv`
/// and `stack.rs::mysql_spec` (spec D4) EXACTLY — this IS the production
/// readiness-probe argv, rebuilt here because a core-crate test cannot
/// import the tauri crate. Drift between the two is exactly what the
/// whole-branch review is expected to catch.
fn mysqladmin_ping_argv(mysqladmin: &Path, socket: &Path) -> Vec<String> {
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

/// `mysqld --no-defaults --initialize-insecure --datadir=<staging>
/// --mysqlx=OFF` (spec D2 step 1). Mirrors `commands.rs::mysqld_init_spec`.
///
/// `--no-defaults` is deliberate containment, kept on its own merits — NOT a
/// fix for a datadir-mismatch bug, which does not exist (an earlier fix
/// wave claimed combining `--defaults-file=<my_cnf>` with argv
/// `--datadir=<staging>` corrupted InnoDB's undo-tablespace bookkeeping;
/// that diagnosis was WRONG, a misdiagnosis of the leading-dot
/// staging-basename bug, and is retracted — see spec D2's dated correction
/// note for the corrected history). A SEPARATE earlier claim — that
/// `--no-defaults` gains exclusion of machine-wide option files
/// (`/etc/my.cnf`, `~/.my.cnf`) — was ALSO wrong: `--defaults-file=<path>`
/// already excludes those on its own. The genuine gain: the rendered
/// my.cnf ends with `!includedir <custom_confd>`, so under `--defaults-file`
/// this step would read whatever the USER has dropped into that directory
/// — arbitrary user-controlled configuration reaching the init sequence.
/// `--no-defaults` removes all of it.
///
/// `--mysqlx=OFF`: this step starts no server at all (`--initialize-insecure`
/// writes the datadir and exits), so there is no listener of any kind here
/// regardless — added purely for symmetry with [`mysqld_temp_server_spec`]
/// below, where the identical flag is load-bearing, not decorative.
fn mysqld_init_spec(mysqld: &Path, staging: &Path) -> SpawnSpec {
    let mut datadir_arg = OsString::from("--datadir=");
    datadir_arg.push(staging.as_os_str());
    SpawnSpec {
        program: mysqld.to_path_buf(),
        args: vec![
            OsString::from("--no-defaults"),
            OsString::from("--initialize-insecure"),
            datadir_arg,
            OsString::from("--mysqlx=OFF"),
        ],
        cwd: None,
        env: vec![],
    }
}

/// `mysqld --no-defaults --datadir=<staging> --skip-networking
/// --socket=<init_socket> --mysqlx=OFF` (spec D2 step 2). Mirrors
/// `commands.rs::mysqld_temp_server_spec`. Same deliberate-containment
/// reasoning for `--no-defaults` as [`mysqld_init_spec`] above. This step's
/// real STARTUP failure mode, confirmed live and unrelated to
/// `--defaults-file`, was a datadir basename starting with a dot (the
/// fatal InnoDB undo-tablespace error, "Can't create UNDO tablespace
/// innodb_undo_001 since './undo_001' already exists", is genuinely what a
/// real mysqld prints for that cause too — see `staging_dir_path`'s doc
/// comment and spec D2's dated correction note).
///
/// `--mysqlx=OFF` is LOAD-BEARING here, not decorative — do not "simplify"
/// it away. Measured directly against real mysql@8.4.11: with this flag
/// absent (mysqlx at its default), this exact invocation binds
/// `/tmp/mysqlx.sock` at mode `srwxrwxrwx` — world read/write, OUTSIDE the
/// 0700 home this app otherwise confines every socket to — AND `*:33060`.
/// `--skip-networking` suppresses the CLASSIC protocol's TCP listener only;
/// it does not touch the X Plugin's listener, unix socket or TCP, at all.
/// Both are live for the entire window between this server starting and
/// `SetPassword`'s `ALTER USER` succeeding — i.e. while `root@localhost`
/// still has the EMPTY password `--initialize-insecure` left it with. Any
/// local user (not just a network-adjacent one) could connect as root
/// through that world-writable socket during that window. Adding
/// `--mysqlx=OFF` was verified, in the same measurement session, to bind no
/// socket at all.
fn mysqld_temp_server_spec(mysqld: &Path, staging: &Path, init_socket: &Path) -> SpawnSpec {
    let mut datadir_arg = OsString::from("--datadir=");
    datadir_arg.push(staging.as_os_str());
    SpawnSpec {
        program: mysqld.to_path_buf(),
        args: vec![
            OsString::from("--no-defaults"),
            datadir_arg,
            OsString::from("--skip-networking"),
            socket_arg(init_socket),
            OsString::from("--mysqlx=OFF"),
        ],
        cwd: None,
        env: vec![],
    }
}

/// Mirrors `commands.rs::parse_version_and_port`: one line, tab-separated,
/// no header (`--batch --skip-column-names`).
fn parse_version_and_port(stdout: &str) -> Option<(String, u32)> {
    let line = stdout.lines().next()?;
    let mut cols = line.split('\t');
    let version = cols.next()?.trim().to_string();
    let port: u32 = cols.next()?.trim().parse().ok()?;
    (!version.is_empty()).then_some((version, port))
}

/// A deterministic snapshot of every regular file under `dir`: (path
/// relative to `dir`, byte length, modified time), sorted. Used to prove
/// `MysqlValidator::validate` does not touch the datadir it is pointed at
/// (spec D5 caveat i) — any write, even a zero-length touch, changes at
/// least one entry.
fn datadir_fingerprint(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue; // vanished mid-walk or unreadable: nothing to fingerprint
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((rel, meta.len(), mtime));
            }
        }
    }
    out.sort();
    out
}

/// A quick, bounded probe: is ANYTHING already listening on
/// `127.0.0.1:port`? Guards against a pre-existing `brew services`-managed
/// mysqld already holding 3306 (spec Owner Caveat 1) — a condition this test
/// cannot fix (it must never stop another process) and must not be mistaken
/// for a bug in this slice.
fn port_in_use(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_ok()
}

/// The last `n` lines of `lines`, joined — bounds how much captured child
/// output a failure message embeds. Failure-reporting fix: every step's
/// panic now calls this rather than omitting captured output entirely (a
/// real StartTempServer failure once cost a blind debugging round precisely
/// because its panic had nothing captured to show).
fn tail(lines: &[String], n: usize) -> String {
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Mirrors `openvhost-proc/tests/readiness.rs::pid_is_alive`.
fn pid_is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 performs no action; it only checks existence/permission.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// The pgrep-style sweep the task brief asks for: any process whose FULL
/// command line still mentions the hermetic home's path, after every
/// managed child has been stopped. Catches a leak this test's own two
/// tracked pids (the temp server, the supervised server) would miss if a
/// THIRD, untracked process ever appeared (e.g. a forked grandchild).
fn pgrep_matches(needle: &str) -> String {
    std::process::Command::new("/usr/bin/pgrep")
        .args(["-f", needle])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// Secrets discipline (spec D3): the password must never be observable in
/// captured child output. Panics with `label` so a failure names WHICH call
/// leaked it.
fn assert_no_secret(label: &str, text: &str, secret: &str) {
    assert!(
        !text.contains(secret),
        "{label}: captured command output contains the plaintext secret: {text:?}"
    );
}

// ---------------------------------------------------------------------------
// Async helpers.
// ---------------------------------------------------------------------------

/// Probe every known Homebrew prefix for an installed `mysql@8.4`, mirroring
/// `commands.rs::discover_all_mysql`'s `spawn_blocking` + `Handle::block_on`
/// bridge exactly (`discover_mysql`'s probe closure must be synchronous;
/// `probe_mysqld_version` is not).
async fn discover_target_runtime() -> Option<MysqlRuntime> {
    let prefixes: Vec<&'static Path> = BREW_PREFIXES.iter().map(Path::new).collect();
    let found = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            discover_mysql(&prefixes, &|bin| {
                handle.block_on(openvhost_conf::probe_mysqld_version(bin))
            })
        }),
    )
    .await
    .expect("mysql discovery timed out")
    .expect("the discovery blocking task panicked");
    found
        .into_iter()
        .find(|rt| rt.major.as_str() == TARGET_MAJOR)
}

/// Run `spec` to completion via `openvhost_proc::run_task` (its own
/// process-group containment + kill-on-drop), bounded by `timeout` — unlike
/// `run_bounded`'s fixed 5s `PROBE_TIMEOUT` (meant for short admin-CLI
/// calls), `mysqld --initialize-insecure` can legitimately take a few
/// seconds, so this accepts its own, more generous bound.
///
/// Returns the exit code (or `None` if killed by a signal) ALONGSIDE every
/// captured stdout/stderr line — failure-reporting fix: the OLD version
/// only surfaced captured output on a TIMEOUT, so a real mysqld's own
/// fatal-error banner on a non-zero exit (exactly what
/// `--initialize-insecure` would print) was silently dropped, leaving the
/// caller's own exit-code assertion with nothing to show. The caller decides
/// how much of `lines` to embed (via `tail`) in its own message.
async fn run_to_completion(
    spec: SpawnSpec,
    timeout: Duration,
) -> Result<(Option<i32>, Vec<String>), String> {
    let program = spec.program.display().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let drain = tokio::spawn(async move {
        let mut lines = Vec::new();
        while let Some(ev) = rx.recv().await {
            if let TaskEvent::Line { text, .. } = ev {
                lines.push(text);
            }
        }
        lines
    });
    let result = tokio::time::timeout(timeout, run_task(default_driver(), spec, tx)).await;
    let lines = drain.await.unwrap_or_default();
    match result {
        Ok(Ok(code)) => Ok((code, lines)),
        Ok(Err(e)) => Err(format!("{program} failed to run: {e}")),
        Err(_) => Err(format!(
            "{program} did not finish within {timeout:?}; last output:\n{}",
            tail(&lines, OUTPUT_TAIL_LINES)
        )),
    }
}

/// ONE `mysqladmin ping` attempt — succeeds even against a server that would
/// deny authentication (it proves only that something is listening on
/// `socket`). Mirrors `mysql_admin.rs::mysqladmin_ping`.
async fn mysqladmin_ping(mysqladmin: &Path, socket: &Path) -> bool {
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

/// Poll `mysqladmin ping` against `socket` until it succeeds, the temp
/// server dies on its own, or `deadline` elapses (spec D2 step 3's 10s cap).
/// Mirrors `commands.rs::poll_until_ready`.
async fn poll_until_ready(
    mysqladmin: &Path,
    socket: &Path,
    server_child: &mut SpawnedChild,
    deadline: Duration,
) -> bool {
    let deadline_at = Instant::now() + deadline;
    loop {
        if mysqladmin_ping(mysqladmin, socket).await {
            return true;
        }
        if matches!(server_child.try_wait(), Ok(Some(_))) {
            return false; // the temp server died on its own
        }
        if Instant::now() >= deadline_at {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Drains a pipe to completion, appending each line to `into` (shared with
/// the OTHER stream of the same child, so stdout and stderr interleave into
/// one chronological-ish tail). Without draining, a chatty startup could
/// fill the pipe and stall the child (mirrors
/// `service_task::spawn_reader`'s hands-off drain).
///
/// Failure-reporting fix: this REPLACES a previous `drain_silently`, which
/// discarded everything on the theory that the temp server's own
/// stdout/stderr was never needed — a real StartTempServer failure proved
/// that theory wrong: mysqld's own fatal-error banner (e.g. the InnoDB
/// undo-tablespace error) landed on this EXACT stream, and discarding it
/// meant the failing panic had nothing to show, costing a blind debugging
/// round. The caller bounds how much of `into` it embeds in a panic message
/// via `tail`.
async fn drain_capturing(stream: OutputStream, into: Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        into.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }
}

/// Mirrors `mysql_admin.rs::mysql_exec_with_defaults_file`:
/// `--defaults-file=<path> --batch --skip-column-names`, `sql` over stdin —
/// never argv/env. `--batch --skip-column-names` makes a query's output
/// deterministic (tab-separated, no header) instead of depending on
/// `mysql`'s own tty auto-detection.
async fn exec_via_defaults_file(
    mysql_bin: &Path,
    defaults_file: &Path,
    sql: &str,
) -> (bool, String, String) {
    let mut cmd = tokio::process::Command::new(mysql_bin);
    cmd.arg(defaults_file_arg(defaults_file))
        .arg("--batch")
        .arg("--skip-column-names");
    match run_bounded(&mut cmd, Some(sql.as_bytes())).await {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

/// Write an ephemeral 0600 defaults-file authenticating as `password`, run
/// `sql` through it, then delete the file — mirrors every defaults-file call
/// site in `commands.rs`: "RAII delete, before acting on the result".
async fn exec_as(
    mysql_bin: &Path,
    socket: &Path,
    password: &RootPassword,
    sql: &str,
) -> (bool, String, String) {
    let defaults = EphemeralDefaultsFile::write(socket, password)
        .expect("failed to write the ephemeral 0600 defaults-file");
    let result = exec_via_defaults_file(mysql_bin, &defaults.path, sql).await;
    drop(defaults);
    result
}

/// Mirrors `openvhost-proc/tests/readiness.rs::wait_state` exactly: consume
/// events until a `StateChanged` for `id` satisfies `pred`, or panic at
/// timeout. Event-driven — never sleep-and-hope.
async fn wait_state(
    rx: &mut broadcast::Receiver<SupervisorEvent>,
    id: &str,
    timeout: Duration,
    pred: impl Fn(&ServiceState) -> bool,
) -> ServiceState {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for a matching state on '{id}'"
        );
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(SupervisorEvent::StateChanged { id: eid, state, .. }))
                if eid == id && pred(&state) =>
            {
                return state;
            }
            // Exhaustive rather than `Ok(Ok(_))`: a new `SupervisorEvent`
            // variant must fail to compile HERE too. A wildcard would keep
            // compiling and keep skipping — which is correct for THIS helper
            // (it only ever waits on states), but it is exactly how a variant
            // that a future waiter DOES need slips past unnoticed.
            Ok(Ok(SupervisorEvent::StateChanged { .. }))
            | Ok(Ok(SupervisorEvent::Log { .. }))
            | Ok(Ok(SupervisorEvent::Registered { .. }))
            | Ok(Ok(SupervisorEvent::Unregistered { .. })) => continue,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("event channel closed while waiting on '{id}': {e}"),
            Err(_) => panic!("timed out waiting for a matching state on '{id}'"),
        }
    }
}

/// Drives spec D2's staged-init sequence against a real `mysqld`, end to
/// end: `--initialize-insecure` into a staging dir, a network-less temp
/// server, a bounded `mysqladmin ping` poll, `ALTER USER` over stdin against
/// the unauthenticated root account, `mysqladmin shutdown` via an ephemeral
/// 0600 defaults-file, then `finalize_staging`. Mirrors
/// `apps/desktop/src-tauri/src/commands.rs::run_mysql_init` in argv shape
/// and ordering — that function is the production twin; it cannot be called
/// from here (it needs a `tauri::AppHandle` to stream log events), so this
/// reproduces the SEQUENCE against the same real binaries and the same
/// `openvhost_core::mysql` primitives (`staging_dir_path`,
/// `finalize_staging`) rather than re-testing a fake.
///
/// Panics with a step-labeled message on any failure — this test proves the
/// happy path; failure-path behavior (Foreign datadir, a rejected config,
/// mid-sequence errors) is already covered by Task 4/5's fake-binary and
/// unit tests.
async fn run_staged_init(
    runtime: &MysqlRuntime,
    paths: &MysqlPaths,
    major: &MysqlMajor,
) -> RootPassword {
    let staging = staging_dir_path(&paths.staging_parent, major);
    std::fs::create_dir_all(&staging).expect("failed to create the staging directory");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))
            .expect("failed to lock down staging directory permissions");
    }

    // ---- Initialize (step 1) ----
    let init_spec = mysqld_init_spec(&runtime.mysqld, &staging);
    let (init_code, init_output) = run_to_completion(init_spec, INITIALIZE_TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("Initialize: {e}"));
    assert_eq!(
        init_code,
        Some(0),
        "Initialize: mysqld --initialize-insecure must exit 0, got {init_code:?}; last output:\n{}",
        tail(&init_output, OUTPUT_TAIL_LINES)
    );

    // ---- StartTempServer (step 2) ----
    let run_dir = paths.init_socket.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(run_dir).expect("failed to create the run directory");
    let temp_spec = mysqld_temp_server_spec(&runtime.mysqld, &staging, &paths.init_socket);
    let driver = default_driver();
    let mut server = TempServerGuard {
        driver: Arc::clone(&driver),
        child: driver
            .spawn(&temp_spec)
            .expect("StartTempServer: failed to spawn the temp server"),
        finished: false,
    };
    // Shared, captured (not discarded — the failure-reporting fix): a real
    // mysqld's own fatal-error banner lands on THESE exact streams, so every
    // panic below that could plausibly be caused by the temp server itself
    // embeds a tail of this buffer.
    let temp_server_output: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    if let Some(out) = server.child.take_stdout() {
        tokio::spawn(drain_capturing(out, Arc::clone(&temp_server_output)));
    }
    if let Some(err) = server.child.take_stderr() {
        tokio::spawn(drain_capturing(err, Arc::clone(&temp_server_output)));
    }
    let temp_server_tail = || {
        let captured = temp_server_output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tail(&captured, OUTPUT_TAIL_LINES)
    };

    let ready = poll_until_ready(
        &runtime.mysqladmin,
        &paths.init_socket,
        &mut server.child,
        TEMP_SERVER_READY_TIMEOUT,
    )
    .await;
    if !ready {
        let _ = driver.kill(&mut server.child);
        let _ = server.child.wait().await;
        server.finished = true;
        panic!(
            "StartTempServer: the temp server never answered mysqladmin ping within \
             {TEMP_SERVER_READY_TIMEOUT:?}; last output:\n{}",
            temp_server_tail()
        );
    }

    // ---- SetPassword (step 4) — unauthenticated: root@localhost has an
    // empty password right after --initialize-insecure. ----
    let password = generate_root_password();
    let alter_sql = alter_user_sql(&password);
    let mut alter_cmd = tokio::process::Command::new(&runtime.mysql);
    alter_cmd
        .arg("--no-defaults")
        .arg("--protocol=SOCKET")
        .arg(socket_arg(&paths.init_socket))
        .arg("--user=root");
    let alter_out = run_bounded(&mut alter_cmd, Some(alter_sql.as_bytes()))
        .await
        .unwrap_or_else(|e| panic!("SetPassword: {e}"));
    if !alter_out.status.success() {
        let _ = driver.kill(&mut server.child);
        let _ = server.child.wait().await;
        server.finished = true;
        panic!(
            "SetPassword: unauthenticated ALTER USER failed:\n{}",
            String::from_utf8_lossy(&alter_out.stderr)
        );
    }
    assert_no_secret(
        "SetPassword stdout",
        &String::from_utf8_lossy(&alter_out.stdout),
        password.expose(),
    );
    assert_no_secret(
        "SetPassword stderr",
        &String::from_utf8_lossy(&alter_out.stderr),
        password.expose(),
    );

    // ---- Shutdown (step 5) ----
    let shutdown_defaults = EphemeralDefaultsFile::write(&paths.init_socket, &password)
        .unwrap_or_else(|e| panic!("Shutdown: failed to write the ephemeral defaults-file: {e}"));
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&shutdown_defaults.path)
            .expect("ephemeral defaults-file must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the ephemeral defaults-file must be created at mode 0600"
        );
    }
    let mut shutdown_cmd = tokio::process::Command::new(&runtime.mysqladmin);
    shutdown_cmd
        .arg(defaults_file_arg(&shutdown_defaults.path))
        .arg("shutdown");
    let shutdown_out = run_bounded(&mut shutdown_cmd, None)
        .await
        .unwrap_or_else(|e| panic!("Shutdown: {e}"));
    drop(shutdown_defaults); // RAII-delete before acting on the result, mirroring production.
    if !shutdown_out.status.success() {
        let _ = driver.kill(&mut server.child);
        let _ = server.child.wait().await;
        server.finished = true;
        panic!(
            "Shutdown: mysqladmin shutdown failed:\n{}",
            String::from_utf8_lossy(&shutdown_out.stderr)
        );
    }
    assert_no_secret(
        "Shutdown stdout",
        &String::from_utf8_lossy(&shutdown_out.stdout),
        password.expose(),
    );
    assert_no_secret(
        "Shutdown stderr",
        &String::from_utf8_lossy(&shutdown_out.stderr),
        password.expose(),
    );

    match tokio::time::timeout(Duration::from_secs(10), server.child.wait()).await {
        Ok(_) => server.finished = true,
        Err(_) => {
            let _ = driver.kill(&mut server.child);
            let _ = server.child.wait().await;
            server.finished = true;
            panic!(
                "Shutdown: the temp server did not exit after mysqladmin shutdown succeeded; \
                 last output:\n{}",
                temp_server_tail()
            );
        }
    }

    // ---- Finalize (step 6) ----
    let outcome = finalize_staging(&staging, &paths.datadir);
    assert_eq!(
        outcome,
        MysqlInitOutcome::Initialized,
        "Finalize: expected Initialized, got {outcome:?}"
    );

    password
}

// ---------------------------------------------------------------------------
// The one test.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn mysql_lifecycle_end_to_end_against_real_mysqld() {
    if std::env::var(LIVE_ENV_VAR).as_deref() != Ok("1") {
        eprintln!(
            "SKIP mysql_live: set {LIVE_ENV_VAR}=1 to run the real mysql@8.4 lifecycle proof \
             (requires `brew install mysql@8.4` first — this test never installs it)"
        );
        return;
    }

    // ---- Discovery (spec D1) ----
    let Some(runtime) = discover_target_runtime().await else {
        eprintln!(
            "SKIP mysql_live: mysql@8.4 is not installed (`brew install mysql@8.4`) — this test \
             does not install it; that is the controller's manual step"
        );
        return;
    };
    assert!(
        runtime.mysqld.is_file(),
        "discovery must resolve a real mysqld binary"
    );
    assert!(
        runtime.mysql.is_file(),
        "discovery must resolve a real mysql client binary"
    );
    assert!(
        runtime.mysqladmin.is_file(),
        "discovery must resolve a real mysqladmin binary"
    );
    assert!(
        runtime.major.is_cataloged(),
        "8.4 must be a cataloged major"
    );
    let major = runtime.major.clone();

    // ---- Hermetic home ----
    let home_dir = hermetic_home();
    let home = home_dir.path();
    openvhost_core::platform::macos::demo_stack::provision_home(home)
        .expect("provision_home must succeed against a fresh tempdir");
    let paths = mysql_paths(home, &major);
    paths
        .check_socket_lengths()
        .expect("a /tmp-based hermetic home must fit the sun_path guard");

    // ---- Render + validate my.cnf (spec D5); datadir untouched pre-init ----
    //
    // POST-LIVE-RUN FINDING (the reason this call site's arguments changed):
    // a REAL mysqld treats a missing `!includedir` target as FATAL to its
    // defaults-file handling — this test originally hit "Fatal error in
    // defaults handling. Program aborted!" right here, at `--validate-config`,
    // because nothing had ever created `custom_confd`. The fix lives in
    // `write_generated_config` itself (the production chokepoint every
    // producer of a my.cnf writes through), NOT a hand-rolled
    // `create_dir_all` bolted onto this test — calling the SAME function
    // production calls, with the SAME arguments, is what makes this test a
    // genuine twin of the real path rather than a workaround that only
    // proves the test's own patched-over version works.
    let ctx = MysqlCtx {
        my_cnf: paths.my_cnf.clone(),
        datadir: paths.datadir.clone(),
        socket: paths.socket.clone(),
        pid_file: paths.pid_file.clone(),
        custom_confd: paths.custom_confd.clone(),
    };
    let generated = openvhost_conf::generate_my_cnf(&ctx).expect("my.cnf must render");
    write_generated_config(&generated, &paths.custom_confd).expect("my.cnf must write atomically");
    assert!(
        !paths.datadir.exists(),
        "the datadir must not exist before init has ever run"
    );

    let validator = MysqlValidator {
        mysqld: runtime.mysqld.clone(),
    };
    let pre_init = validator
        .validate(&paths.my_cnf)
        .await
        .expect("mysqld --validate-config must be spawnable");
    assert!(
        pre_init.ok,
        "mysqld --validate-config rejected the generated my.cnf:\n{}",
        pre_init.stderr
    );
    assert!(
        !paths.datadir.exists(),
        "spec D5 caveat (i): --validate-config must not CREATE a nonexistent datadir"
    );

    // ---- Staged init (spec D2) ----
    let password = run_staged_init(&runtime, &paths, &major).await;
    assert!(
        paths.datadir.join("mysql").is_dir(),
        "sentinel dir missing after finalize"
    );
    assert!(
        paths.datadir.join("auto.cnf").is_file(),
        "sentinel file missing after finalize"
    );
    assert!(matches!(
        classify_datadir(&paths.datadir).expect("classify_datadir must succeed"),
        DatadirState::Initialized
    ));

    // ---- spec D5 caveat (i), definitive form: validate against the REAL,
    // populated datadir and prove nothing about it changed. ----
    let fp_before = datadir_fingerprint(&paths.datadir);
    let mtime_before = std::fs::metadata(&paths.datadir)
        .expect("datadir must exist")
        .modified()
        .expect("mtime must be readable");
    let post_init = validator
        .validate(&paths.my_cnf)
        .await
        .expect("mysqld --validate-config must be spawnable");
    assert!(
        post_init.ok,
        "mysqld --validate-config rejected the REAL initialized datadir's my.cnf:\n{}",
        post_init.stderr
    );
    let fp_after = datadir_fingerprint(&paths.datadir);
    let mtime_after = std::fs::metadata(&paths.datadir)
        .expect("datadir must exist")
        .modified()
        .expect("mtime must be readable");
    assert_eq!(
        fp_before, fp_after,
        "spec D5 caveat (i): --validate-config must not touch the datadir's file contents"
    );
    assert_eq!(
        mtime_before, mtime_after,
        "spec D5 caveat (i): --validate-config must not touch the datadir's own mtime"
    );

    // ---- Persist the generated credential (spec D3) ----
    let db = Db::open_in_memory()
        .await
        .expect("in-memory state.db must open");
    let repo = MysqlInstanceRepo::new(&db);
    repo.upsert(&major, &password)
        .await
        .expect("failed to persist the generated root password");
    let stored = repo
        .get(&major)
        .await
        .expect("repo.get must succeed")
        .expect("must find the just-persisted instance");
    assert_eq!(stored.root_password.expose(), password.expose());
    assert_eq!(stored.major, major);

    // ---- Supervise the FINAL server (spec D4) ----
    assert!(
        !port_in_use(3306),
        "127.0.0.1:3306 is already bound by something else — stop it first \
         (e.g. `brew services stop mysql@8.4`) and re-run"
    );

    let service_id = format!("mysql-{}", major.as_str());
    let mut defaults_arg = OsString::from("--defaults-file=");
    defaults_arg.push(paths.my_cnf.as_os_str());
    let spec = ServiceSpec {
        id: service_id.clone(),
        display_name: format!("MySQL {}", major.as_str()),
        endpoint: Some("127.0.0.1:3306".to_string()),
        spawn: SpawnSpec {
            program: runtime.mysqld.clone(),
            args: vec![defaults_arg],
            cwd: None,
            env: vec![],
        },
        readiness: ReadinessProbe::Command {
            argv: mysqladmin_ping_argv(&runtime.mysqladmin, &paths.socket),
            deadline: SUPERVISOR_READY_DEADLINE,
        },
        grace: SUPERVISOR_GRACE,
    };

    let sup = Supervisor::new(default_driver());
    sup.register(spec);
    let _guard = StopGuard {
        sup: &sup,
        id: service_id.clone(),
    };
    let mut rx = sup.subscribe();
    sup.start(&service_id)
        .expect("start must succeed for a freshly registered service");

    let starting = wait_state(&mut rx, &service_id, Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Starting)
    })
    .await;
    assert!(matches!(starting, ServiceState::Starting));

    // The load-bearing event-order proof (spec D4): the FIRST non-Starting
    // state must be Running, never Failed — a corrupted readiness probe
    // (e.g. the wrong socket path) must break exactly this assertion. See
    // the plan's Task 7 mutation-check requirement.
    let ready = wait_state(
        &mut rx,
        &service_id,
        SUPERVISOR_READY_DEADLINE + Duration::from_secs(20),
        |s| !matches!(s, ServiceState::Starting),
    )
    .await;
    assert!(
        matches!(ready, ServiceState::Running),
        "expected Running, got {ready:?} — the D4 readiness probe never confirmed the real \
         server ready"
    );

    let mysqld_pid = sup
        .snapshot()
        .into_iter()
        .find(|s| s.id == service_id)
        .and_then(|s| s.pid)
        .expect("a Running service must report a pid");

    // ---- SELECT VERSION() proof ----
    let (ok, stdout, stderr) = exec_as(
        &runtime.mysql,
        &paths.socket,
        &password,
        "SELECT VERSION(), @@port;",
    )
    .await;
    assert!(ok, "SELECT VERSION(), @@port failed:\n{stderr}");
    assert_no_secret("SELECT VERSION() stdout", &stdout, password.expose());
    let (version, port) =
        parse_version_and_port(&stdout).unwrap_or_else(|| panic!("unparseable output: {stdout:?}"));
    assert!(
        version.starts_with("8.4"),
        "expected a version starting with 8.4, got {version:?}"
    );
    assert_eq!(port, 3306);

    // ---- ALTER USER password rotation round-trip (spec D3: "reset by
    // regenerate") ----
    let new_password = generate_root_password();
    assert_ne!(password.expose(), new_password.expose());
    let rotate_sql = alter_user_sql(&new_password);
    let (ok, stdout, stderr) = exec_as(&runtime.mysql, &paths.socket, &password, &rotate_sql).await;
    assert!(ok, "ALTER USER (rotate) failed:\n{stderr}");
    assert_no_secret("ALTER USER rotate stdout", &stdout, password.expose());
    assert_no_secret("ALTER USER rotate stdout", &stdout, new_password.expose());
    assert_no_secret("ALTER USER rotate stderr", &stderr, password.expose());
    assert_no_secret("ALTER USER rotate stderr", &stderr, new_password.expose());
    repo.upsert(&major, &new_password)
        .await
        .expect("failed to persist the rotated password");
    let stored_after_rotate = repo
        .get(&major)
        .await
        .expect("repo.get must succeed")
        .expect("must still find the instance");
    assert_eq!(
        stored_after_rotate.root_password.expose(),
        new_password.expose()
    );

    // OLD password must now fail authentication.
    let (old_ok, _old_stdout, old_stderr) =
        exec_as(&runtime.mysql, &paths.socket, &password, "SELECT 1;").await;
    assert!(!old_ok, "the OLD password must be REJECTED after rotation");
    assert!(
        old_stderr.contains("Access denied"),
        "expected an Access-denied auth failure for the old password, got:\n{old_stderr}"
    );
    assert_no_secret("old-password-fails stderr", &old_stderr, password.expose());
    assert_no_secret(
        "old-password-fails stderr",
        &old_stderr,
        new_password.expose(),
    );

    // NEW password must now work.
    let (new_ok, new_stdout, new_stderr) =
        exec_as(&runtime.mysql, &paths.socket, &new_password, "SELECT 1;").await;
    assert!(
        new_ok,
        "the NEW password must authenticate successfully:\n{new_stderr}"
    );
    assert_no_secret(
        "new-password-works stdout",
        &new_stdout,
        new_password.expose(),
    );

    // ---- Clean stop within grace (spec D4) ----
    let stop_t0 = Instant::now();
    sup.stop(&service_id).expect("stop must succeed");
    let stopped = wait_state(
        &mut rx,
        &service_id,
        SUPERVISOR_GRACE + Duration::from_secs(10),
        |s| matches!(s, ServiceState::Stopped | ServiceState::Failed { .. }),
    )
    .await;
    let elapsed = stop_t0.elapsed();
    assert!(
        matches!(stopped, ServiceState::Stopped),
        "expected a clean Stopped, got {stopped:?}"
    );
    assert!(
        elapsed < SUPERVISOR_GRACE,
        "stop took {elapsed:?}, at/after the {SUPERVISOR_GRACE:?} grace — a mostly-idle mysqld \
         should exit cleanly on SIGTERM well before that"
    );
    let tail = sup
        .log_tail(&service_id, 500)
        .expect("log_tail must succeed for a known id");
    assert!(
        !tail.iter().any(|l| l.line.contains("killing")),
        "no SIGKILL escalation was expected within grace: {tail:?}"
    );

    // ---- Zero orphaned processes ----
    let pid_deadline = Instant::now() + Duration::from_secs(5);
    while pid_is_alive(mysqld_pid as i32) {
        assert!(
            Instant::now() < pid_deadline,
            "supervised mysqld pid {mysqld_pid} still alive after Stopped"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let home_needle = home.to_string_lossy().into_owned();
    let leaked = pgrep_matches(&home_needle);
    assert!(
        leaked.trim().is_empty(),
        "process(es) still reference the tempdir after full teardown:\n{leaked}"
    );
}
