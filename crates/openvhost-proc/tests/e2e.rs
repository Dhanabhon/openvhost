// SPDX-License-Identifier: GPL-3.0-or-later
//! P0-9 hermetic E2E: supervise an in-repo HTTP server (`proc_testchild --http`),
//! assert it serves a `200` + sentinel, stop it, and assert the port is no
//! longer served (process gone, no orphan). No external binaries, no network.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
use common::{ephemeral_port, http_get};

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use openvhost_proc::testchild::E2E_BODY;
use openvhost_proc::{
    DEFAULT_GRACE, ReadinessProbe, ServiceSpec, ServiceState, SpawnSpec, Supervisor, default_driver,
};

fn http_spec(port: u16) -> ServiceSpec {
    ServiceSpec {
        id: "http-e2e".into(),
        display_name: "http e2e".into(),
        endpoint: None,
        spawn: SpawnSpec {
            program: PathBuf::from(env!("CARGO_BIN_EXE_proc_testchild")),
            args: vec![OsString::from("--http"), OsString::from(port.to_string())],
            cwd: None,
            env: vec![],
        },
        readiness: ReadinessProbe::default(),
        grace: DEFAULT_GRACE,
    }
}

// Nested if (rather than the `if let ... && pred(...)` let-chain clippy
// suggests) is kept intentionally: collapsing it doesn't touch any
// assertion in the test below, but the brief's helper is reproduced as
// specified. `-D warnings` on this workspace's toolchain flags the
// collapsible form, so it is suppressed here rather than restructured.
#[allow(clippy::collapsible_if)]
async fn wait_state(
    sup: &Supervisor,
    id: &str,
    deadline: Instant,
    pred: impl Fn(&ServiceState) -> bool,
) -> bool {
    loop {
        if let Some(s) = sup.snapshot().into_iter().find(|s| s.id == id) {
            if pred(&s.state) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Force-stops the service on drop so a mid-test panic never leaks the child.
/// Mirrors `orphan_reap.rs` / `macos_stack.rs`: after requesting stop, block
/// (bounded) until the service is terminal, so the tokio-spawned service task
/// has time to deliver SIGTERM and reap the child before the panicking thread
/// unwinds out. This is why the test is `flavor = "multi_thread"` — on a
/// single-threaded runtime this blocking poll would starve the very service
/// task it is waiting on.
struct StopGuard<'a> {
    sup: &'a Supervisor,
    id: &'static str,
}
impl Drop for StopGuard<'_> {
    fn drop(&mut self) {
        let _ = self.sup.stop(self.id);
        let deadline = Instant::now() + Duration::from_secs(3);
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

#[tokio::test(flavor = "multi_thread")]
async fn supervised_http_service_serves_then_stops_clean() {
    let port = ephemeral_port();
    let sup = Supervisor::new(default_driver());
    sup.register(http_spec(port));
    let _guard = StopGuard {
        sup: &sup,
        id: "http-e2e",
    };

    sup.start("http-e2e").unwrap();
    assert!(
        wait_state(
            &sup,
            "http-e2e",
            Instant::now() + Duration::from_secs(5),
            |s| { matches!(s, ServiceState::Running) }
        )
        .await,
        "service never reached Running"
    );

    // Poll until the server actually answers (first accept can lag Running).
    let body = http_get(port, Instant::now() + Duration::from_secs(10))
        .expect("supervised HTTP server never returned a response");
    assert!(body.contains("200 OK"), "not a 200: {body}");
    assert!(
        body.contains(E2E_BODY),
        "200 body lacks the E2E sentinel: {body}"
    );

    sup.stop("http-e2e").unwrap();
    assert!(
        wait_state(
            &sup,
            "http-e2e",
            Instant::now() + Duration::from_secs(5),
            |s| { matches!(s, ServiceState::Stopped) }
        )
        .await,
        "service never reached Stopped"
    );

    // Cross-OS teardown proof: the port is no longer served -> the process is
    // gone and left no orphan holding the socket.
    assert!(
        http_get(port, Instant::now() + Duration::from_secs(2)).is_none(),
        "port still served after Stopped -> leaked/orphaned server"
    );
}
