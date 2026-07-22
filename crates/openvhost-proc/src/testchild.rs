// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic cross-platform test child (spec §7). Std-only, sync.
//! `--ignore-stop` really ignores the platform stop request so the
//! supervisor's kill path gets exercised (Windows: a Ctrl handler that
//! returns TRUE — without it the OS default handler would terminate us
//! and the test would validate the wrong thing).

use std::io::Write;

#[derive(Debug, PartialEq, Eq)]
pub struct TestchildArgs {
    pub lines: u64,
    pub interval_ms: u64,
    pub exit_code: i32,
    pub ignore_stop: bool,
    pub fail_after: Option<u64>,
}

impl Default for TestchildArgs {
    fn default() -> Self {
        Self {
            lines: 10,
            interval_ms: 200,
            exit_code: 0,
            ignore_stop: false,
            fail_after: None,
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
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(out)
}

#[allow(clippy::collapsible_if)]
pub fn run(args: TestchildArgs) -> i32 {
    if args.ignore_stop {
        install_ignore_stop();
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
        ]))
        .unwrap();
        assert_eq!(
            a,
            TestchildArgs {
                lines: 3,
                interval_ms: 50,
                exit_code: 2,
                ignore_stop: true,
                fail_after: Some(1)
            }
        );
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(parse(&s(&["--nope"])).is_err());
    }

    // Binary-level behavior tests live in tests/testchild_bin.rs:
    // CARGO_BIN_EXE_* is only provided when compiling integration tests.
}
