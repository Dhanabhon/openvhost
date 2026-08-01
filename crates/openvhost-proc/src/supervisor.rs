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

/// May a service in `state` be forgotten by [`Supervisor::unregister`], and
/// what is that state called in the refusal?
///
/// Exhaustive over [`ServiceState`] with **no wildcard arm**, deliberately:
/// a new variant must fail to compile HERE rather than silently landing on
/// whichever side a `_` arm happened to pick. Defaulting a new state to
/// "removable" would let the supervisor forget a child it is still
/// supervising — the one thing D4 says this guard exists to prevent — and
/// defaulting it to "refused" would quietly make some future state
/// un-uninstallable. Neither is a decision a wildcard should be making.
///
/// The decision and the human name of the state are produced by the SAME
/// match, one arm each, so a state cannot be classified in one place and
/// named in another that drifted from it.
fn check_terminal(id: &str, state: &ServiceState) -> Result<(), ProcError> {
    match state {
        // Terminal: no child of ours is alive under this id, so the orphan
        // registry has nothing to lose by the entry going away.
        ServiceState::Stopped | ServiceState::Failed { .. } => Ok(()),
        ServiceState::Starting => Err(ProcError::NotTerminal {
            id: id.to_string(),
            state: "starting",
        }),
        ServiceState::Running => Err(ProcError::NotTerminal {
            id: id.to_string(),
            state: "running",
        }),
    }
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

    /// Forget the service registered under `id` — the mirror of
    /// [`Supervisor::register`] (package-uninstall design
    /// `2026-07-31-p1-pkg-uninstall-design.md` D4).
    ///
    /// Refuses with [`ProcError::NotTerminal`], naming the state, unless the
    /// service is `Stopped` or `Failed`; refuses with [`ProcError::NotFound`]
    /// when nothing is registered under `id`. **An unknown id is an error, not
    /// a silent success**: the caller asked the supervisor to forget something
    /// it does not have, and answering `Ok` would hide a typo or a
    /// double-uninstall behind a no-op.
    ///
    /// The lookup, the terminal-state check and the removal all happen under
    /// ONE acquisition of the same `entries` mutex `register`/`start`/`stop`
    /// use, which is what makes this safe against a concurrent start: either
    /// `start` gets the lock first and writes `Starting` (so this call sees it
    /// and refuses), or this call gets it first (so `start` then finds no
    /// entry and returns `NotFound`). There is no interleaving in which a
    /// service is both spawned and forgotten.
    ///
    /// Refusing on a live service is what keeps the crash-orphan registry
    /// honest: it is keyed by the services being supervised, and the next
    /// launch's reaper is identity-matched against exactly those records, so
    /// forgetting a live child would leak it permanently.
    ///
    /// Deliberately does NOT touch the orphan registry itself.
    /// `service_task::finish` already removes a service's record on the way
    /// into a terminal state, so by the time this call is permitted there is
    /// normally nothing left to remove; and a record that somehow outlived
    /// that describes a process this supervisor could not confirm dead.
    /// Deleting it here would convert a reapable orphan into a permanent
    /// leak, which is the opposite of what unregistering should cost.
    ///
    /// On success, broadcasts [`SupervisorEvent::Unregistered`] exactly once,
    /// after the lock is released — the same shape as `register`'s own emit.
    /// A refused call broadcasts nothing at all.
    pub fn unregister(&self, id: &str) -> Result<(), ProcError> {
        {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let entry = entries
                .get(id)
                .ok_or_else(|| ProcError::NotFound(id.to_string()))?;
            // Both fallible steps happen BEFORE the mutation: a refusal must
            // leave the registry exactly as it found it.
            check_terminal(id, &entry.state)?;
            entries.remove(id);
        }
        let _ = self
            .inner
            .tx
            .send(SupervisorEvent::Unregistered { id: id.to_string() });
        Ok(())
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

    // ---------------------------------------------------------------------
    // `unregister` (package-uninstall design D4).
    // ---------------------------------------------------------------------

    /// A `Failed` state with the shape a real failure produces, so the tests
    /// below exercise the variant that carries data rather than only the
    /// fieldless ones.
    fn failed() -> ServiceState {
        ServiceState::Failed {
            exit: Some(1),
            stderr_tail: vec!["boom".to_string()],
        }
    }

    fn ids(sup: &Supervisor) -> Vec<String> {
        sup.snapshot().into_iter().map(|s| s.id).collect()
    }

    /// The load-bearing refusal (D4): a service the supervisor is still
    /// supervising must never be forgotten, and the error has to NAME the
    /// state so the caller can tell the user what to do about it.
    ///
    /// VACUITY (neuter-and-watch-it-fail): `check_terminal`'s `Running` arm
    /// was temporarily changed to `Ok(())` — this test failed on the
    /// `expect_err`. Restoring it made it pass again.
    #[tokio::test]
    async fn unregister_refuses_a_running_service_and_names_the_state() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-run"));
        Inner::set_state(&sup.inner, "svc-run", ServiceState::Running, None);

        let err = sup
            .unregister("svc-run")
            .expect_err("a running service must not be forgotten");

        assert!(matches!(err, ProcError::NotTerminal { .. }));
        let msg = err.to_string();
        assert!(msg.contains("running"), "state not named in: {msg}");
        assert!(msg.contains("svc-run"), "service not named in: {msg}");
        // The refusal is not merely an `Err` — the entry is still there.
        assert_eq!(ids(&sup), vec!["svc-run".to_string()]);
    }

    /// `Starting` is the state a service spends its whole spawn window in,
    /// and the window where a child exists but has not been classified yet —
    /// exactly when forgetting it would leak an orphan.
    ///
    /// VACUITY (neuter-and-watch-it-fail): `check_terminal`'s `Starting` arm
    /// was temporarily changed to `Ok(())` — this test failed on the
    /// `expect_err`. Restoring it made it pass again.
    #[tokio::test]
    async fn unregister_refuses_a_starting_service_and_names_the_state() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-starting"));
        Inner::set_state(&sup.inner, "svc-starting", ServiceState::Starting, None);

        let err = sup
            .unregister("svc-starting")
            .expect_err("a starting service must not be forgotten");

        assert!(matches!(err, ProcError::NotTerminal { .. }));
        let msg = err.to_string();
        assert!(msg.contains("starting"), "state not named in: {msg}");
        assert!(msg.contains("svc-starting"), "service not named in: {msg}");
        assert_eq!(ids(&sup), vec!["svc-starting".to_string()]);
    }

    /// A refusal must be inert on the event stream too: an observer that
    /// dropped the row on a refused unregister would show a service that is
    /// still very much registered as gone.
    ///
    /// VACUITY: the positive control is
    /// `unregister_emits_unregistered_exactly_once` below — it proves this
    /// same receiver DOES see an event when the call succeeds, so "no event"
    /// here cannot pass because events never arrive at all. Separately
    /// neuter-proven: `check_terminal`'s `Running` arm was temporarily
    /// changed to `Ok(())` and this test failed on
    /// `assert!(sup.unregister("svc-live").is_err())` — the refusal, and
    /// therefore the silence, is genuinely this guard's doing.
    #[tokio::test]
    async fn a_refused_unregister_emits_nothing() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-live"));
        Inner::set_state(&sup.inner, "svc-live", ServiceState::Running, None);

        let mut rx = sup.subscribe();
        assert!(sup.unregister("svc-live").is_err());

        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    /// The success path for both terminal states, asserted on `snapshot()` —
    /// the thing every consumer actually reads — not on the `Result`.
    ///
    /// VACUITY (neuter-and-watch-it-fail): `entries.remove(id)` was
    /// temporarily removed from `unregister` (leaving it to return `Ok`) —
    /// this test failed on the FIRST `assert_eq!`, whose left side was
    /// `["svc-failed", "svc-keep", "svc-stopped"]` against the asserted
    /// `["svc-failed", "svc-keep"]`; the second never ran, since a failed
    /// assertion ends the test. Restoring the removal made it pass again.
    #[tokio::test]
    async fn unregister_forgets_a_stopped_or_failed_service() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-stopped"));
        sup.register(spec("svc-failed"));
        sup.register(spec("svc-keep"));
        Inner::set_state(&sup.inner, "svc-failed", failed(), None);

        sup.unregister("svc-stopped")
            .expect("a stopped service is forgettable");
        assert_eq!(
            ids(&sup),
            vec!["svc-failed".to_string(), "svc-keep".to_string()]
        );

        sup.unregister("svc-failed")
            .expect("a failed service is forgettable");
        assert_eq!(ids(&sup), vec!["svc-keep".to_string()]);
    }

    /// Exactly once — not zero (the row would linger in every observer that
    /// only reacts to events) and not twice (a consumer that also drops a
    /// LATER re-registration of the same id would lose a live row).
    ///
    /// VACUITY (neuter-and-watch-it-fail): the `tx.send` in `unregister` was
    /// temporarily duplicated — this test failed on the trailing
    /// `Empty` assertion, receiving a second `Unregistered`. Deleting the
    /// send instead failed the first `try_recv`'s `expect`.
    #[tokio::test]
    async fn unregister_emits_unregistered_exactly_once() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-once"));
        let mut rx = sup.subscribe();

        sup.unregister("svc-once").expect("stopped is forgettable");

        match rx.try_recv().expect("expected a queued Unregistered event") {
            SupervisorEvent::Unregistered { id } => assert_eq!(id, "svc-once"),
            other => panic!("expected Unregistered, got {other:?}"),
        }
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    /// An unknown id is a TYPED error, not a silent success — including the
    /// second of two calls, which is the shape a double-click on an Uninstall
    /// button takes. One event total, not two.
    ///
    /// VACUITY (neuter-and-watch-it-fail): `unregister`'s lookup was
    /// temporarily changed to `let Some(entry) = entries.get(id) else {
    /// return Ok(()) }` (a missing entry becoming a silent success) — this
    /// test failed on the first `expect_err`, and it was the ONLY test in the
    /// module that failed, so nothing else here covers that claim. Restoring
    /// the `ok_or_else` made it pass again.
    #[tokio::test]
    async fn unregistering_an_unknown_id_is_a_typed_error() {
        let sup = Supervisor::new(default_driver());

        let err = sup
            .unregister("never-registered")
            .expect_err("an unknown id must not answer Ok");
        assert!(matches!(err, ProcError::NotFound(ref id) if id == "never-registered"));

        // ... and the same is true for the second of two calls. Subscribed
        // AFTER the register so the only event this receiver can hold is the
        // one the unregisters produce (a `Registered` would otherwise sit at
        // the head of the queue and the "exactly one" assertion below would
        // be reading the wrong event).
        sup.register(spec("svc-twice"));
        let mut rx = sup.subscribe();
        sup.unregister("svc-twice").expect("first call succeeds");
        let again = sup
            .unregister("svc-twice")
            .expect_err("the second call has nothing left to forget");
        assert!(matches!(again, ProcError::NotFound(ref id) if id == "svc-twice"));

        // Exactly one event across both calls: the failed second call must not
        // tell observers to drop the row a second time.
        assert!(matches!(
            rx.try_recv(),
            Ok(SupervisorEvent::Unregistered { .. })
        ));
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    /// Reentrancy, ordering A (a start wins the race): `start` writes
    /// `Starting` synchronously under the entries lock BEFORE it returns, so
    /// an `unregister` that arrives after it can only ever see a live service
    /// and refuse. Deterministic — the spawned service task cannot run until
    /// this test awaits, and it never does.
    ///
    /// VACUITY: the assertion is the same refusal
    /// `unregister_refuses_a_starting_service_and_names_the_state` neuter-
    /// proves; what this adds is that `start` really does publish `Starting`
    /// before returning, which the trailing `snapshot()` assertion pins.
    #[tokio::test]
    async fn unregister_after_a_start_sees_starting_and_refuses() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-race"));

        sup.start("svc-race").expect("start dispatches");
        let err = sup
            .unregister("svc-race")
            .expect_err("a service whose start was just dispatched is live");

        assert!(matches!(err, ProcError::NotTerminal { state, .. } if state == "starting"));
        assert_eq!(ids(&sup), vec!["svc-race".to_string()]);
    }

    /// Reentrancy, ordering B (the unregister wins): a later `start`/`stop`
    /// must report the id as unknown rather than silently succeeding against
    /// an entry that no longer exists.
    ///
    /// VACUITY (neuter-and-watch-it-fail): `entries.remove(id)` was
    /// temporarily removed from `unregister` — this test failed on the first
    /// `expect_err` (`start` still found the entry and returned `Ok`); the
    /// `stop` half never ran. The same neuter also failed
    /// `unregister_forgets_a_stopped_or_failed_service`,
    /// `a_subscriber_that_misses_the_event_still_sees_it_gone_in_snapshot`
    /// and `unregistering_an_unknown_id_is_a_typed_error`.
    #[tokio::test]
    async fn starting_or_stopping_a_forgotten_service_is_unknown() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-gone"));
        sup.unregister("svc-gone").expect("stopped is forgettable");

        assert!(matches!(
            sup.start("svc-gone").expect_err("start must not resurrect"),
            ProcError::NotFound(_)
        ));
        assert!(matches!(
            sup.stop("svc-gone").expect_err("stop must not resurrect"),
            ProcError::NotFound(_)
        ));
    }

    /// A subscriber that MISSES the event (it subscribed afterwards, or was
    /// lagged out) must still be able to see the truth from `snapshot()` —
    /// the tray relies on exactly this, recomputing from a snapshot rather
    /// than applying event deltas.
    #[tokio::test]
    async fn a_subscriber_that_misses_the_event_still_sees_it_gone_in_snapshot() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-missed"));
        sup.unregister("svc-missed")
            .expect("stopped is forgettable");

        let mut rx = sup.subscribe();
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(ids(&sup).is_empty());
    }

    /// Re-registering an id that was forgotten is an ordinary registration —
    /// the entry is fresh (no stale pid/log ring inherited), and observers
    /// get the `Registered` that says so. This is the reinstall path.
    #[tokio::test]
    async fn an_id_can_be_registered_again_after_being_forgotten() {
        let sup = Supervisor::new(default_driver());
        sup.register(spec("svc-again"));
        Inner::set_state(&sup.inner, "svc-again", failed(), None);
        sup.unregister("svc-again").expect("failed is forgettable");

        let mut rx = sup.subscribe();
        sup.register(spec("svc-again"));

        match rx.try_recv().expect("expected a queued Registered event") {
            SupervisorEvent::Registered { status } => {
                assert_eq!(status.id, "svc-again");
                assert_eq!(status.state, ServiceState::Stopped);
                assert_eq!(status.pid, None);
            }
            other => panic!("expected Registered, got {other:?}"),
        }
        assert_eq!(ids(&sup), vec!["svc-again".to_string()]);
    }
}
