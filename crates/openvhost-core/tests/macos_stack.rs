// SPDX-License-Identifier: GPL-3.0-or-later
//! P0-4 exit-criterion proof: real nginx + php-fpm under the supervisor,
//! phpinfo served over the provisioned unix socket, clean teardown.
//! Auto-skips (loudly) when the Homebrew binaries are absent — NOT
//! `#[ignore]`, so `cargo test --workspace` on a dev Mac always runs it.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]
// `wait_state`'s `if let Some(s) = &st { if pred(s) { return; } }` triggers
// clippy's let-chain collapse suggestion on this toolchain; kept as
// nested ifs to stay byte-faithful to the brief's exact test logic.
#![allow(clippy::collapsible_if)]

use std::ffi::OsString;
use std::time::{Duration, Instant};

use openvhost_core::platform::macos::demo_stack::{
    BrewStack, find_brew_binaries, provision_macos_demo_stack,
};
use openvhost_proc::{ServiceSpec, ServiceState, SpawnSpec, Supervisor, default_driver};

fn ephemeral_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `curl -sf` — exit 0 only on 2xx; returns the body.
fn curl(port: u16) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/curl")
        .args(["-sf", "-m", "2", &format!("http://127.0.0.1:{port}/")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn wait_state(
    sup: &Supervisor,
    id: &str,
    timeout: Duration,
    pred: impl Fn(&ServiceState) -> bool,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let st = sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.state);
        if let Some(s) = &st {
            if pred(s) {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting on '{id}'; last state: {st:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn spec(id: &str, program: std::path::PathBuf, args: Vec<OsString>) -> ServiceSpec {
    ServiceSpec {
        id: id.into(),
        display_name: id.into(),
        endpoint: None,
        spawn: SpawnSpec {
            program,
            args,
            cwd: None,
            env: vec![],
        },
    }
}

/// Force-stops both services even when an assertion panics mid-test:
/// `Supervisor` has no Drop kill path, so a failing run would otherwise
/// leak live nginx/php-fpm processes. Best-effort: `stop` is fired for
/// both ids and each is polled briefly toward a terminal state; the
/// Step-4 pgrep audit remains the backstop.
struct StopGuard<'a> {
    sup: &'a Supervisor,
    ids: [&'static str; 2],
}

impl Drop for StopGuard<'_> {
    fn drop(&mut self) {
        for id in self.ids {
            let _ = self.sup.stop(id);
        }
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
async fn stack_serves_phpinfo_and_tears_down_clean() {
    let Some(BrewStack { nginx, php_fpm }) = find_brew_binaries() else {
        eprintln!("SKIP macos_stack: Homebrew nginx/php not found (brew install nginx php)");
        return;
    };

    // /tmp keeps the socket far under Darwin's 104-byte sun_path limit
    // (TMPDIR is /var/folders/... — brittle; scratchpads measured over it).
    let home = tempfile::Builder::new()
        .prefix("ovh")
        .tempdir_in("/tmp")
        .unwrap();
    let port = ephemeral_port();
    let paths = provision_macos_demo_stack(home.path(), port).unwrap();

    let sup = Supervisor::new(default_driver());
    let _guard = StopGuard {
        sup: &sup,
        ids: ["php-fpm", "nginx"],
    };
    sup.register(spec(
        "php-fpm",
        php_fpm,
        vec![
            OsString::from("-F"),
            OsString::from("-O"),
            OsString::from("-n"),
            OsString::from("-y"),
            paths.fpm_conf.clone().into_os_string(),
        ],
    ));
    sup.register(spec(
        "nginx",
        nginx,
        vec![
            OsString::from("-e"),
            paths.nginx_error_log.clone().into_os_string(),
            OsString::from("-c"),
            paths.nginx_conf.clone().into_os_string(),
        ],
    ));

    // fpm first; wait for the socket file, not just Running.
    sup.start("php-fpm").unwrap();
    wait_state(&sup, "php-fpm", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;
    let sock_deadline = Instant::now() + Duration::from_secs(5);
    while !paths.socket.exists() {
        assert!(
            Instant::now() < sock_deadline,
            "php-fpm socket never appeared"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    sup.start("nginx").unwrap();
    wait_state(&sup, "nginx", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;

    // Poll curl until phpinfo arrives (first fpm worker spawn can lag).
    let curl_deadline = Instant::now() + Duration::from_secs(10);
    let body = loop {
        if let Some(b) = curl(port) {
            break b;
        }
        assert!(Instant::now() < curl_deadline, "curl never got HTTP 200");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(body.contains("phpinfo"), "200 body lacks phpinfo");

    // Teardown: group-TERM per service; both must reach Stopped, socket
    // unlinked by fpm, port closed.
    sup.stop("nginx").unwrap();
    wait_state(&sup, "nginx", Duration::from_secs(3), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
    sup.stop("php-fpm").unwrap();
    wait_state(&sup, "php-fpm", Duration::from_secs(3), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
    assert!(!paths.socket.exists(), "fpm socket not unlinked on stop");
    assert!(curl(port).is_none(), "port still serving after stop");
}
