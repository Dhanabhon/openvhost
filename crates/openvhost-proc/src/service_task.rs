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

/// How long a terminal transition waits for this child's output readers to
/// finish before snapshotting the stderr tail (see [`drain_readers`]).
///
/// Short on purpose: it is paid on EVERY terminal transition, and it delays
/// the `Stopped`/`Failed` event reaching the UI and any waiting
/// `openvhost stop`. In the normal case it costs ~nothing — the child is
/// already gone, so both pipes are at EOF and the readers finish as fast as
/// they can be scheduled. The budget only bites when a descendant that
/// inherited the fd is still alive (nginx workers outliving a killed master),
/// which is exactly the case where waiting longer would not help either.
const READER_DRAIN_BUDGET: Duration = Duration::from_millis(500);

/// Spawn a reader for one of the child's pipes, returning its handle so
/// [`drain_readers`] can wait for it before the tail is classified.
fn spawn_reader(
    inner: Arc<Inner>,
    id: String,
    source: StreamSource,
    stream: OutputStream,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            Inner::push_log(&inner, &id, source, line);
        }
    })
}

/// Wait (bounded) for this child's output readers to finish, so a tail
/// snapshot taken immediately afterwards contains everything the child
/// actually wrote.
///
/// Without this, `child.wait()` resolving is the ONLY thing gating
/// classification, and the reader task is a separate task with its own
/// wakeups: whatever is still sitting in the pipe when the child exits — up
/// to a full pipe buffer — never reaches `stderr_tail`. Nothing between
/// `child.wait()` and the snapshot yields, so the reader gets no scheduling
/// opportunity at all. A service that writes the reason it is dying and then
/// dies (`nginx: [emerg] bind() ... failed`) is precisely the case that loses
/// the race, which is precisely the case a human needs the tail for.
///
/// Handles are TAKEN, so a timeout leaves the vector empty and a second call
/// is a no-op. Dropping a `JoinHandle` detaches rather than aborts: a reader
/// that outlives the budget keeps streaming into the log ring exactly as it
/// does today, it just stops holding up the terminal state. Aborting would
/// throw away output from a still-live descendant for no gain.
async fn drain_readers(readers: &mut Vec<tokio::task::JoinHandle<()>>, id: &str) {
    if readers.is_empty() {
        return;
    }
    let handles = std::mem::take(readers);
    let joined = async {
        for h in handles {
            let _ = h.await;
        }
    };
    if tokio::time::timeout(READER_DRAIN_BUDGET, joined)
        .await
        .is_err()
    {
        tracing::warn!(
            service_id = id,
            budget = ?READER_DRAIN_BUDGET,
            "output readers did not finish before classification (a descendant still holds the pipe); the stderr tail may be incomplete"
        );
    }
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
///
/// `readers` is drained (bounded) before the snapshot: the tail must describe
/// the run that just ended, not the part of it that happened to have been
/// consumed already. See [`drain_readers`].
async fn finish(
    inner: &Arc<Inner>,
    id: &str,
    stop_flag: &AtomicBool,
    status: Option<std::process::ExitStatus>,
    extra_tail: Vec<String>,
    readers: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    drain_readers(readers, id).await;
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
    readers: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    drain_readers(readers, id).await;
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

/// Like [`format_probe_detail`], but for an attempt that was still running
/// when the overall deadline elapsed and got killed by us — never had a
/// chance to report its own exit code, so the framing says so rather than
/// implying the probe's own logic decided to stop.
fn format_deadline_kill_detail(stdout: &str, stderr: &str) -> String {
    let mut detail = "probe still running when the deadline elapsed (killed)".to_string();
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
    NotReady {
        detail: String,
    },
    ChildExited(Option<std::process::ExitStatus>),
    /// `Some(detail)` when an attempt was actually spawned and in flight
    /// when the deadline hit — its captured stdout/stderr IS the most
    /// relevant diagnostic (review fix: this used to be silently dropped).
    /// `None` when the deadline had already elapsed before this call could
    /// even start an attempt — nothing fresher than the caller's own
    /// last-seen detail from a PRIOR completed attempt.
    DeadlineElapsed {
        detail: Option<String>,
    },
}

async fn probe_attempt_raced(
    inner: &Arc<Inner>,
    child: &mut SpawnedChild,
    argv: &[String],
    deadline_at: Instant,
) -> ProbeRace {
    let remaining = deadline_at.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return ProbeRace::DeadlineElapsed { detail: None };
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
            // Review fix: capture this (killed, in-flight) attempt's own
            // output the same way the `Ok(s)` branch above does, instead of
            // discarding it — this is the attempt the deadline actually
            // caught, so its diagnostics are the most relevant of all.
            let _ = inner.driver.kill(&mut probe);
            let _ = probe.wait().await;
            let stdout = out_task.await.unwrap_or_default();
            let stderr = err_task.await.unwrap_or_default();
            ProbeRace::DeadlineElapsed {
                detail: Some(format_deadline_kill_detail(&stdout, &stderr)),
            }
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
            ProbeRace::DeadlineElapsed { detail } => {
                // Review fix: prefer the JUST-KILLED in-flight attempt's own
                // fresh output when there is one — only fall back to the
                // last COMPLETED attempt's detail when the deadline elapsed
                // between attempts (nothing was in flight to report on).
                let detail = detail.unwrap_or(last_detail);
                Inner::push_supervisor_log(
                    inner,
                    id,
                    format!("readiness probe deadline elapsed: {detail}"),
                );
                return ReadyOutcome::ProbeDeadlineElapsed {
                    diagnostics: vec![format!("probe: {detail}")],
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
    // Every terminal path below drains these before classifying, so the
    // stderr tail describes THIS run in full. Declared before the spawn
    // attempt so the spawn-failure path shares one exit shape (it drains an
    // empty vec, which is a no-op).
    let mut readers: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut child: SpawnedChild = match inner.driver.spawn(&spec) {
        Ok(c) => c,
        Err(e) => {
            Inner::push_log(
                &inner,
                &id,
                StreamSource::Stderr,
                format!("ERROR spawn failed: {e} — {}", spec.program.display()),
            );
            finish(&inner, &id, &stop_flag, None, Vec::new(), &mut readers).await;
            return;
        }
    };
    Inner::set_pid(&inner, &id, child.id());
    if let Some(pid) = child.id() {
        Inner::record_running(&inner, &id, pid);
    }
    Inner::push_supervisor_log(&inner, &id, format!("spawned pid {:?}", child.id()));
    if let Some(out) = child.take_stdout() {
        readers.push(spawn_reader(
            Arc::clone(&inner),
            id.clone(),
            StreamSource::Stdout,
            out,
        ));
    }
    if let Some(err) = child.take_stderr() {
        readers.push(spawn_reader(
            Arc::clone(&inner),
            id.clone(),
            StreamSource::Stderr,
            err,
        ));
    }

    match await_readiness(&inner, &id, &mut child, &readiness).await {
        ReadyOutcome::Ready => {
            Inner::push_supervisor_log(&inner, &id, "state Starting → Running".to_string());
            Inner::set_state(&inner, &id, ServiceState::Running, None);
        }
        ReadyOutcome::ChildExitedBeforeAlive(status) => {
            finish(&inner, &id, &stop_flag, status, Vec::new(), &mut readers).await;
            return;
        }
        ReadyOutcome::ChildExitedDuringProbe(status) => {
            // The child already exited on its own — nothing left to kill.
            finish_never_ready(&inner, &id, &stop_flag, status, Vec::new(), &mut readers).await;
            return;
        }
        ReadyOutcome::ProbeDeadlineElapsed { diagnostics } => {
            // The child is (as far as we know) still running but never
            // confirmed ready — never leave it unmanaged. Tear it down the
            // same SIGTERM → grace → SIGKILL way a user stop would, using
            // this spec's own grace.
            let status = terminate_child(&inner, &id, &mut child, grace).await;
            finish_never_ready(&inner, &id, &stop_flag, status, diagnostics, &mut readers).await;
            return;
        }
    }

    loop {
        tokio::select! {
            status = child.wait() => {
                finish(&inner, &id, &stop_flag, status.ok(), Vec::new(), &mut readers).await;
                return;
            }
            ctl = control_rx.recv() => {
                if ctl.is_none() {
                    continue; // sender dropped without a stop request
                }
                let status = terminate_child(&inner, &id, &mut child, grace).await;
                finish(&inner, &id, &stop_flag, status, Vec::new(), &mut readers).await;
                return;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::platform::default_driver;
    use crate::supervisor::{DEFAULT_GRACE, ReadinessProbe, ServiceSpec, Supervisor};

    /// A registered-but-never-started service: these tests drive [`finish`]
    /// directly, so nothing is ever spawned.
    fn supervisor_with(id: &str) -> Supervisor {
        let sup = Supervisor::new(default_driver());
        sup.register(ServiceSpec {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: None,
            spawn: SpawnSpec {
                program: PathBuf::from("/does/not/exist"),
                args: vec![],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        });
        sup
    }

    fn tail_of(sup: &Supervisor, id: &str) -> Vec<String> {
        match sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == id)
            .expect("registered")
            .state
        {
            ServiceState::Failed { stderr_tail, .. } => stderr_tail,
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// THE DRAIN HALF (spec: the A4 fix wave).
    ///
    /// The reader is a task of its own; `child.wait()` resolving says nothing
    /// about whether it has consumed what the child wrote on its way out. This
    /// pins the missing happens-before edge exactly, with no pipe and no
    /// scheduling luck involved: the reader here has provably NOT pushed when
    /// `finish` is entered (its first act is to yield), and every step of
    /// `finish` after the drain is synchronous. So the line can only appear in
    /// the tail if `finish` waited for the reader.
    ///
    /// VACUITY: delete the `drain_readers` call from `finish` and this fails
    /// with an empty tail — deterministically, every run, because without it
    /// `finish` completes without a single yield point and the spawned reader
    /// is never polled at all.
    #[tokio::test]
    async fn finish_waits_for_a_reader_that_has_not_pushed_yet() {
        let sup = supervisor_with("svc");
        let inner = Arc::clone(&sup.inner);
        let stop = AtomicBool::new(false);

        let reader = {
            let inner = Arc::clone(&inner);
            tokio::spawn(async move {
                // The state a real reader is in when a service writes the
                // reason it is dying and dies: woken, but not yet polled to
                // the point of delivering.
                tokio::task::yield_now().await;
                Inner::push_log(
                    &inner,
                    "svc",
                    StreamSource::Stderr,
                    "ERROR the reason it died".to_string(),
                );
            })
        };
        let mut readers = vec![reader];

        finish(&inner, "svc", &stop, None, Vec::new(), &mut readers).await;

        assert_eq!(
            tail_of(&sup, "svc"),
            vec!["ERROR the reason it died".to_string()],
            "the failure was classified before its own stderr had been read"
        );
        assert!(readers.is_empty(), "drained handles must be taken");
    }

    /// …but the wait is a budget, not a promise. A descendant that inherited
    /// the pipe (nginx workers outliving a killed master) keeps the reader
    /// alive indefinitely, and a terminal state that never arrives is far
    /// worse than an incomplete tail.
    ///
    /// Time is paused, so the 500 ms budget costs nothing here and the
    /// assertion is on virtual elapsed time — a real 500 ms sleep in a unit
    /// test would be the wrong trade and would also not prove the bound is
    /// [`READER_DRAIN_BUDGET`] rather than an accident.
    ///
    /// VACUITY: replace the `timeout` in `drain_readers` with a bare join and
    /// this test hangs instead of passing.
    #[tokio::test(start_paused = true)]
    async fn finish_is_not_hostage_to_a_reader_that_never_finishes() {
        let sup = supervisor_with("svc");
        let inner = Arc::clone(&sup.inner);
        let stop = AtomicBool::new(false);

        let never = tokio::spawn(std::future::pending::<()>());
        let mut readers = vec![never];

        let t0 = tokio::time::Instant::now();
        finish(&inner, "svc", &stop, None, Vec::new(), &mut readers).await;
        let waited = t0.elapsed();

        assert!(
            waited >= READER_DRAIN_BUDGET,
            "the drain must actually wait its budget out, waited {waited:?}"
        );
        assert!(
            waited < READER_DRAIN_BUDGET * 2,
            "the drain must not wait longer than its budget, waited {waited:?}"
        );
        // Still classified, with whatever the tail held.
        assert!(tail_of(&sup, "svc").is_empty());
    }
}
