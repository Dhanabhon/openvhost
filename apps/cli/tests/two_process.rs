// SPDX-License-Identifier: GPL-3.0-or-later
//! The two-process story: a real control socket in one process, the **real**
//! `openvhost` binary in another (spec D7).
//!
//! Everything else in this crate tests pure functions. This is the only thing
//! that proves the binary a user actually runs connects to a real socket,
//! parses a real answer, writes it to the real stdout and exits with the real
//! code — and it needs no GUI to do it.
//!
//! The server side is a recording fake [`ControlHandler`], so the assertions
//! cover the CLI's half of the contract without dragging in a `Supervisor`.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Output;
use std::sync::{Arc, Mutex};

use openvhost_proc::control::{
    self, ControlHandler, Disposition, ErrorCode, Request, Response, async_trait,
};
use openvhost_proc::events::{ServiceState, ServiceStatus};
use serde_json::{Value, json};

/// The registered service the fake reports in its table. Anything unregistered
/// is an unknown id, which is what proves the CLI reports 66 rather than
/// inventing a service.
const KNOWN: &str = "nginx";

/// A second registered service, parked in `Failed`: it crashed earlier and
/// nothing brought it back.
///
/// `stop` on this one is the only shape that pairs a `failed` row with a
/// **successful** verb — the desktop handler's
/// `settled(Target::Stopped, Failed) => true` shortcut — and spec D3 says that
/// is exit 0, because being down is exactly what was asked for.
///
/// Deliberately absent from `list`/`status`: the fake answers per request
/// rather than modelling a registry, and keeping the table at one row keeps
/// the byte-exact envelope assertions small.
const CRASHED: &str = "php-fpm-8.4";

fn nginx(state: ServiceState) -> ServiceStatus {
    ServiceStatus {
        id: KNOWN.into(),
        display_name: "Nginx".into(),
        endpoint: Some("http://127.0.0.1:80".into()),
        pid: Some(4242),
        state,
    }
}

/// The [`CRASHED`] row, carrying the real stderr its child died with.
fn crashed() -> ServiceStatus {
    ServiceStatus {
        id: CRASHED.into(),
        display_name: "PHP 8.4".into(),
        endpoint: Some("unix:/run/php-fpm-8.4.sock".into()),
        // Nothing is alive, so there is no pid to report.
        pid: None,
        state: ServiceState::Failed {
            exit: Some(78),
            stderr_tail: vec![
                "ERROR: failed to open configuration file '/x/php-fpm.conf'".into(),
                "ERROR: FPM initialization failed".into(),
            ],
        },
    }
}

/// A handler that answers plausibly and records what it was asked, so the
/// tests can prove a flag reached the wire rather than being silently dropped.
struct Fake {
    seen: Arc<Mutex<Vec<Request>>>,
}

#[async_trait]
impl ControlHandler for Fake {
    async fn execute(&self, req: Request) -> Response {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(req.clone());
        }
        let known = |id: &control::ServiceId| id.as_str() == KNOWN;
        match req {
            Request::List | Request::Status { id: None } => Response::Services {
                services: vec![nginx(ServiceState::Running)],
            },
            Request::Status { id: Some(id) } => {
                if known(&id) {
                    Response::Services {
                        services: vec![nginx(ServiceState::Running)],
                    }
                } else {
                    Response::error(
                        ErrorCode::UnknownService,
                        format!("no service is registered as {id}"),
                    )
                }
            }
            Request::Start { id, .. } | Request::Restart { id, .. } => {
                if known(&id) {
                    Response::Transition {
                        service: nginx(ServiceState::Running),
                        disposition: Disposition::Changed,
                    }
                } else {
                    Response::error(
                        ErrorCode::UnknownService,
                        format!("no service is registered as {id}"),
                    )
                }
            }
            Request::Stop { id, .. } => match id.as_str() {
                KNOWN => Response::Transition {
                    service: nginx(ServiceState::Stopped),
                    disposition: Disposition::Unchanged,
                },
                // Already down because it crashed. The handler answers
                // `Unchanged` and hands back the failed row exactly as it
                // stands, stderr tail and all.
                CRASHED => Response::Transition {
                    service: crashed(),
                    disposition: Disposition::Unchanged,
                },
                other => Response::error(
                    ErrorCode::UnknownService,
                    format!("no service is registered as {other}"),
                ),
            },
            Request::StopAll => Response::StopAll {
                stragglers: vec!["mysql-8.4".into()],
            },
        }
    }
}

/// A bound, served socket in a tempdir, torn down on drop.
struct Server {
    home: tempfile::TempDir,
    seen: Arc<Mutex<Vec<Request>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start() -> Self {
        let home = new_home();
        // `bind` is synchronous and already `listen`ing when it returns, so a
        // connection made before `serve` gets scheduled waits in the backlog
        // rather than being refused — no start-up race to poll for.
        let listener = control::bind(home.path()).expect("bind the control socket");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<dyn ControlHandler> = Arc::new(Fake {
            seen: Arc::clone(&seen),
        });
        let (tx, rx) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a runtime for the control server");
            rt.block_on(control::serve(listener, handler, async move {
                let _ = rx.await;
            }));
        });
        Server {
            home,
            seen,
            shutdown: Some(tx),
            thread: Some(thread),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.seen.lock().expect("the fake's log").clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A tempdir short enough for `sun_path`, with the failure named if not.
fn new_home() -> tempfile::TempDir {
    let home = tempfile::Builder::new()
        .prefix("ovh")
        .tempdir()
        .expect("a tempdir for OPENVHOST_HOME");
    control::socket_path(home.path()).unwrap_or_else(|e| {
        panic!("this machine's temp dir is too deep for a unix socket ({e}); set TMPDIR shorter")
    });
    home
}

/// Run the **real** binary against `home`.
fn openvhost(home: &std::path::Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_openvhost"))
        .args(args)
        .env("OPENVHOST_HOME", home)
        .output()
        .expect("run the openvhost binary")
}

/// Exit code, parsed stdout, raw stderr.
fn json_run(home: &std::path::Path, args: &[&str]) -> (i32, Value, String) {
    let out = openvhost(home, args);
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(out.stderr).expect("stderr is UTF-8");
    assert_eq!(
        stdout.lines().count(),
        1,
        "--json must print exactly one line, got {stdout:?}"
    );
    let value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object ({e}): {stdout:?}"));
    (
        out.status.code().expect("the child was not signalled"),
        value,
        stderr,
    )
}

fn running_nginx_row() -> Value {
    json!({
        "id": "nginx",
        "displayName": "Nginx",
        "endpoint": "http://127.0.0.1:80",
        "pid": 4242,
        "state": { "kind": "running" },
    })
}

#[test]
fn list_returns_the_servers_service_table_verbatim() {
    let server = Server::start();
    let (code, value, stderr) = json_run(server.home.path(), &["list", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(stderr, "", "--json keeps stderr empty");
    assert_eq!(
        value,
        json!({
            "schemaVersion": 1,
            "ok": true,
            "command": "list",
            "result": { "kind": "services", "services": [running_nginx_row()] },
            "supervisor": "running",
        })
    );
}

#[test]
fn status_carries_the_supervisor_home_and_version_header() {
    let server = Server::start();
    let (code, value, _) = json_run(server.home.path(), &["status", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(value["supervisor"], "running");
    assert_eq!(value["home"], server.home.path().display().to_string());
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["result"]["services"][0], running_nginx_row());
}

#[test]
fn start_on_a_known_service_succeeds_and_reports_the_transition() {
    let server = Server::start();
    let (code, value, stderr) = json_run(server.home.path(), &["start", "nginx", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(stderr, "");
    assert_eq!(
        value,
        json!({
            "schemaVersion": 1,
            "ok": true,
            "command": "start",
            "result": {
                "kind": "transition",
                "service": running_nginx_row(),
                "disposition": "changed",
            },
        }),
        "a control verb passes the server's envelope through untouched"
    );
}

#[test]
fn start_on_an_unknown_service_exits_66_with_json_on_stdout() {
    let server = Server::start();
    let (code, value, stderr) = json_run(server.home.path(), &["start", "no-such-thing", "--json"]);
    assert_eq!(code, 66);
    // D5: the error envelope goes to stdout so a `jq` pipeline still parses.
    assert_eq!(stderr, "");
    assert_eq!(value["ok"], false);
    assert_eq!(value["command"], "start");
    assert_eq!(value["error"]["code"], "unknownService");
}

#[test]
fn an_unchanged_stop_is_a_success() {
    let server = Server::start();
    let (code, value, _) = json_run(server.home.path(), &["stop", "nginx", "--json"]);
    assert_eq!(code, 0, "already stopped is an explicit success");
    assert_eq!(value["result"]["disposition"], "unchanged");
}

/// `openvhost stop` on a service that had already crashed. The user asked for
/// it to be down and it is down, so this is spec D3's "0 = success, including
/// an explicit unchanged result" — in both modes.
///
/// It used to answer `ok:true` and exit **70**, printing `php-fpm-8.4 failed
/// (exit 78):` and a wall of stderr, which reads as though the stop had
/// failed. `stop-all` over the same failed service already said `All services
/// stopped.` and exited 0.
#[test]
fn stopping_an_already_crashed_service_exits_0_in_both_modes() {
    let server = Server::start();

    let (code, value, stderr) = json_run(server.home.path(), &["stop", CRASHED, "--json"]);
    assert_eq!(code, 0, "being down is what was asked for");
    assert_eq!(stderr, "", "--json keeps stderr empty");
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"]["disposition"], "unchanged");
    // The stale failure still travels: `ServiceStatus` is reused verbatim, so
    // exiting 0 hides nothing (spec D5).
    assert_eq!(
        value["result"]["service"]["state"],
        json!({
            "kind": "failed",
            "exit": 78,
            "stderrTail": [
                "ERROR: failed to open configuration file '/x/php-fpm.conf'",
                "ERROR: FPM initialization failed",
            ],
        })
    );

    let out = openvhost(server.home.path(), &["stop", CRASHED]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "",
        "a success is quiet on stderr, whatever the row says"
    );
    assert!(stdout.contains("nothing to do"), "{stdout}");
    assert!(
        !stdout.contains("php-fpm-8.4 failed"),
        "must not read as though the stop failed: {stdout}"
    );
    // …while still saying the service is parked in a failed state.
    assert!(
        stdout.contains("failed state from an earlier run (exit 78)"),
        "{stdout}"
    );
    assert!(stdout.contains("FPM initialization failed"), "{stdout}");
}

/// `restart`'s happy path over a real socket, which nothing covered.
#[test]
fn restart_reaches_the_server_and_reports_a_changed_transition() {
    let server = Server::start();
    let (code, value, stderr) = json_run(server.home.path(), &["restart", "nginx", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(stderr, "");
    assert_eq!(
        value,
        json!({
            "schemaVersion": 1,
            "ok": true,
            "command": "restart",
            "result": {
                "kind": "transition",
                "service": running_nginx_row(),
                "disposition": "changed",
            },
        })
    );
    assert_eq!(
        server.requests(),
        vec![Request::Restart {
            id: control::ServiceId::parse("nginx").unwrap(),
            wait: true,
        }],
        "the default is to wait"
    );
}

/// `restart --no-wait` — the combination D4 gives an asymmetric rule to (the
/// stop half is waited on regardless; the flag skips only the readiness wait
/// on the way back up) and which had no coverage at all.
///
/// The asymmetry itself is the handler's, so what the CLI owes is putting
/// `wait:false` on the wire under the verb that has two halves. A `restart`
/// that silently sent `wait:true` would make `--help` a lie and would block a
/// CI script for a full readiness deadline.
#[test]
fn restart_no_wait_puts_wait_false_on_the_wire() {
    let server = Server::start();
    let (code, value, stderr) = json_run(
        server.home.path(),
        &["restart", "nginx", "--no-wait", "--json"],
    );
    assert_eq!(code, 0);
    assert_eq!(stderr, "");
    assert_eq!(value["ok"], true);
    assert_eq!(value["command"], "restart");
    assert_eq!(value["result"]["disposition"], "changed");
    assert_eq!(
        server.requests(),
        vec![Request::Restart {
            id: control::ServiceId::parse("nginx").unwrap(),
            wait: false,
        }],
    );
}

#[test]
fn stop_all_reporting_stragglers_exits_70() {
    let server = Server::start();
    let (code, value, _) = json_run(server.home.path(), &["stop-all", "--json"]);
    assert_eq!(code, 70);
    assert_eq!(value["ok"], true, "the verb ran; it just did not finish");
    assert_eq!(value["result"]["stragglers"], json!(["mysql-8.4"]));
}

/// A flag that never reaches the wire is a lie in `--help`.
#[test]
fn no_wait_reaches_the_server() {
    let server = Server::start();
    assert_eq!(
        openvhost(server.home.path(), &["start", "nginx", "--no-wait"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        openvhost(server.home.path(), &["stop", "nginx"])
            .status
            .code(),
        Some(0)
    );
    let seen = server.requests();
    assert_eq!(
        seen,
        vec![
            Request::Start {
                id: control::ServiceId::parse("nginx").unwrap(),
                wait: false,
            },
            Request::Stop {
                id: control::ServiceId::parse("nginx").unwrap(),
                wait: true,
            },
        ],
        "--no-wait must clear `wait`, and its absence must leave it set"
    );
}

/// Human mode: the table on stdout, and nothing on stderr when it worked.
#[test]
fn human_mode_prints_a_table_on_stdout_and_leaves_stderr_clean() {
    let server = Server::start();
    let out = openvhost(server.home.path(), &["list"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stderr).unwrap(), "");
    assert!(stdout.contains("ID"), "{stdout}");
    assert!(stdout.contains("nginx"), "{stdout}");
    assert!(stdout.contains("running"), "{stdout}");
    assert!(stdout.contains("http://127.0.0.1:80"), "{stdout}");
}

/// THE rule of the slice, proven end to end: with no app at all, `status` is
/// an *answer* and every control verb is a failure.
#[test]
fn with_no_server_status_exits_0_while_start_exits_69() {
    let home = new_home();

    let (code, value, stderr) = json_run(home.path(), &["status", "--json"]);
    assert_eq!(code, 0, "a script must be able to LEARN the app is down");
    assert_eq!(stderr, "");
    assert_eq!(value["ok"], true);
    assert_eq!(value["supervisor"], "notRunning");
    assert_eq!(value["result"]["services"], json!([]));

    let (code, value, stderr) = json_run(home.path(), &["list", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(stderr, "");
    assert_eq!(value["supervisor"], "notRunning");
    assert_eq!(value["result"]["services"], json!([]));

    for verb in [
        vec!["start", "nginx"],
        vec!["stop", "nginx"],
        vec!["restart", "nginx"],
        vec!["stop-all"],
    ] {
        let mut args = verb.clone();
        args.push("--json");
        let (code, value, stderr) = json_run(home.path(), &args);
        assert_eq!(code, 69, "{verb:?} with no app must exit 69");
        assert_eq!(stderr, "");
        assert_eq!(value["ok"], false, "{verb:?}");
        assert_eq!(value["error"]["code"], "supervisorUnavailable", "{verb:?}");
    }
}

/// The no-app answer has to be readable, not just parseable: a bare empty
/// table would look like a broken install.
#[test]
fn with_no_server_human_mode_says_so_loudly_on_stdout() {
    let home = new_home();
    let out = openvhost(home.path(), &["list"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stderr).unwrap(), "");
    assert!(stdout.contains("not running"), "{stdout}");

    let out = openvhost(home.path(), &["start", "nginx"]);
    assert_eq!(out.status.code(), Some(69));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "",
        "errors go to stderr in human mode"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("not running"), "{stderr}");
    assert!(
        stderr.contains("Start it"),
        "the message must name the fix: {stderr}"
    );
}

/// Leave behind exactly what a force quit leaves: the socket **file**, with
/// nothing listening on it and nothing holding the run lock.
///
/// Bound with `std`'s listener and dropped, deliberately **not** through
/// [`control::bind`] — dropping a `std` `UnixListener` does not unlink, and
/// going through the app's own binder would let an unlink-on-drop added there
/// later silently turn this into a "there is no socket at all" test, which
/// takes a different path and would pass for the wrong reason.
fn leave_a_stale_socket(home: &std::path::Path) {
    use std::os::unix::fs::FileTypeExt as _;

    let path = control::socket_path(home).expect("a home short enough for sun_path");
    std::fs::create_dir_all(path.parent().expect("the socket has a parent"))
        .expect("create <home>/run");
    drop(std::os::unix::net::UnixListener::bind(&path).expect("bind a socket to leave behind"));
    assert!(
        std::fs::symlink_metadata(&path)
            .expect("the leftover socket")
            .file_type()
            .is_socket(),
        "the leftover has to still be a socket, or this tests the wrong path"
    );
}

/// After a force quit (or a crash, or `pkill`) the socket file outlives the
/// app. `connect` then fails with ECONNREFUSED — but the lock probe says
/// nothing is alive, so the app is definitively down and that is an *answer*.
///
/// This exited **69** with `supervisor:"unknown"` for as long as the leftover
/// file survived, which is until the next launch: the CLI had the probe result
/// in hand, said "No app is running" in its own message, and then reported
/// that it could not answer. Spec D3 scopes 69 to "lock held but socket
/// unreachable" — the genuinely ambiguous case — and click-list item 7
/// anticipates this one.
#[test]
fn a_force_quit_leftover_socket_still_lets_status_and_list_answer() {
    let home = new_home();
    leave_a_stale_socket(home.path());

    for verb in ["status", "list"] {
        let (code, value, stderr) = json_run(home.path(), &[verb, "--json"]);
        assert_eq!(code, 0, "{verb} must still answer");
        assert_eq!(stderr, "");
        assert_eq!(value["ok"], true, "{verb}");
        assert_eq!(value["supervisor"], "notRunning", "{verb}");
        assert_eq!(value["result"]["services"], json!([]), "{verb}");
    }

    // Human mode says what happened and how to clear it, on stdout.
    let out = openvhost(home.path(), &["status"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stderr).unwrap(), "");
    assert!(stdout.contains("not running"), "{stdout}");
    assert!(stdout.contains("force quit"), "{stdout}");

    // Control verbs are unmoved: there is still nothing to control.
    for verb in [
        vec!["start", "nginx"],
        vec!["stop", "nginx"],
        vec!["restart", "nginx"],
        vec!["stop-all"],
    ] {
        let mut args = verb.clone();
        args.push("--json");
        let (code, value, stderr) = json_run(home.path(), &args);
        assert_eq!(code, 69, "{verb:?} over a stale socket must still exit 69");
        assert_eq!(stderr, "");
        assert_eq!(value["ok"], false, "{verb:?}");
    }
}

/// The other half of the same story, and the reason the probe is consulted
/// rather than assumed: a socket that refuses connections **while an app holds
/// the run lock** is genuinely "I could not answer" — an app still starting up
/// — and stays a failure even for `status`.
#[test]
fn an_unreachable_socket_with_a_live_app_is_still_a_failure_for_status() {
    let home = new_home();
    leave_a_stale_socket(home.path());
    // Hold the run lock the way a live app does, in a process that is not the
    // CLI, so the CLI's own probe contends with it exactly as it would in
    // production.
    let held = openvhost_proc::InstanceLock::acquire(&home.path().join("run"))
        .expect("acquire the run lock")
        .expect("nothing else holds it");

    let (code, value, stderr) = json_run(home.path(), &["status", "--json"]);
    assert_eq!(code, 69, "an app IS alive, so this is not an answer");
    assert_eq!(stderr, "");
    assert_eq!(value["ok"], false);
    assert_eq!(value["supervisor"], "unknown");
    assert_eq!(value["error"]["code"], "controlChannelUnavailable");

    drop(held);
}

/// Never, under any verb: an absent app is reported, not fixed.
#[test]
fn no_verb_launches_the_app_or_provisions_a_home() {
    let home = new_home();
    for args in [
        vec!["status", "--json"],
        vec!["list", "--json"],
        vec!["start", "nginx", "--json"],
        vec!["stop-all", "--json"],
    ] {
        openvhost(home.path(), &args);
    }
    assert!(
        !home.path().join("run").exists(),
        "the CLI must not provision <home>/run"
    );
    assert_eq!(
        std::fs::read_dir(home.path()).unwrap().count(),
        0,
        "the CLI must leave a pristine home untouched"
    );
}

#[test]
fn bad_arguments_exit_64() {
    let home = new_home();
    for args in [
        vec!["bogus-verb"],
        vec!["start"],
        vec![],
        vec!["--nope", "list"],
        vec!["start", "nginx", "extra"],
    ] {
        let out = openvhost(home.path(), &args);
        assert_eq!(out.status.code(), Some(64), "{args:?}");
        assert!(
            !out.stderr.is_empty(),
            "{args:?} must explain itself on stderr"
        );
    }
}

/// A usage error under `--json` still has to be JSON on stdout, or a pipeline
/// that only ever sees `--json` output breaks on a typo.
#[test]
fn a_usage_error_under_json_is_still_one_json_line_on_stdout() {
    let home = new_home();
    let (code, value, stderr) = json_run(home.path(), &["bogus-verb", "--json"]);
    assert_eq!(code, 64);
    assert_eq!(stderr, "");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "badRequest");
}

/// A syntactically impossible id is refused locally — it never reaches the
/// socket at all.
#[test]
fn a_malformed_service_id_is_refused_without_contacting_the_server() {
    let server = Server::start();
    let (code, value, _) = json_run(server.home.path(), &["start", "ngi nx\tbad", "--json"]);
    assert_eq!(code, 64);
    assert_eq!(value["error"]["code"], "badRequest");
    assert!(
        server.requests().is_empty(),
        "a malformed id must not reach the server: {:?}",
        server.requests()
    );
}

/// `--help` and `--version` are successes, not usage errors.
#[test]
fn help_and_version_exit_0_on_stdout() {
    let home = new_home();
    for args in [vec!["--help"], vec!["--version"], vec!["help"]] {
        let out = openvhost(home.path(), &args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(!out.stdout.is_empty(), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}");
    }
}

/// Other crates spawn this binary as a supervised child fixture; breaking the
/// intercept breaks them, and it must stay out of `--help`.
#[test]
fn the_testchild_fixture_still_runs_and_stays_hidden() {
    let home = new_home();
    let out = openvhost(home.path(), &["__testchild", "--lines", "2", "--exit", "3"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 2);

    let bad = openvhost(home.path(), &["__testchild", "--lines", "not-a-number"]);
    assert_eq!(bad.status.code(), Some(64));

    let help = openvhost(home.path(), &["--help"]);
    assert!(
        !String::from_utf8(help.stdout)
            .unwrap()
            .contains("__testchild")
    );
}
