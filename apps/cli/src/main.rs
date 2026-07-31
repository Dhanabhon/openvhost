// SPDX-License-Identifier: GPL-3.0-or-later
//! `openvhost` — the OpenVHost command line interface.
//!
//! One round trip over the app's local control socket (`openvhost-proc`'s
//! `control` module), rendered as either a table or one line of JSON. See
//! `docs/superpowers/specs/2026-07-31-p1-cli-design.md`; D3 (no-app behaviour
//! and the exit table) and D5 (output discipline) are the rules that shape
//! this crate.
//!
//! Two things this binary deliberately does **not** do:
//!
//! - **Launch the app.** Never, under any verb. It is an unrecoverable side
//!   effect under `ssh` or CI, the bundle path is not reliably knowable, and
//!   it would make `stop-all` *start* something (D3).
//! - **Supervise anything.** It constructs no `Supervisor` and never takes the
//!   instance lock, so it cannot record a process the next app launch would
//!   reap as an orphan (D1).

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser as _;
use openvhost_proc::control::{self, ErrorCode};
use openvhost_proc::{InstanceLock, SupervisorPresence};

use crate::cli::{Cli, Header};
use crate::exchange::Exchange;
use crate::exit::{Exit, exit_for};

mod cli;
mod exchange;
mod exit;
mod render;

/// Internal fixture subcommand, intercepted before `clap` ever sees the
/// arguments — **in debug builds only** (install design D7).
///
/// The raw intercept is what guarantees its flags reach
/// [`openvhost_proc::testchild::parse`] exactly as written rather than being
/// reinterpreted by this binary's own global options. It is not declared to
/// `clap` at all, which is also why it cannot appear in `--help`.
#[cfg(debug_assertions)]
const TESTCHILD: &str = "__testchild";

fn main() -> ExitCode {
    // `args_os` rather than `args`: the latter panics on an argument that is
    // not valid UTF-8, and a CLI must not abort on hostile argv.
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    intercept_testchild(&argv);
    // Read off raw argv, because a usage error has to be reported before
    // parsing has succeeded and still has to be JSON when JSON was asked for.
    let wants_json = cli::json_requested(&argv);
    let parsed = match Cli::try_parse_from(std::env::args_os()) {
        Ok(c) => c,
        Err(e) => return report_argument_failure(&e, wants_json),
    };
    ExitCode::from(run(&parsed).code())
}

/// Divert to the hidden fixture, if that is what was asked — debug builds only.
///
/// [`openvhost_proc::testchild`] exists to give the supervisor a deterministic
/// child to drive. `--probe-state P` writes `P` and `P.pid`, `--http` binds a
/// listener and `--spawn-child` re-execs this binary detached — all at the
/// caller's direction, none of it confined. That is unremarkable for a binary
/// living in `target/debug/`, and quite another thing once this slice symlinks
/// the binary into a directory on the user's PATH, where anything it can do is
/// a capability of the shipped tool. So a release build simply does not contain
/// the intercept, and `__testchild` reaches `clap` as the unrecognised verb it
/// should have been all along.
///
/// `debug_assertions` rather than `cfg(test)`, matching `demo_ticker_spec` in
/// the desktop crate: the dev and test profiles share the flag, so the fixture
/// stays available to `cargo test` and `tauri dev` — including to the demo
/// ticker, which spawns exactly this — and disappears only from `--release`.
#[cfg(debug_assertions)]
fn intercept_testchild(argv: &[String]) {
    if argv.first().map(String::as_str) == Some(TESTCHILD) {
        run_testchild(&argv[1..]);
    }
}

/// A release build has no fixture to divert to. See the debug counterpart.
#[cfg(not(debug_assertions))]
fn intercept_testchild(_argv: &[String]) {}

/// Run the fixture child and exit; never returns to the CLI proper.
#[cfg(debug_assertions)]
fn run_testchild(args: &[String]) -> ! {
    match openvhost_proc::testchild::parse(args) {
        Ok(a) => std::process::exit(openvhost_proc::testchild::run(a)),
        Err(e) => {
            eprintln!("openvhost {TESTCHILD}: {e}");
            std::process::exit(i32::from(Exit::Usage.code()));
        }
    }
}

/// One exchange, start to finish.
fn run(parsed: &Cli) -> Exit {
    let command = parsed.command_name();
    let header = parsed.header();
    let home = match openvhost_core::resolve_home() {
        Ok(h) => h,
        // Nothing was asked, so there is no supervisor to report on and no
        // home to name — this answer is deliberately header-less.
        Err(e) => {
            return emit(
                parsed.json,
                command,
                Header::Bare,
                Path::new(""),
                &Exchange::refused(
                    ErrorCode::OperationFailed,
                    format!("could not work out where the OpenVHost home directory is: {e}"),
                ),
            );
        }
    };
    let exchange = match parsed.request() {
        // A malformed id never reaches the socket, and never triggers a probe:
        // the closure is not called on this path.
        Err(e) => Exchange::from_client_error(&e, || SupervisorPresence::Absent),
        Ok(req) => match control::request(&home, &req) {
            Ok(response) => Exchange::answered(response),
            // The connect result is authoritative; the probe only improves the
            // wording (spec D3).
            Err(e) => Exchange::from_client_error(&e, || InstanceLock::probe(&home.join("run"))),
        },
    };
    // THE rule of the slice: for `status` and `list`, and only for those, an
    // absent app is the answer rather than a failure.
    let exchange = if header.reports_supervisor() {
        exchange.absent_supervisor_is_an_answer()
    } else {
        exchange
    };
    emit(parsed.json, command, header, &home, &exchange)
}

/// Write the answer and report the exit status.
fn emit(json: bool, command: &str, header: Header, home: &Path, ex: &Exchange) -> Exit {
    if json {
        // D5: one line on stdout and nothing on stderr, success or failure, so
        // a `jq` pipeline still parses when a verb fails.
        write_out(&format!(
            "{}\n",
            render::json_line(command, header, home, ex)
        ));
    } else {
        let rendered = render::human(header, home, ex);
        write_out(&rendered.out);
        write_err(&rendered.err);
    }
    exit_for(&ex.response)
}

/// `clap` could not make sense of the arguments.
///
/// `--help` and `--version` arrive here too — they are how `clap` reports a
/// successful short-circuit, not failures — and keep their conventional
/// stdout-and-exit-0 behaviour even under `--json`: help text is for a human,
/// and a machine asking for it would have nothing to do with a JSON envelope.
///
/// `DisplayHelpOnMissingArgumentOrSubcommand` is deliberately **not** in that
/// company. `clap` raises it for a bare `openvhost` with no verb and renders
/// help, but nothing ran, so reporting success would tell a script the
/// opposite of the truth. It is a usage error that happens to print help.
fn report_argument_failure(err: &clap::Error, wants_json: bool) -> ExitCode {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            // `clap` sends these to stdout and everything else to stderr.
            let _ = err.print();
            ExitCode::SUCCESS
        }
        _ if wants_json => {
            let ex = Exchange::refused(ErrorCode::BadRequest, err.render().to_string());
            ExitCode::from(emit(true, "unknown", Header::Bare, Path::new(""), &ex).code())
        }
        _ => {
            let _ = err.print();
            ExitCode::from(Exit::Usage.code())
        }
    }
}

/// Write to stdout, tolerating a closed pipe.
///
/// `print!` would panic when stdout is a pipe the reader has already closed
/// (`openvhost list | head -1`), turning a routine shell idiom into a Rust
/// backtrace. The exit code still reports what the *verb* did.
fn write_out(s: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// Write to stderr, tolerating a closed pipe. See [`write_out`].
fn write_err(s: &str) {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(s.as_bytes());
    let _ = err.flush();
}
