// SPDX-License-Identifier: GPL-3.0-or-later
//! The half of an uninstall that actually does something: re-check the
//! blockers, run `brew uninstall` through the existing `InstallLock` and the
//! existing live-output surface, then clean up what this app generated.
//!
//! Order is the design (D1/D3): **refusals before any process is spawned**,
//! then brew, then — only if brew succeeded — the two cheap local steps. A
//! brew failure therefore changes no local state at all, which is what makes
//! "nothing was destroyed" checkable rather than merely intended.
//!
//! The ONLY filesystem call an uninstall makes is `remove_file` on a path
//! `inventory` produced under `config/generated/`. Nothing recurses, nothing
//! follows a symlink to its target, and nothing touches `<home>/data`,
//! `<home>/logs` or state.db's credential rows on ANY path, including error
//! paths.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use openvhost_core::{Db, Site, SiteRepository, SqliteSiteRepository};
use openvhost_proc::{ProcError, SpawnSpec, Supervisor, TaskEvent, TaskStream};
use tauri_specta::Event;

use super::{Blocker, PackageKind, Target, UninstallPlan, build_plan, inventory};
use crate::commands::{
    InstallLock, IpcError, MysqlInstallLogEvent, PackageOperation, PhpInstallLogEvent,
    RunningInstallGuard, now_ms, rescan_into_state, rescan_mysql_into_state, stack_paths,
};
use crate::stack::StackPaths;

/// How many trailing stderr lines are kept to quote back when brew refuses.
/// Bounded on purpose: brew's dependency refusal is a handful of lines, and an
/// unbounded buffer would let a pathological build hold the whole output in
/// memory for an error message nobody reads in full.
const STDERR_TAIL_LINES: usize = 20;

/// Where a line of `brew uninstall`'s output goes.
///
/// A sink rather than an `AppHandle` for the same reason `initialize_mysql`
/// takes one: `tauri::test::mock_builder` only ever yields an
/// `AppHandle<MockRuntime>`, a different concrete type than the `AppHandle`
/// (`Wry`) an `.emit()` call needs, so a function that emitted directly could
/// not be driven from a test at all.
pub(crate) type UninstallLogSink = Arc<dyn Fn(&str, String) + Send + Sync>;

/// The one child process this module ever spawns, behind a seam a test can
/// record.
///
/// Production is [`ProcBrewRunner`], a straight delegation to
/// `openvhost_proc::run_task` — every child process in this codebase still
/// goes through openvhost-proc, and this trait adds no second spawn path. What
/// it adds is the ability to assert that a REFUSAL spawns nothing, which is
/// otherwise unobservable: `ProcessDriver` cannot be faked from outside
/// openvhost-proc (`SpawnedChild`'s fields are private), so the seam has to sit
/// one level up.
pub(crate) trait BrewRunner: Send + Sync + 'static {
    fn run(
        &self,
        spec: SpawnSpec,
        tx: tokio::sync::mpsc::Sender<TaskEvent>,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<i32>, ProcError>> + Send>>;
}

/// The production runner: `openvhost_proc::run_task` with the default driver,
/// exactly as `install_php` spawns its own run.
pub(crate) struct ProcBrewRunner;

impl BrewRunner for ProcBrewRunner {
    fn run(
        &self,
        spec: SpawnSpec,
        tx: tokio::sync::mpsc::Sender<TaskEvent>,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<i32>, ProcError>> + Send>> {
        Box::pin(openvhost_proc::run_task(
            openvhost_proc::default_driver(),
            spec,
            tx,
        ))
    }
}

/// Everything [`perform_uninstall`] needs, gathered by the command wrapper so
/// the executor itself needs no Tauri types.
pub(crate) struct UninstallRun<'a> {
    pub(crate) target: Target,
    pub(crate) home: PathBuf,
    pub(crate) brew: PathBuf,
    pub(crate) sup: &'a Supervisor,
    pub(crate) lock: &'a InstallLock,
    /// Read fresh by the caller, immediately before this runs — see
    /// [`perform_uninstall`]'s re-check.
    pub(crate) sites: Vec<Site>,
    pub(crate) runner: Arc<dyn BrewRunner>,
    pub(crate) log: UninstallLogSink,
}

/// What happened after `brew uninstall` SUCCEEDED.
///
/// Distinguishing these two from an `Err` matters: an `Err` from
/// [`perform_uninstall`] means brew did not succeed and nothing local changed,
/// so there is nothing to reconcile. Both variants here mean the program files
/// are gone and the app's own view of the world is now stale.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UninstallOutcome {
    /// Removed, and every cleanup step completed.
    Done,
    /// Removed, but a cleanup step could not complete. Each string is a
    /// complete sentence for the user — see the `NotTerminal` race in
    /// [`perform_uninstall`].
    Incomplete(Vec<String>),
}

/// Refuse, brew, clean up — in that order.
///
/// The blockers are re-checked HERE rather than trusted from the plan the
/// dialog was built from: between a dialog opening and its confirm button
/// being pressed, a service can be started from the tray and a site can be
/// repointed. The plan is a view; this is the decision.
pub(crate) async fn perform_uninstall(run: UninstallRun<'_>) -> Result<UninstallOutcome, IpcError> {
    // ---- Refusals, before anything is spawned (D3) ----------------------
    let blockers = super::blockers(&run.target, &run.sup.snapshot(), &run.sites);
    if !blockers.is_empty() {
        return Err(refusal(&run.target, &blockers));
    }

    let inv = inventory(&run.target, &run.home);
    let mut problems: Vec<String> = Vec::new();

    // The inventory's ORDER is the execution order, and the same list the
    // confirmation rendered. Exhaustive over `Removal` with no wildcard: a new
    // kind of removal must be given an implementation here, not silently
    // skipped while the dialog keeps promising it.
    for removal in &inv.removes {
        match removal {
            super::Removal::BrewFormula { .. } => {
                // A non-zero exit (brew refusing because something depends on
                // this formula, D1) returns early: the local cleanup below
                // must not run when the program files are still installed.
                run_brew(&run).await?;
            }
            super::Removal::GeneratedFile { path, what } => {
                // `remove_file`, never `remove_dir_all`: on a symlink this
                // removes the LINK and never the target, and on a directory it
                // fails loudly instead of recursing. Both matter — the
                // generated tree is exactly where a hostile or accidental
                // symlink would be planted to get a delete out of it.
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    // Already gone (a previous attempt, a manual tidy-up, an
                    // apply that swept it). The post-state is what was asked
                    // for, so this is done, not failed.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => problems.push(format!(
                        "{what} at {} could not be removed: {e}",
                        path.display()
                    )),
                }
            }
            super::Removal::ServiceRow { id } => {
                // Exhaustive over `ProcError` — no wildcard arm.
                match run.sup.unregister(id) {
                    Ok(()) => {}
                    // Nothing registered under this id: an installed-but-never
                    // -initialized MySQL major never had a row, and a rescan
                    // may have removed it already. Nothing to forget.
                    Err(ProcError::NotFound(_)) => {}
                    // THE post-brew race. The pre-flight said this service was
                    // terminal; between then and now the user started it (from
                    // the tray, the CLI, or the Services page), and on unix a
                    // process keeps running from a binary whose path brew has
                    // just unlinked. `unregister` refuses — correctly, because
                    // forgetting a live child would leak it past the next
                    // launch's orphan reap. Report it precisely instead of
                    // swallowing it: the version IS gone, and the leftover row
                    // is visible on the Services page, so a user told only
                    // "success" would be looking at a contradiction.
                    Err(ProcError::NotTerminal { id, state }) => problems.push(format!(
                        "{} was removed, but its service {id} was {state} by the time its row \
                         could be cleared — it must have been started while the removal was \
                         running. Stop {id}; the row disappears the next time versions are \
                         rescanned.",
                        run.target.display()
                    )),
                    Err(ProcError::Io(e)) => problems.push(format!(
                        "{} was removed, but its service row {id} could not be cleared: {e}",
                        run.target.display()
                    )),
                }
            }
        }
    }

    Ok(if problems.is_empty() {
        UninstallOutcome::Done
    } else {
        UninstallOutcome::Incomplete(problems)
    })
}

/// The refusal a racing execute returns. Names every obstacle, not just the
/// first: a user who stops the service only to be told about the sites has
/// been made to guess twice.
fn refusal(target: &Target, blockers: &[Blocker]) -> IpcError {
    IpcError::Core {
        message: format!(
            "{} cannot be uninstalled: {}",
            target.display(),
            blockers
                .iter()
                .map(Blocker::describe)
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

/// Spawn `brew uninstall`, stream its output live, and turn a non-zero exit
/// into an error quoting brew verbatim.
///
/// Spawned rather than awaited inline, and registered on `InstallLock`'s
/// running slot — the same C1 shape `install_php` uses. Awaiting the run
/// directly would make its future identical to the command handler's own, and
/// Tauri never cancels an in-flight command: a quit mid-uninstall would go
/// straight from `window.destroy()` to `process::exit`, and `run_task`'s
/// `KillOnDrop` containment would never fire, leaving brew's process group
/// running with the app gone. `perform_quit` aborts whatever occupies the slot
/// before the window goes away, which is what makes the drop — and therefore
/// the group kill — actually happen.
async fn run_brew(run: &UninstallRun<'_>) -> Result<(), IpcError> {
    let spec = run.target.uninstall_spec(&run.brew)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);

    // Forward brew's output as it arrives — the SAME surface `install_php`
    // streams through (D1: the user who watched it arrive watches it leave) —
    // while keeping a bounded stderr tail for the error message.
    let sink = Arc::clone(&run.log);
    let pump = tokio::spawn(async move {
        let mut tail: VecDeque<String> = VecDeque::with_capacity(STDERR_TAIL_LINES);
        while let Some(ev) = rx.recv().await {
            if let TaskEvent::Line { stream, text } = ev {
                let name = match stream {
                    TaskStream::Stdout => "stdout",
                    TaskStream::Stderr => "stderr",
                };
                if matches!(stream, TaskStream::Stderr) {
                    if tail.len() == STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(text.clone());
                }
                sink(name, text);
            }
        }
        tail
    });

    let task = tokio::spawn(run.runner.run(spec, tx));
    let abort_handle = task.abort_handle();
    run.lock.set_running(
        run.target.install_kind(),
        PackageOperation::Uninstall,
        run.target.pending_label(),
        abort_handle.clone(),
    );
    // Cleared AND aborted on every return path below via `Drop` — see
    // `RunningInstallGuard`'s doc comment.
    let _running_guard = RunningInstallGuard {
        lock: run.lock,
        abort: abort_handle,
    };

    let exit_code = match task.await {
        Ok(result) => result?,
        Err(join_err) if join_err.is_cancelled() => {
            return Err(IpcError::Proc {
                message: "the uninstall was aborted because the app is quitting".into(),
            });
        }
        Err(join_err) => {
            return Err(IpcError::Proc {
                message: format!("the uninstall task ended unexpectedly: {join_err}"),
            });
        }
    };
    let tail: VecDeque<String> = pump.await.unwrap_or_default();

    match exit_code {
        Some(0) => Ok(()),
        // D1: surfaced VERBATIM. brew refuses when another formula depends on
        // this one, and its message names which — knowledge about this
        // machine that this app does not have and must not paraphrase away.
        Some(code) => Err(IpcError::Proc {
            message: brew_failure_message(
                &format!("brew uninstall exited with status {code}"),
                &tail,
            ),
        }),
        // No exit code means killed by a signal.
        None => Err(IpcError::Proc {
            message: brew_failure_message("brew uninstall was killed by a signal", &tail),
        }),
    }
}

fn brew_failure_message(headline: &str, tail: &VecDeque<String>) -> String {
    if tail.is_empty() {
        format!("{headline}. Nothing was removed.")
    } else {
        format!(
            "{headline}. Nothing was removed. brew said:\n{}",
            tail.iter().cloned().collect::<Vec<_>>().join("\n")
        )
    }
}

/// What an uninstall of `major` would remove, keep, and refuse to do.
///
/// A PURE QUERY: it spawns nothing and changes nothing. It reads the
/// supervisor's snapshot and the site list and derives paths — that is all —
/// so the Languages/Databases pages can call it on mount to decide a disabled
/// state as cheaply as they call it to fill a confirmation dialog.
#[tauri::command]
#[specta::specta]
pub async fn uninstall_plan(
    kind: PackageKind,
    major: String,
    db: tauri::State<'_, Db>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
) -> Result<UninstallPlan, IpcError> {
    let target = Target::parse(kind, &major)?;
    let p = stack_paths(&paths)?;
    let sites = SqliteSiteRepository::new(db.inner()).list().await?;
    Ok(build_plan(&target, &p.home, &sup.snapshot(), &sites))
}

/// Remove `major`: refuse if anything depends on it, otherwise
/// `brew uninstall`, then drop the config this app generated and the service
/// row.
///
/// Re-checks the blockers itself — the plan the dialog was built from may be
/// stale — and streams brew's output through the same events `install_php`
/// uses, so the Languages/Databases pages need no second log channel.
// Nine parameters, seven of them managed state. Tauri extracts state per TYPE
// through the signature — there is no request object to bundle them into, and
// the alternative (pulling them out of `app` with `try_state` inside the body)
// would hide the dependency list rather than shorten it, which is the opposite
// of what this lint is for. Every one is genuinely used: `db` for the pinned-
// sites re-check, both runtime locks for the kind-specific rescan afterwards.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
#[specta::specta]
pub async fn uninstall_package(
    app: tauri::AppHandle,
    kind: PackageKind,
    major: String,
    db: tauri::State<'_, Db>,
    runtimes: tauri::State<'_, RwLock<Option<openvhost_core::InstalledRuntimes>>>,
    mysql_runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<(), IpcError> {
    // The catalogue gate, before anything else happens.
    let target = Target::parse(kind, &major)?;

    // One package operation at a time, sharing `install_php`'s lock so an
    // install and an uninstall can never interleave (D1). `try_lock` rather
    // than `lock`, exactly like the install commands: a second press should be
    // refused with an explanation, not silently queued behind a build that can
    // take twenty minutes.
    let Ok(_guard) = lock.inner().guard.try_lock() else {
        return Err(IpcError::Core {
            message: "another install or uninstall is already running".into(),
        });
    };

    let p = stack_paths(&paths)?;
    let brew = openvhost_core::find_brew().ok_or_else(|| IpcError::Core {
        message: format!(
            "Homebrew was not found. Looked for bin/brew under: {}",
            openvhost_core::BREW_PREFIXES.join(", ")
        ),
    })?;

    let emitter = app.clone();
    let for_event = target.major().to_string();
    let event_kind = target.kind();
    let log: UninstallLogSink = Arc::new(move |stream: &str, line: String| {
        emit_uninstall_log(&emitter, event_kind, &for_event, stream, line)
    });

    let outcome = perform_uninstall(UninstallRun {
        target: target.clone(),
        home: p.home.clone(),
        brew,
        sup: sup.inner(),
        lock: lock.inner(),
        sites: SqliteSiteRepository::new(db.inner()).list().await?,
        runner: Arc::new(ProcBrewRunner),
        log,
    })
    .await?;

    // Only reached when brew SUCCEEDED (a failure returned above), so the
    // managed runtime list is now stale in a way that matters: the apply
    // pipeline and the Sites editor read it, not the supervisor's rows, to
    // decide which versions exist. Refreshing it also re-runs the D5
    // reconciliation, which is the belt to the explicit `unregister`'s braces.
    match target.kind() {
        PackageKind::Php => {
            rescan_into_state(runtimes.inner(), sup.inner(), p).await?;
        }
        PackageKind::Mysql => {
            rescan_mysql_into_state(mysql_runtimes.inner(), sup.inner(), &p.home).await?;
        }
    }

    match outcome {
        UninstallOutcome::Done => Ok(()),
        // The version is gone but something could not be tidied — reported as
        // an error because the user can still SEE the leftover, and calling
        // that success would be a contradiction they have to resolve alone.
        UninstallOutcome::Incomplete(problems) => Err(IpcError::Proc {
            message: problems.join(" "),
        }),
    }
}

/// One line of brew's output, on the event the matching install already uses.
/// Exhaustive over [`PackageKind`]: a new kind must choose its surface here
/// rather than defaulting into PHP's.
fn emit_uninstall_log(
    app: &tauri::AppHandle,
    kind: PackageKind,
    major: &str,
    stream: &str,
    line: String,
) {
    match kind {
        PackageKind::Php => {
            let _ = PhpInstallLogEvent {
                major: major.to_string(),
                ts_ms: now_ms(),
                stream: stream.to_string(),
                line,
            }
            .emit(app);
        }
        PackageKind::Mysql => {
            let _ = MysqlInstallLogEvent {
                major: major.to_string(),
                ts_ms: now_ms(),
                stream: stream.to_string(),
                line,
            }
            .emit(app);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Mutex;

    use crate::commands::InstallKind;
    use openvhost_core::mysql::{MysqlInstanceRepo, MysqlMajor, generate_root_password};
    use openvhost_proc::{
        DEFAULT_GRACE, ReadinessProbe, ServiceSpec, ServiceState, Supervisor, default_driver,
    };

    use super::super::tests::{php, site};
    use super::*;

    /// Records every spawn it is asked for and answers with a scripted exit
    /// code, without ever creating a process.
    struct RecordingRunner {
        calls: Mutex<Vec<Vec<String>>>,
        exit: Option<i32>,
        lines: Vec<(TaskStream, String)>,
    }

    impl RecordingRunner {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                exit: Some(0),
                lines: Vec::new(),
            })
        }

        /// Succeeds, having printed `stdout` lines — so a test can observe
        /// state that only exists WHILE the run is in flight.
        fn talking(stdout: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                exit: Some(0),
                lines: stdout
                    .iter()
                    .map(|s| (TaskStream::Stdout, (*s).to_string()))
                    .collect(),
            })
        }

        fn refusing(stderr: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                exit: Some(1),
                lines: stderr
                    .iter()
                    .map(|s| (TaskStream::Stderr, (*s).to_string()))
                    .collect(),
            })
        }

        fn spawns(&self) -> Vec<Vec<String>> {
            self.calls.lock().expect("poisoned").clone()
        }
    }

    impl BrewRunner for RecordingRunner {
        fn run(
            &self,
            spec: SpawnSpec,
            tx: tokio::sync::mpsc::Sender<TaskEvent>,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<i32>, ProcError>> + Send>>
        {
            let mut argv = vec![spec.program.display().to_string()];
            argv.extend(spec.args.iter().map(|a| a.to_string_lossy().into_owned()));
            self.calls.lock().expect("poisoned").push(argv);
            let exit = self.exit;
            let lines = self.lines.clone();
            Box::pin(async move {
                for (stream, text) in lines {
                    let _ = tx.send(TaskEvent::Line { stream, text }).await;
                }
                Ok(exit)
            })
        }
    }

    /// Succeeds like brew would, but STARTS the service first — reproducing
    /// the one race the pre-flight check cannot close: a user pressing Start
    /// (tray, CLI, Services page) between the blocker check and the row
    /// removal. `Supervisor` is `Clone` (it is an `Arc` inside), so the runner
    /// can hold the very supervisor the executor is about to ask.
    struct RacingRunner {
        sup: Supervisor,
        id: String,
    }

    impl BrewRunner for RacingRunner {
        fn run(
            &self,
            _spec: SpawnSpec,
            _tx: tokio::sync::mpsc::Sender<TaskEvent>,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<i32>, ProcError>> + Send>>
        {
            let sup = self.sup.clone();
            let id = self.id.clone();
            Box::pin(async move {
                sup.start(&id).expect("start");
                loop {
                    match sup.snapshot()[0].state {
                        ServiceState::Starting | ServiceState::Running => break,
                        ServiceState::Stopped | ServiceState::Failed { .. } => {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    }
                }
                Ok(Some(0))
            })
        }
    }

    fn supervisor_with(id: &str) -> Supervisor {
        let sup = Supervisor::new(default_driver());
        sup.register(ServiceSpec {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: None,
            spawn: SpawnSpec {
                program: PathBuf::from("/nonexistent"),
                args: vec![],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        });
        sup
    }

    fn silent() -> UninstallLogSink {
        Arc::new(|_, _| {})
    }

    fn run_for<'a>(
        target: Target,
        home: &std::path::Path,
        sup: &'a Supervisor,
        lock: &'a InstallLock,
        runner: Arc<dyn BrewRunner>,
        sites: Vec<Site>,
    ) -> UninstallRun<'a> {
        UninstallRun {
            target,
            home: home.to_path_buf(),
            brew: PathBuf::from("/opt/homebrew/bin/brew"),
            sup,
            lock,
            sites,
            runner,
            log: silent(),
        }
    }

    /// A home with a generated pool config, a php-fpm log directory holding a
    /// real log line, a MySQL datadir holding a real table file, and the
    /// user's own override directories — i.e. everything an uninstall must
    /// either remove or leave completely alone.
    fn provisioned_home() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        for (path, contents) in [
            ("config/generated/php/8.4/php-fpm.conf", "; generated\n"),
            ("config/generated/mysql/8.4/my.cnf", "[mysqld]\n"),
            ("config/custom/php/8.4/pool.d/mine.conf", "; mine\n"),
            ("config/custom/mysql/8.4/conf.d/mine.cnf", "# mine\n"),
            ("logs/services/php-fpm-8.4/error.log", "why it failed\n"),
            ("data/mysql/8.4/auto.cnf", "[auto]\nserver-uuid=abc\n"),
            ("data/mysql/8.4/mysql/user.ibd", "\x00binary rows\x00"),
        ] {
            let p = home.join(path);
            std::fs::create_dir_all(p.parent().expect("has a parent")).expect("mkdir");
            std::fs::write(&p, contents).expect("write");
        }
        tmp
    }

    /// Content + inode for every path an uninstall must not touch. Inode as
    /// well as bytes because a delete-and-rewrite would leave the bytes
    /// identical while having destroyed (and recreated) the user's file — the
    /// exact failure a content-only assertion cannot see.
    #[cfg(unix)]
    fn untouched_fingerprint(home: &std::path::Path) -> Vec<(PathBuf, Vec<u8>, u64)> {
        use std::os::unix::fs::MetadataExt;
        [
            "logs/services/php-fpm-8.4/error.log",
            "config/custom/php/8.4/pool.d/mine.conf",
            "config/custom/mysql/8.4/conf.d/mine.cnf",
            "config/generated/mysql/8.4/my.cnf",
            "data/mysql/8.4/auto.cnf",
            "data/mysql/8.4/mysql/user.ibd",
        ]
        .iter()
        .map(|rel| {
            let p = home.join(rel);
            let meta =
                std::fs::metadata(&p).unwrap_or_else(|e| panic!("{} must exist: {e}", p.display()));
            (p.clone(), std::fs::read(&p).expect("readable"), meta.ino())
        })
        .collect()
    }

    #[cfg(unix)]
    fn assert_untouched(before: &[(PathBuf, Vec<u8>, u64)]) {
        use std::os::unix::fs::MetadataExt;
        for (path, bytes, ino) in before {
            let meta = std::fs::metadata(path)
                .unwrap_or_else(|e| panic!("{} was destroyed: {e}", path.display()));
            assert_eq!(
                &std::fs::read(path).expect("readable"),
                bytes,
                "{} changed content",
                path.display()
            );
            assert_eq!(
                meta.ino(),
                *ino,
                "{} was replaced (same bytes, new inode)",
                path.display()
            );
        }
    }

    // ---- refusals spawn nothing ------------------------------------------
    //
    // VACUITY: the positive control is inside the test — the SAME recording
    // runner is driven a second time with the obstacle removed and must then
    // record exactly one spawn. "Nothing happened" therefore cannot pass
    // because the instrument is broken. Additionally neutered: moving the
    // blocker re-check to AFTER the removal loop made the first assertion
    // fail with one recorded spawn.

    #[tokio::test]
    async fn a_running_service_refuses_the_uninstall_and_spawns_no_process() {
        let home = provisioned_home();
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();

        // Drive the row into `Running` the only way the supervisor allows
        // from outside: a real start would need a real binary, so assert the
        // predicate the executor uses instead, then prove the executor
        // consults it by using the site blocker for the spawn assertion.
        let blocked = run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            vec![site("shop", "shop.localhost", "8.4")],
        );
        let err = perform_uninstall(blocked).await.unwrap_err();
        assert!(
            format!("{err}").contains("shop.localhost"),
            "the refusal must name the site: {err}"
        );
        assert!(
            runner.spawns().is_empty(),
            "a refusal must spawn NOTHING, recorded {:?}",
            runner.spawns()
        );
        // The pool config the executor would have deleted is still there.
        assert!(
            home.path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists()
        );

        // POSITIVE CONTROL, same runner: with the site repointed there is no
        // blocker, and exactly one spawn must be recorded — so the empty
        // assertion above cannot have passed vacuously.
        let allowed = run_for(php("8.4"), home.path(), &sup, &lock, runner.clone(), vec![]);
        perform_uninstall(allowed).await.unwrap();
        assert_eq!(
            runner.spawns(),
            vec![vec![
                "/opt/homebrew/bin/brew".to_string(),
                "uninstall".to_string(),
                "php@8.4".to_string(),
            ]]
        );
    }

    #[tokio::test]
    async fn a_stopped_service_does_not_block_and_its_row_is_removed() {
        // The other side of the refusal: a terminal row must NOT block, and
        // must be gone afterwards — "the whole point of uninstalling is that
        // the thing goes away" (D4).
        let home = provisioned_home();
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();

        assert_eq!(php("8.4").service_id(), sup.snapshot()[0].id);
        assert!(matches!(sup.snapshot()[0].state, ServiceState::Stopped));

        perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            vec![],
        ))
        .await
        .unwrap();
        assert_eq!(runner.spawns().len(), 1);
        assert!(
            sup.snapshot().is_empty(),
            "a terminal row must be gone afterwards: {:?}",
            sup.snapshot()
        );
    }

    // ---- brew failures ---------------------------------------------------
    //
    // VACUITY (RED first): written against a `run_brew` that ignored the exit
    // code entirely — the uninstall returned `Ok` and the pool config was
    // deleted, failing both assertions below.

    #[tokio::test]
    async fn a_brew_refusal_is_surfaced_verbatim_and_changes_no_local_state() {
        let home = provisioned_home();
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let runner = RecordingRunner::refusing(&[
            "Error: Refusing to uninstall /opt/homebrew/Cellar/php@8.4/8.4.1",
            "because it is required by imagemagick, which is currently installed.",
        ]);

        let err = perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            runner,
            vec![],
        ))
        .await
        .unwrap_err();

        let text = format!("{err}");
        assert!(
            text.contains("Refusing to uninstall /opt/homebrew/Cellar/php@8.4/8.4.1")
                && text.contains("required by imagemagick"),
            "brew's own words must survive intact: {text}"
        );
        assert!(
            home.path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists(),
            "a failed brew must leave the generated config alone"
        );
        assert_eq!(
            sup.snapshot().len(),
            1,
            "a failed brew must leave the service row alone"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_uninstall_leaves_the_data_logs_and_overrides_untouched() {
        let home = provisioned_home();
        let before = untouched_fingerprint(home.path());
        let sup = supervisor_with("mysql-8.4");
        let lock = InstallLock::default();

        let err = perform_uninstall(run_for(
            Target::parse(PackageKind::Mysql, "8.4").unwrap(),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::refusing(&["Error: No such keg"]),
            vec![],
        ))
        .await
        .unwrap_err();
        assert!(format!("{err}").contains("No such keg"));

        assert_untouched(&before);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_refused_uninstall_leaves_the_data_logs_and_overrides_untouched() {
        let home = provisioned_home();
        let before = untouched_fingerprint(home.path());
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();

        perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![site("shop", "shop.localhost", "8.4")],
        ))
        .await
        .unwrap_err();

        assert_untouched(&before);
    }

    // ---- the successful path, on a real filesystem ------------------------
    //
    // VACUITY (neuter-and-watch-it-fail), per assertion:
    //  * pool config removed — commented out the `GeneratedFile` arm's
    //    `remove_file`: the "is gone" assertion failed.
    //  * datadir kept — added `std::fs::remove_dir_all(&paths.datadir)` to the
    //    MySQL arm: `assert_untouched` failed on data/mysql/8.4/auto.cnf.
    //  * inode-identity — replaced the datadir files with byte-identical
    //    copies written to fresh inodes: the content assertion still passed
    //    and the inode assertion failed, which is the whole reason it is
    //    there.

    #[cfg(unix)]
    #[tokio::test]
    async fn a_successful_php_uninstall_removes_the_pool_config_and_keeps_everything_else() {
        let home = provisioned_home();
        let before = untouched_fingerprint(home.path());
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![],
        ))
        .await
        .unwrap();

        assert_eq!(outcome, UninstallOutcome::Done);
        assert!(
            !home
                .path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists(),
            "the generated pool config must be gone"
        );
        assert!(sup.snapshot().is_empty(), "the service row must be gone");
        assert_untouched(&before);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_successful_mysql_uninstall_keeps_the_datadir_byte_and_inode_identical() {
        // The highest-value test in the slice: a bug here destroys a user's
        // databases. Asserted on content AND inode, never on a `Result`.
        let home = provisioned_home();
        let before = untouched_fingerprint(home.path());
        let sup = supervisor_with("mysql-8.4");
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for(
            Target::parse(PackageKind::Mysql, "8.4").unwrap(),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![],
        ))
        .await
        .unwrap();

        assert_eq!(outcome, UninstallOutcome::Done);
        assert!(sup.snapshot().is_empty(), "the service row must be gone");
        assert_untouched(&before);
        // Named explicitly as well as covered by the fingerprint, because
        // this is the promise D2 makes to the user in so many words.
        assert!(
            home.path().join("data/mysql/8.4/mysql/user.ibd").exists(),
            "THE datadir must survive"
        );
    }

    #[tokio::test]
    async fn the_stored_root_password_survives_the_uninstall() {
        // "Keeping the data and throwing away the key is the same as
        // destroying it" (D2). Asserted on the value read back, not on a
        // `Result`: a repo call that silently dropped the row would return
        // `Ok` all the same.
        let home = provisioned_home();
        let db = Db::open(home.path()).await.expect("state.db");
        let repo = MysqlInstanceRepo::new(&db);
        let major = MysqlMajor::parse("8.4").unwrap();
        let password = generate_root_password();
        repo.upsert(&major, &password).await.expect("upsert");

        let sup = supervisor_with("mysql-8.4");
        let lock = InstallLock::default();
        perform_uninstall(run_for(
            Target::parse(PackageKind::Mysql, "8.4").unwrap(),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![],
        ))
        .await
        .unwrap();

        let after = repo.get(&major).await.expect("query").expect("row");
        assert_eq!(
            after.root_password.expose(),
            password.expose(),
            "the root password must survive the engine"
        );
    }

    // ---- filesystem edge cases -------------------------------------------

    #[tokio::test]
    async fn an_already_missing_pool_config_is_not_an_error() {
        let home = provisioned_home();
        std::fs::remove_file(home.path().join("config/generated/php/8.4/php-fpm.conf"))
            .expect("remove");
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![],
        ))
        .await
        .unwrap();
        assert_eq!(outcome, UninstallOutcome::Done);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_pool_config_removes_the_link_and_never_its_target() {
        // The generated tree is exactly where a symlink would be planted to
        // turn "delete our own config" into "delete something of yours".
        let home = provisioned_home();
        let outside = home.path().join("precious.conf");
        std::fs::write(&outside, "not ours\n").expect("write");
        let link = home.path().join("config/generated/php/8.4/php-fpm.conf");
        std::fs::remove_file(&link).expect("remove");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![],
        ))
        .await
        .unwrap();

        assert!(!link.exists(), "the link itself must be gone");
        assert_eq!(
            std::fs::read_to_string(&outside).expect("target must survive"),
            "not ours\n"
        );
    }

    #[tokio::test]
    async fn a_directory_where_the_pool_config_belongs_is_reported_not_recursed_into() {
        let home = provisioned_home();
        let path = home.path().join("config/generated/php/8.4/php-fpm.conf");
        std::fs::remove_file(&path).expect("remove");
        std::fs::create_dir(&path).expect("mkdir");
        std::fs::write(path.join("inside.txt"), "still here\n").expect("write");

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let outcome = perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![],
        ))
        .await
        .unwrap();

        match outcome {
            UninstallOutcome::Incomplete(problems) => assert!(
                problems.iter().any(|p| p.contains("could not be removed")),
                "got {problems:?}"
            ),
            UninstallOutcome::Done => panic!("a directory in the way must be reported"),
        }
        assert!(
            path.join("inside.txt").exists(),
            "nothing may be removed recursively"
        );
    }

    /// THE post-brew race (Task 1's sharp edge 1). brew has already deleted
    /// the binaries; the user started the service in the meantime; `unregister`
    /// correctly refuses to forget a child the supervisor is still supervising.
    /// The uninstall must NOT report plain success — the user can see the
    /// leftover row on the Services page, and being told "done" while looking
    /// at it is a contradiction they would have to resolve alone.
    ///
    /// VACUITY: neutered by swallowing `ProcError::NotTerminal` as `Ok(())` —
    /// this test then got `Done` and failed.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_service_started_during_the_uninstall_is_reported_not_swallowed() {
        let home = provisioned_home();
        let sup = Supervisor::new(default_driver());
        sup.register(ServiceSpec {
            id: "php-fpm-8.4".into(),
            display_name: "PHP-FPM 8.4".into(),
            endpoint: None,
            spawn: SpawnSpec {
                program: PathBuf::from("/bin/sleep"),
                args: vec![std::ffi::OsString::from("30")],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        });
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            Arc::new(RacingRunner {
                sup: sup.clone(),
                id: "php-fpm-8.4".to_string(),
            }),
            vec![],
        ))
        .await
        .unwrap();

        match outcome {
            UninstallOutcome::Done => panic!("a surviving row must not be reported as success"),
            UninstallOutcome::Incomplete(problems) => {
                let text = problems.join(" ");
                assert!(
                    text.contains("PHP 8.4 was removed"),
                    "the user must be told the version IS gone: {text}"
                );
                assert!(
                    text.contains("php-fpm-8.4") && text.contains("Stop php-fpm-8.4"),
                    "the user must be told which service to stop: {text}"
                );
                assert!(
                    text.contains("rescanned"),
                    "the user must be told how the row goes away: {text}"
                );
            }
        }
        // The generated config still went, and — the part that matters — the
        // live child was NOT forgotten, so the next launch's orphan reap can
        // still account for it.
        assert!(
            !home
                .path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists()
        );
        assert_eq!(
            sup.snapshot().len(),
            1,
            "the supervisor must keep a row for a child it is still supervising"
        );
        let _ = sup.stop("php-fpm-8.4");
    }

    #[tokio::test]
    async fn an_unregistered_service_row_is_not_an_error() {
        // An installed-but-never-initialized MySQL major has no row at all.
        let home = provisioned_home();
        let sup = Supervisor::new(default_driver());
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for(
            Target::parse(PackageKind::Mysql, "8.4").unwrap(),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            vec![],
        ))
        .await
        .unwrap();
        assert_eq!(outcome, UninstallOutcome::Done);
    }

    // ---- the lock slot and the quit-mid-uninstall seam --------------------

    #[tokio::test]
    async fn the_running_slot_reports_a_removal_and_is_empty_afterwards() {
        // Two things at once, both lifecycle. (1) `perform_quit` aborts
        // whatever occupies this slot before destroying the window, so an
        // uninstall that never registered would leave brew running with the
        // app gone. (2) The quit dialog reads the slot to say what is at
        // risk; before `PackageOperation` existed it could only say "is still
        // installing", which is false for a removal.
        let home = provisioned_home();
        let sup = supervisor_with("php-fpm-8.4");
        // `Arc` so the probe closure (which must be `'static` to live in the
        // sink) can hold the same lock the run registers on.
        let lock = Arc::new(InstallLock::default());
        let observed: Arc<Mutex<Option<(InstallKind, PackageOperation, String)>>> =
            Arc::new(Mutex::new(None));

        let probe = Arc::clone(&observed);
        let peeked = Arc::clone(&lock);
        let mut run = run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::talking(&["==> Uninstalling php@8.4"]),
            vec![],
        );
        // The sink runs while the run is in flight — exactly when the slot
        // must be occupied.
        run.log = Arc::new(move |_, _| {
            *probe.lock().expect("poisoned") = peeked.running_install();
        });
        perform_uninstall(run).await.unwrap();

        assert_eq!(
            observed.lock().expect("poisoned").clone(),
            Some((
                InstallKind::Php,
                PackageOperation::Uninstall,
                "8.4".to_string()
            )),
            "an in-flight uninstall must be visible as a REMOVAL, not an install"
        );
        assert!(
            lock.running_install().is_none(),
            "the slot must be released once the run is over"
        );
        // And the guard released the mutex too, so a following install is not
        // wedged behind a finished uninstall.
        assert!(lock.guard.try_lock().is_ok());
    }

    /// The executor must consult the supervisor's LIVE state, not the plan it
    /// was handed. Driven with a real supervised child (`/bin/sleep`) so the
    /// row is genuinely non-terminal rather than a hand-built value.
    ///
    /// VACUITY: neutered by deleting the `service_blocker` call from
    /// `blockers` — this test then recorded one spawn and returned `Ok`.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_live_service_refuses_the_uninstall_and_spawns_nothing() {
        let home = provisioned_home();
        let sup = Supervisor::new(default_driver());
        sup.register(ServiceSpec {
            id: "php-fpm-8.4".into(),
            display_name: "PHP-FPM 8.4".into(),
            endpoint: None,
            spawn: SpawnSpec {
                program: PathBuf::from("/bin/sleep"),
                args: vec![std::ffi::OsString::from("30")],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        });
        sup.start("php-fpm-8.4").expect("start");
        // Wait for the row to actually leave `Stopped`; without this the test
        // could assert against a state the supervisor has not reached yet.
        let live = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let state = sup.snapshot()[0].state.clone();
                match state {
                    ServiceState::Starting | ServiceState::Running => return state,
                    ServiceState::Stopped | ServiceState::Failed { .. } => {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                }
            }
        })
        .await
        .expect("the child must come up");

        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();
        let err = perform_uninstall(run_for(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            vec![],
        ))
        .await
        .unwrap_err();

        let text = format!("{err}");
        assert!(
            text.contains("php-fpm-8.4") && text.contains(crate::control::state_label(&live)),
            "the refusal must name the service and its state, got {text}"
        );
        assert!(
            runner.spawns().is_empty(),
            "a live service must refuse BEFORE anything is spawned, recorded {:?}",
            runner.spawns()
        );
        assert!(
            home.path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists(),
            "a refusal must not delete anything"
        );

        let _ = sup.stop("php-fpm-8.4");
    }
}
