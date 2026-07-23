// SPDX-License-Identifier: GPL-3.0-or-later
//! One task per running service: spawn → raced 500ms bound → run loop →
//! two-phase stop → classify (spec §4). Readers drain pipes immediately.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::events::{ServiceState, StreamSource};
use crate::platform::{OutputStream, SpawnSpec, SpawnedChild};
use crate::state::classify_exit;
use crate::supervisor::Inner;

const RUNNING_AFTER: Duration = Duration::from_millis(500);
const GRACE_DEADLINE: Duration = Duration::from_secs(5);

fn spawn_reader(inner: Arc<Inner>, id: String, source: StreamSource, stream: OutputStream) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            Inner::push_log(&inner, &id, source, line);
        }
    });
}

async fn finish(
    inner: &Arc<Inner>,
    id: &str,
    stop_flag: &AtomicBool,
    status: Option<std::process::ExitStatus>,
) {
    let tail = Inner::stderr_tail_snapshot(inner, id);
    let state = classify_exit(stop_flag.load(Ordering::SeqCst), status.as_ref(), tail);
    let detail = match (&state, status) {
        (ServiceState::Failed { .. }, Some(s)) => Some(format!("exited with {s}")),
        (ServiceState::Failed { .. }, None) => {
            Some("exit status unavailable (spawn failed or wait errored)".to_string())
        }
        (_, Some(s)) => Some(format!("{s}")),
        _ => None,
    };
    let label = match &state {
        ServiceState::Stopped => "Stopped",
        ServiceState::Failed { .. } => "Failed",
        _ => "?",
    };
    Inner::push_supervisor_log(inner, id, format!("state → {label}"));
    // A process we ourselves observed exit (however it exited — clean stop,
    // failure, or a spawn that never got a pid) is by definition not an
    // orphan: remove its record so a future reap never considers it.
    // Best-effort: a failed remove only risks a future leaked orphan (an
    // already-dead pid reaps to a harmless no-op), never a wrong kill.
    if let Err(e) = inner.registry.remove(id) {
        tracing::warn!(service_id = id, error = %e, "failed to remove supervised-process record on terminal state");
    }
    Inner::set_state(inner, id, state, detail);
}

pub(crate) async fn run(
    inner: Arc<Inner>,
    id: String,
    spec: SpawnSpec,
    stop_flag: Arc<AtomicBool>,
    mut control_rx: mpsc::Receiver<()>,
) {
    let mut child: SpawnedChild = match inner.driver.spawn(&spec) {
        Ok(c) => c,
        Err(e) => {
            Inner::push_log(
                &inner,
                &id,
                StreamSource::Stderr,
                format!("ERROR spawn failed: {e} — {}", spec.program.display()),
            );
            finish(&inner, &id, &stop_flag, None).await;
            return;
        }
    };
    Inner::set_pid(&inner, &id, child.id());
    if let Some(pid) = child.id() {
        Inner::record_running(&inner, &id, pid);
    }
    Inner::push_supervisor_log(&inner, &id, format!("spawned pid {:?}", child.id()));
    if let Some(out) = child.take_stdout() {
        spawn_reader(Arc::clone(&inner), id.clone(), StreamSource::Stdout, out);
    }
    if let Some(err) = child.take_stderr() {
        spawn_reader(Arc::clone(&inner), id.clone(), StreamSource::Stderr, err);
    }

    // Raced 500ms bound: death during the window reports instantly (spec §4).
    tokio::select! {
        _ = tokio::time::sleep(RUNNING_AFTER) => {
            Inner::push_supervisor_log(&inner, &id, "state Starting → Running".to_string());
            Inner::set_state(&inner, &id, ServiceState::Running, None);
        }
        status = child.wait() => {
            finish(&inner, &id, &stop_flag, status.ok()).await;
            return;
        }
    }

    loop {
        tokio::select! {
            status = child.wait() => {
                finish(&inner, &id, &stop_flag, status.ok()).await;
                return;
            }
            ctl = control_rx.recv() => {
                if ctl.is_none() {
                    continue; // sender dropped without a stop request
                }
                // Two-phase stop. ESRCH-style errors mean "already gone" —
                // fall through to wait() either way (spec §5).
                let _ = inner.driver.request_graceful_stop(&child);
                tokio::select! {
                    status = child.wait() => {
                        finish(&inner, &id, &stop_flag, status.ok()).await;
                        return;
                    }
                    _ = tokio::time::sleep(GRACE_DEADLINE) => {
                        Inner::push_supervisor_log(&inner, &id, "grace deadline passed — killing".to_string());
                        let _ = inner.driver.kill(&mut child);
                        let status = child.wait().await.ok();
                        finish(&inner, &id, &stop_flag, status).await;
                        return;
                    }
                }
            }
        }
    }
}
