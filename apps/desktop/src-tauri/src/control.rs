// SPDX-License-Identifier: GPL-3.0-or-later
//! Desktop-side *policy* for the local control channel — the half of the
//! `openvhost` CLI slice that knows about this app (P1 CLI design,
//! `docs/superpowers/specs/2026-07-31-p1-cli-design.md`, D4/D6/D7).
//!
//! `openvhost_proc::control` owns the transport: it binds the socket, checks
//! the peer's uid, bounds the ingress, parses a [`Request`] and writes back a
//! [`Response`]. It deliberately knows nothing about the bulk lock,
//! `quit::stop_all`, or Tauri managed state. Everything it hands over lands
//! here, in [`DesktopHandler`].
//!
//! # The three rules this module exists to enforce
//!
//! 1. **The server waits, the client does not** (D4). `Supervisor::start` and
//!    `Supervisor::stop` are synchronous and only *kick off* a transition —
//!    `start` returns the instant the service task is spawned, `stop` the
//!    instant the request is flagged. A handler that answered there would
//!    report "started" for a service that is about to fail its readiness
//!    probe. So every per-service verb subscribes to [`SupervisorEvent`]
//!    **before** it does anything else, then waits for the terminal state
//!    with a deadline ([`TRANSITION_TIMEOUT`]).
//! 2. **`stop-all` is rejected, never queued** (D7). It takes the same two
//!    locks the tray takes, through the same
//!    [`try_acquire_bulk`](crate::tray::service_control::try_acquire_bulk)
//!    helper, and answers [`ErrorCode::Busy`] the moment either is held.
//!    On admission it calls [`crate::quit::stop_all`] — the literal function
//!    Quit and the tray's Stop-all use, not a copy.
//!
//!    That admission is a **consistency guard, not an authorization
//!    boundary**: N × `openvhost stop <id>` reaches the same end state
//!    unchecked, deliberately, matching what the tray's per-row Stop already
//!    allows. Flagged so a later reader does not mistake the `Busy` rejection
//!    for a security control — the authorization decision on this channel is
//!    the peer-uid check in `openvhost-proc`, and nothing else.
//! 3. **Per-service verbs take no lock at all** (D7). `Supervisor::start` and
//!    `stop` are already idempotent inside the entries mutex: a duplicate
//!    start for a `Starting`/`Running` id returns early, a duplicate stop for
//!    a `Stopped`/`Failed` one likewise. Serializing them behind the bulk
//!    lock would make `openvhost start nginx` fail while an unrelated
//!    Start-all was in flight, for no safety gained.
//! 4. **Nothing mutating is admitted once a quit has begun** (A3 audit fix).
//!    The socket keeps accepting until the process dies, and a connection
//!    accepted just before the teardown is still served — so an
//!    `openvhost start nginx` landing after [`crate::quit::stop_all`] returned
//!    would spawn nginx and then lose its supervisor, leaving something
//!    listening after the user believes the stack is down. [`mutates`] decides
//!    which verbs that covers; reads stay open, because answering "what is
//!    running?" during a quit is harmless and honest.
//!
//! # Containment
//!
//! [`Request`] cannot express a path, an argv, a pid or an environment — that
//! is the wire type's invariant (proven in `openvhost-proc`). This module is
//! where that invariant meets a *real* `Supervisor`: an id nobody registered
//! reaches [`Supervisor::start`], which answers `NotFound` before any process
//! driver is touched, and this module turns that into
//! [`ErrorCode::UnknownService`]. `an_unregistered_id_is_refused_for_every_verb_and_nothing_is_spawned`
//! proves it against a recording [`openvhost_proc::ProcessDriver`] rather
//! than against a fake handler.

use std::sync::Arc;
use std::time::{Duration, Instant};

use openvhost_proc::control::{
    ControlHandler, Disposition, ErrorCode, Request, Response, ServiceId, async_trait,
};
use openvhost_proc::{ProcError, ServiceState, ServiceStatus, Supervisor, SupervisorEvent};
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::broadcast;

/// How long a per-service verb waits for the terminal state before answering
/// [`ErrorCode::Timeout`].
///
/// Sized against the two real budgets already in the tree, not picked round:
///
/// - **A start that never becomes ready.** MySQL's readiness probe runs for
///   `stack::MYSQL_READY_DEADLINE` (15s), and when it elapses the service task
///   tears the child down with that spec's own `stack::MYSQL_GRACE` (15s)
///   before `Failed` is ever broadcast — 30s of legitimate waiting on the
///   slowest registered service's *worst* path.
/// - **A stop.** `quit::STOP_ALL_TIMEOUT` (18s) is what this app already
///   considers long enough for every registered service to stop, MySQL's 15s
///   grace included. A per-service `stop` deadline shorter than that would
///   time out on a service the bulk primitive would still have been waiting
///   for.
///
/// 45s clears the larger of the two (30s) with 15s of slack for a loaded
/// machine. `transition_timeout_outlives_mysqls_readiness_deadline_plus_its_stop_grace`
/// and `transition_timeout_outlives_the_bulk_stop_budget` pin both against the
/// REAL constants, so a future spec with a longer probe or grace fails a test
/// here rather than silently making `openvhost start` lie.
///
/// A value DERIVED per call from whichever spec is being started would be
/// better still, and is deliberately not done: `Supervisor` exposes no
/// accessor for a spec's `readiness`/`grace` (the same gap `STOP_ALL_TIMEOUT`
/// documents), and inventing one to serve this single call site is a change to
/// `openvhost-proc`'s public API for no behavioural gain today. If a THIRD,
/// longer budget ever lands, apply the identical reasoning again rather than
/// letting this drift silently.
pub const TRANSITION_TIMEOUT: Duration = Duration::from_secs(45);

/// The state a per-service verb is driving towards. `start` and `stop` are
/// the same algorithm — subscribe, admit, kick, wait — differing only in
/// which state counts as arrival, so they share one implementation and this
/// names the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `start`.
    Running,
    /// `stop`.
    Stopped,
}

impl Target {
    /// The verb, for human messages.
    fn verb(self) -> &'static str {
        match self {
            Target::Running => "start",
            Target::Stopped => "stop",
        }
    }

    /// The state being waited for, for human messages.
    fn state_name(self) -> &'static str {
        match self {
            Target::Running => "running",
            Target::Stopped => "stopped",
        }
    }
}

/// What one observed [`ServiceState`] means for a wait in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Not a resolution — keep waiting.
    Pending,
    /// The service reached the state the verb asked for.
    Reached,
    /// The service reached `Failed`.
    Failed,
    /// The service settled in the OTHER terminal state: a `start` that ended
    /// `Stopped` without ever reaching `Running`.
    ///
    /// The reachable cause is a child that exits **cleanly** before its
    /// readiness bound — `classify_exit` calls a code-0 death `Stopped`, not
    /// `Failed`, so a program that self-daemonizes or quits on a config it
    /// dislikes lands here. Reporting that as a successful start is precisely
    /// the boolean-collapse this codebase has hit before: the caller asked
    /// for `Running` and the service is not running.
    ///
    /// A concurrent stop is NOT this case, as
    /// `a_start_stopped_during_its_readiness_window_still_observes_running`
    /// records: `Supervisor::stop` during an `AliveAfter` window only flags
    /// the request, and the service task does not read its control channel
    /// until after readiness resolves — so the start genuinely observes
    /// `Running` first, and the stop's own `Stopped` follows as a separate
    /// transition.
    Diverted,
}

/// How one observed state resolves a wait, for both targets.
///
/// Matched over the full `(Target, ServiceState)` product with **no wildcard
/// arm on either side**: adding a `ServiceState` variant must fail to compile
/// here rather than silently fall into "keep waiting", which is how a new
/// state would turn every verb into a 45s hang.
fn step(target: Target, state: &ServiceState) -> Step {
    match (target, state) {
        (Target::Running, ServiceState::Running) => Step::Reached,
        (Target::Running, ServiceState::Failed { .. }) => Step::Failed,
        (Target::Running, ServiceState::Stopped) => Step::Diverted,
        (Target::Running, ServiceState::Starting) => Step::Pending,
        (Target::Stopped, ServiceState::Stopped) => Step::Reached,
        (Target::Stopped, ServiceState::Failed { .. }) => Step::Failed,
        // A stop is only *requested* synchronously; the service stays
        // `Running` (or `Starting`) for its whole grace period, so neither is
        // a resolution.
        (Target::Stopped, ServiceState::Running) => Step::Pending,
        (Target::Stopped, ServiceState::Starting) => Step::Pending,
    }
}

/// Whether the verb has nothing to do, checked BEFORE the supervisor is
/// touched — the [`Disposition::Unchanged`] decision.
///
/// Mirrors the supervisor's own early-return rule rather than inventing a
/// second one: `Supervisor::stop` returns early for `Stopped` **and**
/// `Failed`, because both mean "no live child", and `Supervisor::start`
/// returns early for `Running`.
///
/// `Starting` is deliberately NOT settled for a `start`: the service is not in
/// the target state yet, and the honest answer is to wait for the in-flight
/// attempt's real outcome instead of reporting a success that has not
/// happened. (The disposition then describes the observable transition, not
/// causality — this handler cannot tell its own start attempt from a tray
/// click's, because the supervisor has no per-attempt identity, and inventing
/// one is out of this slice's scope.)
///
/// Exhaustive over both enums, same reasoning as [`step`].
fn settled(target: Target, state: &ServiceState) -> bool {
    match (target, state) {
        (Target::Running, ServiceState::Running) => true,
        (Target::Running, ServiceState::Starting) => false,
        (Target::Running, ServiceState::Stopped) => false,
        (Target::Running, ServiceState::Failed { .. }) => false,
        (Target::Stopped, ServiceState::Stopped) => true,
        // Nothing is running, so a stop is a no-op success — the row this
        // returns still carries `failed` with its stderr tail, so nothing is
        // hidden by answering `Unchanged` rather than `OperationFailed`.
        //
        // NOTE for the CLI's exit-code table (spec D3: "0 = success,
        // including an explicit unchanged result"): this is the one
        // `Transition` response that carries a `failed` row on a SUCCESSFUL
        // verb. Mapping "row is `failed`" to a non-zero exit without looking
        // at which verb was sent would report `openvhost stop nginx` as a
        // failure for a service that was already down — the thing the user
        // asked for.
        (Target::Stopped, ServiceState::Failed { .. }) => true,
        (Target::Stopped, ServiceState::Running) => false,
        (Target::Stopped, ServiceState::Starting) => false,
    }
}

/// The outcome of one verb (or one half of `restart`), before it becomes a
/// [`Response`].
///
/// A typed intermediate rather than a `Response` so `restart` can sequence two
/// halves and adjust the disposition without pattern-matching a `Response`
/// apart and putting it back together.
#[derive(Debug)]
enum Outcome {
    /// The service is in the requested state.
    Reached {
        /// The row to report.
        service: ServiceStatus,
        /// Whether anything actually moved.
        disposition: Disposition,
    },
    /// The verb did not achieve what was asked. Carries the wire answer.
    Refused {
        /// Machine-readable class.
        code: ErrorCode,
        /// Human detail — carries the real stderr tail when there is one.
        message: String,
    },
}

impl Outcome {
    fn into_response(self) -> Response {
        match self {
            Outcome::Reached {
                service,
                disposition,
            } => Response::Transition {
                service,
                disposition,
            },
            Outcome::Refused { code, message } => Response::Error { code, message },
        }
    }
}

/// The message an [`ErrorCode::OperationFailed`] carries when a verb ends in
/// `Failed`.
///
/// The stderr tail is joined with newlines **verbatim** — never
/// `Debug`-formatted, never truncated. Routing `ServiceState::Failed` over the
/// wire is pointless if the CLI cannot print what the process actually printed
/// (D5), and a `{stderr_tail:?}` here would wrap every line in escaped quotes
/// and commas. Same rule, same reason, as the tray's own failure dialog.
fn failure_message(
    target: Target,
    display_name: &str,
    exit: Option<i32>,
    stderr_tail: &[String],
) -> String {
    let code = match exit {
        Some(c) => format!("exit code {c}"),
        None => "no exit code (killed by a signal, or never launched)".to_string(),
    };
    let tail = if stderr_tail.is_empty() {
        "(no stderr captured)".to_string()
    } else {
        stderr_tail.join("\n")
    };
    format!(
        "{display_name} failed to {} ({code}):\n{tail}",
        target.verb()
    )
}

/// Implements `openvhost-proc`'s policy seam over this app's managed state.
///
/// Holds the `Arc<Supervisor>` directly (it is constructed in `lib.rs`'s
/// bootstrap alongside the supervisor itself, before `app.manage` takes
/// ownership) and an [`AppHandle`] for the two locks `stop-all` needs. The
/// locks are read through `try_state` at execute time rather than cloned in:
/// they are single, app-wide mutexes whose whole job is mutual exclusion with
/// the tray and with `apply_config`, so a second handle to a *copy* would
/// exclude nothing.
pub struct DesktopHandler<R: Runtime> {
    app: AppHandle<R>,
    sup: Arc<Supervisor>,
    transition_timeout: Duration,
}

impl<R: Runtime> DesktopHandler<R> {
    /// A handler over this app's supervisor, waiting up to
    /// [`TRANSITION_TIMEOUT`] for a per-service verb.
    pub fn new(app: AppHandle<R>, sup: Arc<Supervisor>) -> Self {
        Self {
            app,
            sup,
            transition_timeout: TRANSITION_TIMEOUT,
        }
    }

    /// Shorten the transition deadline. Test-only: proving the timeout path
    /// against the real 45s would mean a 45s test.
    #[cfg(test)]
    fn with_transition_timeout(mut self, timeout: Duration) -> Self {
        self.transition_timeout = timeout;
        self
    }

    /// The current row for `id`, or `None` when nothing is registered under
    /// it.
    fn status(&self, id: &str) -> Option<ServiceStatus> {
        self.sup.snapshot().into_iter().find(|s| s.id == id)
    }

    /// `list` / `status` — the whole table, or one row.
    ///
    /// `Supervisor::snapshot` already sorts by id, which is the order D4
    /// promises; `list_returns_every_registered_service_id_sorted` pins that
    /// contract from this side so a change there fails a test here.
    fn services(&self, id: Option<&ServiceId>) -> Response {
        let all = self.sup.snapshot();
        match id {
            None => Response::Services { services: all },
            Some(id) => match all.into_iter().find(|s| s.id == id.as_str()) {
                Some(row) => Response::Services {
                    services: vec![row],
                },
                None => Response::error(ErrorCode::UnknownService, self.unknown_message(id)),
            },
        }
    }

    /// "no service 'x' is registered", plus what *is* — the ids are this
    /// app's own and the peer is already uid-equal, so naming them costs
    /// nothing and turns a typo into a one-line fix.
    fn unknown_message(&self, id: &ServiceId) -> String {
        let known: Vec<String> = self.sup.snapshot().into_iter().map(|s| s.id).collect();
        if known.is_empty() {
            format!("no service '{id}' is registered (this instance has no services at all)")
        } else {
            format!(
                "no service '{id}' is registered; known services: {}",
                known.join(", ")
            )
        }
    }

    /// Kick off the transition. Synchronous — it returns as soon as the
    /// service task is spawned (`start`) or the stop is flagged (`stop`).
    fn kick(&self, target: Target, id: &str) -> Result<(), ProcError> {
        match target {
            Target::Running => self.sup.start(id),
            Target::Stopped => self.sup.stop(id),
        }
    }

    /// One per-service verb: subscribe, admit, kick, wait.
    ///
    /// **Subscription happens before the admission snapshot**, not merely
    /// before the kick. Between reading the snapshot and subscribing, another
    /// actor could drive the very transition being asked for; the kick would
    /// then be the supervisor's documented no-op, no further event would ever
    /// arrive, and the wait would burn the whole deadline before answering
    /// `Timeout` for a service that was already where the caller wanted it.
    /// Subscribing first makes that window empty.
    async fn transition(&self, target: Target, id: &ServiceId, wait: bool) -> Outcome {
        let rx = self.sup.subscribe();
        let Some(before) = self.status(id.as_str()) else {
            return Outcome::Refused {
                code: ErrorCode::UnknownService,
                message: self.unknown_message(id),
            };
        };
        if settled(target, &before.state) {
            return Outcome::Reached {
                service: before,
                disposition: Disposition::Unchanged,
            };
        }
        if let Err(e) = self.kick(target, id.as_str()) {
            // Exhaustive over `ProcError`: `NotFound` is the containment
            // answer (an id `stack.rs` never registered, or one deregistered
            // between the snapshot above and this call), anything else is a
            // genuine failure to dispatch.
            return match e {
                ProcError::NotFound(_) => Outcome::Refused {
                    code: ErrorCode::UnknownService,
                    message: self.unknown_message(id),
                },
                ProcError::Io(io) => Outcome::Refused {
                    code: ErrorCode::OperationFailed,
                    message: format!("could not {} '{id}': {io}", target.verb()),
                },
                // Only `Supervisor::unregister` produces this, and `kick`
                // calls `start`/`stop` — so this is unreachable today. It is
                // still handled honestly rather than collapsed into a
                // wildcard: the whole point of the exhaustive match here is
                // that a future `ProcError` variant has to be considered at
                // this seam, and `{e}` carries the supervisor's own wording
                // (which already names the service and its state) instead of
                // inventing a second phrasing that could drift from it.
                e @ ProcError::NotTerminal { .. } => Outcome::Refused {
                    code: ErrorCode::OperationFailed,
                    message: format!("could not {} '{id}': {e}", target.verb()),
                },
            };
        }
        if !wait {
            // `--no-wait`: answer with the row as it stands now (typically
            // `Starting`), which is honest about what has actually happened
            // rather than predicting what will.
            let service = self.status(id.as_str()).unwrap_or(before);
            return Outcome::Reached {
                service,
                disposition: Disposition::Changed,
            };
        }
        self.await_settled(rx, target, id.as_str(), self.transition_timeout)
            .await
    }

    /// Wait for `id` to settle, on the receiver subscribed before the kick.
    async fn await_settled(
        &self,
        mut rx: broadcast::Receiver<SupervisorEvent>,
        target: Target,
        id: &str,
        timeout: Duration,
    ) -> Outcome {
        // `Instant`, not wall-clock arithmetic — a clock adjustment mid-wait
        // must not turn a 45s deadline into an instant give-up or a hang.
        // Same reasoning as `quit::stop_all_with`.
        let started = Instant::now();
        loop {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return self.timed_out(target, id, timeout);
            }
            let observed = match tokio::time::timeout(remaining, rx.recv()).await {
                Err(_elapsed) => return self.timed_out(target, id, timeout),
                Ok(Ok(SupervisorEvent::StateChanged {
                    id: changed, state, ..
                })) => (changed == id).then_some(state),
                // Never a resolution: `Supervisor::register` is a no-op for a
                // `Starting`/`Running` id, and a service with a transition in
                // flight is always one of those — so a `Registered` for this
                // id cannot describe the transition being waited on.
                Ok(Ok(SupervisorEvent::Registered { .. })) => None,
                Ok(Ok(SupervisorEvent::Log { .. })) => None,
                // The service being waited on was REMOVED (an in-app
                // uninstall — package-uninstall design D4). The wait can
                // never be satisfied now: no further event will ever carry
                // this id, and the `Lagged` arm below would re-read a row
                // that no longer exists (`None`, i.e. "keep waiting") and
                // burn the whole 45s deadline before answering `Timeout`.
                // `UnknownService` is the honest code — by the time this is
                // read, the id genuinely names nothing.
                //
                // Reachable only by racing a GUI uninstall against a CLI
                // verb: `unregister` refuses a non-terminal service, so the
                // transition either already resolved (its `StateChanged` is
                // AHEAD of this event in the same broadcast stream and
                // returned above) or this receiver was lagged past it.
                Ok(Ok(SupervisorEvent::Unregistered { id: gone })) if gone == id => {
                    return Outcome::Refused {
                        code: ErrorCode::UnknownService,
                        message: format!(
                            "'{id}' was removed while waiting for it to become {}",
                            target.state_name()
                        ),
                    };
                }
                Ok(Ok(SupervisorEvent::Unregistered { .. })) => None,
                // A chatty service can push this receiver past the broadcast
                // channel's capacity, and the dropped events may include the
                // very one being waited for. Re-read the authoritative state
                // instead of waiting for an event that will never come again.
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => self.status(id).map(|s| s.state),
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Outcome::Refused {
                        code: ErrorCode::OperationFailed,
                        message: format!(
                            "the supervisor stopped broadcasting events while waiting for '{id}' to \
                             become {}",
                            target.state_name()
                        ),
                    };
                }
            };
            let Some(state) = observed else {
                continue;
            };
            match step(target, &state) {
                Step::Pending => continue,
                Step::Reached => {
                    return Outcome::Reached {
                        service: self.row_as_observed(id, state),
                        disposition: Disposition::Changed,
                    };
                }
                Step::Failed => {
                    let (exit, stderr_tail) = match &state {
                        ServiceState::Failed { exit, stderr_tail } => (*exit, stderr_tail.clone()),
                        // Unreachable: `step` only answers `Failed` for the
                        // `Failed` variant. Degrading beats an `expect` on a
                        // path a peer can reach.
                        ServiceState::Running | ServiceState::Starting | ServiceState::Stopped => {
                            (None, Vec::new())
                        }
                    };
                    let display = self
                        .status(id)
                        .map(|s| s.display_name)
                        .unwrap_or_else(|| id.to_string());
                    return Outcome::Refused {
                        code: ErrorCode::OperationFailed,
                        message: failure_message(target, &display, exit, &stderr_tail),
                    };
                }
                Step::Diverted => {
                    return Outcome::Refused {
                        code: ErrorCode::OperationFailed,
                        message: format!(
                            "'{id}' did not {}: it settled as {} instead — the process exited \
                             cleanly without ever becoming ready, or something else (the menu \
                             bar, an Apply, another caller) stopped it",
                            target.verb(),
                            state_label(&state),
                        ),
                    };
                }
            }
        }
    }

    /// The deadline answer, factored out so both exits from the wait loop
    /// (the `saturating_sub` guard and `tokio::time::timeout`) report
    /// identically.
    fn timed_out(&self, target: Target, id: &str, timeout: Duration) -> Outcome {
        let now = self
            .status(id)
            .map(|s| state_label(&s.state).to_string())
            .unwrap_or_else(|| "gone".to_string());
        // The trailing hint is target-aware: "still starting" is the wrong
        // advice for a `stop` that ran out of grace, and a message that
        // describes the wrong operation is worse than none.
        let hint = match target {
            Target::Running => "it may still be coming up",
            Target::Stopped => "it may still be shutting down",
        };
        Outcome::Refused {
            code: ErrorCode::Timeout,
            message: format!(
                "'{id}' did not become {} within {timeout:?} (it is {now}); {hint} — check the app",
                target.state_name(),
            ),
        }
    }

    /// The current row with the state this wait actually observed.
    ///
    /// The identity fields (`display_name`, `endpoint`, `pid`) come from a
    /// fresh snapshot so the answer is current, but the `state` is the
    /// observed one: a service that crashed in the microseconds after
    /// reaching `Running` must not produce a `Transition` response whose
    /// disposition says the start succeeded and whose row says `failed`. That
    /// later change is its own event, which the GUI and any watching CLI see
    /// next.
    fn row_as_observed(&self, id: &str, state: ServiceState) -> ServiceStatus {
        match self.status(id) {
            Some(row) => ServiceStatus { state, ..row },
            None => ServiceStatus {
                id: id.to_string(),
                display_name: id.to_string(),
                endpoint: None,
                pid: None,
                state,
            },
        }
    }

    /// `restart`, sequenced server-side (D4): the start half is not
    /// dispatched until the stop half is **observed** complete, so a client
    /// can never ask a service to start while it is still inside its stop
    /// grace.
    ///
    /// **Sequencing is all it is.** This takes no lock — rule 3 above, and
    /// deliberately — so a tray click, an Apply, or another caller can act
    /// between the two halves. What that costs is bounded and reported: a
    /// concurrent start is why the second half's disposition is forced to
    /// `Changed` below, and a concurrent stop or an Apply's own restart
    /// surfaces as this call's own `OperationFailed`/`Timeout` rather than as
    /// a false success. Making it exclusive would mean taking the bulk lock
    /// on a per-service verb, which is what rule 3 explains this codebase
    /// will not do.
    ///
    /// **The stop half is always waited on, even under `--no-wait`.**
    /// `Supervisor::stop` only requests a stop; the service stays `Running`
    /// for its whole grace period, so a `start` dispatched immediately
    /// afterwards would hit the supervisor's already-live early return, do
    /// nothing, and leave the service stopping with nothing to bring it back
    /// — a silent lie. `--no-wait` therefore skips only the readiness wait on
    /// the way back up, which is the part a caller can meaningfully opt out
    /// of.
    async fn restart(&self, id: &ServiceId, wait: bool) -> Outcome {
        match self.transition(Target::Stopped, id, true).await {
            Outcome::Refused { code, message } => Outcome::Refused { code, message },
            Outcome::Reached { .. } => match self.transition(Target::Running, id, wait).await {
                Outcome::Refused { code, message } => Outcome::Refused { code, message },
                // Always `Changed`: even when the start half found the service
                // already `Running` (something raced us back up), this call
                // stopped it first, so reporting `Unchanged` would be false.
                Outcome::Reached {
                    service,
                    disposition: _,
                } => Outcome::Reached {
                    service,
                    disposition: Disposition::Changed,
                },
            },
        }
    }

    /// `stop-all` (D4/D7): the same admission check the tray's Stop-all
    /// makes, then the same primitive Quit uses.
    ///
    /// Rejects rather than queues. A bulk stop can legitimately take
    /// `quit::STOP_ALL_TIMEOUT` (18s, covering MySQL's 15s grace); queuing
    /// behind one would leave a CLI caller blocked with no way to know why,
    /// and would flap the stack with no user intent behind it.
    async fn stop_all(&self) -> Response {
        // Both locks are managed unconditionally and BEFORE the instance-lock
        // arm that binds the socket, so absence here means the app never
        // finished its bootstrap — a genuine operational failure, not a
        // caller error. Fails closed either way: nothing is stopped.
        let Some(bulk) = self.app.try_state::<crate::tray::BulkLock>() else {
            return Response::error(
                ErrorCode::OperationFailed,
                "this instance has no bulk-operation lock; it did not finish starting up",
            );
        };
        let Some(apply) = self.app.try_state::<crate::commands::ApplyLock>() else {
            return Response::error(
                ErrorCode::OperationFailed,
                "this instance has no apply lock; it did not finish starting up",
            );
        };
        let Some(_guards) =
            crate::tray::service_control::try_acquire_bulk(&bulk.inner().0, &apply.inner().0)
        else {
            return Response::error(
                ErrorCode::Busy,
                "another bulk operation (menu-bar Start all / Stop all, or an Apply) is already \
                 in flight; try again when it finishes",
            );
        };
        let stragglers = crate::quit::stop_all(Arc::clone(&self.sup)).await;
        Response::StopAll { stragglers }
    }
}

/// Whether a request changes what is running, as opposed to reporting it.
///
/// Exhaustive over [`Request`] with no wildcard arm, on purpose: a verb added
/// to the protocol must fail to compile here and be classified deliberately,
/// rather than defaulting to "harmless" and slipping past the quit gate.
fn mutates(req: &Request) -> bool {
    match req {
        Request::List | Request::Status { .. } => false,
        Request::Start { .. }
        | Request::Stop { .. }
        | Request::Restart { .. }
        | Request::StopAll => true,
    }
}

/// The wire-ish name of a state, for human messages only.
///
/// Exhaustive over [`ServiceState`], like every other match in this module.
/// The app's ONE vocabulary for a service state, shared by `openvhost status`
/// and by the uninstall refusal that names a service which is not stopped
/// (package-uninstall design D3).
///
/// `pub(crate)` rather than duplicated: `openvhost_proc`'s own
/// `check_terminal` deliberately kept its naming table private (it produces
/// `ProcError::NotTerminal`'s `&'static str`), so widening this one is what
/// stops the desktop crate from growing a third, drifting table. Exhaustive
/// with no wildcard arm — a new [`ServiceState`] must be given a name here on
/// purpose, not inherit some other state's.
pub(crate) fn state_label(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Stopped => "stopped",
        ServiceState::Starting => "starting",
        ServiceState::Running => "running",
        ServiceState::Failed { .. } => "failed",
    }
}

#[async_trait]
impl<R: Runtime> ControlHandler for DesktopHandler<R> {
    /// Exhaustive over [`Request`] with no wildcard arm: a verb added to the
    /// protocol must fail to compile here rather than silently become a
    /// no-op.
    async fn execute(&self, req: Request) -> Response {
        // Rule 4: a quit in flight refuses everything that would change what
        // is running. Fails OPEN when `Quitting` was never managed — an app
        // that never finished its bootstrap has no quit in flight either, and
        // the same `try_state` posture as every other read in this module.
        if mutates(&req)
            && self
                .app
                .try_state::<crate::quit::Quitting>()
                .is_some_and(|q| q.has_begun())
        {
            return Response::error(
                ErrorCode::Busy,
                "OpenVHost is quitting; it is stopping its services and will not start anything \
                 new — relaunch the app and try again",
            );
        }
        match req {
            Request::List => self.services(None),
            Request::Status { id } => self.services(id.as_ref()),
            Request::Start { id, wait } => self
                .transition(Target::Running, &id, wait)
                .await
                .into_response(),
            Request::Stop { id, wait } => self
                .transition(Target::Stopped, &id, wait)
                .await
                .into_response(),
            Request::Restart { id, wait } => self.restart(&id, wait).await.into_response(),
            Request::StopAll => self.stop_all().await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use openvhost_proc::{
        DEFAULT_GRACE, ProcessDriver, ReadinessProbe, ServiceSpec, SpawnSpec, SpawnedChild,
        default_driver,
    };

    // ------------------------------------------------------------------
    // Harness
    // ------------------------------------------------------------------

    /// A [`ProcessDriver`] that records every spawn before delegating to the
    /// real one. The containment test asserts this stays EMPTY for an
    /// unregistered id — the one claim `openvhost-proc`'s own suite could
    /// only make against a fake handler.
    struct RecordingDriver {
        inner: Arc<dyn ProcessDriver>,
        spawned: Mutex<Vec<PathBuf>>,
    }

    impl RecordingDriver {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: default_driver(),
                spawned: Mutex::new(Vec::new()),
            })
        }

        fn spawned(&self) -> Vec<PathBuf> {
            self.spawned.lock().expect("spawn log poisoned").clone()
        }
    }

    impl ProcessDriver for RecordingDriver {
        fn spawn(&self, spec: &SpawnSpec) -> std::io::Result<SpawnedChild> {
            self.spawned
                .lock()
                .expect("spawn log poisoned")
                .push(spec.program.clone());
            self.inner.spawn(spec)
        }
        fn request_graceful_stop(&self, child: &SpawnedChild) -> std::io::Result<()> {
            self.inner.request_graceful_stop(child)
        }
        fn kill(&self, child: &mut SpawnedChild) -> std::io::Result<()> {
            self.inner.kill(child)
        }
    }

    /// A mock app with the two locks `stop-all` needs managed, plus a real
    /// supervisor over `driver`.
    fn handler_with(
        driver: Arc<dyn ProcessDriver>,
    ) -> (
        tauri::App<tauri::test::MockRuntime>,
        Arc<Supervisor>,
        DesktopHandler<tauri::test::MockRuntime>,
    ) {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(crate::tray::BulkLock::default());
        app.manage(crate::commands::ApplyLock::default());
        let sup = Arc::new(Supervisor::new(driver));
        let handler = DesktopHandler::new(app.handle().clone(), Arc::clone(&sup));
        (app, sup, handler)
    }

    fn handler() -> (
        tauri::App<tauri::test::MockRuntime>,
        Arc<Supervisor>,
        DesktopHandler<tauri::test::MockRuntime>,
    ) {
        handler_with(default_driver())
    }

    fn id(raw: &str) -> ServiceId {
        ServiceId::parse(raw).expect("test id must parse")
    }

    /// A spec that is only ever registered, never started — no real binary
    /// needed, so this is not unix-gated.
    fn registered_only_spec(name: &str) -> ServiceSpec {
        ServiceSpec {
            id: name.to_string(),
            display_name: format!("{name} display"),
            endpoint: Some(format!("endpoint-{name}")),
            spawn: SpawnSpec {
                program: PathBuf::from("/does/not/exist"),
                args: vec![],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        }
    }

    /// A real, long-lived child (mirrors `quit.rs`'s and the tray's own
    /// `#[cfg(unix)]` precedent for driving a REAL `Supervisor`).
    /// `ready_after` is how long it must survive before `Running` — a value
    /// well above zero is what makes "did the handler actually WAIT?"
    /// observable.
    #[cfg(unix)]
    fn sleepy_spec(name: &str, ready_after: Duration) -> ServiceSpec {
        ServiceSpec {
            id: name.to_string(),
            display_name: format!("{name} display"),
            endpoint: None,
            spawn: SpawnSpec {
                program: PathBuf::from("/bin/sh"),
                args: vec![OsString::from("-c"), OsString::from("exec sleep 30")],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::AliveAfter(ready_after),
            grace: DEFAULT_GRACE,
        }
    }

    /// The exact line `a_failing_start_is_operation_failed_with_the_stderr_tail_verbatim`
    /// expects to survive, unescaped and unquoted, all the way to the wire.
    #[cfg(unix)]
    const FAILING_STDERR: &str =
        "nginx: [emerg] bind() to 0.0.0.0:80 failed (48: Address already in use)";

    /// A child that writes one stderr line, pauses, then exits 3.
    ///
    /// The pause used to be load-bearing: `service_task::finish` snapshotted
    /// the tail concurrently with the reader task, so a child that wrote and
    /// exited in the same instant could legitimately be classified with an
    /// empty tail. The A4 fix wave closed that — `finish` now drains its
    /// readers before it classifies — so the pause is kept only because it
    /// makes this child's ordering obvious to a reader, not because the
    /// assertion depends on it.
    #[cfg(unix)]
    fn failing_spec(name: &str) -> ServiceSpec {
        ServiceSpec {
            id: name.to_string(),
            display_name: format!("{name} display"),
            endpoint: None,
            spawn: SpawnSpec {
                program: PathBuf::from("/bin/sh"),
                args: vec![
                    OsString::from("-c"),
                    OsString::from(format!("echo '{FAILING_STDERR}' >&2; sleep 0.3; exit 3")),
                ],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::AliveAfter(Duration::from_secs(2)),
            grace: DEFAULT_GRACE,
        }
    }

    async fn wait_until(mut cond: impl FnMut() -> bool, deadline: Duration, msg: &str) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() < deadline, "{msg}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn services_of(response: &Response) -> Vec<String> {
        match response {
            Response::Services { services } => services.iter().map(|s| s.id.clone()).collect(),
            other => panic!("expected Services, got {other:?}"),
        }
    }

    fn transition_of(response: &Response) -> (&ServiceStatus, Disposition) {
        match response {
            Response::Transition {
                service,
                disposition,
            } => (service, *disposition),
            other => panic!("expected Transition, got {other:?}"),
        }
    }

    fn error_of(response: &Response) -> (ErrorCode, &str) {
        match response {
            Response::Error { code, message } => (*code, message.as_str()),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // Pure decision tables
    // ------------------------------------------------------------------

    #[test]
    fn step_resolves_each_target_and_state_pair() {
        let failed = ServiceState::Failed {
            exit: Some(1),
            stderr_tail: vec![],
        };
        assert_eq!(step(Target::Running, &ServiceState::Running), Step::Reached);
        assert_eq!(step(Target::Running, &failed), Step::Failed);
        assert_eq!(
            step(Target::Running, &ServiceState::Stopped),
            Step::Diverted
        );
        assert_eq!(
            step(Target::Running, &ServiceState::Starting),
            Step::Pending
        );
        assert_eq!(step(Target::Stopped, &ServiceState::Stopped), Step::Reached);
        assert_eq!(step(Target::Stopped, &failed), Step::Failed);
        // The load-bearing pair: a stop is only REQUESTED synchronously, so a
        // `Running` observation during a stop wait must keep waiting — an
        // implementation that treated any event as a resolution would answer
        // "stopped" for a service still inside its 15s MySQL grace.
        assert_eq!(step(Target::Stopped, &ServiceState::Running), Step::Pending);
        assert_eq!(
            step(Target::Stopped, &ServiceState::Starting),
            Step::Pending
        );
    }

    #[test]
    fn settled_matches_the_supervisors_own_early_return_rule() {
        let failed = ServiceState::Failed {
            exit: Some(1),
            stderr_tail: vec![],
        };
        assert!(settled(Target::Running, &ServiceState::Running));
        assert!(!settled(Target::Running, &ServiceState::Starting));
        assert!(!settled(Target::Running, &ServiceState::Stopped));
        // A `Failed` service must be startable again — `Supervisor::start`
        // does not early-return for it, so neither does this.
        assert!(!settled(Target::Running, &failed));
        assert!(settled(Target::Stopped, &ServiceState::Stopped));
        assert!(settled(Target::Stopped, &failed));
        assert!(!settled(Target::Stopped, &ServiceState::Running));
        assert!(!settled(Target::Stopped, &ServiceState::Starting));
    }

    #[test]
    fn transition_timeout_outlives_mysqls_readiness_deadline_plus_its_stop_grace() {
        // Pinned against the REAL constants, not literals: a MySQL spec that
        // raises either budget must fail HERE rather than silently make
        // `openvhost start mysql-8.4` report a timeout for a service that was
        // still legitimately coming up.
        assert!(
            TRANSITION_TIMEOUT > crate::stack::MYSQL_READY_DEADLINE + crate::stack::MYSQL_GRACE
        );
    }

    #[test]
    fn transition_timeout_outlives_the_bulk_stop_budget() {
        // A per-service `stop` must not give up before the bulk primitive
        // would have.
        assert!(TRANSITION_TIMEOUT > crate::quit::STOP_ALL_TIMEOUT);
    }

    #[test]
    fn failure_message_carries_the_stderr_tail_verbatim() {
        let tail = [
            "nginx: [emerg] bind() to 0.0.0.0:80 failed (48: Address already in use)".to_string(),
            "nginx: still could not bind()".to_string(),
        ];
        let msg = failure_message(Target::Running, "nginx display", Some(3), &tail);
        assert!(msg.contains("nginx display failed to start"), "{msg}");
        assert!(msg.contains("exit code 3"), "{msg}");
        // The NEWLINE-JOINED form, not merely "each line appears somewhere":
        // a `{tail:?}` dump would still contain both lines as substrings
        // (wrapped in escaped quotes and separated by ", "), so a
        // per-line `contains` check alone could not tell the two apart.
        assert!(msg.contains(&tail.join("\n")), "{msg}");
        assert!(
            !msg.contains(&format!("{tail:?}")),
            "the tail must not be Debug-formatted: {msg}"
        );
    }

    #[test]
    fn failure_message_is_honest_about_a_missing_exit_code_and_an_empty_tail() {
        let msg = failure_message(Target::Stopped, "mysql-8.4", None, &[]);
        assert!(msg.contains("failed to stop"), "{msg}");
        assert!(msg.contains("no exit code"), "{msg}");
        assert!(msg.contains("(no stderr captured)"), "{msg}");
    }

    // ------------------------------------------------------------------
    // list / status
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn list_returns_every_registered_service_id_sorted() {
        let (_app, sup, handler) = handler();
        // Registered OUT of order: the response must still be id-sorted (D4).
        sup.register(registered_only_spec("php-fpm-8.4"));
        sup.register(registered_only_spec("nginx"));
        sup.register(registered_only_spec("mysql-8.4"));

        let response = handler.execute(Request::List).await;
        assert_eq!(
            services_of(&response),
            ["mysql-8.4", "nginx", "php-fpm-8.4"]
        );
    }

    #[tokio::test]
    async fn status_with_no_id_matches_list() {
        let (_app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx"));
        sup.register(registered_only_spec("php-fpm-8.4"));

        let listed = handler.execute(Request::List).await;
        let status = handler.execute(Request::Status { id: None }).await;
        assert_eq!(listed, status);
    }

    #[tokio::test]
    async fn status_for_one_id_returns_only_that_row() {
        let (_app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx"));
        sup.register(registered_only_spec("php-fpm-8.4"));

        let response = handler
            .execute(Request::Status {
                id: Some(id("nginx")),
            })
            .await;
        assert_eq!(services_of(&response), ["nginx"]);
    }

    #[tokio::test]
    async fn status_for_an_unregistered_id_is_unknown_service_and_names_what_is() {
        let (_app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx"));

        let response = handler
            .execute(Request::Status {
                id: Some(id("nope")),
            })
            .await;
        let (code, message) = error_of(&response);
        assert_eq!(code, ErrorCode::UnknownService);
        assert!(message.contains("nope"), "{message}");
        assert!(message.contains("nginx"), "{message}");
    }

    // ------------------------------------------------------------------
    // A3: a verb that raced the quit must not start anything.
    // ------------------------------------------------------------------

    /// THE A3 REGRESSION TEST. `perform_quit` marks [`crate::quit::Quitting`]
    /// before it unlinks the socket, and a connection accepted a moment
    /// earlier is still served — so this is what stands between
    /// `openvhost start nginx` and a service spawned seconds before its
    /// supervisor disappears, left listening after the user believes the stack
    /// is down.
    ///
    /// The strong assertion is the driver's, not the response code's: a
    /// refusal that still spawned would be worse than no refusal at all.
    ///
    /// VACUITY: the positive control below is in the same test and shares the
    /// same driver, so "nothing spawned" cannot pass by the recorder never
    /// working. Delete the quit gate from `execute` and this fails twice over
    /// — `Busy` becomes `OperationFailed`, and the spawn log is no longer
    /// empty.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_mutating_verb_is_refused_once_a_quit_has_begun() {
        let driver = RecordingDriver::new();
        let (app, sup, handler) = handler_with(Arc::clone(&driver) as Arc<dyn ProcessDriver>);
        app.manage(crate::quit::Quitting::default());
        sup.register(registered_only_spec("nginx"));

        // POSITIVE CONTROL FIRST, while the app is NOT quitting: the same
        // verb, the same handler, the same driver — admitted, and it really
        // does reach a spawn. Without this the refusal below could be a gate
        // that is simply always closed.
        let admitted = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: true,
            })
            .await;
        assert_ne!(
            error_of(&admitted).0,
            ErrorCode::Busy,
            "nothing is quitting yet; the verb must be admitted"
        );
        assert_eq!(
            driver.spawned().len(),
            1,
            "the positive control must actually have reached the driver"
        );

        app.state::<crate::quit::Quitting>().mark();

        for req in [
            Request::Start {
                id: id("nginx"),
                wait: true,
            },
            Request::Start {
                id: id("nginx"),
                wait: false,
            },
            Request::Stop {
                id: id("nginx"),
                wait: true,
            },
            Request::Restart {
                id: id("nginx"),
                wait: true,
            },
            Request::StopAll,
        ] {
            let label = format!("{req:?}");
            let response = handler.execute(req).await;
            let (code, message) = error_of(&response);
            assert_eq!(code, ErrorCode::Busy, "{label}");
            assert!(message.contains("quitting"), "{label}: {message}");
        }

        assert_eq!(
            driver.spawned().len(),
            1,
            "a verb arriving during a quit must not reach the process driver, got {:?}",
            driver.spawned()
        );
    }

    /// Reads stay open during a quit. Refusing them would make `openvhost
    /// status` lie about a stack that is genuinely still winding down, and
    /// nothing about answering the question can leave a process behind.
    #[tokio::test]
    async fn reads_still_answer_during_a_quit() {
        let (app, sup, handler) = handler();
        app.manage(crate::quit::Quitting::default());
        sup.register(registered_only_spec("nginx"));
        app.state::<crate::quit::Quitting>().mark();

        assert_eq!(
            services_of(&handler.execute(Request::List).await),
            ["nginx"]
        );
        assert_eq!(
            services_of(
                &handler
                    .execute(Request::Status {
                        id: Some(id("nginx"))
                    })
                    .await
            ),
            ["nginx"]
        );
    }

    /// The gate fails OPEN when `Quitting` was never managed — an app that
    /// never finished its bootstrap has no quit in flight either, and this is
    /// the same `try_state` posture as every other read in this module. Pinned
    /// so a future "fail closed here too" change is a deliberate one: it would
    /// make every CLI verb answer `Busy` under `mock_builder`, which is a
    /// silent and very confusing failure mode.
    #[tokio::test]
    async fn an_unmanaged_quit_flag_does_not_refuse_anything() {
        let (_app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx"));
        assert_ne!(
            error_of(
                &handler
                    .execute(Request::Start {
                        id: id("nginx"),
                        wait: true,
                    })
                    .await
            )
            .0,
            ErrorCode::Busy
        );
    }

    /// `mutates` decides what the quit gate covers, and it is the kind of
    /// classification that goes wrong silently. Pinned per variant.
    #[test]
    fn mutates_classifies_every_verb() {
        assert!(!mutates(&Request::List));
        assert!(!mutates(&Request::Status { id: None }));
        assert!(!mutates(&Request::Status {
            id: Some(id("nginx"))
        }));
        assert!(mutates(&Request::Start {
            id: id("nginx"),
            wait: true
        }));
        assert!(mutates(&Request::Stop {
            id: id("nginx"),
            wait: false
        }));
        assert!(mutates(&Request::Restart {
            id: id("nginx"),
            wait: true
        }));
        assert!(mutates(&Request::StopAll));
    }

    // ------------------------------------------------------------------
    // Containment: an unregistered id never reaches a spawn.
    // ------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unregistered_id_is_refused_for_every_verb_and_nothing_is_spawned() {
        let driver = RecordingDriver::new();
        let (_app, sup, handler) = handler_with(Arc::clone(&driver) as Arc<dyn ProcessDriver>);
        sup.register(registered_only_spec("nginx"));

        for req in [
            Request::Start {
                id: id("not-registered"),
                wait: true,
            },
            Request::Stop {
                id: id("not-registered"),
                wait: true,
            },
            Request::Restart {
                id: id("not-registered"),
                wait: true,
            },
            Request::Start {
                id: id("not-registered"),
                wait: false,
            },
        ] {
            let label = format!("{req:?}");
            let response = handler.execute(req).await;
            let (code, message) = error_of(&response);
            assert_eq!(code, ErrorCode::UnknownService, "{label}");
            assert!(message.contains("not-registered"), "{label}: {message}");
        }

        // THE containment claim, against a REAL supervisor: no process driver
        // was ever asked to spawn anything.
        assert!(
            driver.spawned().is_empty(),
            "an unregistered id must never reach a spawn, got {:?}",
            driver.spawned()
        );
        // And the registered service was not touched either.
        assert_eq!(
            sup.snapshot()
                .into_iter()
                .map(|s| s.state)
                .collect::<Vec<_>>(),
            vec![ServiceState::Stopped]
        );

        // POSITIVE CONTROL, so "the log is empty" cannot pass by the recorder
        // simply never working: the SAME driver, in the SAME test, must
        // record a spawn for an id that IS registered. (The program does not
        // exist, so the spawn fails — `RecordingDriver` records the attempt
        // before delegating, which is exactly the granularity the claim
        // needs: "was a spawn ATTEMPTED".)
        let _ = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: true,
            })
            .await;
        assert_eq!(
            driver.spawned(),
            vec![PathBuf::from("/does/not/exist")],
            "a REGISTERED id must reach the driver — otherwise the assertion above proves nothing"
        );
    }

    // ------------------------------------------------------------------
    // start / stop / restart against a real supervisor
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_waits_for_running_and_the_response_carries_it() {
        let (_app, sup, handler) = handler();
        // 700ms of readiness: a handler that answered as soon as
        // `Supervisor::start` returned would report `Starting`, not `Running`.
        sup.register(sleepy_spec("nginx", Duration::from_millis(700)));

        let began = Instant::now();
        let response = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, disposition) = transition_of(&response);
        assert_eq!(service.state, ServiceState::Running);
        assert_eq!(service.id, "nginx");
        assert_eq!(service.display_name, "nginx display");
        assert!(service.pid.is_some(), "a running service must carry a pid");
        assert_eq!(disposition, Disposition::Changed);
        assert!(
            began.elapsed() >= Duration::from_millis(700),
            "the server must WAIT for readiness, answered after {:?}",
            began.elapsed()
        );

        sup.stop("nginx").expect("cleanup stop");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_start_is_operation_failed_with_the_stderr_tail_verbatim() {
        let (_app, sup, handler) = handler();
        sup.register(failing_spec("php-fpm-8.4"));

        let response = handler
            .execute(Request::Start {
                id: id("php-fpm-8.4"),
                wait: true,
            })
            .await;
        let (code, message) = error_of(&response);
        assert_eq!(code, ErrorCode::OperationFailed);
        assert!(message.contains("exit code 3"), "{message}");
        // VERBATIM: the exact bytes the process wrote, not a Debug dump.
        assert!(message.contains(FAILING_STDERR), "{message}");
        assert!(
            !message.contains(&format!("{:?}", vec![FAILING_STDERR])),
            "the tail must not be Debug-formatted: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn starting_an_already_running_service_is_unchanged() {
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));
        sup.start("nginx").expect("setup start");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "setup: nginx never reached Running",
        )
        .await;
        let pid_before = sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == "nginx")
            .unwrap()
            .pid;

        let response = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, disposition) = transition_of(&response);
        assert_eq!(disposition, Disposition::Unchanged);
        assert_eq!(service.state, ServiceState::Running);
        // Unchanged means UNCHANGED: the running child was not recycled.
        assert_eq!(service.pid, pid_before);

        sup.stop("nginx").expect("cleanup stop");
    }

    #[tokio::test]
    async fn stopping_an_already_stopped_service_is_unchanged() {
        let (_app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx"));

        let response = handler
            .execute(Request::Stop {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, disposition) = transition_of(&response);
        assert_eq!(disposition, Disposition::Unchanged);
        assert_eq!(service.state, ServiceState::Stopped);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_waits_for_stopped_and_the_child_is_gone() {
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));
        sup.start("nginx").expect("setup start");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "setup: nginx never reached Running",
        )
        .await;

        let response = handler
            .execute(Request::Stop {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, disposition) = transition_of(&response);
        assert_eq!(disposition, Disposition::Changed);
        assert_eq!(service.state, ServiceState::Stopped);
        // The supervisor agrees — the answer is not a story this handler told
        // itself.
        assert_eq!(
            sup.snapshot()
                .into_iter()
                .find(|s| s.id == "nginx")
                .map(|s| s.state),
            Some(ServiceState::Stopped)
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_recycles_a_running_service_and_leaves_it_running() {
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));
        sup.start("nginx").expect("setup start");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "setup: nginx never reached Running",
        )
        .await;
        let pid_before = sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == "nginx")
            .unwrap()
            .pid;

        let response = handler
            .execute(Request::Restart {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, disposition) = transition_of(&response);
        assert_eq!(disposition, Disposition::Changed);
        // Ends RUNNING, not stopped: a restart that only performed its stop
        // half would leave the service down and still look successful.
        assert_eq!(service.state, ServiceState::Running);
        assert_eq!(
            sup.snapshot()
                .into_iter()
                .find(|s| s.id == "nginx")
                .map(|s| s.state),
            Some(ServiceState::Running)
        );
        // A DIFFERENT child: proves the stop half really happened rather than
        // the whole thing collapsing into "already running, nothing to do".
        assert!(service.pid.is_some());
        assert_ne!(service.pid, pid_before);

        sup.stop("nginx").expect("cleanup stop");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_of_a_stopped_service_brings_it_up() {
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));

        let response = handler
            .execute(Request::Restart {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, disposition) = transition_of(&response);
        assert_eq!(service.state, ServiceState::Running);
        // The stop half was `Unchanged` (nothing was running) but the verb as
        // a whole moved the service, so `Unchanged` here would be false.
        assert_eq!(disposition, Disposition::Changed);

        sup.stop("nginx").expect("cleanup stop");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_wait_answers_before_the_service_is_ready() {
        let (_app, sup, handler) = handler();
        // 3s of readiness, so "did not wait" is unambiguous.
        sup.register(sleepy_spec("nginx", Duration::from_secs(3)));

        let began = Instant::now();
        let response = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: false,
            })
            .await;
        let elapsed = began.elapsed();
        let (service, disposition) = transition_of(&response);
        assert_eq!(disposition, Disposition::Changed);
        assert_eq!(service.state, ServiceState::Starting);
        assert!(
            elapsed < Duration::from_secs(1),
            "--no-wait must answer immediately, took {elapsed:?}"
        );

        sup.stop("nginx").expect("cleanup stop");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_start_that_does_not_settle_in_time_is_a_timeout() {
        let (_app, sup, handler) = handler();
        let handler = handler.with_transition_timeout(Duration::from_millis(300));
        // Readiness far beyond the handler's deadline: the service is still
        // legitimately `Starting` when the wait gives up.
        sup.register(sleepy_spec("mysql-8.4", Duration::from_secs(20)));

        let response = handler
            .execute(Request::Start {
                id: id("mysql-8.4"),
                wait: true,
            })
            .await;
        let (code, message) = error_of(&response);
        assert_eq!(code, ErrorCode::Timeout);
        assert!(message.contains("mysql-8.4"), "{message}");
        assert!(message.contains("starting"), "{message}");

        sup.stop("mysql-8.4").expect("cleanup stop");
    }

    /// A service UNREGISTERED while a verb waits on it (a GUI uninstall
    /// racing `openvhost start`, package-uninstall design D4) must be
    /// answered at once. Before `Unregistered` existed this could not happen;
    /// now, the wrong handling — treating it as "not a resolution" — would
    /// make the CLI sit on a dead id for the full 45s and then blame a
    /// timeout, because no further event can ever carry that id and the
    /// `Lagged` re-read finds no row either.
    ///
    /// Drives `await_settled` directly: the arm is only reachable when the
    /// removal is observed AFTER the wait began, which a full `execute` round
    /// trip cannot stage deterministically (`unregister` refuses a
    /// non-terminal service, so a genuine in-flight transition never reaches
    /// it — only a lagged receiver does).
    ///
    /// VACUITY (neuter-and-watch-it-fail): the `Unregistered` arm was
    /// temporarily changed to `None` (keep waiting) — this test failed with
    /// `ErrorCode::Timeout` and a "did not become running within" message,
    /// after burning the full 500ms deadline instead of answering in <50ms.
    /// Restoring the arm made it pass again.
    #[tokio::test]
    async fn a_service_removed_mid_wait_is_answered_at_once_not_left_to_time_out() {
        let (_app, sup, handler) = handler();
        sup.register(registered_only_spec("php-fpm-8.3"));

        // Subscribed BEFORE the removal, exactly as `transition` subscribes
        // before it kicks — so this receiver holds the `Unregistered`.
        let rx = sup.subscribe();
        sup.unregister("php-fpm-8.3")
            .expect("a stopped service is forgettable");

        let began = Instant::now();
        let outcome = handler
            .await_settled(
                rx,
                Target::Running,
                "php-fpm-8.3",
                Duration::from_millis(500),
            )
            .await;
        let elapsed = began.elapsed();

        match outcome {
            Outcome::Refused { code, message } => {
                assert_eq!(code, ErrorCode::UnknownService);
                assert!(message.contains("php-fpm-8.3"), "{message}");
                assert!(message.contains("removed"), "{message}");
            }
            Outcome::Reached { .. } => panic!("a removed service was never reached"),
        }
        assert!(
            elapsed < Duration::from_millis(400),
            "must answer immediately, took {elapsed:?}"
        );
    }

    /// The other half of the arm above: an `Unregistered` for a DIFFERENT
    /// service is none of this wait's business and must not resolve it.
    /// Without the `gone == id` guard, uninstalling PHP 8.3 would abort an
    /// unrelated `openvhost start nginx` that happened to be in flight.
    ///
    /// VACUITY (neuter-and-watch-it-fail): the guard was temporarily dropped
    /// (making every `Unregistered` resolve the wait) — this test failed with
    /// `ErrorCode::UnknownService` naming `nginx-not-touched`, a service that
    /// was never removed. Restoring the guard made it pass again.
    #[tokio::test]
    async fn an_unregistered_event_for_another_service_does_not_resolve_this_wait() {
        let (_app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx-not-touched"));
        sup.register(registered_only_spec("php-fpm-8.3"));

        let rx = sup.subscribe();
        sup.unregister("php-fpm-8.3")
            .expect("a stopped service is forgettable");

        let outcome = handler
            .await_settled(
                rx,
                Target::Running,
                "nginx-not-touched",
                Duration::from_millis(300),
            )
            .await;

        match outcome {
            Outcome::Refused { code, message } => {
                assert_eq!(code, ErrorCode::Timeout, "{message}");
                assert!(message.contains("nginx-not-touched"), "{message}");
            }
            Outcome::Reached { .. } => panic!("nothing started this service"),
        }
    }

    /// A start whose child exits **cleanly** before it is ever ready must not
    /// be reported as a success. `classify_exit` calls a code-0 death
    /// `Stopped`, not `Failed` — the shape of a binary that self-daemonizes,
    /// or quits on a config it dislikes without an error code — so a handler
    /// that only distinguished "Failed = bad, anything else = fine" would
    /// answer exit 0 for a service that is not running.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_start_that_settles_stopped_without_ever_running_is_operation_failed() {
        let (_app, sup, handler) = handler();
        let mut spec = sleepy_spec("nginx", Duration::from_secs(2));
        // Exits 0 immediately — well inside the readiness window, so the
        // supervisor classifies it `Stopped`, never `Running`.
        spec.spawn.args = vec![OsString::from("-c"), OsString::from("exit 0")];
        sup.register(spec);

        let response = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (code, message) = error_of(&response);
        assert_eq!(code, ErrorCode::OperationFailed);
        assert!(message.contains("stopped"), "{message}");
        assert!(message.contains("nginx"), "{message}");
    }

    /// The scenario `Step::Diverted` is often ASSUMED to cover, recorded
    /// because it does not: a stop landing during an `AliveAfter` readiness
    /// window does NOT divert the start.
    ///
    /// `Supervisor::stop` only flags the request and pushes to the service
    /// task's control channel; that task is inside `await_readiness`, which
    /// races the readiness sleep against `child.wait()` and never reads the
    /// control channel — so `Running` is broadcast first and the requested
    /// stop resolves afterwards as its own transition. This test exists so a
    /// future change to that ordering (a readiness wait that also selects on
    /// the control channel) surfaces HERE, in the control channel's
    /// contract, rather than as a CLI that starts reporting a failure for
    /// a start that used to succeed.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_start_stopped_during_its_readiness_window_still_observes_running() {
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(600)));

        let interloper = Arc::clone(&sup);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = interloper.stop("nginx");
        });

        let response = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, _) = transition_of(&response);
        assert_eq!(service.state, ServiceState::Running);
        // …and the stop the interloper asked for still lands, right after.
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Stopped)
            },
            Duration::from_secs(5),
            "the deferred stop never took effect after readiness resolved",
        )
        .await;
    }

    /// Reentrancy: a second control request arriving while the first is still
    /// waiting must be answered on its own merits — each connection gets its
    /// own task and its own subscription, and per-service verbs take no lock.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_concurrent_starts_both_see_the_terminal_state() {
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(800)));

        let first = handler.execute(Request::Start {
            id: id("nginx"),
            wait: true,
        });
        let second = handler.execute(Request::Start {
            id: id("nginx"),
            wait: true,
        });
        // A `list` in the middle must not be blocked by either wait.
        let third = handler.execute(Request::List);
        let (a, b, c) = tokio::join!(first, second, third);

        for (label, response) in [("first", &a), ("second", &b)] {
            let (service, _) = transition_of(response);
            assert_eq!(service.state, ServiceState::Running, "{label}");
        }
        assert_eq!(services_of(&c), ["nginx"]);
        // Exactly one child: the duplicate start was the supervisor's
        // documented no-op, not a second spawn.
        assert_eq!(transition_of(&a).0.pid, transition_of(&b).0.pid);

        sup.stop("nginx").expect("cleanup stop");
    }

    /// D7's other half, stated positively: **per-service verbs take no
    /// lock.** A bulk operation in flight (the tray's Start-all, an Apply)
    /// holds both mutexes for as long as 18s; making `openvhost start nginx`
    /// fail — or worse, queue — behind it would buy nothing, because
    /// `Supervisor::start` is already idempotent inside the entries mutex.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_start_is_admitted_while_a_bulk_operation_holds_both_locks() {
        let (app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));

        let bulk = app.state::<crate::tray::BulkLock>();
        let apply = app.state::<crate::commands::ApplyLock>();
        let _held =
            crate::tray::service_control::try_acquire_bulk(&bulk.inner().0, &apply.inner().0)
                .expect("setup: both locks must be free");

        let response = handler
            .execute(Request::Start {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, _) = transition_of(&response);
        assert_eq!(service.state, ServiceState::Running);

        sup.stop("nginx").expect("cleanup stop");
    }

    /// A2: `restart` claimed — in the wire type's own docs and in spec D4 —
    /// that "a tray click or an Apply cannot interleave between the two
    /// halves". Nothing implemented that, and it contradicted rule 3 two
    /// paragraphs above it. This pins what is actually true, from the side
    /// that would have to change first: `restart` takes no lock, so an Apply
    /// holding both mutexes does not even delay it, let alone exclude it.
    ///
    /// The real guarantee — the start half is not dispatched until the stop
    /// half is *observed* complete — is pinned separately by
    /// `restart_recycles_a_running_service_and_leaves_it_running`: a restart
    /// that dispatched the start early would hit `Supervisor::start`'s
    /// already-live early return and leave the service `Stopped`.
    ///
    /// VACUITY: make `restart` (or the `transition` under it) acquire the
    /// bulk lock — the exclusion the deleted sentence described — and this
    /// test blocks or fails.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_is_sequenced_but_not_exclusive_with_an_apply() {
        let (app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));
        sup.start("nginx").expect("setup start");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "setup: nginx never reached Running",
        )
        .await;

        // Exactly what an Apply in flight holds.
        let bulk = app.state::<crate::tray::BulkLock>();
        let apply = app.state::<crate::commands::ApplyLock>();
        let _held =
            crate::tray::service_control::try_acquire_bulk(&bulk.inner().0, &apply.inner().0)
                .expect("setup: both locks must be free");

        let response = handler
            .execute(Request::Restart {
                id: id("nginx"),
                wait: true,
            })
            .await;
        let (service, disposition) = transition_of(&response);
        assert_eq!(service.state, ServiceState::Running);
        assert_eq!(disposition, Disposition::Changed);

        sup.stop("nginx").expect("cleanup stop");
    }

    // ------------------------------------------------------------------
    // stop-all (D7): reject, never queue.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_all_stops_a_running_service_and_reports_no_stragglers() {
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));
        sup.start("nginx").expect("setup start");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "setup: nginx never reached Running",
        )
        .await;

        let response = handler.execute(Request::StopAll).await;
        match &response {
            Response::StopAll { stragglers } => {
                assert!(
                    stragglers.is_empty(),
                    "unexpected stragglers: {stragglers:?}"
                )
            }
            other => panic!("expected StopAll, got {other:?}"),
        }
        assert_eq!(
            sup.snapshot()
                .into_iter()
                .find(|s| s.id == "nginx")
                .map(|s| s.state),
            Some(ServiceState::Stopped)
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_all_while_the_bulk_lock_is_held_is_busy_and_stops_nothing() {
        let (app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));
        sup.start("nginx").expect("setup start");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "setup: nginx never reached Running",
        )
        .await;

        // The REAL managed lock, held exactly as an in-flight tray Stop-all
        // would hold it.
        let bulk = app.state::<crate::tray::BulkLock>();
        let _held = bulk.inner().0.try_lock().expect("setup: lock must be free");

        let response = handler.execute(Request::StopAll).await;
        let (code, message) = error_of(&response);
        assert_eq!(code, ErrorCode::Busy);
        assert!(message.contains("already"), "{message}");
        // REJECTED, not queued: the service is still running.
        assert_eq!(
            sup.snapshot()
                .into_iter()
                .find(|s| s.id == "nginx")
                .map(|s| s.state),
            Some(ServiceState::Running),
            "a rejected stop-all must not have stopped anything"
        );

        drop(_held);
        sup.stop("nginx").expect("cleanup stop");
    }

    #[tokio::test]
    async fn stop_all_while_the_apply_lock_is_held_is_busy() {
        // The OTHER half of the admission check: the literal mutex
        // `apply_config` takes, not a second lock invented here.
        let (app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx"));
        let apply = app.state::<crate::commands::ApplyLock>();
        let _held = apply
            .inner()
            .0
            .try_lock()
            .expect("setup: lock must be free");

        let response = handler.execute(Request::StopAll).await;
        assert_eq!(error_of(&response).0, ErrorCode::Busy);
    }

    #[tokio::test]
    async fn a_rejected_stop_all_leaves_the_bulk_lock_free_for_the_next_caller() {
        let (app, sup, handler) = handler();
        sup.register(registered_only_spec("nginx"));
        let apply = app.state::<crate::commands::ApplyLock>();
        let held = apply
            .inner()
            .0
            .try_lock()
            .expect("setup: lock must be free");
        assert_eq!(
            error_of(&handler.execute(Request::StopAll).await).0,
            ErrorCode::Busy
        );
        drop(held);

        // The rejected attempt must not have leaked the bulk lock it probed
        // first — otherwise one rejected CLI call would wedge every later
        // tray Stop-all.
        let bulk = app.state::<crate::tray::BulkLock>();
        assert!(bulk.inner().0.try_lock().is_ok());
    }

    #[tokio::test]
    async fn stop_all_without_the_locks_managed_fails_closed() {
        // No `BulkLock`/`ApplyLock` at all — the shape of an app that never
        // finished bootstrapping. Must be a typed failure, not a panic and
        // not a silent teardown that skipped the admission check.
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let sup = Arc::new(Supervisor::new(default_driver()));
        let handler = DesktopHandler::new(app.handle().clone(), Arc::clone(&sup));

        let response = handler.execute(Request::StopAll).await;
        assert_eq!(error_of(&response).0, ErrorCode::OperationFailed);
    }

    // ------------------------------------------------------------------
    // The vertical slice, minus the GUI (spec D7's "real supervisor" case).
    // ------------------------------------------------------------------

    /// THIS handler, behind the REAL transport, driven by the REAL client the
    /// CLI uses: `openvhost_proc::control::bind` → `serve` → a `SOCK_STREAM`
    /// connection → `control::request`. The only thing missing versus
    /// production is `lib.rs`'s own `setup()` closure.
    ///
    /// Everything else in this file calls `execute` directly, and everything
    /// in `openvhost-proc`'s and `apps/cli`'s suites drives the socket with a
    /// *fake* handler — so without this test, "the real handler answers over
    /// the real socket" is the one claim in the slice nobody makes.
    ///
    /// It also carries D7's stderr claim end to end: the failing service's
    /// output must survive classification, the envelope and the client's own
    /// decode to arrive at the caller unescaped.
    ///
    /// The socket assertion at the bottom proves the MECHANISM only — this
    /// hands `serve` a real shutdown future, which the app never does (it
    /// passes `std::future::pending()`). That gap is what let a socket
    /// surviving every quit reach a live proof; `quit.rs`'s
    /// `quitting_removes_the_control_socket_although_serve_never_stops` is
    /// what pins the production shape.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_real_client_drives_a_real_supervisor_over_a_real_socket() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let (_app, sup, handler) = handler();
        sup.register(sleepy_spec("nginx", Duration::from_millis(100)));
        sup.register(failing_spec("php-fpm-8.4"));

        let listener = openvhost_proc::control::bind(home.path()).expect("bind");
        let socket = listener.path().to_path_buf();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let served: Arc<dyn ControlHandler> = Arc::new(handler);
        let server = tokio::spawn(openvhost_proc::control::serve(
            listener,
            served,
            async move {
                let _ = shutdown_rx.await;
            },
        ));

        // The client is SYNC by design (the CLI is one round trip), so it
        // goes on a blocking thread rather than stalling this runtime.
        let ask = |req: Request| {
            let home = home.path().to_path_buf();
            async move {
                tokio::task::spawn_blocking(move || openvhost_proc::control::request(&home, &req))
                    .await
                    .expect("client thread panicked")
                    .expect("the control request itself failed")
            }
        };

        assert_eq!(
            services_of(&ask(Request::List).await),
            ["nginx", "php-fpm-8.4"]
        );

        let started = ask(Request::Start {
            id: id("nginx"),
            wait: true,
        })
        .await;
        let (service, disposition) = transition_of(&started);
        assert_eq!(service.state, ServiceState::Running);
        assert_eq!(disposition, Disposition::Changed);

        let status = ask(Request::Status {
            id: Some(id("nginx")),
        })
        .await;
        assert_eq!(services_of(&status), ["nginx"]);

        // The stderr tail survives classification, the envelope, and the
        // client's decode — verbatim.
        let failed = ask(Request::Start {
            id: id("php-fpm-8.4"),
            wait: true,
        })
        .await;
        let (code, message) = error_of(&failed);
        assert_eq!(code, ErrorCode::OperationFailed);
        assert!(message.contains(FAILING_STDERR), "{message}");

        let unknown = ask(Request::Start {
            id: id("not-registered"),
            wait: true,
        })
        .await;
        assert_eq!(error_of(&unknown).0, ErrorCode::UnknownService);

        let stopped = ask(Request::Stop {
            id: id("nginx"),
            wait: true,
        })
        .await;
        assert_eq!(transition_of(&stopped).0.state, ServiceState::Stopped);

        let _ = shutdown_tx.send(());
        server.await.expect("the server task panicked");
        // An orderly shutdown removes the socket it bound.
        assert!(
            !socket.exists(),
            "the socket outlived an orderly shutdown: {}",
            socket.display()
        );
    }
}
