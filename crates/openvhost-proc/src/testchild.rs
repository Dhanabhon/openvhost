// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic cross-platform test child (spec §7). Std-only, sync.
//! `--ignore-stop` really ignores the platform stop request so the
//! supervisor's kill path gets exercised (Windows: a Ctrl handler that
//! returns TRUE — without it the OS default handler would terminate us
//! and the test would validate the wrong thing). `--probe-state` turns this
//! same binary into a stateful readiness-probe double for the supervisor's
//! `ReadinessProbe::Command` tests (P1 MySQL lifecycle design, spec D4).

use std::io::Write;
use std::path::PathBuf;

/// Fixed response body the `--http` server returns and the E2E asserts on.
pub const E2E_BODY: &str = "openvhost-e2e-ok";

#[derive(Debug, PartialEq, Eq)]
pub struct TestchildArgs {
    pub lines: u64,
    pub interval_ms: u64,
    pub exit_code: i32,
    pub ignore_stop: bool,
    pub fail_after: Option<u64>,
    pub http_port: Option<u16>,
    pub spawn_child: bool,
    /// Path to a small counter file: present together with
    /// `probe_succeed_after` switches `run` into probe mode (see
    /// [`crate::testchild`] module docs) instead of the tick loop.
    pub probe_state: Option<PathBuf>,
    /// Exit `0` once the counter at `probe_state` reaches this value;
    /// exit `1` (with a stderr diagnostic) otherwise.
    pub probe_succeed_after: Option<u64>,
    /// Sleep this long before checking/advancing the counter — widens the
    /// window during which one probe attempt is "in flight", for tests that
    /// need to race it against the supervised child's own exit.
    pub probe_delay_ms: u64,
}

impl Default for TestchildArgs {
    fn default() -> Self {
        Self {
            lines: 10,
            interval_ms: 200,
            exit_code: 0,
            ignore_stop: false,
            fail_after: None,
            http_port: None,
            spawn_child: false,
            probe_state: None,
            probe_succeed_after: None,
            probe_delay_ms: 0,
        }
    }
}

pub fn parse(args: &[String]) -> Result<TestchildArgs, String> {
    let mut out = TestchildArgs::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut next_u64 = |name: &str| -> Result<u64, String> {
            it.next()
                .ok_or_else(|| format!("{name} needs a value"))?
                .parse::<u64>()
                .map_err(|_| format!("{name} needs a number"))
        };
        match a.as_str() {
            "--lines" => out.lines = next_u64("--lines")?,
            "--interval-ms" => out.interval_ms = next_u64("--interval-ms")?,
            "--exit" => {
                out.exit_code = it
                    .next()
                    .ok_or("--exit needs a value")?
                    .parse::<i32>()
                    .map_err(|_| "--exit needs a number".to_string())?;
            }
            "--fail-after" => out.fail_after = Some(next_u64("--fail-after")?),
            "--ignore-stop" => out.ignore_stop = true,
            "--spawn-child" => out.spawn_child = true,
            "--http" => {
                out.http_port = Some(
                    it.next()
                        .ok_or("--http needs a value")?
                        .parse::<u16>()
                        .map_err(|_| "--http needs a port number".to_string())?,
                );
            }
            "--probe-state" => {
                out.probe_state = Some(PathBuf::from(
                    it.next().ok_or("--probe-state needs a path")?,
                ));
            }
            "--probe-succeed-after" => {
                out.probe_succeed_after = Some(next_u64("--probe-succeed-after")?)
            }
            "--probe-delay-ms" => out.probe_delay_ms = next_u64("--probe-delay-ms")?,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(out)
}

#[allow(clippy::collapsible_if)]
pub fn run(args: TestchildArgs) -> i32 {
    if let (Some(state), Some(succeed_after)) = (&args.probe_state, args.probe_succeed_after) {
        return run_probe(state, succeed_after, args.probe_delay_ms);
    }
    if let Some(port) = args.http_port {
        return serve_http(port);
    }
    if args.ignore_stop {
        install_ignore_stop();
    }
    if args.spawn_child {
        match spawn_grandchild() {
            Ok(pid) => {
                let stdout = std::io::stdout();
                let mut lock = stdout.lock();
                let _ = writeln!(lock, "child-pid: {pid}");
                let _ = lock.flush();
            }
            Err(e) => {
                eprintln!("proc_testchild: failed to spawn grandchild: {e}");
                return 1;
            }
        }
    }
    let stdout = std::io::stdout();
    for i in 1..=args.lines {
        if let Some(n) = args.fail_after {
            if i > n {
                eprintln!("ERROR simulated failure after {n} ticks");
                return 1;
            }
        }
        {
            let mut lock = stdout.lock();
            let _ = writeln!(lock, "tick {i}/{}", args.lines);
            let _ = lock.flush();
        }
        std::thread::sleep(std::time::Duration::from_millis(args.interval_ms));
    }
    args.exit_code
}

/// One readiness-probe attempt: a fresh process runs per attempt (exactly
/// like a real `mysqladmin ping` invocation would), so `state` — a small
/// counter file — is the only memory across attempts. Exits `0` once the
/// count reaches `succeed_after`; otherwise prints a diagnostic to stderr
/// (the text the supervisor's `Failed` state is expected to surface) and
/// exits `1`.
///
/// Writes its own pid to `<state>.pid` FIRST, before any delay — tests that
/// need to prove this attempt actually started (and later, that it left no
/// process behind) read that file rather than needing any handle back into
/// the supervisor internals that spawned it.
fn run_probe(state: &std::path::Path, succeed_after: u64, delay_ms: u64) -> i32 {
    let pid_path = state.with_extension("pid");
    let _ = std::fs::write(&pid_path, std::process::id().to_string());
    if delay_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
    }
    let prev: u64 = std::fs::read_to_string(state)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let attempt = prev + 1;
    let _ = std::fs::write(state, attempt.to_string());
    if attempt >= succeed_after {
        println!("probe ready on attempt {attempt}");
        0
    } else {
        eprintln!("ERROR probe not ready (attempt {attempt}/{succeed_after})");
        1
    }
}

/// Spawn a long-lived grandchild by re-executing this same binary with
/// `--lines 100000 --interval-ms 200 --ignore-stop` — the cheapest way to get
/// something that stays alive and does not exit on its own, since the binary
/// is already on disk and already knows how to ignore a polite stop.
///
/// Deliberately does NOT call `.process_group(..)` on the child: the whole
/// point is that this process inherits ITS OWN process group (set by the
/// caller of `proc_testchild` — see `platform/unix.rs`'s `process_group(0)`),
/// so the grandchild becomes a member of the same group under test. Setting a
/// new group here would defeat the test this flag exists to support.
fn spawn_grandchild() -> std::io::Result<u32> {
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .args(["--lines", "100000", "--interval-ms", "200", "--ignore-stop"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(child.id())
}

/// Minimal HTTP/1.1 server: serve a fixed `200` + `E2E_BODY` on every
/// connection until the supervisor kills the process. No signal handling —
/// SIGTERM / console-ctrl terminates us (the child is not `--ignore-stop`).
fn serve_http(port: u16) -> i32 {
    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("proc_testchild: cannot bind 127.0.0.1:{port}: {e}");
            return 1;
        }
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        E2E_BODY.len(),
        E2E_BODY
    );
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        // Best-effort: consume up to 1 KiB of the request so the client's
        // write side isn't reset before it reads the response. Discarded.
        let mut buf = [0u8; 1024];
        let _ = std::io::Read::read(&mut stream, &mut buf);
        let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
        let _ = std::io::Write::flush(&mut stream);
    }
    0 // unreachable: incoming() blocks forever until the process is killed
}

#[cfg(unix)]
fn install_ignore_stop() {
    // SAFETY: setting a signal disposition to SIG_IGN takes no user memory.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
}

#[cfg(windows)]
fn install_ignore_stop() {
    // windows-sys 0.61 moved BOOL to `core` (it's no longer re-exported from
    // `Win32::Foundation`); `Win32_Foundation` stays a required feature
    // regardless, since Console's own bindings reference `Foundation::HANDLE`.
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    use windows_sys::core::BOOL;
    unsafe extern "system" fn handler(_ctrl_type: u32) -> BOOL {
        1 // handled: ignore CTRL_C / CTRL_BREAK
    }
    // SAFETY: registering a static handler fn.
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_defaults() {
        assert_eq!(parse(&[]).unwrap(), TestchildArgs::default());
    }

    #[test]
    fn parse_all_flags() {
        let a = parse(&s(&[
            "--lines",
            "3",
            "--interval-ms",
            "50",
            "--exit",
            "2",
            "--ignore-stop",
            "--fail-after",
            "1",
            "--spawn-child",
        ]))
        .unwrap();
        assert_eq!(
            a,
            TestchildArgs {
                lines: 3,
                interval_ms: 50,
                exit_code: 2,
                ignore_stop: true,
                fail_after: Some(1),
                http_port: None,
                spawn_child: true,
                probe_state: None,
                probe_succeed_after: None,
                probe_delay_ms: 0,
            }
        );
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(parse(&s(&["--nope"])).is_err());
    }

    #[test]
    fn parse_http_flag() {
        let a = parse(&s(&["--http", "8081"])).unwrap();
        assert_eq!(a.http_port, Some(8081));
    }

    #[test]
    fn parse_http_rejects_non_numeric() {
        assert!(parse(&s(&["--http", "notaport"])).is_err());
    }

    #[test]
    fn parse_probe_flags() {
        let a = parse(&s(&[
            "--probe-state",
            "/tmp/ovh-probe-state",
            "--probe-succeed-after",
            "3",
            "--probe-delay-ms",
            "50",
        ]))
        .unwrap();
        assert_eq!(a.probe_state, Some(PathBuf::from("/tmp/ovh-probe-state")));
        assert_eq!(a.probe_succeed_after, Some(3));
        assert_eq!(a.probe_delay_ms, 50);
    }

    #[test]
    fn parse_probe_succeed_after_rejects_non_numeric() {
        assert!(parse(&s(&["--probe-succeed-after", "nope"])).is_err());
    }

    // Binary-level behavior tests live in tests/testchild_bin.rs:
    // CARGO_BIN_EXE_* is only provided when compiling integration tests.
}
