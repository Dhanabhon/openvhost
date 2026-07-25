// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only inspection of an installed web server: what version is it, and is
//! a given config file valid? Both shell out, so both are bounded by
//! `PROBE_TIMEOUT` and kill the child on expiry — these run behind a UI action,
//! where an unbounded wait is a hung spinner.
//!
//! Deliberately NOT `WebServerAdapter::validate`: that materializes generated
//! files into `ctx.home` first, and `validate::materialize`'s contract forbids
//! pointing it at a live home. `validate_live` validates a config file that
//! already exists, in place, writing nothing.
//!
//! # Golden-rule-4 reading (PENDING security-auditor confirmation)
//!
//! `CLAUDE.md` golden rule 4 says every child process in the codebase goes
//! through `openvhost-proc`. Spawning a one-shot tool directly from THIS crate
//! has shipped precedent (`webserver.rs`'s `NginxAdapter::validate`, P0-7), but
//! the raw `libc::kill(-pgid, SIGKILL)` in `kill_process_group` below does not:
//! it makes this the SECOND independent implementation of the same POSIX
//! containment invariant in the workspace. The other copy is
//! `crates/openvhost-proc/src/platform/unix.rs` (`UnixDriver::spawn`'s
//! `process_group(0)` plus `signal_group`) — read it when you change this, and
//! change both or neither.
//!
//! Two alternatives were considered and rejected: an
//! `openvhost-conf → openvhost-proc` dependency edge (couples config generation
//! to the supervisor, for a mechanism config generation only needs for one-shot
//! tools), and making `openvhost-proc`'s `signal_group` `pub` (the same coupling
//! by another route, plus it widens that crate's API surface for an
//! out-of-crate caller). The reading of rule 4 taken here is: SUPERVISED
//! SERVICES — anything with a lifecycle, restart policy, health check or orphan
//! record — go through `openvhost-proc`; one-shot tool invocations that live and
//! die inside a single function call spawn directly. That reading is NOT settled
//! — it is pending security-auditor confirmation later in this slice. If it is
//! rejected, this module is what moves.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::ValidationReport;
use crate::error::ConfError;

/// Both probes are short-lived local process launches; 5s is far beyond a
/// healthy `nginx -v`/`-t` and short enough that a wedged binary surfaces as an
/// error instead of a spinner that never resolves.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a bounded probe run failed. Private: each probe maps this onto its own
/// public failure shape (`ConfError` for `validate_live`, `None` for the version
/// probe), so the shared runner does not have to know either.
enum ProbeFailure {
    /// The binary could not be launched, or its pipes could not be drained.
    Io(std::io::Error),
    /// `PROBE_TIMEOUT` elapsed. The process group has been killed.
    TimedOut,
}

/// Spawn `cmd` in its own process group, capture stdout+stderr, and wait for it
/// to finish — bounded by [`PROBE_TIMEOUT`], group-killing the child and any
/// grandchild on expiry.
///
/// Shared by BOTH probes deliberately. The containment handling is subtle (the
/// drop-ordering note in the timeout arm below is load-bearing) and the two
/// probes previously disagreed about how much of it they needed; one copy is the
/// only way they cannot drift apart again.
///
/// `stdin` is `null` so a probe can never block reading a terminal it inherited
/// — matching what `Command::output()` (which the version probe used to call)
/// already did.
///
/// The GROUP part is `#[cfg(unix)]`. On Windows this degrades to `kill_on_drop`
/// alone, i.e. the direct child only; containment there means Job Objects, which
/// is deferred with the rest of the Windows surface (spec: macOS-first). That is
/// a strict subset of the unix behaviour, not a different contract — the
/// `Result` a caller sees is identical either way.
///
/// LIMITATION: the wait joins on EOF of the child's pipes, not on the child's
/// own exit — see [`validate_live`]'s doc comment.
async fn run_bounded(
    cmd: &mut tokio::process::Command,
) -> Result<std::process::Output, ProbeFailure> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Secondary net, for "the caller drops this whole future" — the timeout
        // arm below does the real reclaiming. On its own this is NOT enough:
        // `kill_on_drop`, exactly like `Child::kill()`, only ever signals the
        // single tracked pid, so a shell-wrapped binary's forked grandchild
        // survives it (empirically proven against the fake in this module's
        // timeout test).
        .kill_on_drop(true);
    // Own process group, set atomically at spawn (posix_spawn attribute, so no
    // post-fork setpgid race), so the timeout arm can reclaim grandchildren too.
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd.spawn().map_err(ProbeFailure::Io)?;
    // Snapshotted BEFORE `wait_with_output` consumes `child` below.
    // `process_group(0)` makes this pid double as the pgid.
    #[cfg(unix)]
    let pgid = child.id();

    match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(res) => res.map_err(ProbeFailure::Io),
        Err(_) => {
            // DROP ORDERING HERE IS LOAD-BEARING. `child` is owned by the
            // `wait_with_output` future inside the `Timeout` temporary in this
            // `match`'s scrutinee, and a scrutinee temporary lives until the END
            // of the `match` expression. So right here the group LEADER IS
            // STILL ALIVE and `kill_on_drop` has not fired yet — which is what
            // makes `-pgid` provably still our own group, with no pid-reuse
            // window between the snapshot and the signal.
            //
            // THE REFACTOR THAT BREAKS IT: `let res = timeout(..).await; match
            // res { .. }`. There the temporary drops at the end of the `let`
            // statement, so `kill_on_drop` kills and reaps the leader first, and
            // by the time this arm runs `-pgid` is no longer guaranteed to name
            // our group. Keep the `.await` in the scrutinee.
            #[cfg(unix)]
            kill_process_group(pgid);
            Err(ProbeFailure::TimedOut)
        }
    }
}

/// SIGKILL the timed-out probe's whole process group.
///
/// The two checks below are for LEGIBILITY and defence-in-depth — NOT because
/// the input is untrusted. `pgid` is a private local that came straight from
/// `Child::id()` on a child this module spawned two statements earlier with
/// `process_group(0)`; nothing outside this module can reach this function or
/// choose its argument. That is the same trust class in which
/// `openvhost-proc`'s `signal_group` (`crates/openvhost-proc/src/platform/unix.rs`)
/// deliberately takes a self-spawned pgid with no floor at all. The floor in
/// that crate's `orphan::reap` exists for a different reason: its pid comes from
/// an on-disk registry and its entry point is publicly callable with an
/// arbitrary `u32`. Neither of those applies here.
///
/// What the checks buy is that the invariants making the NEGATION sound are
/// visible to the compiler and to the next reader, instead of asserted in prose
/// that can quietly stop being true.
#[cfg(unix)]
fn kill_process_group(pgid: Option<u32>) {
    // No pid means the child was already reaped: nothing left to signal.
    let Some(pgid) = pgid else { return };
    // `libc::pid_t` is `i32` on every unix target this builds for. Unreachable
    // for a real pid; the point is that the negation below cannot silently go
    // negative-by-overflow and turn into a `kill(-1, ...)`.
    let Ok(pgid) = i32::try_from(pgid) else {
        return;
    };
    if pgid > 1 {
        // SAFETY: a plain `kill` syscall — no memory is handed over. The two
        // checks above are the negation's preconditions, now visible rather than
        // asserted: `pgid` is in `2..=i32::MAX`, so `-pgid` is in
        // `-i32::MAX..=-2`. It therefore cannot be `0` (which would signal OUR
        // OWN group), cannot be `-1` (every process we are permitted to
        // signal), and cannot evaluate `-(i32::MIN)`, which panics in debug and
        // wraps in release. A negative pid makes `kill` target the whole process
        // GROUP, which is the entire point: it reaches any grandchild the
        // timed-out tool forked. The return value is intentionally ignored —
        // ESRCH here only means the group already exited on its own, the common
        // case, not a failure worth surfacing from a read-only probe.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

/// nginx's version as the binary reports it (`1.27.3` from
/// `nginx version: nginx/1.27.3`), or `None` for any failure — missing binary,
/// unparseable banner, timeout. Deliberately not a `Result`: a page that lists
/// servers should still list them when one version is unknowable, and the caller
/// has nothing actionable to do with the distinction.
///
/// NGINX ONLY, by contract, not by accident. It reads STDERR (where nginx writes
/// its banner) and looks for an `.../<version>` shape. `php-fpm -v` prints
/// `PHP 8.4.23 (fpm-fcgi) ...` to STDOUT — wrong stream AND unparseable shape,
/// so it yields `None` twice over. A php-fpm version probe needs its own
/// function; do not widen this one.
///
/// `-e <err_log>` is passed here for the same reason as in [`validate_live`]:
/// `-e` is mandatory on EVERY nginx invocation, so that nothing this app runs
/// can write into nginx's compiled-in prefix (`/opt/homebrew/var`) instead of
/// our home. `nginx -v` very likely exits before it would ever open an error
/// log, but that rests on nginx internals we cannot verify here, and passing
/// `-e` costs nothing.
///
/// The cost is not quite zero, so write the floor down: `-e` was introduced in
/// **nginx 1.19.5**. An older binary rejects it while parsing options and never
/// prints its banner, so this returns `None` where a bare `-v` would have found a
/// version. That floor already exists app-wide — [`validate_live`] hard-requires
/// `-e` — and openvhost-pkg ships its own nginx, so this only affects someone
/// pointing OpenVHost at a system nginx older than 1.19.5, who sees the version as
/// unknown rather than anything breaking.
pub async fn probe_nginx_version(bin: &Path, err_log: &Path) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-e").arg(err_log).arg("-v");
    let out = run_bounded(&mut cmd).await.ok()?;
    // nginx writes its banner to STDERR, not stdout.
    parse_version(&String::from_utf8_lossy(&out.stderr))
}

/// Pull `1.27.3` out of `nginx version: nginx/1.27.3 (extra build detail)`.
/// Split out so the parsing is testable without spawning anything.
///
/// Scans LINE BY LINE and takes the first line that parses. Scanning the whole
/// blob as one string was a real bug: nginx does emit warnings to stderr, and a
/// single preceding warning containing a path (`nginx: [warn] ...
/// /opt/homebrew/var/log/x`) made the split-on-first-slash consume that path
/// instead of the version, so the UI reported the version as unknown.
fn parse_version(stderr: &str) -> Option<String> {
    stderr.lines().find_map(parse_version_line)
}

/// The single-line half of [`parse_version`]. A pre-release suffix is part of
/// the version and is kept verbatim (`nginx/1.2.3-rc1` ⇒ `1.2.3-rc1`).
fn parse_version_line(line: &str) -> Option<String> {
    let after_slash = line.split_once('/')?.1;
    let token = after_slash
        .split_whitespace()
        .next()?
        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
    if token.is_empty() || !token.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(token.to_string())
}

/// Validate a config file that ALREADY EXISTS, in place. Writes nothing to
/// `conf` and never calls `materialize`.
///
/// `-e <err_log>` is MANDATORY: without it nginx writes into its compiled-in
/// prefix (`/opt/homebrew/var`) rather than our home.
///
/// LIMITATION worth knowing before you call this: the bounded wait joins on EOF
/// of the child's stdout/stderr pipes, not on the child's own exit. A validator
/// that exits 0 promptly but leaves a grandchild holding the inherited stderr
/// pipe open therefore surfaces as [`ConfError::ValidatorTimeout`], not as
/// success. Real `nginx -t` never does that, and this is still strictly better
/// than `webserver.rs`'s untimed `.output()`, which hangs forever in exactly the
/// same situation — recorded so a future caller is not surprised by it.
pub async fn validate_live(
    bin: &Path,
    conf: &Path,
    err_log: &Path,
) -> Result<ValidationReport, ConfError> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-e").arg(err_log).arg("-t").arg("-c").arg(conf);
    match run_bounded(&mut cmd).await {
        Ok(out) => Ok(ValidationReport {
            // Exit code ONLY — nginx writes to stderr even on success.
            ok: out.status.success(),
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

// Unix-only: the fakes are `#!/bin/sh` scripts made executable via
// `PermissionsExt`, and the containment assertions are raw `libc::kill`. Without
// the `unix` gate this module does not COMPILE on Windows, which would break
// `cargo test --workspace` and `cargo clippy --workspace --all-targets` there
// (`crates/openvhost-proc/src/orphan/reap.rs:168` gates its tests the same WAY but
// more narrowly — `all(test, target_os = "macos")`, because those tests assert on
// macOS-specific `sysctl` behaviour. This module needs only POSIX, so `unix` is the
// correct width here. Do NOT widen reap.rs's gate to match this one.)
#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fake "binary": a shell script that writes fixed text and exits with a
    /// fixed code. Lets these tests assert real spawn behaviour without needing
    /// nginx installed, which CI and most dev machines cannot assume.
    fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// Bounded, REAL-TIME poll for the pid the fake validator recorded for its
    /// forked grandchild.
    ///
    /// Deliberately blocking (`std::thread::sleep`) and not `async`: the timeout
    /// test runs on a PAUSED clock, and awaiting anything here would let the
    /// runtime park and auto-advance virtual time, firing the probe's timeout
    /// before the grandchild exists — at which point the test could not observe
    /// the kill at all. Blocking the runtime thread is exactly what keeps the
    /// clock still.
    fn wait_for_grandchild_pid(pidfile: &Path, deadline: Duration) -> u32 {
        let start = std::time::Instant::now();
        loop {
            if let Ok(text) = std::fs::read_to_string(pidfile)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                return pid;
            }
            assert!(
                start.elapsed() < deadline,
                "fake validator never recorded a grandchild pid in {} within {deadline:?} \
                 — the fixture is broken, so this test cannot prove anything about the kill",
                pidfile.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Bounded, REAL-TIME proof that `pid` is gone.
    ///
    /// `kill(pid, 0)` rather than `waitpid`: the grandchild is NOT our child
    /// (the fake shell forked it), so we cannot wait on it. That is also why
    /// `kill(pid, 0)` is trustworthy here, unlike in
    /// `openvhost-proc::orphan::reap`'s tests — once the group kill lands, the
    /// grandchild's parent dies too, so it is reparented to launchd and reaped
    /// immediately rather than lingering as an unreaped zombie of ours.
    ///
    /// Polled, because the kill is asynchronous from this test's point of view;
    /// bounded, because an unbounded loop would hang instead of failing.
    fn wait_until_gone(pid: u32, deadline: Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            // SAFETY: signal 0 delivers nothing — it only probes existence.
            let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
            if !alive {
                return true;
            }
            if start.elapsed() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Unconditionally SIGKILLs the recorded grandchild on drop, so a FAILING
    /// assertion (notably during the deliberate "delete the hardening and watch
    /// it fail" check) cannot leave a stray `sleep 30` behind for the suite's
    /// `pgrep` leak gate to trip over. Same posture as `KillOnDrop` in
    /// `openvhost-proc/src/orphan/reap.rs`'s tests.
    struct KillGrandchildOnDrop(u32);

    impl Drop for KillGrandchildOnDrop {
        fn drop(&mut self) {
            // SAFETY: plain kill syscall, no memory handed over. The pid is the
            // fake validator's own grandchild, recorded by this test moments
            // ago. ESRCH (already dead — the PASSING case) is expected and
            // ignored.
            unsafe {
                libc::kill(self.0 as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    #[tokio::test]
    async fn version_is_read_from_stderr_not_stdout() {
        // nginx prints its banner to STDERR. A stdout-only reader returns None
        // against a real nginx, which is the bug this pins.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", "echo 'nginx version: nginx/1.27.3' 1>&2");
        assert_eq!(
            probe_nginx_version(&bin, Path::new("/tmp/e.log"))
                .await
                .as_deref(),
            Some("1.27.3")
        );
    }

    #[tokio::test]
    async fn version_tolerates_a_banner_with_extra_build_detail() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(
            d.path(),
            "nginx",
            "echo 'nginx version: nginx/1.25.1 (Ubuntu)' 1>&2",
        );
        assert_eq!(
            probe_nginx_version(&bin, Path::new("/tmp/e.log"))
                .await
                .as_deref(),
            Some("1.25.1")
        );
    }

    #[tokio::test]
    async fn version_is_none_when_the_output_has_no_version() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", "echo 'totally unrelated' 1>&2");
        assert_eq!(
            probe_nginx_version(&bin, Path::new("/tmp/e.log")).await,
            None
        );
    }

    #[tokio::test]
    async fn version_is_none_when_the_binary_does_not_exist() {
        // A missing binary must not be an error that fails a whole page load.
        assert_eq!(
            probe_nginx_version(Path::new("/nonexistent/nginx"), Path::new("/tmp/e.log")).await,
            None
        );
    }

    #[tokio::test]
    async fn version_probe_passes_the_mandatory_error_log_flag() {
        // `-e` is mandatory on EVERY nginx invocation, the version probe
        // included — otherwise nginx may write into its compiled-in Homebrew
        // prefix instead of our home. The fake records its argv to a file rather
        // than echoing it to stderr, because stderr is the channel the version
        // parser reads.
        let d = tempfile::tempdir().unwrap();
        let argv = d.path().join("argv.txt");
        let bin = fake_bin(
            d.path(),
            "nginx",
            &format!(
                "echo \"$@\" > \"{}\"\necho 'nginx version: nginx/1.27.3' 1>&2",
                argv.display()
            ),
        );
        let v = probe_nginx_version(&bin, Path::new("/tmp/err.log")).await;
        assert_eq!(v.as_deref(), Some("1.27.3"));
        let recorded = std::fs::read_to_string(&argv).unwrap();
        assert!(recorded.contains("-e /tmp/err.log"), "argv was: {recorded}");
        assert!(recorded.contains("-v"), "argv was: {recorded}");
    }

    #[tokio::test]
    async fn validate_reports_ok_from_the_exit_code_alone() {
        // Success still writes to stderr (nginx says "syntax is ok" there), so
        // deriving `ok` from stderr emptiness would report every success as a
        // failure.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", "echo 'syntax is ok' 1>&2; exit 0");
        let r = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap();
        assert!(r.ok);
        assert!(r.stderr.contains("syntax is ok"));
    }

    #[tokio::test]
    async fn validate_reports_failure_and_keeps_stderr_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(
            d.path(),
            "nginx",
            "echo 'unknown directive \"bogus\"' 1>&2; exit 1",
        );
        let r = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap();
        assert!(!r.ok);
        assert!(r.stderr.contains("unknown directive"));
    }

    #[tokio::test]
    async fn validate_passes_the_mandatory_error_log_flag_and_the_config_path() {
        // Pins the argv shape. Without `-e`, nginx writes into the Homebrew
        // prefix; without `-c`, it validates its own compiled-in config instead
        // of ours. The fake echoes its args so the test can read them back.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", r#"echo "$@" 1>&2"#);
        let r = validate_live(&bin, Path::new("/tmp/live.conf"), Path::new("/tmp/err.log"))
            .await
            .unwrap();
        assert!(
            r.stderr.contains("-e /tmp/err.log"),
            "argv was: {}",
            r.stderr
        );
        assert!(r.stderr.contains("-t"), "argv was: {}", r.stderr);
        assert!(
            r.stderr.contains("-c /tmp/live.conf"),
            "argv was: {}",
            r.stderr
        );
    }

    #[tokio::test]
    async fn validate_errors_when_the_binary_cannot_be_launched() {
        let e = validate_live(
            Path::new("/nonexistent/nginx"),
            Path::new("/tmp/x.conf"),
            Path::new("/tmp/e.log"),
        )
        .await
        .unwrap_err();
        assert!(matches!(e, ConfError::ValidatorSpawn { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn validate_times_out_instead_of_hanging_forever() {
        // TWO properties are pinned here, and the second one is the reason this
        // test looks the way it does.
        //
        // 1. The P0-7 validator uses a bare `.output().await` with no timeout;
        //    this UI-facing path must not inherit that.
        // 2. The timeout must RECLAIM THE WHOLE PROCESS GROUP, not just the
        //    direct child. The fake is a shell that FORKS `sleep 30`, and
        //    `kill_on_drop`/`Child::kill()` only ever signal the one tracked
        //    pid, so without `process_group(0)` + `kill(-pgid, SIGKILL)` the
        //    grandchild survives for the whole app lifetime.
        //
        // Asserting only on `ValidatorTimeout` PASSED against the leaking
        // implementation — that assertion cannot fail if someone deletes the
        // containment. The grandchild assertion at the bottom is the one that
        // can, and it is the only regression protection the hardening has.
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("grandchild.pid");
        let bin = fake_bin(
            d.path(),
            "nginx",
            &format!("sleep 30 & echo $! > \"{}\"\nwait", pidfile.display()),
        );

        // Run the probe as its own task so THIS task keeps control of the paused
        // clock: the grandchild has to exist before the timeout fires, or there
        // is nothing to observe.
        let probe = tokio::spawn(async move {
            validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log")).await
        });
        // One poll is enough to reach the probe's first await point: `spawn()`
        // is synchronous, so when this yield returns the child is running and
        // the 5s timer is registered.
        tokio::task::yield_now().await;

        // REAL time from here until the `advance` below, with NO `.await`: the
        // paused clock only auto-advances when the runtime parks, and it cannot
        // park while this task is blocking its thread.
        let grandchild = wait_for_grandchild_pid(&pidfile, Duration::from_secs(10));
        let _cleanup = KillGrandchildOnDrop(grandchild);

        // Virtual time (Fix 8): jump past PROBE_TIMEOUT rather than burning 5
        // real seconds of every test run.
        tokio::time::advance(PROBE_TIMEOUT + Duration::from_secs(1)).await;

        let e = probe.await.unwrap().unwrap_err();
        assert!(matches!(e, ConfError::ValidatorTimeout { .. }), "got {e:?}");
        assert!(
            wait_until_gone(grandchild, Duration::from_secs(2)),
            "grandchild {grandchild} (the `sleep 30` forked by the fake validator) \
             SURVIVED the timeout: process-group containment is gone, so every \
             timed-out validation now leaks a process for the app's lifetime"
        );
    }

    #[test]
    fn parse_version_pulls_the_token_after_the_slash() {
        assert_eq!(
            parse_version("nginx version: nginx/1.27.3").as_deref(),
            Some("1.27.3")
        );
        assert_eq!(parse_version("nginx/1.2.3 (x)").as_deref(), Some("1.2.3"));
        assert_eq!(parse_version("no slash here"), None);
        assert_eq!(parse_version("trailing/"), None);
    }

    #[test]
    fn parse_version_skips_an_earlier_stderr_line_containing_a_path() {
        // nginx DOES emit warnings to stderr. Splitting the whole blob on its
        // first `/` consumed the warning's path and reported the version as
        // unknown; parsing line by line is what fixes it.
        assert_eq!(
            parse_version("nginx: [warn] ... /opt/homebrew/var/log/x\nnginx version: nginx/1.27.3")
                .as_deref(),
            Some("1.27.3")
        );
    }

    #[test]
    fn parse_version_keeps_a_prerelease_suffix() {
        // `-rc1` is part of the version, not trailing noise to trim. Pinned so
        // the intent is recorded rather than incidental.
        assert_eq!(
            parse_version("nginx version: nginx/1.2.3-rc1").as_deref(),
            Some("1.2.3-rc1")
        );
    }
}
