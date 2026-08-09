// SPDX-License-Identifier: GPL-3.0-or-later
//! How far the boot got, as one value the whole app can read (degraded-boot
//! design, D1).
//!
//! `lib.rs`'s `setup` used to bootstrap inside two nested matches whose three
//! bail arms each did nothing but `eprintln!` and fall through to `Ok(())`.
//! **The window still opened** with almost nothing managed, so nearly every
//! command came back as Tauri's own refusal — *"state not managed for field
//! `db` on command `php_environment`. You must call `.manage()` before using
//! this command."* — rendered verbatim to a **user**. PR #69 removed that
//! sentence from the store-unavailable path and could not help here: its
//! `state_store_status` reads a `DbHandle`, which is itself managed inside the
//! one arm that succeeded. **The app was quietest exactly when it was most
//! broken.**
//!
//! [`bootstrap`] replaces those three fall-throughs. Its return type is
//! [`BootState`], not `()`, so **the compiler** — not a comment and not a test
//! — is what makes every path yield one: an early `return` must still carry a
//! `BootState`, and there is no `?` to skip past because nothing here returns a
//! `Result`. `lib.rs` then manages that value **once, at the top level, outside
//! every arm**, for the same reason `db_state` gives: `Manager::manage` does not
//! overwrite an existing value, so a "manage a placeholder early, the real value
//! later" split would silently pin the placeholder.
//!
//! Three things live here rather than in `lib.rs` because they are the parts a
//! test can actually reach — `AppHandle<Wry>` cannot be constructed under
//! `mock_builder`, so `bootstrap` itself is plumbing and only plumbing:
//!
//! 1. [`boot_dto`] — the rendering decision, matched exhaustively with no
//!    wildcard, so a fifth state cannot silently inherit a fourth's screen.
//! 2. [`stderr_line`] — the sentence a developer running from a terminal sees,
//!    as an `Option` so `Ready` printing nothing is structural rather than
//!    remembered.
//! 3. [`acquire_with_one_retry`] — D5's single retry, generic over the lock so
//!    both directions are testable: a spurious contention is retried, and a
//!    genuinely held lock is still reported held.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use openvhost_proc::{
    FileRegistry, InstanceLock, Supervisor, SupervisorEvent, default_driver, default_reaper,
};
use tauri::Manager;
// `tauri_specta::Event`, not `tauri::Emitter`: the emit method on a
// `#[derive(tauri_specta::Event)]` type comes from this trait, and it is what
// keeps the event name in sync with the generated TS binding.
use tauri_specta::Event as _;

use crate::commands::IpcError;
use crate::db_state;

/// How long [`bootstrap`] waits before re-trying a contended run lock (D5).
///
/// `lock.rs:147-149` records that between `fork` and `exec` a child transiently
/// duplicates every open descriptor, this lock file's included, and `O_CLOEXEC`
/// clears it at `exec`. This app spawns nginx, php-fpm and mysqld continuously,
/// so a launch that races a spawn can observe a **spurious `Ok(None)` for
/// milliseconds** — and would then show a takeover screen that disappears on the
/// next try, which is the most maddening kind of wrong.
///
/// Paid **only** on contention: [`acquire_with_one_retry`] does not wait when
/// the first attempt settles the question, so an ordinary launch is not 100 ms
/// slower.
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(100);

/// How far [`bootstrap`] got — managed once, read by every screen.
///
/// Deliberately **not** a `bool` and not a per-command "unavailable" value.
/// `DbHandle`'s per-command classification works because a broken store is a
/// *partial* failure of a *running* app; here nothing works at all, so a
/// per-command answer carries zero information and the "unavailable" values
/// would be **indistinguishable from legitimate empty states** — `Option<
/// StackPaths> = None` is the *normal* state on a non-macOS target, and an empty
/// `Supervisor` looks exactly like a machine with nothing installed. That trades
/// a frightening developer string for a plausible lie (design D2).
///
/// No serde/specta derives: this is managed state, never the wire type.
/// [`boot_dto`] is the one conversion, and keeping them separate is what lets
/// the wire field names dodge a trap recorded in this repo (see
/// [`BootStatusDto`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootState {
    /// The lock was taken, the store was opened best-effort, and the supervisor,
    /// the stack, the tray and the control socket were all wired up. The only
    /// state in which the app is the app.
    Ready,
    /// Another instance holds the run lock on `home`, and this one deliberately
    /// did not take it over.
    ///
    /// The ordinary GUI double-launch never reaches here: `lib.rs` handles
    /// `RunEvent::Reopen`, and on macOS LaunchServices activates the running
    /// instance for a bundled app rather than starting a second process. The
    /// real producers are developer scenarios — `pnpm tauri dev`, running
    /// `Contents/MacOS/OpenVHost` directly, `open -n`, two copies of the bundle
    /// at different paths, or a second GUI session (design §2) — which is why
    /// naming the contended `home` is worth more than silently redirecting.
    AlreadyRunning {
        /// The `OPENVHOST_HOME` whose `run/lock` is already held.
        home: PathBuf,
    },
    /// `<home>/run` could not be created, tightened to 0700, or locked.
    ///
    /// A user-fixable permissions problem, so the path and the errno *are* the
    /// payload — measured live as `Permission denied (os error 13)` on a
    /// read-only home.
    RunDirUnusable {
        /// The `<home>/run` directory the lock lives in.
        run_dir: PathBuf,
        /// The failing syscall's own `Display`. For humans; never parse it.
        reason: String,
    },
    /// `OPENVHOST_HOME` would not resolve at all, so there is no path to name.
    ///
    /// Near-unreachable on macOS: `home.rs` filters an empty override, so this
    /// needs `$HOME` unset *and* a failing passwd lookup, or a deleted cwd.
    /// Fail CLOSED rather than falling back to a cwd-relative `./run` — a
    /// relative run dir would lock and reap against whatever directory the OS
    /// happened to launch us from (P0-8 merge-gate fix wave C5).
    HomeUnresolvable {
        /// Why it would not resolve. For humans; never parse it.
        reason: String,
    },
}

/// [`BootState`] on the wire — what `+layout.svelte` gates the whole app on.
///
/// A 1:1 copy rather than a reuse, for two reasons that are both load-bearing:
/// `BootState` holds a live `PathBuf` (not serializable as the UI wants to read
/// it), and the wire field names have to survive a trap this repo has already
/// been bitten by — **`rename_all` on an enum renames its VARIANTS, not its
/// fields.** Every tagged enum on this command surface therefore uses
/// single-word fields, and so does this one: `path`, not `run_dir`, is what
/// keeps the wire shape honest without an untested serde attribute.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BootStatusDto {
    /// The app is the app. `+layout.svelte` renders its children.
    Ready,
    /// Another instance holds the run lock on `home`.
    AlreadyRunning {
        /// The contended `OPENVHOST_HOME`, for the screen to name.
        home: String,
    },
    /// `<home>/run` is unusable; `path` and `reason` are both the payload.
    RunDirUnusable {
        /// The `<home>/run` directory — also what "Reveal in Finder" opens.
        path: String,
        /// The OS error, **verbatim**. Render it; do not summarise it.
        reason: String,
    },
    /// `OPENVHOST_HOME` would not resolve, so there is no path to reveal.
    HomeUnresolvable {
        /// Why it would not resolve, verbatim.
        reason: String,
    },
}

/// The rendering decision, as a pure function.
///
/// This is where the tests live. `bootstrap` takes an `AppHandle<Wry>`-shaped
/// argument, which `mock_builder` cannot construct (documented in five places in
/// this crate), so the split is what makes the decision reachable at all while
/// leaving only the plumbing untested.
///
/// **Exhaustive, with no wildcard arm.** A fifth [`BootState`] fails to compile
/// here rather than quietly inheriting whichever screen a `_ =>` pointed at —
/// proven by adding one and watching `cargo check` reject it.
pub fn boot_dto(boot: &BootState) -> BootStatusDto {
    match boot {
        BootState::Ready => BootStatusDto::Ready,
        BootState::AlreadyRunning { home } => BootStatusDto::AlreadyRunning {
            home: home.display().to_string(),
        },
        BootState::RunDirUnusable { run_dir, reason } => BootStatusDto::RunDirUnusable {
            path: run_dir.display().to_string(),
            reason: reason.clone(),
        },
        BootState::HomeUnresolvable { reason } => BootStatusDto::HomeUnresolvable {
            reason: reason.clone(),
        },
    }
}

/// What a developer running from a terminal sees, or `None` on a healthy boot.
///
/// The three sentences are the ones `lib.rs`'s bail arms printed before this
/// module existed, carried over word for word — a developer who knows the old
/// output keeps finding it. `None` for [`BootState::Ready`] is what makes "a
/// healthy boot prints nothing" structural at the one call site (`if let
/// Some(…)`) rather than a rule someone has to keep remembering.
///
/// The `openvhost: ` prefix belongs to the caller, matching
/// `db_state::unavailable_message`'s split: one wording, one place that decides
/// how it is emitted.
///
/// Exhaustive with no wildcard, same as [`boot_dto`]: a fifth state must be
/// given its own sentence, or say plainly that it has none.
pub fn stderr_line(boot: &BootState) -> Option<String> {
    match boot {
        BootState::Ready => None,
        BootState::AlreadyRunning { .. } => {
            Some("another instance holds the run lock; not starting the supervisor".to_string())
        }
        BootState::RunDirUnusable { reason, .. } => {
            Some(format!("failed to acquire the run lock: {reason}"))
        }
        BootState::HomeUnresolvable { reason } => Some(format!(
            "cannot resolve OPENVHOST_HOME ({reason}); not starting the supervisor"
        )),
    }
}

/// Whether closing the window should hide it rather than let it close (D6).
///
/// Hide-on-close exists because a Dock click, the tray's "Open OpenVHost" or
/// `RunEvent::Reopen` can all bring the window back — but **the tray is built
/// inside the `Ready` arm only**, so a closed degraded window with hide-on-close
/// becomes a hidden zombie with nothing left to reveal it, in a process that
/// goes on holding whatever it holds.
///
/// Takes an `Option` and fails CLOSED on `None`: every `try_state` read in this
/// app does, and "let the close proceed" is the non-trapping direction to be
/// wrong in. In practice the state is always managed by the time a window event
/// can fire — `setup` runs before the event loop — so `None` means something has
/// gone wrong that this function should not paper over.
pub fn hides_on_close(boot: Option<&BootState>) -> bool {
    matches!(boot, Some(BootState::Ready))
}

/// Acquire, and on contention only, wait and try exactly once more (D5).
///
/// Generic over the lock and the error rather than written against
/// [`InstanceLock`] directly, because **both directions have to be reachable
/// from a test**: a fake `acquire` can produce the transient `Ok(None)` that the
/// fork/exec window produces in production, and the real `InstanceLock` can
/// produce a genuinely held lock. A retry that eventually succeeded regardless
/// would silently defeat single-instance protection, which is the half that
/// matters.
///
/// **`Ok(None)` only.** An `Err` is not retried: the documented race makes
/// `flock` report the lock *held* (`EWOULDBLOCK`), never fail, so retrying an
/// error would delay every unusable run dir by the wait for no reason. And a
/// first-try `Ok(Some(_))` does not wait at all, so an ordinary launch pays
/// nothing.
fn acquire_with_one_retry<T, E>(
    mut acquire: impl FnMut() -> Result<Option<T>, E>,
    wait: impl FnOnce(),
) -> Result<Option<T>, E> {
    match acquire() {
        Ok(None) => {
            wait();
            acquire()
        }
        settled => settled,
    }
}

/// Bring up the whole app, and say how far it got.
///
/// **Every return path yields a [`BootState`]** — enforced by the return type
/// rather than asserted: this function returns no `Result`, so there is no `?`
/// to bail through, and any `return` must carry a state. The caller manages the
/// result exactly once, outside every arm.
///
/// Best-effort throughout the `Ready` arm, and each of those failures is logged
/// where it happens rather than promoted to a boot state: a missing `state.db`,
/// a tray that would not build and an unbindable control socket are all real,
/// nameable degradations of a **running** app, which is exactly the situation
/// `DbHandle` and its siblings already answer for.
pub fn bootstrap(app: &tauri::App) -> BootState {
    // Single-instance lock (design spec §7): reap MUST run only while this is
    // held, otherwise a second live instance would reap the first's HEALTHY
    // services (identity matches — it really is their process — but the
    // "orphan" premise is false).
    let boot = match openvhost_core::resolve_home() {
        Ok(home) => {
            let run_dir = home.join("run");
            // D5: one retry on contention, because this repo documents the
            // fork/exec window that makes a spurious `Ok(None)` possible. See
            // `acquire_with_one_retry` for why an `Err` is not retried.
            match acquire_with_one_retry(
                || InstanceLock::acquire(&run_dir),
                || std::thread::sleep(LOCK_RETRY_DELAY),
            ) {
                Ok(Some(lock)) => {
                    // Keep the lock alive for the app's lifetime — dropping
                    // it releases the flock and lets a later instance
                    // acquire it.
                    app.manage(lock);
                    // Open the persistent state store best-effort: a
                    // missing/unreadable state.db must never stop the
                    // supervisor from starting. Twenty-seven commands
                    // read it, so the failed arm is a real state this
                    // app has to render, not a footnote.
                    //
                    // The HANDLE is managed on BOTH arms OF THE OPEN —
                    // exactly once, the same shape `stack_paths` uses below
                    // and for the same `Manager::manage`-never-overwrites
                    // reason. Extraction therefore succeeds whichever way
                    // the open went, and each command answers for itself
                    // (`DbHandle::require` / `optional`) instead of Tauri
                    // refusing the whole command and telling a USER to
                    // call `.manage()`.
                    //
                    // Both arms of the OPEN, not unconditionally: this
                    // line sits inside `resolve_home() == Ok(home)` and
                    // `InstanceLock::acquire() == Ok(Some(lock))`. On the
                    // other three paths nothing here is managed at all —
                    // which is what `BootState` is for: the frontend gates
                    // on it and shows a takeover screen, rather than every
                    // command failing with Tauri's `.manage()` string.
                    //
                    // The bare `Db` is deliberately NOT managed anywhere,
                    // on either arm: that is what makes a future
                    // `State<'_, Db>` parameter fail on every machine
                    // rather than only on a broken one (design D6).
                    let db = match tauri::async_runtime::block_on(openvhost_core::Db::open(&home)) {
                        Ok(db) => db_state::DbHandle::Ready(db),
                        Err(e) => {
                            let reason = e.to_string();
                            // Kept for the developer running from a
                            // terminal, and worded from the same shared
                            // sentence the refusals and the banner use —
                            // one condition, one wording.
                            eprintln!("openvhost: {}", db_state::unavailable_message(&reason));
                            db_state::DbHandle::Unavailable { reason }
                        }
                    };
                    app.manage(db);
                    let registry = Arc::new(FileRegistry::new(&run_dir));
                    let supervisor = Arc::new(Supervisor::with_orphan_cleanup(
                        default_driver(),
                        registry,
                        default_reaper(),
                    ));
                    #[cfg(debug_assertions)]
                    supervisor.register(crate::demo_ticker_spec());
                    #[cfg(target_os = "macos")]
                    let (
                        stack_paths,
                        stack_runtimes,
                        mysql_runtimes,
                        mariadb_runtimes,
                        nginx_source,
                    ) = {
                        let stack = crate::stack::macos_stack();
                        for spec in stack.specs {
                            supervisor.register(spec);
                        }
                        (
                            stack.paths,
                            stack.runtimes,
                            stack.mysql_runtimes,
                            stack.mariadb_runtimes,
                            stack.nginx_source,
                        )
                    };
                    // No stack builder for this target yet, so `None` is the
                    // NORMAL state here — the home resolved fine, there is
                    // simply nothing to point the Web Server page at. See
                    // `commands::stack_paths` for the message that renders.
                    #[cfg(not(target_os = "macos"))]
                    let (
                        stack_paths,
                        stack_runtimes,
                        mysql_runtimes,
                        mariadb_runtimes,
                        nginx_source,
                    ): (
                        Option<crate::stack::StackPaths>,
                        Option<openvhost_core::InstalledRuntimes>,
                        Option<Vec<openvhost_core::mysql::MysqlRuntime>>,
                        Option<Vec<openvhost_core::mariadb::MariadbRuntime>>,
                        Option<openvhost_core::nginx::NginxRuntimeSource>,
                    ) = (None, None, None, None, None);
                    // Manage the Option ITSELF, unconditionally. Tauri implements
                    // `CommandArg` only for `State<'r, T>` — there is no impl for
                    // `Option<State<'r, T>>` — so a command cannot take an
                    // optionally-managed state. Making `Option<StackPaths>` the
                    // managed type is what lets a command distinguish "no stack on
                    // this platform" from "not wired up", while always having
                    // something to extract.
                    //
                    // Exactly ONE `manage` call per state type: `Manager::manage`
                    // does NOT overwrite an existing value (its own doc example
                    // asserts `assert!(!app.manage(MyInt(1)))`), so a "manage None
                    // early, the real value later" split would silently pin every
                    // user to `None`.
                    app.manage(stack_paths);
                    // Nginx source design D2: managed the same "bare `Option`,
                    // exactly once" shape as `stack_paths` above rather than the
                    // `RwLock`-wrapped shape the three lists below use — nothing in
                    // this slice rescans or reinstalls nginx after launch, so unlike
                    // those three there is no later writer to guard against.
                    // `list_web_servers` reads this instead of re-discovering, for
                    // the same reason `stack_paths` itself exists: a fresh walk could
                    // disagree with the binary the supervisor actually registered if
                    // `current` moved after launch.
                    app.manage(nginx_source);
                    // Same `Option<T>`-managed-unconditionally shape as `stack_paths`
                    // above, for the same reason: `Manager::manage` never overwrites,
                    // so every arm must yield a value rather than some arms skipping
                    // the call. `None` on a target with no stack builder, or when the
                    // php-fpm version could not be probed (see `stack::macos_stack`'s
                    // doc comment) — either way a later command that reads this state
                    // sees an honest absence rather than a stale value from a call
                    // that never happened.
                    //
                    // Wrapped in an `RwLock` (unlike `stack_paths` above): Tauri's
                    // managed state cannot be replaced once set, but the installed PHP
                    // runtimes CAN change after launch — the Languages page installs a
                    // version at runtime, and the apply pipeline must see it without a
                    // relaunch. The lock is the seam a later rescan/install writes
                    // through; every reader here just takes the read side.
                    app.manage(std::sync::RwLock::new(stack_runtimes));
                    // Same reasoning as `stack_runtimes` above, for MySQL's own
                    // runtime list (P1 MySQL lifecycle design): `install_mysql`/
                    // `rescan_mysql` write through this after launch, and
                    // `initialize_mysql`/`reset_mysql_root_password`/
                    // `verify_mysql_connection` read it rather than re-probing.
                    app.manage(std::sync::RwLock::new(mysql_runtimes));
                    // Same reasoning again, for MariaDB's own runtime list (P1
                    // MariaDB UI design D7): `install_mariadb`/`rescan_mariadb`
                    // write through this after launch, and
                    // `initialize_mariadb`/`reset_mariadb_root_password`/
                    // `verify_mariadb_connection` read it rather than re-probing.
                    app.manage(std::sync::RwLock::new(mariadb_runtimes));
                    let mut rx = supervisor.subscribe();
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        loop {
                            match rx.recv().await {
                                Ok(SupervisorEvent::StateChanged { id, state, detail }) => {
                                    let _ =
                                        crate::commands::ServiceStateEvent { id, state, detail }
                                            .emit(&handle);
                                }
                                Ok(SupervisorEvent::Log {
                                    id,
                                    ts_ms,
                                    level,
                                    line,
                                }) => {
                                    let _ = crate::commands::ServiceLogEvent {
                                        id,
                                        ts_ms,
                                        level,
                                        line,
                                    }
                                    .emit(&handle);
                                }
                                Ok(SupervisorEvent::Registered { status }) => {
                                    let _ = crate::commands::ServiceRegisteredEvent { status }
                                        .emit(&handle);
                                }
                                Ok(SupervisorEvent::Unregistered { id }) => {
                                    let _ = crate::commands::ServiceUnregisteredEvent { id }
                                        .emit(&handle);
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    continue;
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                    // Best-effort, like the state.db open above: a
                    // menu-bar tray is a quality-of-life feature, not
                    // a boot-blocking one, so a failure here is
                    // logged and the app continues without it rather
                    // than aborting the whole bootstrap. Gated to
                    // macOS only (P1 tray design, spec D10 — Windows
                    // is out for this slice; see `tray`'s own module
                    // docs for what a Windows-enablement slice still
                    // needs to do). `Arc::clone`, not a move: `Db`
                    // was NOT similarly needed again, but `supervisor`
                    // itself is moved into `app.manage` on the very
                    // next line.
                    //
                    // Built in THIS arm only, which is precisely why
                    // `hides_on_close` exists: a degraded window that hid
                    // itself would have no tray left to bring it back.
                    #[cfg(target_os = "macos")]
                    if let Err(e) = crate::tray::build(app.handle(), Arc::clone(&supervisor)) {
                        eprintln!(
                            "openvhost: failed to build the tray icon ({e}); continuing without it"
                        );
                    }

                    // The local control socket the `openvhost` CLI
                    // connects to (P1 CLI design, spec D1). Bound
                    // INSIDE this arm on purpose: the socket must
                    // exist if and only if a supervisor does, so the
                    // degraded-boot arms below — instance lock held
                    // elsewhere, or no resolvable home — deliberately
                    // do NOT bind, and a CLI meeting no socket
                    // correctly reports "the app is not running"
                    // rather than reaching a second, supervisor-less
                    // instance.
                    //
                    // Best-effort, exactly like the state.db open and
                    // the tray above: a control socket is how a
                    // terminal drives this app, not how the app
                    // works. A bind failure (a stale non-socket file
                    // at the path, an over-long OPENVHOST_HOME, a
                    // non-unix target) is logged and the GUI carries
                    // on.
                    //
                    // `bind` deliberately returns a wrapper around a
                    // STD listener: this function is not running
                    // inside a tokio runtime, and
                    // `tokio::net::UnixListener::bind` panics there.
                    // `serve` — spawned onto tauri's runtime below —
                    // is what converts it. `std::future::pending()`
                    // means "serve for the process lifetime": there
                    // is no orderly-shutdown event, only a quit.
                    //
                    // Which is exactly why the socket's IDENTITY is
                    // managed here, before `serve` consumes the
                    // listener (A1 audit fix). `serve`'s own unlink
                    // sits after a loop this future never lets break,
                    // so it does not run in this app — and a unix
                    // socket is not unlinked when its process exits.
                    // Left behind, the path outlives the app and the
                    // next `openvhost status` gets ECONNREFUSED and
                    // reports "not accepting control connections"
                    // (exit 69) instead of the truthful "not running"
                    // (exit 0). `quit::perform_quit` removes it
                    // through this managed handle, first thing.
                    match openvhost_proc::control::bind(&home) {
                        Ok(listener) => {
                            app.manage(listener.socket());
                            let handler: Arc<dyn openvhost_proc::control::ControlHandler> =
                                Arc::new(crate::control::DesktopHandler::new(
                                    app.handle().clone(),
                                    Arc::clone(&supervisor),
                                ));
                            tauri::async_runtime::spawn(openvhost_proc::control::serve(
                                listener,
                                handler,
                                std::future::pending::<()>(),
                            ));
                        }
                        Err(e) => {
                            eprintln!(
                                "openvhost: control socket unavailable ({e}); the openvhost CLI cannot reach this instance"
                            );
                        }
                    }
                    app.manage(supervisor);
                    BootState::Ready
                }
                Ok(None) => BootState::AlreadyRunning { home },
                Err(e) => BootState::RunDirUnusable {
                    run_dir,
                    reason: e.to_string(),
                },
            }
        }
        Err(e) => {
            // Fail CLOSED (P0-8 merge-gate fix wave C5): no
            // cwd-relative "./run" fallback. A relative run dir would
            // lock/reap against whatever directory the OS happened to
            // launch us from instead of the real OPENVHOST_HOME —
            // silently wrong identity for both the single-instance
            // lock and the orphan registry. Same posture as the
            // lock-contended arm above: skip the supervisor bootstrap
            // entirely rather than proceed on a guessed path.
            BootState::HomeUnresolvable {
                reason: e.to_string(),
            }
        }
    };
    // One `eprintln!`, driven by one pure function, for the same reason
    // `db_state` shares `unavailable_message`: a developer at a terminal and a
    // user at a takeover screen are told about the same condition, and `Ready`
    // printing nothing is the `None` arm rather than a rule to remember.
    if let Some(line) = stderr_line(&boot) {
        eprintln!("openvhost: {line}");
    }
    boot
}

/// How far the boot got — what `+layout.svelte` gates the whole app on.
///
/// Zero-arg: there is nothing to validate, and a caller learns only what the app
/// already renders. `Result` with an error that never occurs, like
/// `state_store_status` and `pending_install`: this is a status read with
/// nothing to fail, and every command on this surface shares the one envelope
/// the frontend's `unwrap` understands.
///
/// **This command answers on every boot path**, which is the whole point — it
/// extracts only [`BootState`], and `lib.rs` manages that outside every arm.
#[tauri::command]
#[specta::specta]
pub async fn boot_status(boot: tauri::State<'_, BootState>) -> Result<BootStatusDto, IpcError> {
    Ok(boot_dto(&boot))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn already_running() -> BootState {
        BootState::AlreadyRunning {
            home: PathBuf::from("/Users/dev/Library/Application Support/OpenVHost"),
        }
    }

    fn run_dir_unusable() -> BootState {
        BootState::RunDirUnusable {
            run_dir: PathBuf::from("/Users/dev/OpenVHost/run"),
            reason: "Permission denied (os error 13)".to_string(),
        }
    }

    fn home_unresolvable() -> BootState {
        BootState::HomeUnresolvable {
            reason: "cannot determine the home directory".to_string(),
        }
    }

    // ---- boot_dto: one screen decision, per variant ------------------------
    //
    // Four tests rather than one table, because a table can only assert that
    // the mapping is *some* function of the state — and the failure this guards
    // is a variant landing on the WRONG screen, which needs each pairing named.
    //
    // VACUITY (neuter-and-watch-it-fail), measured in both directions:
    //
    // - `boot_dto` pinned to `BootStatusDto::Ready` for every input: 6 failed,
    //   13 passed. The three per-variant tests below, plus
    //   `every_state_maps_to_a_distinct_screen`,
    //   `the_wire_shape_is_tagged_camel_case_with_single_word_fields` and
    //   `boot_status_reports_the_degraded_state_it_was_managed_with`.
    //   `a_ready_boot_asks_for_no_screen_at_all` and
    //   `boot_status_is_ready_on_a_healthy_boot` stayed green.
    // - `boot_dto` pinned to an `AlreadyRunning` DTO instead: 8 failed,
    //   including both of the two that survived the first neuter.
    //
    // Neither pinning is caught by the other's survivors, so no one test here
    // stands in for another. Restoring the match made all 19 pass.

    #[test]
    fn a_ready_boot_asks_for_no_screen_at_all() {
        assert_eq!(boot_dto(&BootState::Ready), BootStatusDto::Ready);
    }

    #[test]
    fn a_contended_lock_reports_the_home_it_would_not_take_over() {
        assert_eq!(
            boot_dto(&already_running()),
            BootStatusDto::AlreadyRunning {
                home: "/Users/dev/Library/Application Support/OpenVHost".to_string(),
            },
            "the screen names the contended home, which is the only thing that \
             distinguishes two copies of the bundle at different paths"
        );
    }

    #[test]
    fn an_unusable_run_dir_reports_the_path_and_the_os_error_verbatim() {
        assert_eq!(
            boot_dto(&run_dir_unusable()),
            BootStatusDto::RunDirUnusable {
                path: "/Users/dev/OpenVHost/run".to_string(),
                reason: "Permission denied (os error 13)".to_string(),
            },
            "this is the one user-FIXABLE state, so the path and the errno are \
             the payload — summarising either makes it unfixable"
        );
    }

    #[test]
    fn an_unresolvable_home_reports_the_reason_and_no_path() {
        assert_eq!(
            boot_dto(&home_unresolvable()),
            BootStatusDto::HomeUnresolvable {
                reason: "cannot determine the home directory".to_string(),
            },
            "there is no resolved path to name here, and inventing one would be \
             the plausible lie this design exists to avoid"
        );
    }

    /// No two states collapse onto one screen — in **both** directions.
    ///
    /// `boot_dto`'s own match is exhaustive with no wildcard, so a fifth
    /// [`BootState`] fails to compile there; the match below is exhaustive with
    /// no wildcard over [`BootStatusDto`], so a fifth wire variant fails to
    /// compile here.
    ///
    /// **Both measured, not assumed.** Adding a fifth `BootState` variant
    /// produced E0004 (non-exhaustive patterns) at *two* sites — [`boot_dto`]
    /// and [`stderr_line`] — so a new state must be given both a screen and a
    /// terminal line, not just one. Adding a fifth `BootStatusDto` variant
    /// produced the same error against the match below, failing the lib-test
    /// build.
    #[test]
    fn every_state_maps_to_a_distinct_screen() {
        let tags: Vec<&'static str> = [
            BootState::Ready,
            already_running(),
            run_dir_unusable(),
            home_unresolvable(),
        ]
        .iter()
        .map(|state| match boot_dto(state) {
            BootStatusDto::Ready => "ready",
            BootStatusDto::AlreadyRunning { .. } => "alreadyRunning",
            BootStatusDto::RunDirUnusable { .. } => "runDirUnusable",
            BootStatusDto::HomeUnresolvable { .. } => "homeUnresolvable",
        })
        .collect();

        let mut unique = tags.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            tags.len(),
            "two boot states rendered the same screen: {tags:?}"
        );
    }

    /// The wire shape T2 builds against, pinned where a rename would be caught.
    ///
    /// **`rename_all` on an enum renames its VARIANTS, not its fields** — a trap
    /// this repo has already paid for once. So the tag is asserted camelCase and
    /// the fields are asserted to be exactly what they are declared as; if a
    /// future edit reaches for `run_dir` in the DTO, this fails rather than the
    /// frontend silently reading `undefined`.
    #[test]
    fn the_wire_shape_is_tagged_camel_case_with_single_word_fields() {
        let json = serde_json::to_value(boot_dto(&run_dir_unusable())).unwrap();
        assert_eq!(json["kind"], "runDirUnusable");
        assert_eq!(json["path"], "/Users/dev/OpenVHost/run");
        assert_eq!(json["reason"], "Permission denied (os error 13)");

        let ready = serde_json::to_value(boot_dto(&BootState::Ready)).unwrap();
        assert_eq!(ready["kind"], "ready");
    }

    // ---- stderr_line: the developer at a terminal --------------------------
    //
    // VACUITY, measured: `stderr_line` pinned to `None` failed the three
    // degraded tests on their `expect` and left `a_healthy_boot_prints_nothing`
    // green (3 failed, 16 passed). Pinned to `Some("x")` instead, all four
    // failed (4 failed, 15 passed) — the healthy one among them, which is the
    // case the first neuter could not reach. Neither direction is covered by
    // the other.

    #[test]
    fn a_healthy_boot_prints_nothing() {
        assert_eq!(
            stderr_line(&BootState::Ready),
            None,
            "a working launch must not log a bail line — the `if let Some` at \
             the call site is what makes that structural"
        );
    }

    #[test]
    fn a_contended_lock_still_prints_the_line_it_always_did() {
        assert_eq!(
            stderr_line(&already_running()).expect("a degraded boot must say so"),
            "another instance holds the run lock; not starting the supervisor"
        );
    }

    #[test]
    fn an_unusable_run_dir_still_prints_the_line_it_always_did() {
        assert_eq!(
            stderr_line(&run_dir_unusable()).expect("a degraded boot must say so"),
            "failed to acquire the run lock: Permission denied (os error 13)"
        );
    }

    #[test]
    fn an_unresolvable_home_still_prints_the_line_it_always_did() {
        assert_eq!(
            stderr_line(&home_unresolvable()).expect("a degraded boot must say so"),
            "cannot resolve OPENVHOST_HOME (cannot determine the home directory); \
             not starting the supervisor"
        );
    }

    // ---- hides_on_close: D6 --------------------------------------------------
    //
    // VACUITY, measured: `hides_on_close` pinned to `true` failed
    // `a_degraded_app_lets_the_close_proceed_rather_than_hiding_a_zombie` and
    // `an_unmanaged_boot_state_fails_closed_and_does_not_hide` (2 failed, 17
    // passed). Pinned to `false`, only `a_ready_app_hides_the_window_on_close`
    // failed (1 failed, 18 passed). The two groups cannot substitute for each
    // other.

    #[test]
    fn a_ready_app_hides_the_window_on_close() {
        assert!(hides_on_close(Some(&BootState::Ready)));
    }

    #[test]
    fn a_degraded_app_lets_the_close_proceed_rather_than_hiding_a_zombie() {
        for state in [already_running(), run_dir_unusable(), home_unresolvable()] {
            assert!(
                !hides_on_close(Some(&state)),
                "the tray is built in the Ready arm ONLY, so hiding {state:?} \
                 would leave a hidden window nothing can bring back"
            );
        }
    }

    #[test]
    fn an_unmanaged_boot_state_fails_closed_and_does_not_hide() {
        assert!(
            !hides_on_close(None),
            "every try_state read in this app fails closed; letting the close \
             proceed is the non-trapping direction to be wrong in"
        );
    }

    // ---- acquire_with_one_retry: D5 ----------------------------------------
    //
    // `a_genuinely_held_lock_is_still_reported_held` is the one that matters: a
    // retry that always eventually succeeded would silently defeat
    // single-instance protection, so a real held lock must survive it. It uses
    // the real `InstanceLock`; the other three drive the helper with fakes,
    // because the transient `Ok(None)` the fork/exec window produces cannot be
    // staged on demand with a real one.
    //
    // VACUITY, measured in both directions:
    //
    // - Retry deleted (body reduced to `acquire()`): 2 failed —
    //   `…_is_retried_once_and_the_lock_is_taken`, and
    //   `a_genuinely_held_lock_is_still_reported_held` on its `tries` count.
    // - Retry turned into "keep going until it stops saying `Ok(None)`" (20
    //   attempts): `a_genuinely_held_lock_is_still_reported_held` failed with
    //   `left: 21, right: 2`.
    //
    // Note honestly WHICH assertion caught the second one: the `tries` count,
    // not `second.is_none()`. Against a genuinely held flock no number of
    // retries can succeed, and this helper **cannot fabricate a lock** — `T` is
    // opaque to it, so the only thing it can return is what `acquire` returned.
    // That half is a type-level property; the test pins how many times we ask.

    #[test]
    fn an_uncontended_lock_is_taken_on_the_first_try_without_waiting() {
        let mut tries = 0;
        let mut waited = false;
        let taken: Result<Option<&str>, std::io::Error> = acquire_with_one_retry(
            || {
                tries += 1;
                Ok(Some("lock"))
            },
            || waited = true,
        );

        assert_eq!(taken.unwrap(), Some("lock"));
        assert_eq!(tries, 1, "an ordinary launch must not acquire twice");
        assert!(
            !waited,
            "an ordinary launch must not pay the retry delay — every user's \
             launch would be that much slower"
        );
    }

    #[test]
    fn a_spurious_contention_is_retried_once_and_the_lock_is_taken() {
        // Exactly what the fork/exec window produces: the lock reads held, then
        // moments later it does not (`lock.rs:147-149`).
        let mut tries = 0;
        let mut waited = false;
        let taken: Result<Option<&str>, std::io::Error> = acquire_with_one_retry(
            || {
                tries += 1;
                if tries == 1 {
                    Ok(None)
                } else {
                    Ok(Some("lock"))
                }
            },
            || waited = true,
        );

        assert_eq!(
            taken.unwrap(),
            Some("lock"),
            "a transient contention must not become a takeover screen that \
             disappears on the next try"
        );
        assert_eq!(tries, 2);
        assert!(waited, "the retry must wait, not spin");
    }

    #[test]
    fn a_genuinely_held_lock_is_still_reported_held() {
        // The real `InstanceLock`, not a fake: this is the half that must not
        // regress. flock is scoped to the open file description, so a second
        // open in this same process contends exactly as another process would.
        let home = tempfile::tempdir().unwrap();
        let run_dir = home.path().join("run");
        let held = InstanceLock::acquire(&run_dir)
            .unwrap()
            .expect("the first instance takes the lock");

        let mut tries = 0;
        let second = acquire_with_one_retry(
            || {
                tries += 1;
                InstanceLock::acquire(&run_dir)
            },
            || std::thread::sleep(Duration::from_millis(1)),
        )
        .unwrap();

        assert!(
            second.is_none(),
            "expected a held lock to stay held — a retry that eventually \
             succeeds would silently defeat single-instance protection"
        );
        assert_eq!(tries, 2, "the retry must actually have been attempted");
        // The second instance must never acquire OR release the first's lock
        // (design D4). Dropping `held` last is what proves nothing above took
        // it away: `probe` still reads it as held while it is alive.
        assert_eq!(
            openvhost_proc::SupervisorPresence::Present,
            InstanceLock::probe(&run_dir),
            "the first instance's lock must survive the second's attempts"
        );
        drop(held);
    }

    #[test]
    fn an_acquire_error_is_reported_rather_than_retried() {
        let mut tries = 0;
        let mut waited = false;
        let failed: Result<Option<&str>, std::io::Error> = acquire_with_one_retry(
            || {
                tries += 1;
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            },
            || waited = true,
        );

        assert!(failed.is_err(), "an unusable run dir is a third answer");
        assert_eq!(
            tries, 1,
            "the documented race makes flock report the lock HELD, never fail — \
             retrying an error would delay every unusable run dir for nothing"
        );
        assert!(!waited);
    }

    // ---- boot_status: the command behind the takeover screen ---------------
    //
    // Both directions, because a status command that answers the same thing
    // either way is worse than none: the takeover would be permanently on (a
    // blank app on every healthy machine) or permanently off (which is the
    // silence this slice exists to break).
    //
    // VACUITY: covered by the two `boot_dto` neuters recorded at the top of
    // this module, since this command is a one-line delegation to it. Pinning
    // `boot_dto` to `Ready` reddened `…_reports_the_degraded_state…` and left
    // `…_is_ready_on_a_healthy_boot` green; pinning it to the `AlreadyRunning`
    // DTO reddened the other.

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap()
    }

    #[tokio::test]
    async fn boot_status_is_ready_on_a_healthy_boot() {
        let app = mock_app();
        app.manage(BootState::Ready);

        assert_eq!(
            boot_status(app.state::<BootState>()).await.unwrap(),
            BootStatusDto::Ready
        );
    }

    #[tokio::test]
    async fn boot_status_reports_the_degraded_state_it_was_managed_with() {
        let app = mock_app();
        app.manage(run_dir_unusable());

        assert_eq!(
            boot_status(app.state::<BootState>()).await.unwrap(),
            BootStatusDto::RunDirUnusable {
                path: "/Users/dev/OpenVHost/run".to_string(),
                reason: "Permission denied (os error 13)".to_string(),
            },
            "the command must carry the managed state's own facts, not a \
             generic 'something went wrong'"
        );
    }
}
