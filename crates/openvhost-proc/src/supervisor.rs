// SPDX-License-Identifier: GPL-3.0-or-later
//! Registry + control surface + single broadcast event stream (spec §3).
//! Locks are short and never held across an await.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc};

use crate::error::ProcError;
use crate::events::{LogLine, ServiceState, ServiceStatus, StreamSource, SupervisorEvent};
use crate::log::{RING_CAPACITY, RingBuffer, STDERR_TAIL, classify_level};
use crate::platform::{ProcessDriver, SpawnSpec};

/// How the supervisor decides a freshly spawned child has actually become
/// ready to use, distinct from merely having survived spawning (P1 MySQL
/// lifecycle design, spec D4). `Default` is today's `nginx`/`php-fpm`
/// behavior, verbatim — an existing [`ServiceSpec`] that does not opt in
/// keeps this shape byte-for-byte.
#[derive(Debug, Clone)]
pub enum ReadinessProbe {
    /// `Running` fires once the child has survived this long without
    /// exiting, raced against the child's own exit — today's behavior.
    AliveAfter(Duration),
    /// Re-run `argv` (a fresh process per attempt, spawned through the same
    /// [`ProcessDriver`] as every other child) until one exits `0` (ready)
    /// or `deadline` elapses without a success — whichever comes first. The
    /// service stays `Starting` for the whole wait.
    ///
    /// A child that exits — with ANY code — while a `Command` probe is
    /// still outstanding is always classified `Failed` (unless a stop was
    /// independently requested): unlike `AliveAfter`, "ready" was never
    /// confirmed, so a clean-looking exit code is not a clean stop.
    Command {
        argv: Vec<String>,
        deadline: Duration,
    },
}

impl Default for ReadinessProbe {
    /// `AliveAfter(500ms)` — today's behavior.
    fn default() -> Self {
        Self::AliveAfter(DEFAULT_READY_AFTER)
    }
}

/// The `AliveAfter` bound backing [`ReadinessProbe::default`] — today's
/// 500ms race window, named so it is defined exactly once (was a private
/// `RUNNING_AFTER` constant in `service_task.rs`).
pub const DEFAULT_READY_AFTER: Duration = Duration::from_millis(500);

/// The stop grace backing any [`ServiceSpec`] that does not set its own —
/// today's 5s, named so it is defined exactly once (was a private
/// `GRACE_DEADLINE` constant in `service_task.rs`; now per-spec, spec D4).
pub const DEFAULT_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub id: String,
    pub display_name: String,
    pub endpoint: Option<String>,
    pub spawn: SpawnSpec,
    /// How to decide `Starting` → `Running`. Defaults to
    /// [`ReadinessProbe::default`] (today's behavior, unchanged).
    pub readiness: ReadinessProbe,
    /// Stop path: SIGTERM → wait this long → SIGKILL. Defaults to
    /// [`DEFAULT_GRACE`] (5s, today's hardcoded value).
    pub grace: Duration,
}

pub(crate) struct Entry {
    pub(crate) spec: ServiceSpec,
    pub(crate) state: ServiceState,
    pub(crate) pid: Option<u32>,
    pub(crate) logs: RingBuffer,
    pub(crate) stderr_tail: VecDeque<String>,
    pub(crate) stop_requested: Arc<AtomicBool>,
    pub(crate) control: Option<mpsc::Sender<()>>,
}

pub(crate) struct Inner {
    pub(crate) driver: Arc<dyn ProcessDriver>,
    pub(crate) entries: Mutex<HashMap<String, Entry>>,
    pub(crate) tx: broadcast::Sender<SupervisorEvent>,
    pub(crate) registry: Arc<dyn crate::orphan::ProcessRegistry>,
}

#[derive(Clone)]
pub struct Supervisor {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Supervisor {
    /// Construct a supervisor with NO crash-orphan cleanup: nothing is ever
    /// recorded or reaped (backed by a crate-private `NoopRegistry`).
    /// Delegates to [`Supervisor::with_orphan_cleanup`] — reaping an always-
    /// empty `NoopRegistry` touches no filesystem and kills nothing, so this
    /// keeps every pre-P0-8-Task-4 caller's observable behavior byte-for-byte
    /// unchanged while sharing one code path. Prefer
    /// [`Supervisor::with_orphan_cleanup`] for any supervisor that should
    /// participate in crash-orphan cleanup (spec §6/§7/§9) — that is what the
    /// desktop app uses.
    pub fn new(driver: Arc<dyn ProcessDriver>) -> Self {
        Self::with_orphan_cleanup(
            driver,
            Arc::new(crate::orphan::NoopRegistry),
            crate::platform::default_reaper(),
        )
    }

    /// Construct a supervisor that participates in crash-orphan cleanup:
    /// reaps any orphans recorded by a PRIOR run against this same
    /// `registry` before returning, then holds `registry` for record-at-spawn
    /// (`Inner::record_running`) and remove-on-terminal-state
    /// (`service_task::finish`).
    ///
    /// Safety invariant (spec §6): the reap runs BEFORE `Inner`/its entry map
    /// exist, so no service can be registered or started until it completes
    /// — a freshly spawned child can never be mistaken for a record this
    /// same run just wrote.
    pub fn with_orphan_cleanup(
        driver: Arc<dyn ProcessDriver>,
        registry: Arc<dyn crate::orphan::ProcessRegistry>,
        reaper: Arc<dyn crate::orphan::OrphanReaper>,
    ) -> Self {
        let report = crate::orphan::reap_orphans(&*registry, &*reaper);
        tracing::info!(?report, "supervisor: orphan reap complete");
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                driver,
                entries: Mutex::new(HashMap::new()),
                tx,
                registry,
            }),
        }
    }

    /// Register a service under `spec.id`.
    ///
    /// Re-registering a live service is a no-op; stop it first. Otherwise
    /// broadcasts [`SupervisorEvent::Registered`] with the freshly stored
    /// [`ServiceStatus`] — the observer-visibility fix for a service
    /// registered after launch (spec `2026-07-31-p1-tray-design.md` D2). A
    /// receiver that subscribed before this call sees it; one that
    /// subscribes later does not (ordinary broadcast-channel semantics), so
    /// the boot sequence — which registers every initial service BEFORE
    /// `subscribe` is ever called — observes no change in behavior.
    pub fn register(&self, spec: ServiceSpec) {
        let id = spec.id.clone();
        let status = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let is_live = entries
                .get(&id)
                .is_some_and(|e| matches!(e.state, ServiceState::Starting | ServiceState::Running));
            if is_live {
                return;
            }
            let status = ServiceStatus {
                id: id.clone(),
                display_name: spec.display_name.clone(),
                endpoint: spec.endpoint.clone(),
                pid: None,
                state: ServiceState::Stopped,
            };
            entries.insert(
                id,
                Entry {
                    spec,
                    state: ServiceState::Stopped,
                    pid: None,
                    logs: RingBuffer::new(RING_CAPACITY),
                    stderr_tail: VecDeque::new(),
                    stop_requested: Arc::new(AtomicBool::new(false)),
                    control: None,
                },
            );
            status
        };
        let _ = self.inner.tx.send(SupervisorEvent::Registered { status });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SupervisorEvent> {
        self.inner.tx.subscribe()
    }

    pub fn snapshot(&self) -> Vec<ServiceStatus> {
        let entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<ServiceStatus> = entries
            .values()
            .map(|e| ServiceStatus {
                id: e.spec.id.clone(),
                display_name: e.spec.display_name.clone(),
                endpoint: e.spec.endpoint.clone(),
                pid: e.pid,
                state: e.state.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn log_tail(&self, id: &str, n: usize) -> Result<Vec<LogLine>, ProcError> {
        let entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        let e = entries
            .get(id)
            .ok_or_else(|| ProcError::NotFound(id.to_string()))?;
        Ok(e.logs.tail(n))
    }

    /// Must be called from within a tokio runtime context: spawns the
    /// service task with `tokio::spawn`.
    pub fn start(&self, id: &str) -> Result<(), ProcError> {
        let (spawn, readiness, grace, stop_flag, control_rx, prior) = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = entries
                .get_mut(id)
                .ok_or_else(|| ProcError::NotFound(id.to_string()))?;
            if matches!(e.state, ServiceState::Starting | ServiceState::Running) {
                return Ok(());
            }
            let prior = match &e.state {
                ServiceState::Stopped => "Stopped",
                ServiceState::Failed { .. } => "Failed",
                _ => "?",
            };
            e.stop_requested.store(false, Ordering::SeqCst);
            // A run's diagnostics must be ITS OWN. `stderr_tail` is the input
            // `classify_exit` puts inside `Failed { stderr_tail }` — the one
            // thing the GUI's Failed chip and `openvhost start` show a human
            // as the REASON a service failed. It was created once in
            // `register` and only ever appended to, so without this a failure
            // reports the previous run's lines: measured over five identical
            // nginx start-fail cycles, three reported nothing from the failing
            // run at all — only the prior run's clean-shutdown `[notice]`
            // lines, presented as the cause of a different process failing to
            // start.
            //
            // Cleared HERE, inside the same locked guard-and-transition block
            // as the `Starting` write below, so a run's tail cannot be cleared
            // by a `start` that then returns early at the no-op guard.
            //
            // The log RING is deliberately NOT cleared: that is history, and
            // the log viewer wants it across runs. Only the classification
            // input is per-run.
            e.stderr_tail.clear();
            let (ctl_tx, ctl_rx) = mpsc::channel(1);
            e.control = Some(ctl_tx);
            // The Starting write MUST happen inside this same locked block,
            // right alongside the no-op guard above. If it were done via a
            // second `entries.lock()` after this block ends (as a call to
            // `Inner::set_state` used to do), a concurrent `start(id)` could
            // acquire the lock in the gap, observe the still-stale
            // pre-Starting state, pass the guard too, and spawn a second
            // service_task for the same id (double-spawn).
            e.state = ServiceState::Starting;
            (
                e.spec.spawn.clone(),
                e.spec.readiness.clone(),
                e.spec.grace,
                Arc::clone(&e.stop_requested),
                ctl_rx,
                prior,
            )
        };
        // The state was already written above under the lock; only emit the
        // notification here. Do NOT re-lock to "set" it again — that would
        // reopen the exact TOCTOU window this split closes.
        Inner::emit_state(
            &self.inner,
            id,
            ServiceState::Starting,
            Some("requested by user".into()),
        );
        Inner::push_supervisor_log(
            &self.inner,
            id,
            format!("state {prior} → Starting (requested by user)"),
        );
        let inner = Arc::clone(&self.inner);
        let id_owned = id.to_string();
        tokio::spawn(crate::service_task::run(
            inner, id_owned, spawn, readiness, grace, stop_flag, control_rx,
        ));
        Ok(())
    }

    pub fn stop(&self, id: &str) -> Result<(), ProcError> {
        let control = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = entries
                .get_mut(id)
                .ok_or_else(|| ProcError::NotFound(id.to_string()))?;
            if matches!(e.state, ServiceState::Stopped | ServiceState::Failed { .. }) {
                return Ok(());
            }
            // Flag FIRST — classification consults it (spec §4).
            e.stop_requested.store(true, Ordering::SeqCst);
            e.control.clone()
        };
        Inner::push_supervisor_log(&self.inner, id, "stop requested by user".to_string());
        if let Some(ctl) = control {
            let _ = ctl.try_send(());
        }
        Ok(())
    }
}

impl Inner {
    pub(crate) fn set_state(
        inner: &Arc<Inner>,
        id: &str,
        state: ServiceState,
        detail: Option<String>,
    ) {
        {
            let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = entries.get_mut(id) {
                e.state = state.clone();
                if matches!(state, ServiceState::Stopped | ServiceState::Failed { .. }) {
                    e.pid = None;
                    e.control = None;
                }
            }
        }
        Self::emit_state(inner, id, state, detail);
    }

    /// Broadcast a `StateChanged` event only — no entry-map write.
    ///
    /// For callers that already wrote the new state into the entry
    /// themselves while holding the `entries` lock (currently just
    /// `Supervisor::start`'s atomic guard-and-transition) so they never need
    /// to re-lock purely to notify. Re-locking there would reopen the same
    /// TOCTOU window that writing the state inside the original lock scope
    /// was meant to close.
    pub(crate) fn emit_state(
        inner: &Arc<Inner>,
        id: &str,
        state: ServiceState,
        detail: Option<String>,
    ) {
        let _ = inner.tx.send(SupervisorEvent::StateChanged {
            id: id.to_string(),
            state,
            detail,
        });
    }

    pub(crate) fn set_pid(inner: &Arc<Inner>, id: &str, pid: Option<u32>) {
        let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.get_mut(id) {
            e.pid = pid;
        }
    }

    /// Record identity at SPAWN, not at Running (spec §9, adopted): the
    /// start-time is read immediately — same source as the reap-time compare
    /// — shrinking the unrecorded-orphan window to ~0 (recording only once
    /// `Running` is reached would leave the whole `Starting` window
    /// unrecorded). Best-effort throughout: a registry write failure only
    /// risks a future leaked orphan, never a wrong kill.
    pub(crate) fn record_running(inner: &Arc<Inner>, id: &str, pid: u32) {
        match crate::platform::process_start_time(pid) {
            Ok(Some(start_time)) => {
                let rec = crate::orphan::SupervisedRecord {
                    service_id: id.to_string(),
                    identity: crate::orphan::ProcIdentity { pid, start_time },
                    recorded_at_ms: now_ms(),
                };
                if let Err(e) = inner.registry.record(&rec) {
                    tracing::warn!(service_id = id, pid, error = %e, "failed to record supervised process");
                }
            }
            Ok(None) => { /* already dead; nothing to record */ }
            Err(e) => {
                tracing::warn!(service_id = id, pid, error = %e, "could not read start-time to record")
            }
        }
    }

    pub(crate) fn stderr_tail_snapshot(inner: &Arc<Inner>, id: &str) -> Vec<String> {
        let entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .get(id)
            .map(|e| e.stderr_tail.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn push_log(inner: &Arc<Inner>, id: &str, source: StreamSource, line: String) {
        let level = classify_level(source, &line);
        let ts_ms = now_ms();
        {
            let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = entries.get_mut(id) {
                e.logs.push(LogLine {
                    ts_ms,
                    level,
                    line: line.clone(),
                });
                if source == StreamSource::Stderr {
                    if e.stderr_tail.len() == STDERR_TAIL {
                        e.stderr_tail.pop_front();
                    }
                    e.stderr_tail.push_back(line.clone());
                }
            }
        }
        let _ = inner.tx.send(SupervisorEvent::Log {
            id: id.to_string(),
            ts_ms,
            level,
            line,
        });
    }

    pub(crate) fn push_supervisor_log(inner: &Arc<Inner>, id: &str, line: String) {
        Self::push_log(
            inner,
            id,
            StreamSource::Stdout,
            format!("supervisor: {line}"),
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::platform::{SpawnSpec, default_driver};

    /// A [`ServiceSpec`] that is never actually spawned in these tests —
    /// `register()` only touches the entry map, so `spawn` just needs to be
    /// well-formed, not runnable.
    fn spec(id: &str) -> ServiceSpec {
        ServiceSpec {
            id: id.to_string(),
            display_name: format!("{id} display"),
            endpoint: Some("127.0.0.1:0".to_string()),
            spawn: SpawnSpec {
                program: std::path::PathBuf::from("/does/not/exist"),
                args: vec![],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        }
    }

    /// Task 1 (spec D2): the observer-visibility gap this variant closes —
    /// registering after a subscriber already exists (the shape a
    /// post-launch PHP/MySQL install takes) must reach it with the full,
    /// freshly stored status, exactly once.
    #[tokio::test]
    async fn register_after_subscribe_emits_registered_with_full_status() {
        let sup = Supervisor::new(default_driver());
        let mut rx = sup.subscribe();

        sup.register(spec("svc-a"));

        match rx.try_recv().expect("expected a queued Registered event") {
            SupervisorEvent::Registered { status } => {
                assert_eq!(status.id, "svc-a");
                assert_eq!(status.display_name, "svc-a display");
                assert_eq!(status.endpoint.as_deref(), Some("127.0.0.1:0"));
                assert_eq!(status.pid, None);
                assert_eq!(status.state, ServiceState::Stopped);
            }
            other => panic!("expected Registered, got {other:?}"),
        }
        // Exactly once — not a double-emit.
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    /// Startup-safety regression net: `lib.rs`'s real boot sequence calls
    /// `register()` for every initial service BEFORE `Supervisor::subscribe`
    /// is ever called. A `Registered` emitted with zero receivers must not
    /// somehow surface to whoever subscribes afterward, and the first event
    /// such a subscriber later sees (e.g. from a state probe) must be
    /// unchanged in shape and order — exactly what it would have been before
    /// this variant existed.
    #[tokio::test]
    async fn boot_time_registrations_leave_no_trace_for_a_later_subscriber() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("boot-svc"));
        sup.register(spec("boot-svc-2"));

        let mut rx = sup.subscribe();
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "a late subscriber must not see registrations that predate it"
        );

        // The next thing that happens to either service (a probe result, not
        // a fresh registration) must still be the ONLY thing this subscriber
        // observes — no ordering change, no phantom Registered.
        Inner::set_state(
            &sup.inner,
            "boot-svc",
            ServiceState::Running,
            Some("probe".into()),
        );
        match rx
            .try_recv()
            .expect("expected the StateChanged from set_state")
        {
            SupervisorEvent::StateChanged { id, .. } => assert_eq!(id, "boot-svc"),
            other => panic!("expected StateChanged, got {other:?}"),
        }
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    /// Pre-existing no-op guard, unaffected: re-registering a `Starting`/
    /// `Running` id must still emit nothing at all — a live service is never
    /// mistaken for a newly registered one.
    #[tokio::test]
    async fn registering_a_live_service_stays_a_no_op_and_emits_nothing() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-b"));
        Inner::set_state(&sup.inner, "svc-b", ServiceState::Running, None);

        let mut rx = sup.subscribe();
        sup.register(spec("svc-b"));

        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }
}
