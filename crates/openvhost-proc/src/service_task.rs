// SPDX-License-Identifier: GPL-3.0-or-later
//! One task per running service: spawn → readiness (raced 500ms bound, or a
//! repeated `Command` probe) → run loop → two-phase stop → classify (spec
//! §4; readiness probe + per-service grace: P1 MySQL lifecycle design, spec
//! D4). Readers drain pipes immediately.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::events::{ServiceState, StreamSource};
use crate::platform::{OutputStream, SpawnSpec, SpawnedChild};
use crate::state::{classify_exit, classify_exit_during_probe};
use crate::supervisor::{Inner, ReadinessProbe};

/// How long to wait between unsuccessful `Command` probe attempts before
/// retrying. Independent of a probe's overall `deadline`, which bounds the
/// whole wait regardless of how many attempts fit inside it.
const PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(200);

fn spawn_reader(inner: Arc<Inner>, id: String, source: StreamSource, stream: OutputStream) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            Inner::push_log(&inner, &id, source, line);
        }
    });
}

/// Shared terminal-state bookkeeping: compute the broadcast `detail`, log the
/// transition, drop the crash-orphan record (a state we ourselves observed is
/// by definition not an orphan), and broadcast the new state. Takes an
/// already-classified `state` so [`finish`] (today's rule) and
/// [`finish_never_ready`] (spec D4's stricter "was never confirmed ready"
/// rule) can share this without duplicating the bookkeeping.
async fn finish_with_state(
    inner: &Arc<Inner>,
    id: &str,
    state: ServiceState,
    status: Option<std::process::ExitStatus>,
) {
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

/// Terminal path for every service EXCEPT one killed for never confirming
/// readiness — see [`finish_never_ready`] for that one. `extra_tail` is
/// appended to the child's own captured stderr tail before classification
/// (empty for every pre-existing call site; today's behavior, unchanged).
async fn finish(
    inner: &Arc<Inner>,
    id: &str,
    stop_flag: &AtomicBool,
    status: Option<std::process::ExitStatus>,
    extra_tail: Vec<String>,
) {
    let mut tail = Inner::stderr_tail_snapshot(inner, id);
    tail.extend(extra_tail);
    let state = classify_exit(stop_flag.load(Ordering::SeqCst), status.as_ref(), tail);
    finish_with_state(inner, id, state, status).await;
}

/// Terminal path for a service that exited (or was killed after its
/// `Command` readiness probe's deadline elapsed) before ever confirming
/// readiness (spec D4). Unlike [`finish`], a clean exit code does not mean
/// `Stopped` here — see [`classify_exit_during_probe`].
async fn finish_never_ready(
    inner: &Arc<Inner>,
    id: &str,
    stop_flag: &AtomicBool,
    status: Option<std::process::ExitStatus>,
    extra_tail: Vec<String>,
) {
    let mut tail = Inner::stderr_tail_snapshot(inner, id);
    tail.extend(extra_tail);
    let state = classify_exit_during_probe(stop_flag.load(Ordering::SeqCst), status.as_ref(), tail);
    finish_with_state(inner, id, state, status).await;
}

/// Two-phase stop: politely ask, then wait up to `grace`, then force-kill.
/// Shared by the user-initiated stop path and the probe-deadline teardown
/// path (spec D4) — a service that never became ready is torn down exactly
/// like one the user stopped, using the SAME per-spec grace, so nothing
/// walks away un-reaped and no second grace-duration knob needs inventing.
async fn terminate_child(
    inner: &Arc<Inner>,
    id: &str,
    child: &mut SpawnedChild,
    grace: Duration,
) -> Option<std::process::ExitStatus> {
    // ESRCH-style errors mean "already gone" — fall through to wait() either
    // way (spec §5).
    let _ = inner.driver.request_graceful_stop(child);
    tokio::select! {
        status = child.wait() => status.ok(),
        _ = tokio::time::sleep(grace) => {
            Inner::push_supervisor_log(inner, id, "grace deadline passed — killing".to_string());
            let _ = inner.driver.kill(child);
            child.wait().await.ok()
        }
    }
}

/// Drains an optional pipe to completion into one string (lines joined with
/// `" | "`). Used for a `Command` probe's stdout/stderr: unlike the
/// long-lived service child (whose output streams into the shared log ring
/// via [`spawn_reader`]), a probe's tiny, bounded output is collected
/// privately so it can be folded into the probe's own diagnostics rather
/// than mixed into the service's regular log stream.
async fn drain_to_string(stream: Option<OutputStream>) -> String {
    let Some(s) = stream else {
        return String::new();
    };
    let mut lines = BufReader::new(s).lines();
    let mut out = String::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if !out.is_empty() {
            out.push_str(" | ");
        }
        out.push_str(&line);
    }
    out
}

fn format_probe_detail(code: Option<i32>, stdout: &str, stderr: &str) -> String {
    let mut detail = match code {
        Some(c) => format!("probe exited {c}"),
        None => "probe terminated by signal".to_string(),
    };
    if !stderr.is_empty() {
        detail.push_str(": ");
        detail.push_str(stderr);
    } else if !stdout.is_empty() {
        detail.push_str(": ");
        detail.push_str(stdout);
    }
    detail
}

/// Outcome of ONE `Command` probe attempt, raced against the supervised
/// child's own exit and the overall probe deadline. Whichever of the three
/// loses, the probe subprocess (if it was ever spawned) is explicitly killed
/// and reaped before this returns — `tokio::process::Child` is never
/// `kill_on_drop` in this crate (spec §5), so simply letting a losing
/// branch's future drop would leak it.
enum ProbeRace {
    Success,
    NotReady { detail: String },
    ChildExited(Option<std::process::ExitStatus>),
    DeadlineElapsed,
}

async fn probe_attempt_raced(
    inner: &Arc<Inner>,
    child: &mut SpawnedChild,
    argv: &[String],
    deadline_at: Instant,
) -> ProbeRace {
    let remaining = deadline_at.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return ProbeRace::DeadlineElapsed;
    }
    let Some((program, rest)) = argv.split_first() else {
        return ProbeRace::NotReady {
            detail: "probe argv is empty".to_string(),
        };
    };
    let probe_spec = SpawnSpec {
        program: PathBuf::from(program),
        args: rest.iter().map(OsString::from).collect(),
        cwd: None,
        env: vec![],
    };
    let mut probe = match inner.driver.spawn(&probe_spec) {
        Ok(c) => c,
        Err(e) => {
            return ProbeRace::NotReady {
                detail: format!("probe spawn failed: {e} — {}", probe_spec.program.display()),
            };
        }
    };
    let out_task = tokio::spawn(drain_to_string(probe.take_stdout()));
    let err_task = tokio::spawn(drain_to_string(probe.take_stderr()));

    tokio::select! {
        status = probe.wait() => {
            let stdout = out_task.await.unwrap_or_default();
            let stderr = err_task.await.unwrap_or_default();
            match status {
                Ok(s) if s.success() => ProbeRace::Success,
                Ok(s) => ProbeRace::NotReady {
                    detail: format_probe_detail(s.code(), &stdout, &stderr),
                },
                Err(e) => ProbeRace::NotReady {
                    detail: format!("probe wait failed: {e}"),
                },
            }
        }
        status = child.wait() => {
            // The SUPERVISED child died while this probe attempt was still
            // in flight: without this, the probe subprocess would be
            // leaked — nothing else will ever await or kill it.
            let _ = inner.driver.kill(&mut probe);
            let _ = probe.wait().await;
            let _ = out_task.await;
            let _ = err_task.await;
            ProbeRace::ChildExited(status.ok())
        }
        _ = tokio::time::sleep(remaining) => {
            let _ = inner.driver.kill(&mut probe);
            let _ = probe.wait().await;
            let _ = out_task.await;
            let _ = err_task.await;
            ProbeRace::DeadlineElapsed
        }
    }
}

/// Outcome of the whole readiness wait — either variant of
/// [`ReadinessProbe`]. The two `Command`-only variants are kept distinct
/// (rather than folding "deadline elapsed" into a `status: None` case)
/// because `None` can ALSO mean "`child.wait()` itself errored" for
/// [`ChildExitedDuringProbe`](ReadyOutcome::ChildExitedDuringProbe) —
/// conflating the two would make [`run`] guess whether the child is still
/// alive from an ambiguous `Option`.
enum ReadyOutcome {
    Ready,
    /// The child exited during the raced `AliveAfter` bound. Classified by
    /// the STANDARD rule ([`classify_exit`]) — today's behavior, unchanged.
    ChildExitedBeforeAlive(Option<std::process::ExitStatus>),
    /// The child exited (with a status, or `None` if the wait itself
    /// errored) while a `Command` probe was still outstanding — nothing left
    /// to kill. Always `Failed` unless a stop was independently requested —
    /// see [`classify_exit_during_probe`].
    ChildExitedDuringProbe(Option<std::process::ExitStatus>),
    /// The probe's own `deadline` elapsed while the child was, as far as we
    /// know, still running: [`run`] must kill it (see [`terminate_child`])
    /// before classifying — it must never be left unmanaged. `diagnostics`
    /// carries the probe's own last exit/stderr detail.
    ProbeDeadlineElapsed {
        diagnostics: Vec<String>,
    },
}

async fn run_command_probe(
    inner: &Arc<Inner>,
    id: &str,
    child: &mut SpawnedChild,
    argv: &[String],
    deadline: Duration,
) -> ReadyOutcome {
    let deadline_at = Instant::now() + deadline;
    let mut last_detail = "probe deadline elapsed before any attempt completed".to_string();
    loop {
        match probe_attempt_raced(inner, child, argv, deadline_at).await {
            ProbeRace::Success => return ReadyOutcome::Ready,
            ProbeRace::ChildExited(status) => return ReadyOutcome::ChildExitedDuringProbe(status),
            ProbeRace::DeadlineElapsed => {
                Inner::push_supervisor_log(
                    inner,
                    id,
                    format!("readiness probe deadline elapsed: {last_detail}"),
                );
                return ReadyOutcome::ProbeDeadlineElapsed {
                    diagnostics: vec![format!("probe: {last_detail}")],
                };
            }
            ProbeRace::NotReady { detail } => {
                Inner::push_supervisor_log(
                    inner,
                    id,
                    format!("readiness probe not ready: {detail}"),
                );
                last_detail = detail;
            }
        }
        let remaining = deadline_at.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            continue; // next loop iteration reports the timeout uniformly
        }
        tokio::select! {
            _ = tokio::time::sleep(PROBE_RETRY_INTERVAL.min(remaining)) => {}
            status = child.wait() => {
                return ReadyOutcome::ChildExitedDuringProbe(status.ok());
            }
        }
    }
}

async fn await_readiness(
    inner: &Arc<Inner>,
    id: &str,
    child: &mut SpawnedChild,
    readiness: &ReadinessProbe,
) -> ReadyOutcome {
    match readiness {
        ReadinessProbe::AliveAfter(d) => {
            // Raced bound: death during the window reports instantly (spec §4).
            tokio::select! {
                _ = tokio::time::sleep(*d) => ReadyOutcome::Ready,
                status = child.wait() => ReadyOutcome::ChildExitedBeforeAlive(status.ok()),
            }
        }
        ReadinessProbe::Command { argv, deadline } => {
            run_command_probe(inner, id, child, argv, *deadline).await
        }
    }
}

pub(crate) async fn run(
    inner: Arc<Inner>,
    id: String,
    spec: SpawnSpec,
    readiness: ReadinessProbe,
    grace: Duration,
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
            finish(&inner, &id, &stop_flag, None, Vec::new()).await;
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

    match await_readiness(&inner, &id, &mut child, &readiness).await {
        ReadyOutcome::Ready => {
            Inner::push_supervisor_log(&inner, &id, "state Starting → Running".to_string());
            Inner::set_state(&inner, &id, ServiceState::Running, None);
        }
        ReadyOutcome::ChildExitedBeforeAlive(status) => {
            finish(&inner, &id, &stop_flag, status, Vec::new()).await;
            return;
        }
        ReadyOutcome::ChildExitedDuringProbe(status) => {
            // The child already exited on its own — nothing left to kill.
            finish_never_ready(&inner, &id, &stop_flag, status, Vec::new()).await;
            return;
        }
        ReadyOutcome::ProbeDeadlineElapsed { diagnostics } => {
            // The child is (as far as we know) still running but never
            // confirmed ready — never leave it unmanaged. Tear it down the
            // same SIGTERM → grace → SIGKILL way a user stop would, using
            // this spec's own grace.
            let status = terminate_child(&inner, &id, &mut child, grace).await;
            finish_never_ready(&inner, &id, &stop_flag, status, diagnostics).await;
            return;
        }
    }

    loop {
        tokio::select! {
            status = child.wait() => {
                finish(&inner, &id, &stop_flag, status.ok(), Vec::new()).await;
                return;
            }
            ctl = control_rx.recv() => {
                if ctl.is_none() {
                    continue; // sender dropped without a stop request
                }
                let status = terminate_child(&inner, &id, &mut child, grace).await;
                finish(&inner, &id, &stop_flag, status, Vec::new()).await;
                return;
            }
        }
    }
}
