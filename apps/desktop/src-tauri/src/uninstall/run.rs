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
//! [`perform_uninstall`] itself makes exactly three kinds of filesystem WRITE
//! (it also reads — `symlink_metadata`, `canonicalize`, `metadata` — and
//! `run_brew` spawns a process that writes plenty of its own):
//!
//! * `remove_file` on a path `inventory` produced under `config/generated/`
//!   (PHP's generated pool config — the only [`super::Removal::GeneratedFile`]
//!   any inventory emits today). It does not follow a symlink at the LEAF: on
//!   one it removes the LINK, and on a directory it fails loudly. It does
//!   follow every component ABOVE the leaf, so **that call is guarded**
//!   (see [`confine`], applied to the file's parent by
//!   [`remove_generated_file`]) — and against `<home>/config/generated`, not
//!   the packages root, because `config/custom` and `data/` sit beside it and
//!   are things an uninstall KEEPS. Measured before the guard existed: a
//!   symlinked major directory sent the unlink into an unrelated tree and the
//!   uninstall reported `Done`.
//! * `remove_dir_all` on a [`super::Removal::PackageTree`] path — MariaDB's
//!   series directory (P1 MariaDB UI design D5/D7) and, since off-Homebrew
//!   slice 5D, a packaged PHP's version directory. **That call is guarded**
//!   (see [`confine`]): the path is required to resolve inside
//!   `<home>/packages`, and to be a directory rather than a symlink, before it
//!   is made. A symlinked intermediate component redirects the delete and there
//!   is no signal afterwards to interpret — measured on this toolchain, a
//!   redirected `remove_dir_all` deletes the real contents it reached and
//!   returns `Ok(())` — while a symlink at the LEAF is the object `confine`
//!   and `remove_dir_all` would otherwise disagree about.
//! * `openvhost_core::remove_current_link` on the per-major `current` link,
//!   and only when it has been left pointing at nothing. That is a `remove_file`
//!   on a symlink behind `openvhost-pkg`'s platform facade (a junction needs
//!   `remove_dir` on Windows), and it refuses anything that is not a link.
//!   **Guarded by the same [`confine`] predicate, applied to the link's
//!   parent**: `remove_file` does not follow the final component but does
//!   follow every component above it, so without that check this one call
//!   could reach outside the region the other two are confined to.
//!
//! All three are gated by that one predicate — two against `<home>/packages`,
//! one against `<home>/config/generated` — so no filesystem write this executor
//! makes is checked by nothing. The regions differ; the check does not, which
//! is why [`confine`] takes its root as a parameter instead of knowing one.
//!
//! Nothing here touches `<home>/data`, `<home>/logs` or state.db's credential
//! rows on ANY path, including error paths.
//!
//! That is a statement about the EXECUTOR, not about this file, and the
//! difference matters when you audit it. The command wrapper
//! [`uninstall_package`] also runs the kind-appropriate rescan afterwards
//! (design D5's reconciliation), which reaches two more filesystem effects
//! neither of which is the executor's:
//!
//! * `rescan_mysql_into_state` calls `sweep_stale_staging`, a `remove_dir_all`
//!   under `<home>/data/mysql/`. It cannot reach a live datadir: a staging
//!   directory's name must match `init-<digits>-<digits>-`, a live datadir is a
//!   bare major (`8.4`), and the sweep's `file_type()` does not follow symlinks
//!   — so `<home>/data/mysql/8.4` is unreachable from it. (Verified by the
//!   security auditor; the property holds, the old wording did not.)
//! * `rescan_into_state` can `create_dir_all` under `<home>/logs/` while
//!   registering a pool — a creation, never a removal.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use openvhost_core::{Db, Site, SiteRepository, SqliteSiteRepository};
use openvhost_proc::{ProcError, SpawnSpec, Supervisor, TaskEvent, TaskStream};
use tauri_specta::Event;

use super::{Blocker, PackageKind, PackagedPhp, Target, UninstallPlan, build_plan, inventory};
use crate::commands::{
    InstallLock, IpcError, MariadbInstallLogEvent, MysqlInstallLogEvent, PackageOperation,
    PhpInstallLogEvent, RunningInstallGuard, now_ms, rescan_into_state, rescan_mariadb_into_state,
    rescan_mysql_into_state, stack_paths,
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
    /// What `<prefix>/opt/<formula>` actually resolves to, read fresh by the
    /// caller for the same reason `sites` is. Passed in rather than read here
    /// so [`perform_uninstall`] stays a function of its inputs — otherwise
    /// every test of the executor would consult the developer's own
    /// `/opt/homebrew` and refuse or proceed depending on what is installed
    /// there.
    ///
    /// `None` for a target with no Homebrew formula at all (MariaDB, D5; and a
    /// PHP major this app packaged, off-Homebrew slice 5D D2) — there is no
    /// `brew uninstall` for an alias to redirect, so there is nothing for the
    /// caller to read.
    pub(crate) keg: Option<openvhost_core::KegProvenance>,
    /// What OpenVHost's own package tree holds for this target, read fresh by
    /// the caller for the same reason `sites` and `keg` are: an install can
    /// finish, or a `current` link swing, while a confirmation dialog sits
    /// open. `None` on every machine with no packaged PHP for this major, which
    /// is the unchanged Homebrew path. See [`super::PackagedPhp`].
    pub(crate) packaged: Option<PackagedPhp>,
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
    let blockers = super::blockers(
        &run.target,
        &run.sup.snapshot(),
        &run.sites,
        run.keg.as_ref(),
        run.packaged.as_ref(),
    );
    if !blockers.is_empty() {
        return Err(refusal(&run.target, &blockers));
    }

    let inv = inventory(&run.target, &run.home, run.packaged.as_ref());
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
                // Same `root`-then-`confine` shape as the two calls below, and
                // the same reason: what `remove_file` FOLLOWS on the way to
                // this file is checked rather than assumed from the fact that
                // `inventory` built the path out of well-formed components.
                if let Some(problem) = remove_generated_file(
                    &crate::stack::generated_config_root(&run.home),
                    path,
                    what,
                ) {
                    problems.push(problem);
                }
            }
            super::Removal::PackageTree { path, what } => {
                // `remove_dir_all`, not `remove_file`: this names a whole
                // package tree THIS app's own installer created, for a target
                // with no Homebrew formula to spawn `brew uninstall` for at all
                // (MariaDB always; a packaged PHP major since off-Homebrew
                // slice 5D — see `Target::formula`).
                //
                // THE call this module has to justify, and it validates its own
                // target rather than trusting a check made three layers up by a
                // caller it cannot see (spec 5D D4). Two measured facts drive
                // the shape of that guard:
                //
                //  * `remove_dir_all` FOLLOWS a symlinked INTERMEDIATE
                //    component and deletes the real contents behind it. Replace
                //    `<home>/packages/php/8.4` with a link and the delete lands
                //    wherever that link goes.
                //  * neither that case nor a symlink at the leaf returns an
                //    error. A redirected delete is indistinguishable from a
                //    correct one by its return value, so the check has to
                //    happen BEFORE the call — there is nothing afterwards to
                //    interpret.
                //
                // `path` itself is well-formed by construction (see
                // `Removal::PackageTree`'s doc), and that is exactly what is
                // NOT sufficient: every component of it can be a plain, legal
                // name while the path resolves somewhere else entirely.
                let root = openvhost_core::PackagesRoot::from_home(&run.home);
                match confine(root.as_path(), path) {
                    // Already gone — same "the post-state is what was asked
                    // for" reasoning as `GeneratedFile` above. Note this is
                    // "nothing is there at all", not "it did not resolve":
                    // `confine` tells those two apart.
                    Confinement::Absent => {}
                    // A REFUSAL, not a problem to collect. The program-files
                    // removal is the first step of every inventory that has
                    // one, so returning here means nothing has been removed
                    // and nothing will be — the same shape `run_brew`'s `?`
                    // gives a brew that refused, and for the same reason: the
                    // local cleanup below must not run while the version is
                    // still installed.
                    Confinement::Refused { reason } => {
                        return Err(IpcError::Core {
                            message: format!(
                                "{} was not removed. {what} at {} {reason}. Nothing was removed.",
                                run.target.display(),
                                path.display(),
                            ),
                        });
                    }
                    Confinement::Contained => match std::fs::remove_dir_all(path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => problems.push(format!(
                            "{what} at {} could not be removed: {e}",
                            path.display()
                        )),
                    },
                }
                // The tree is gone; the link that selected it must not be left
                // pointing at it. A dangling `current` is not cosmetic:
                // `looks_like_a_broken_install` counts ANY entry in the major
                // directory, so leaving the link behind makes discovery report
                // the major as an install it could not identify — a successful
                // uninstall that renders as a broken one.
                //
                // Reached on the `Absent` path too, deliberately: a link left
                // over from a previous attempt that removed the tree and then
                // failed is exactly the state this closes.
                //
                // MariaDB needs nothing here and gets nothing: its `current`
                // lives INSIDE the series directory that was just removed, so
                // there is no link left to dangle. That is decided by looking,
                // not by an arm remembering it — see `Target::packaged_current_link`.
                // Same `root`, same predicate as the removal above: what
                // `remove_file` follows on the way to the link is checked, not
                // assumed from the fact that the removal beside it was.
                if let Some(problem) = clear_dangling_current(
                    root.as_path(),
                    &run.target.packaged_current_link(&run.home),
                ) {
                    problems.push(problem);
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

/// Whether a [`super::Removal::PackageTree`] path may be handed to
/// `remove_dir_all`.
///
/// Three states, not a `bool` with an error on the side. "Nothing is there"
/// and "something is there that I could not prove is ours" are opposite
/// answers — the first means the uninstall is already done, the second means
/// it must not proceed — and a boolean would have to collapse one into the
/// other. (This codebase has now shipped three defects whose shape was a
/// boolean standing where a state belonged.)
#[derive(Debug, PartialEq, Eq)]
enum Confinement {
    /// Nothing exists at the path — not a directory, not a file, not even a
    /// dangling symlink. There is nothing to remove and nothing to confine.
    Absent,
    /// The path resolves to a location strictly inside the packages root, so
    /// removing it cannot reach anything the package tree does not own.
    Contained,
    /// Something is there, and it is not provably ours. `reason` is a verb
    /// phrase whose subject is the removal's own `what` — "The PHP 8.4 program
    /// files at `<path>` **resolve to …**" — so the caller composes one
    /// sentence rather than concatenating two.
    Refused { reason: String },
}

/// Require `path` to resolve strictly inside `root` before a delete is pointed
/// at it (off-Homebrew slice 5D design D4).
///
/// **`root` is a parameter, not `<home>/packages`.** This is the one
/// containment predicate the module has, asked about three different objects
/// against two different regions: a package tree and a `current` link's parent
/// against `<home>/packages`, a generated config file's parent against
/// `<home>/config/generated`. A second spelling for the second region is how
/// two checks that are meant to agree stop agreeing, so there is one. Nothing
/// below reads the root's ROLE — only its resolved path — which is what makes
/// that reuse honest rather than convenient.
///
/// **A symlink at the leaf is refused outright, before anything is resolved.**
/// That is not extra strictness, it is what makes the rest of this function
/// true of the call it authorises: `canonicalize` follows the final component
/// and `remove_dir_all` does not, so on a link the two halves would be judging
/// and deleting different objects — and a link pointing back INSIDE the root
/// passes every comparison below while the entry removed is wherever the link
/// itself lives. (Found by the security audit as a live escape; see the arm
/// for what is and is not lost by refusing the class.)
///
/// **Both sides are canonicalized, and canonicalizing the ROOT is
/// load-bearing rather than symmetry.** `openvhost_core::keg_provenance` makes
/// the identical point in its own containment check: on macOS `/var` resolves
/// to `/private/var`, so comparing a resolved path against an unresolved
/// prefix fails for every tempdir — which reads as a broken check rather than
/// a strict one, and would be "fixed" by deleting it.
///
/// [`Path::starts_with`] rather than a string prefix, and that is also
/// load-bearing: it compares whole components, so a sibling named
/// `packages-elsewhere` is outside `packages`, where `str::starts_with` would
/// call it inside.
///
/// Equality is refused as well as escape. `starts_with` is true of the root
/// itself, and the root resolving to the root is reachable — a symlinked
/// series directory plus a `current` naming `packages` lands exactly there —
/// and would authorise a recursive delete of every engine's package tree at
/// once.
///
/// **What this does NOT claim**, stated because a comment that outruns its
/// code is worse than none:
///
/// * it is a check, not a lock. A component swapped between this call and the
///   `remove_dir_all` would defeat it. That needs write access inside the
///   OpenVHost home — this app's own privilege level — so it crosses no
///   boundary; it is simply not a property a path check can provide.
/// * relocating the root ITSELF (making `<home>/packages` a symlink) is
///   accepted, because the root is canonicalized: the tree moves, and the
///   removal stays inside the moved tree. That is the honest reading — a
///   redirected root is not an escape from the package tree, it *is* the
///   package tree, and everything the installer writes goes there too.
///   Refusing it would break a user who put their packages on another volume.
///   What is caught is any component BELOW the root diverging from it.
fn confine(root: &Path, path: &Path) -> Confinement {
    // `symlink_metadata`, never `exists()`: a dangling symlink where the
    // version directory belongs is something rather than nothing, and reading
    // it as "already gone" would leave it behind for discovery to trip over.
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Confinement::Absent,
        Err(e) => {
            return Confinement::Refused {
                reason: format!("could not be examined: {e}"),
            };
        }
        // A SYMLINK AT THE LEAF, refused here rather than resolved, because
        // past this point the check and the call stop naming the same object:
        // `canonicalize` FOLLOWS the leaf and answers about the target, while
        // the `remove_dir_all` this authorises does NOT follow it and unlinks
        // the LINK. A leaf pointing back inside the root therefore passed the
        // comparison below while the entry actually removed sat wherever the
        // link did — outside the root, if an intermediate component left it.
        // Measured live by the security audit; the property this function's
        // doc claims only holds once the two halves agree on the object.
        //
        // Nothing legitimate is lost by refusing the whole class:
        // `openvhost-pkg`'s installer creates a version directory as a real
        // directory (an atomic rename of a staged tree), never as a link, so
        // the refused set is exactly the set that was never ours. And the
        // plain-correctness half needs no attacker at all — unlinking a link
        // and reporting `Done` leaves the program files installed.
        Ok(md) if md.file_type().is_symlink() => {
            return Confinement::Refused {
                reason: "are a symlink rather than a directory, and OpenVHost's installer never \
                         creates one"
                    .to_string(),
            };
        }
        Ok(_) => {}
    }
    let real_path = match std::fs::canonicalize(path) {
        Ok(p) => p,
        // Reached by a dangling symlink at the leaf, among other things. We
        // cannot establish where this would land, so we do not remove it —
        // "could not tell" must never collapse into "proceed" on the way to a
        // recursive delete.
        Err(e) => {
            return Confinement::Refused {
                reason: format!("could not be resolved: {e}"),
            };
        }
    };
    let real_root = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) => {
            return Confinement::Refused {
                reason: format!(
                    "could not be checked against {}, which could not be resolved: {e}",
                    root.display()
                ),
            };
        }
    };
    if real_path.starts_with(&real_root) && real_path != real_root {
        Confinement::Contained
    } else {
        // The root is named by PATH rather than described ("OpenVHost's package
        // directory"), because this reason is composed for whichever region the
        // caller passed and there is now more than one. The caller that splices
        // it names the object; this half names where it had to stay.
        Confinement::Refused {
            reason: format!(
                "resolve to {}, which is outside {}",
                real_path.display(),
                real_root.display()
            ),
        }
    }
}

/// Remove one file THIS app generated, and only if `remove_file` cannot leave
/// the generated tree on the way to it. Returns the problem to report, or
/// `None` when there was nothing left to do.
///
/// `remove_file`, never `remove_dir_all`: on a symlink this removes the LINK
/// and never the target, and on a directory it fails loudly instead of
/// recursing. Both matter — the generated tree is exactly where a hostile or
/// accidental symlink would be planted to get a delete out of it.
///
/// **Confined by [`confine`], applied to the file's PARENT**, for exactly the
/// reason [`clear_dangling_current`] is: `remove_file` does not follow the
/// final component, but it does follow every component above it, so a major
/// directory that is a symlink out of the generated tree puts this unlink
/// wherever that link points. Measured before the check was written: with
/// `<home>/config/generated/php/8.4` replaced by a link, the call unlinked a
/// `php-fpm.conf` in an unrelated tree and the uninstall reported `Done` — so
/// there is nothing after the fact to interpret, and the parent, being the part
/// `remove_file` resolves, is the part to check.
///
/// Reusing [`confine`] rather than spelling a third predicate is deliberate,
/// and it is why that function takes its root as a parameter: the packages root
/// and the generated root are different regions, the question asked about them
/// is the same one, and two containment checks that could disagree is how one
/// guarded call and one unguarded call ended up in the same module.
///
/// The root is `<home>/config/generated` rather than `<home>` or
/// `<home>/config`, and that choice is the whole value of the check:
/// `config/custom` (the user's own overrides) and `data/` (their databases) sit
/// beside it and are listed under an uninstall's KEEPS, so a redirect into
/// either has to be refused rather than permitted by a root wide enough to
/// contain them. It is not narrowed to `config/generated/php` either, because
/// [`super::Removal::GeneratedFile`] names no engine — PHP's pool config is the
/// only one emitted today, and a per-engine root would have to be widened by
/// the next one rather than simply holding. The one shape it does not admit is
/// a generated file sitting DIRECTLY in the root — [`confine`] refuses the root
/// itself, so `<home>/config/generated/foo.conf` would be reported rather than
/// removed. Everything generated today lives one engine directory down, and a
/// future emitter that did not would be told so by its own test rather than
/// silently skipped.
///
/// A refusal is a PROBLEM, not an early return. Unlike
/// [`super::Removal::PackageTree`], this step is never an inventory's first: by
/// the time it runs the program files are already gone, so returning here would
/// skip the service row and leave the Services page listing a version that no
/// longer exists.
fn remove_generated_file(generated_root: &Path, path: &Path, what: &str) -> Option<String> {
    // Not `.expect("a parent")`: unreachable for a path `inventory` built is
    // still not a reason to panic in the module that deletes things.
    let Some(parent) = path.parent() else {
        return Some(format!(
            "{what} at {} was left alone: it names no directory to check. Delete it by hand.",
            path.display()
        ));
    };
    match confine(generated_root, parent) {
        Confinement::Contained => {}
        // Nothing above the file exists, so the file does not either — the same
        // "the post-state is what was asked for" reading as the `NotFound` arm
        // below, reached one level earlier.
        Confinement::Absent => return None,
        // `reason` is deliberately not spliced in, for the reason
        // `clear_dangling_current` gives: it is a verb phrase written for the
        // removal's own `what`, and this sentence's subject is the file's
        // DIRECTORY rather than the file itself.
        Confinement::Refused { .. } => {
            return Some(format!(
                "{what} at {} was left alone: its directory {} could not be shown to be inside \
                 the config directory OpenVHost generates, {}, and nothing outside that \
                 directory is removed. Delete it by hand.",
                path.display(),
                parent.display(),
                generated_root.display()
            ));
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => None,
        // Already gone (a previous attempt, a manual tidy-up, an apply that
        // swept it). The post-state is what was asked for, so this is done,
        // not failed.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(format!(
            "{what} at {} could not be removed: {e}",
            path.display()
        )),
    }
}

/// Remove a `current` link that has been left pointing at nothing, and only
/// that. Returns the problem to report, or `None` when there was nothing to do.
///
/// **Confined by [`confine`], applied to the link's PARENT** — the one
/// filesystem write in this module that used to have no containment at all.
/// `remove_file` does not follow the final component, which is what makes it
/// safe on the link itself, but it does follow every component above it: a
/// series directory that is a symlink out of the tree puts this call outside
/// the region the module header claims for it, and `remove_current_link`
/// returns `Ok(())` either way. The parent is exactly the part `remove_file`
/// resolves, so it is exactly the part to check — and reusing [`confine`]
/// rather than spelling a second predicate is deliberate: two containment
/// checks that could disagree is how the guarded call and the unguarded one
/// ended up in the same function.
///
/// Then exhaustive over the three answers `metadata` can give, because they
/// mean three different things:
///
/// * `Ok` — whatever is at `link` resolves to something that still exists. A
///   `current` pointing at a version directory that survived is none of this
///   function's business, and removing it would break a live install.
/// * `NotFound` — either there is no link at all, or there is one and it
///   resolves to nothing. Both are handed to the platform facade, which tells
///   them apart itself (an absent link is `Ok(())`) and refuses anything that
///   is not a link, so this does not have to duplicate either rule.
/// * anything else — we cannot tell whether it dangles, so nothing is removed.
///   Reported rather than swallowed: a `current` left pointing at a removed
///   tree is the difference between a clean uninstall and a major that
///   discovery reports as an install it cannot identify.
fn clear_dangling_current(packages_root: &Path, link: &Path) -> Option<String> {
    // Not `.expect("a parent")`: unreachable for a link built by
    // `Target::packaged_current_link` (always `<root>/<package>/<major>/current`)
    // is still not a reason to panic in the module that deletes directories.
    let Some(parent) = link.parent() else {
        return Some(format!(
            "the version directory is gone, but the {} link to it was left alone: it names no \
             directory to check. Delete it by hand; until then this version still appears as an \
             install OpenVHost cannot identify.",
            link.display()
        ));
    };
    match confine(packages_root, parent) {
        Confinement::Contained => {}
        // Nothing above the link exists, so there is no link either. MariaDB
        // arrives here on every uninstall — its `current` lives INSIDE the
        // series directory just removed — and so does a PHP major whose whole
        // tree a user deleted by hand.
        Confinement::Absent => return None,
        // `reason` is deliberately not spliced in: it is a verb phrase written
        // for the removal's plural `what` ("The PHP 8.4 program files … resolve
        // to …"), and bending it around a singular subject here would read as
        // broken English in a message whose whole job is to be actionable.
        Confinement::Refused { .. } => {
            return Some(format!(
                "the version directory is gone, but the {} link to it was left alone: its \
                 directory {} could not be shown to be inside OpenVHost's package directory {}, \
                 and nothing outside that directory is removed. Delete the link by hand; until \
                 then this version still appears as an install OpenVHost cannot identify.",
                link.display(),
                parent.display(),
                packages_root.display()
            ));
        }
    }
    match std::fs::metadata(link) {
        Ok(_) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            openvhost_core::remove_current_link(link).err().map(|e| {
                format!(
                    "the version directory is gone, but the {} link to it could not be removed: \
                     {e}. Delete it by hand; until then this version still appears as an install \
                     OpenVHost cannot identify.",
                    link.display()
                )
            })
        }
        Err(e) => Some(format!(
            "the version directory is gone, but the {} link to it could not be examined: {e}. If \
             it is still pointing at the removed version, delete it by hand; until then this \
             version still appears as an install OpenVHost cannot identify.",
            link.display()
        )),
    }
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
/// supervisor's snapshot, the site list and Homebrew's `opt` links, and derives
/// paths — that is all — so the Languages/Databases pages can call it on mount
/// to decide a disabled state as cheaply as they call it to fill a confirmation
/// dialog. `Target::keg_provenance` is a `canonicalize`, not a process: cheap
/// enough for a mount, which is why the aliased-keg refusal can be a BLOCKER
/// (the action is disabled with an explanation) rather than a surprise at
/// confirm time.
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
    // Both filesystem reads happen HERE, and their answers are what cross into
    // `build_plan` — see `uninstall::inventory`'s purity invariant. `packaged`
    // is resolved first because `keg_provenance` depends on it: a packaged row
    // runs no `brew uninstall`, so there is no alias to look up.
    let packaged = target.packaged(&p.home);
    Ok(build_plan(
        &target,
        &p.home,
        &sup.snapshot(),
        &sites,
        target.keg_provenance(packaged.as_ref()).as_ref(),
        packaged.as_ref(),
    ))
}

/// Remove `major`: refuse if anything depends on it, otherwise
/// `brew uninstall`, then drop the config this app generated and the service
/// row.
///
/// Re-checks the blockers itself — the plan the dialog was built from may be
/// stale — and streams brew's output through the same events `install_php`
/// uses, so the Languages/Databases pages need no second log channel.
// Ten parameters, eight of them managed state. Tauri extracts state per TYPE
// through the signature — there is no request object to bundle them into, and
// the alternative (pulling them out of `app` with `try_state` inside the body)
// would hide the dependency list rather than shorten it, which is the opposite
// of what this lint is for. Every one is genuinely used: `db` for the pinned-
// sites re-check, all three runtime locks for the kind-specific rescan
// afterwards.
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
    mariadb_runtimes: tauri::State<
        '_,
        RwLock<Option<Vec<openvhost_core::mariadb::MariadbRuntime>>>,
    >,
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
    // Resolved once, and reused by every question below that depends on it:
    // which formula this uninstall names, whether Homebrew has to be found at
    // all, whether an aliased keg can be in the way, and what the executor
    // removes. Read here rather than carried over from whatever plan the dialog
    // was built from — an install can finish while a confirmation sits open.
    let packaged = target.packaged(&p.home);
    // Homebrew is required only for a formula-having target:
    // `Target::formula` is `None` for MariaDB (P1 MariaDB UI design D5) and for
    // a PHP major this app packaged (off-Homebrew slice 5D D2), neither of
    // which spawns brew at all (see `Removal::PackageTree`). Gating the lookup
    // on that fact — rather than requiring Homebrew unconditionally — is what
    // stops "Homebrew was not found" from refusing to remove a directory on a
    // machine that never touched Homebrew in the first place.
    // The placeholder path is never read on those arms: `run.target
    // .uninstall_spec(&run.brew)` is called only from `run_brew`, itself only
    // reached for a `Removal::BrewFormula` step, which neither inventory
    // produces.
    let brew = match target.formula(packaged.as_ref()) {
        Some(_) => openvhost_core::find_brew().ok_or_else(|| IpcError::Core {
            message: format!(
                "Homebrew was not found. Looked for bin/brew under: {}",
                openvhost_core::BREW_PREFIXES.join(", ")
            ),
        })?,
        None => PathBuf::new(),
    };

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
        // Re-read here, not carried over from whatever plan the dialog was
        // built from: `brew upgrade php` in another terminal can turn a
        // versioned keg into an alias between the two.
        keg: target.keg_provenance(packaged.as_ref()),
        packaged,
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
        // No install seed on either arm: nothing was installed here, and a
        // seed is only ever a record of what THIS app just asked brew for.
        PackageKind::Php => {
            rescan_into_state(runtimes.inner(), sup.inner(), p, None).await?;
        }
        PackageKind::Mysql => {
            rescan_mysql_into_state(mysql_runtimes.inner(), sup.inner(), &p.home, None).await?;
        }
        // No seed parameter: MariaDB's rescan takes none — see
        // `rescan_mariadb_into_state`'s own doc comment.
        PackageKind::Mariadb => {
            rescan_mariadb_into_state(mariadb_runtimes.inner(), sup.inner(), &p.home).await?;
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
///
/// In practice MariaDB's own arm never fires: its uninstall is a
/// `Removal::PackageTree` (a direct `remove_dir_all`, no child process, so
/// nothing ever calls `run.log`), never a `Removal::BrewFormula`. It is still
/// given a real arm rather than folded away, for the same reason
/// `Target::uninstall_spec`'s MariaDB arm is: a future edit that DID wire a
/// brew-style removal to this kind must route it somewhere real, not silently
/// reuse MySQL's or PHP's channel.
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
        PackageKind::Mariadb => {
            // No `major` field: this build ships exactly one series, so a
            // field nothing can vary is left off — the same reasoning
            // `MariadbInstance`'s own doc comment gives for leaving `major`
            // off that struct. `major` (always `MARIADB_SERIES` here) is
            // therefore unused on this arm.
            let _ = MariadbInstallLogEvent {
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
    use openvhost_core::mariadb::MariadbInstanceRepo;
    use openvhost_core::mysql::{MysqlInstanceRepo, MysqlMajor, generate_root_password};
    use openvhost_proc::{
        DEFAULT_GRACE, ReadinessProbe, ServiceSpec, ServiceState, Supervisor, default_driver,
    };

    use super::super::tests::{own_keg, php, site};
    // The packaged fixtures are symlink-shaped, so every test that uses them
    // is `#[cfg(unix)]` and so is the import.
    #[cfg(unix)]
    use super::super::tests::{install_packaged_php, point_current};
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
        run_for_keg(target, home, sup, lock, runner, sites, own_keg())
    }

    /// [`run_for`] with the keg provenance spelled out — only the aliased-keg
    /// tests need to vary it; everything else is about something other than
    /// Homebrew's aliasing and takes the ordinary `OwnKeg`.
    #[allow(clippy::too_many_arguments)]
    fn run_for_keg<'a>(
        target: Target,
        home: &std::path::Path,
        sup: &'a Supervisor,
        lock: &'a InstallLock,
        runner: Arc<dyn BrewRunner>,
        sites: Vec<Site>,
        keg: openvhost_core::KegProvenance,
    ) -> UninstallRun<'a> {
        UninstallRun {
            target,
            home: home.to_path_buf(),
            brew: PathBuf::from("/opt/homebrew/bin/brew"),
            sup,
            lock,
            sites,
            keg: Some(keg),
            // Homebrew's row: no packaged install for this major. Every
            // executor test that existed before off-Homebrew slice 5D is a
            // brew-path test and stays one, which is what makes "the brew path
            // is untouched" checkable rather than asserted.
            packaged: None,
            runner,
            log: silent(),
        }
    }

    /// [`run_for`] for a formula-less target (MariaDB): there is no keg to
    /// resolve, so `keg` is `None` rather than a `KegProvenance` this target
    /// has no formula to have looked one up for.
    fn run_for_mariadb<'a>(
        home: &std::path::Path,
        sup: &'a Supervisor,
        lock: &'a InstallLock,
        runner: Arc<dyn BrewRunner>,
    ) -> UninstallRun<'a> {
        UninstallRun {
            target: Target::Mariadb,
            home: home.to_path_buf(),
            brew: PathBuf::from("/opt/homebrew/bin/brew"),
            sup,
            lock,
            sites: Vec::new(),
            keg: None,
            // MariaDB's `PackageTree` path is built from compile-time
            // constants, so it needs nothing threaded in — see
            // `Target::packaged`'s own MariaDB arm.
            packaged: None,
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

    /// THE R1 refusal, at the executor: an aliased `php@8.5` must not reach
    /// `brew`. `brew uninstall php@8.5` would resolve the alias and remove
    /// `php 8.5.9` — the user's linked PHP — while every string this app shows
    /// says `php@8.5`.
    ///
    /// VACUITY: the positive control is inside the test — the SAME recording
    /// runner is driven a second time with the keg owning itself, and must then
    /// record exactly one spawn. "Nothing happened" therefore cannot pass
    /// because the instrument is broken. Additionally neutered: deleting the
    /// `keg_blocker` push from `blockers` made the first assertion fail with
    /// one recorded spawn of `["...brew", "uninstall", "php@8.5"]`.
    #[tokio::test]
    async fn an_aliased_keg_refuses_the_uninstall_and_spawns_no_process() {
        let home = provisioned_home();
        let sup = Supervisor::new(default_driver());
        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();

        let refused = run_for_keg(
            php("8.5"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            vec![],
            openvhost_core::KegProvenance::ForeignKeg {
                owner: "php".to_string(),
                keg: PathBuf::from("/opt/homebrew/Cellar/php/8.5.9"),
            },
        );
        let err = perform_uninstall(refused).await.unwrap_err();
        let text = format!("{err}");
        assert!(
            text.contains("php@8.5") && text.contains("/opt/homebrew/Cellar/php/8.5.9"),
            "the refusal must name both the formula and the keg that would go: {text}"
        );
        assert!(
            runner.spawns().is_empty(),
            "brew must NEVER be spawned for an aliased keg, recorded {:?}",
            runner.spawns()
        );

        // POSITIVE CONTROL, same runner: with the keg owning itself there is no
        // blocker and exactly one spawn must be recorded — so the empty
        // assertion above cannot have passed because the recorder is broken.
        let allowed = run_for_keg(
            php("8.5"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            vec![],
            openvhost_core::KegProvenance::OwnKeg {
                keg: PathBuf::from("/opt/homebrew/Cellar/php@8.5/8.5.9"),
            },
        );
        perform_uninstall(allowed).await.unwrap();
        assert_eq!(
            runner.spawns(),
            vec![vec![
                "/opt/homebrew/bin/brew".to_string(),
                "uninstall".to_string(),
                "php@8.5".to_string(),
            ]]
        );
    }

    #[tokio::test]
    async fn an_unresolvable_keg_refuses_the_uninstall_and_spawns_no_process() {
        // The state that must not collapse back into "fine, proceed".
        let home = provisioned_home();
        let sup = Supervisor::new(default_driver());
        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();

        let err = perform_uninstall(run_for_keg(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            vec![],
            openvhost_core::KegProvenance::Unresolved {
                searched: vec![PathBuf::from("/opt/homebrew/opt/php@8.4")],
            },
        ))
        .await
        .unwrap_err();
        assert!(
            format!("{err}").contains("could not work out which Homebrew keg"),
            "got {err}"
        );
        assert!(runner.spawns().is_empty(), "recorded {:?}", runner.spawns());
        // And nothing local moved either.
        assert!(
            home.path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists()
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

    /// The arm the containment check ADDED to this path: with the major
    /// directory itself gone, `confine` answers `Absent` and the removal is
    /// finished before `remove_file` is ever called. Before the check it was
    /// `remove_file` returning `NotFound`. Same verdict, different route — and
    /// the route is new, so a future edit that made "could not tell" report a
    /// problem would turn an ordinary second uninstall into a failure.
    #[tokio::test]
    async fn a_missing_generated_directory_is_not_an_error_either() {
        let home = provisioned_home();
        std::fs::remove_dir_all(home.path().join("config/generated/php/8.4")).expect("remove");
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
        assert!(sup.snapshot().is_empty(), "the service row must be gone");
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

    /// The generated-config delete, and the reach it had until this slice.
    ///
    /// `remove_file` does not follow the FINAL component — which is what makes
    /// it safe on a symlinked pool config (the test above) — but it does follow
    /// every component ABOVE it. A major directory that is a symlink out of the
    /// generated tree therefore puts the unlink wherever that link points,
    /// while every component of the path `inventory` produced is still a plain,
    /// legal name.
    ///
    /// Asserted on the DISK before the `Result` is looked at: a redirected
    /// unlink returns `Ok(())`, so the outcome cannot tell the two apart.
    /// Measured on the unguarded code, which is where that ordering came from:
    /// this fixture unlinked the outside file and the run returned
    /// `Ok(Done)` — a clean success report for a delete that left the tree.
    ///
    /// VACUITY, in a disposable worktree with its own target directory:
    /// deleting only [`remove_generated_file`]'s `confine` call failed this
    /// test and left all four of the slice's other symlink tests green;
    /// deleting only `confine`'s leaf-symlink arm failed three of those and
    /// left this one green; deleting only [`clear_dangling_current`]'s call
    /// failed the fourth and left this one green. The three gates are pinned
    /// separately — none of them rides on another's fix.
    ///
    /// The positive control is the whole rest of the module: a `confine` that
    /// refused everything would fail
    /// `a_successful_php_uninstall_removes_the_pool_config_and_keeps_everything_else`,
    /// so this cannot pass by the guard having become unconditional.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_major_directory_does_not_unlink_the_config_beyond_it() {
        let home = provisioned_home();
        let elsewhere = tempfile::tempdir().expect("tempdir");
        // Somebody else's file, outside the generated tree, carrying the name
        // the removal is about to ask for.
        let outside = elsewhere.path().join("php-fpm.conf");
        std::fs::write(&outside, b"not ours\n").expect("write");

        let major_dir = home.path().join("config/generated/php/8.4");
        std::fs::remove_dir_all(&major_dir).expect("remove");
        std::os::unix::fs::symlink(elsewhere.path(), &major_dir).expect("symlink");
        // The path the executor is handed really does run through the link,
        // and it is the path the real builder produces — not one spelled here.
        assert_eq!(
            crate::stack::php_pool_config_path(home.path(), "8.4"),
            major_dir.join("php-fpm.conf")
        );

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
        .await;

        // DISK FIRST, deliberately before the `Result` is even looked at:
        // `remove_file` returns `Ok(())` whether it unlinked our file or
        // somebody else's, so asserting the refusal first would report
        // "expected Incomplete, got Done" for a run that had just deleted
        // someone else's config.
        assert_eq!(
            std::fs::read(&outside).unwrap_or_else(|e| panic!(
                "{} was unlinked: the generated-config delete reached outside {}: {e}",
                outside.display(),
                home.path().join("config/generated").display()
            )),
            b"not ours\n",
            "{} was rewritten",
            outside.display()
        );

        // …and it is REPORTED rather than swallowed: a file the confirmation
        // promised to remove and the executor walked past is exactly the
        // "the dialog promises what the executor never does" failure.
        match outcome.expect("the run itself does not fail") {
            UninstallOutcome::Incomplete(problems) => assert!(
                problems.iter().any(|p| p.contains("php-fpm.conf")),
                "got {problems:?}"
            ),
            UninstallOutcome::Done => {
                panic!("a pool config that could not be removed must not report success")
            }
        }
        // A PROBLEM, not an early return — the distinction from
        // `Removal::PackageTree`'s refusal. brew has already removed the
        // program files by the time this step runs, so bailing here would
        // leave the Services page listing a version that no longer exists.
        assert!(
            sup.snapshot().is_empty(),
            "the service row must still be cleared: {:?}",
            sup.snapshot()
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

    // ------------------------------------------------------------------
    // MariaDB (P1 MariaDB UI design D5/D7): `Removal::PackageTree`, never
    // `Removal::BrewFormula` — its own fixture, mirroring
    // `provisioned_home`/`untouched_fingerprint` but scoped to the paths
    // this engine actually owns.
    // ------------------------------------------------------------------

    /// A home with a fake MariaDB package tree (two files under
    /// `packages/mariadb/11.4/11.4.9/`, standing in for the real
    /// `bin/`+`lib/plugin/` shape), a datadir holding a real "table" file, a
    /// generated my.cnf, and the user's own override directory — everything
    /// a MariaDB uninstall must either remove or leave completely alone.
    fn provisioned_mariadb_home() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        for (path, contents) in [
            ("packages/mariadb/11.4/11.4.9/bin/mariadbd", "#!/bin/sh\n"),
            (
                "packages/mariadb/11.4/11.4.9/lib/plugin/marker",
                "a plugin file",
            ),
            ("config/generated/mariadb/11.4/my.cnf", "[mariadbd]\n"),
            ("config/custom/mariadb/11.4/conf.d/mine.cnf", "# mine\n"),
            ("data/mariadb/11.4/ibdata1", "\x00binary rows\x00"),
        ] {
            let p = home.join(path);
            std::fs::create_dir_all(p.parent().expect("has a parent")).expect("mkdir");
            std::fs::write(&p, contents).expect("write");
        }
        tmp
    }

    #[cfg(unix)]
    fn untouched_mariadb_fingerprint(home: &std::path::Path) -> Vec<(PathBuf, Vec<u8>, u64)> {
        use std::os::unix::fs::MetadataExt;
        [
            "config/custom/mariadb/11.4/conf.d/mine.cnf",
            "config/generated/mariadb/11.4/my.cnf",
            "data/mariadb/11.4/ibdata1",
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

    /// The MariaDB mirror of `a_successful_mysql_uninstall_keeps_the_datadir_
    /// byte_and_inode_identical` — the highest-value test in THIS engine's
    /// slice, for the identical reason: a bug here destroys a user's
    /// databases. Asserted on content AND inode, never on a `Result`.
    ///
    /// VACUITY: run with the datadir/my.cnf/custom_confd fixture paths
    /// removed from `provisioned_mariadb_home` first — `untouched_mariadb_
    /// fingerprint` then panics on a missing path before the executor is
    /// even invoked, which is what proves the fixture (not an always-green
    /// assertion) is what makes this test meaningful.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_successful_mariadb_uninstall_removes_the_package_tree_and_keeps_the_datadir_byte_and_inode_identical()
     {
        let home = provisioned_mariadb_home();
        let before = untouched_mariadb_fingerprint(home.path());
        let sup = supervisor_with("mariadb-11.4");
        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();

        let outcome = perform_uninstall(run_for_mariadb(home.path(), &sup, &lock, runner.clone()))
            .await
            .unwrap();

        assert_eq!(outcome, UninstallOutcome::Done);
        assert!(sup.snapshot().is_empty(), "the service row must be gone");
        assert!(
            !home.path().join("packages/mariadb/11.4").exists(),
            "the whole package tree must be gone"
        );
        assert_untouched(&before);
        // Named explicitly as well as covered by the fingerprint — the same
        // promise D2 (carried from the MySQL slice) makes to the user, in so
        // many words.
        assert!(
            home.path().join("data/mariadb/11.4/ibdata1").exists(),
            "THE datadir must survive"
        );
        // Never Homebrew (D5): MariaDB's inventory has no `Removal::
        // BrewFormula` step at all, so this runner — which WOULD record a
        // call for one — must have recorded nothing.
        assert!(
            runner.spawns().is_empty(),
            "a MariaDB uninstall must spawn NOTHING, recorded {:?}",
            runner.spawns()
        );
    }

    /// The MariaDB mirror of `the_stored_root_password_survives_the_uninstall`
    /// — the same D2 promise ("keeping the data and throwing away the key is
    /// the same as destroying it"), and `uninstall_plan`'s `keeps` list
    /// already says "The stored root password" for this target. Pinned here
    /// because nothing else does (audit Low 2): `MariadbInstanceRepo::delete`
    /// has zero production callers today and `rescan_mariadb_into_state`
    /// performs no deletion, so the property currently holds only by
    /// construction.
    ///
    /// Asserted on the value AND on `initialized_at`, not on content alone:
    /// `upsert` stamps `initialized_at` to `now_ms()` on EVERY call (its own
    /// doc comment), so a row that was deleted and immediately re-upserted
    /// with an identical password would still carry a FRESH timestamp — a
    /// content-only comparison cannot see that rewrite.
    ///
    /// A raw SQLite `rowid` read (the literal analogue of
    /// `untouched_mariadb_fingerprint`'s file-inode check above) was
    /// considered and rejected in favour of `initialized_at`: verified
    /// empirically against this exact schema, deleting and reinserting the
    /// ONLY row of a `major TEXT PRIMARY KEY` table reuses rowid 1 both
    /// times — the table passes through empty between the two statements,
    /// and SQLite's non-`AUTOINCREMENT` allocator restarts there. A file's
    /// inode changing on delete-and-recreate does not carry over to this
    /// table's rowid, so `initialized_at` is the signal that actually holds.
    ///
    /// VACUITY: confirmed by temporarily having `perform_uninstall` also
    /// delete the MariaDB credential row for this target — this test failed
    /// on the missing row (`.expect("row")` panicked) before either
    /// assertion below even ran; reverted immediately after.
    #[tokio::test]
    async fn the_stored_mariadb_root_password_survives_the_uninstall() {
        let home = provisioned_mariadb_home();
        let db = Db::open(home.path()).await.expect("state.db");
        let repo = MariadbInstanceRepo::new(&db);
        let password = generate_root_password();
        repo.upsert(&password).await.expect("upsert");
        let before = repo.get().await.expect("query").expect("row");

        let sup = supervisor_with("mariadb-11.4");
        let lock = InstallLock::default();
        perform_uninstall(run_for_mariadb(
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
        ))
        .await
        .unwrap();

        let after = repo.get().await.expect("query").expect("row");
        assert_eq!(
            after.root_password.expose(),
            password.expose(),
            "the root password must survive the engine"
        );
        assert_eq!(
            after.initialized_at, before.initialized_at,
            "the row was rewritten (a fresh `initialized_at`) even though its \
             content came back the same"
        );
    }

    /// An already-missing package tree is not an error — mirrors
    /// `an_already_missing_pool_config_is_not_an_error`'s reasoning, applied
    /// to `remove_dir_all`'s `NotFound` arm instead of `remove_file`'s.
    #[tokio::test]
    async fn an_already_missing_mariadb_package_tree_is_not_an_error() {
        let home = tempfile::tempdir().expect("tempdir");
        let sup = supervisor_with("mariadb-11.4");
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for_mariadb(
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
        ))
        .await
        .unwrap();

        assert_eq!(outcome, UninstallOutcome::Done);
    }

    /// A refused MariaDB uninstall (its own service still running) must
    /// spawn nothing and remove nothing — mirroring
    /// `a_live_service_refuses_the_uninstall_and_spawns_nothing`'s pattern
    /// for reaching a genuinely live row (a real spawnable `/bin/sleep`,
    /// polled until it leaves `Stopped`; `supervisor_with`'s `/nonexistent`
    /// program cannot reach `Running`/`Starting` at all), adapted to a
    /// target with no keg check (`keg: None`).
    #[tokio::test]
    async fn a_running_mariadb_service_refuses_the_uninstall_and_removes_nothing() {
        let home = provisioned_mariadb_home();
        let sup = Supervisor::new(default_driver());
        sup.register(ServiceSpec {
            id: "mariadb-11.4".into(),
            display_name: "MariaDB 11.4".into(),
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
        sup.start("mariadb-11.4").expect("start");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match sup.snapshot()[0].state {
                    ServiceState::Starting | ServiceState::Running => return,
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

        let err = perform_uninstall(run_for_mariadb(home.path(), &sup, &lock, runner.clone()))
            .await
            .unwrap_err();

        assert!(
            format!("{err}").contains("mariadb-11.4"),
            "the refusal must name the service: {err}"
        );
        assert!(
            runner.spawns().is_empty(),
            "a refusal must spawn NOTHING, recorded {:?}",
            runner.spawns()
        );
        assert!(
            home.path().join("packages/mariadb/11.4").exists(),
            "a refusal must not delete anything"
        );

        let _ = sup.stop("mariadb-11.4");
    }

    // ======================================================================
    // Off-Homebrew slice 5D T2 — the executor for a PHP major THIS app
    // packaged: a `remove_dir_all` that validates its own target.
    // ======================================================================

    /// `provisioned_home` plus a packaged PHP 8.4 laid down exactly the way
    /// `openvhost-pkg` lays one down — the version directory holding
    /// `bin/php-fpm`, and `current` a RELATIVE symlink naming the bare version.
    ///
    /// Built with `uninstall::tests`' own fixtures rather than a second
    /// spelling: the executor's job is to remove what the resolver found, so
    /// both halves have to be looking at the same tree.
    #[cfg(unix)]
    fn packaged_php_home(version: &str) -> tempfile::TempDir {
        let tmp = provisioned_home();
        install_packaged_php(tmp.path(), "8.4", version);
        point_current(tmp.path(), "8.4", version);
        tmp
    }

    /// [`run_for`] for a packaged target: no keg to resolve (`Target::formula`
    /// is `None`, so `uninstall_package` never looks one up) and the resolved
    /// packaged state threaded in.
    fn run_for_packaged<'a>(
        target: Target,
        home: &std::path::Path,
        sup: &'a Supervisor,
        lock: &'a InstallLock,
        runner: Arc<dyn BrewRunner>,
        packaged: PackagedPhp,
    ) -> UninstallRun<'a> {
        UninstallRun {
            target,
            home: home.to_path_buf(),
            brew: PathBuf::new(),
            sup,
            lock,
            sites: Vec::new(),
            keg: None,
            packaged: Some(packaged),
            runner,
            log: silent(),
        }
    }

    #[cfg(unix)]
    fn packages_root(home: &std::path::Path) -> PathBuf {
        openvhost_core::PackagesRoot::from_home(home)
            .as_path()
            .to_path_buf()
    }

    /// A directory outside the packages root holding one identifiable file, so
    /// "it survived" is checked on CONTENT rather than on the directory entry
    /// still being listed.
    #[cfg(unix)]
    fn outside_tree(root: &std::path::Path, rel: &str) -> PathBuf {
        let dir = root.join(rel);
        std::fs::create_dir_all(dir.join("bin")).expect("mkdir");
        std::fs::write(dir.join("bin/php-fpm"), b"someone else's fpm").expect("write");
        std::fs::write(dir.join("precious"), b"not ours").expect("write");
        dir
    }

    #[cfg(unix)]
    fn assert_outside_survived(dir: &std::path::Path) {
        assert_eq!(
            std::fs::read(dir.join("precious")).unwrap_or_else(|e| panic!(
                "{} was destroyed: {e}",
                dir.join("precious").display()
            )),
            b"not ours",
            "{} was rewritten",
            dir.display()
        );
        assert!(
            dir.join("bin/php-fpm").is_file(),
            "{}/bin/php-fpm was destroyed",
            dir.display()
        );
    }

    // ---- the guard, in isolation -----------------------------------------
    //
    // VACUITY for this whole group: weakening `confine` to
    // `Confinement::Contained` unconditionally left `..._is_contained` and
    // `..._is_absent...` passing and failed every refusal test — so the group
    // is discriminating in both directions rather than agreeing with a guard
    // that never refuses. Measured; see the task report.
    //
    // Re-measured for SHAPE 3 with a NARROWER weakening (fix wave): deleting
    // just the leaf-symlink arm from `confine` failed
    // `a_symlink_at_the_leaf_...` and both of its end-to-end tests while every
    // other test in this module — including the `current`-link one below —
    // stayed green. Each of the two changes in that wave is therefore pinned by
    // its own tests rather than by the pair of them together.

    #[cfg(unix)]
    #[test]
    fn a_real_version_directory_under_the_packages_root_is_contained() {
        let home = packaged_php_home("8.4.24");
        assert_eq!(
            confine(
                &packages_root(home.path()),
                &home.path().join("packages/php/8.4/8.4.24")
            ),
            Confinement::Contained
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_with_nothing_at_it_is_absent_rather_than_refused() {
        // The distinction the three-state answer exists for: "already gone" is
        // a finished uninstall, "could not be resolved" is a refusal.
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            confine(
                &packages_root(home.path()),
                &home.path().join("packages/php/8.4/8.4.24")
            ),
            Confinement::Absent
        );
    }

    /// SHAPE 1 of the two the security audit reproduced live. A symlinked
    /// SERIES directory: every component of the path is a plain legal name, the
    /// lexical direct-child check discovery uses passes, and the path resolves
    /// somewhere else entirely.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_series_directory_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        outside_tree(elsewhere.path(), "8.4.24");
        let php_tree = home.path().join("packages/php");
        std::fs::create_dir_all(&php_tree).expect("mkdir");
        std::os::unix::fs::symlink(elsewhere.path(), php_tree.join("8.4")).expect("symlink");

        let target = home.path().join("packages/php/8.4/8.4.24");
        // The lexical checks this does NOT rely on both pass on this path —
        // which is the point: without the guard below, nothing refuses it.
        assert_eq!(target.parent(), Some(php_tree.join("8.4").as_path()));
        assert!(target.starts_with(packages_root(home.path())));

        assert!(matches!(
            confine(&packages_root(home.path()), &target),
            Confinement::Refused { .. }
        ));
    }

    /// SHAPE 2. A symlinked VERSION directory — the leaf. `remove_dir_all`
    /// would unlink this one rather than follow it (measured on this
    /// toolchain), so the guard is not what stops a delete here; it is what
    /// stops the uninstall reporting success having removed a link while the
    /// program files it named are somewhere we never looked.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_version_directory_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let real = outside_tree(elsewhere.path(), "realtree");
        let major = home.path().join("packages/php/8.4");
        std::fs::create_dir_all(&major).expect("mkdir");
        std::os::unix::fs::symlink(&real, major.join("8.4.24")).expect("symlink");

        let target = major.join("8.4.24");
        assert_eq!(target.parent(), Some(major.as_path()));
        assert!(target.starts_with(packages_root(home.path())));

        assert!(matches!(
            confine(&packages_root(home.path()), &target),
            Confinement::Refused { .. }
        ));
    }

    /// The reason `starts_with` alone is not the predicate. `current` may name
    /// any single plain component — `packages` among them — so a symlinked
    /// series directory pointing at the home makes the target resolve to the
    /// packages root ITSELF, which `starts_with` calls contained. A
    /// `remove_dir_all` there takes every engine's tree at once.
    #[cfg(unix)]
    #[test]
    fn a_target_resolving_to_the_packages_root_itself_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let root = packages_root(home.path());
        std::fs::create_dir_all(root.join("mysql/8.4")).expect("mkdir");
        std::fs::create_dir_all(root.join("php")).expect("mkdir");
        // `packages/php/8.4` -> the home, so `packages/php/8.4/packages` IS
        // `packages`.
        std::os::unix::fs::symlink(home.path(), root.join("php/8.4")).expect("symlink");
        let target = root.join("php/8.4/packages");
        assert_eq!(
            std::fs::canonicalize(&target).expect("resolves"),
            std::fs::canonicalize(&root).expect("resolves"),
            "the fixture must actually resolve to the root, or this proves nothing"
        );

        assert!(matches!(
            confine(&root, &target),
            Confinement::Refused { .. }
        ));
    }

    /// `Path::starts_with` compares whole components; `str::starts_with` would
    /// call `<home>/packages-elsewhere/...` inside `<home>/packages`.
    #[cfg(unix)]
    #[test]
    fn a_sibling_directory_whose_name_merely_begins_with_the_root_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let root = packages_root(home.path());
        std::fs::create_dir_all(&root).expect("mkdir");
        let target = home.path().join("packages-elsewhere/php/8.4/8.4.24");
        std::fs::create_dir_all(&target).expect("mkdir");
        // The string comparison this must not be.
        assert!(
            target
                .display()
                .to_string()
                .starts_with(&root.display().to_string())
        );

        assert!(matches!(
            confine(&root, &target),
            Confinement::Refused { .. }
        ));
    }

    /// The accepted case, stated as a test so it is a decision rather than an
    /// oversight: canonicalizing the ROOT means a user who put
    /// `<home>/packages` on another volume keeps working. The tree moves; the
    /// removal stays inside the moved tree — and a component below it that
    /// diverges is still refused.
    #[cfg(unix)]
    #[test]
    fn a_relocated_packages_root_is_accepted_and_still_confines_what_is_below_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let volume = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let real_root = volume.path().join("openvhost-packages");
        std::fs::create_dir_all(real_root.join("php/8.4/8.4.24")).expect("mkdir");
        std::os::unix::fs::symlink(&real_root, packages_root(home.path())).expect("symlink");
        // A tempdir under /var canonicalizes to /private/var: the root check
        // would fail for EVERY test here if only the target were resolved.
        assert_ne!(
            std::fs::canonicalize(packages_root(home.path())).expect("resolves"),
            packages_root(home.path()),
            "the fixture must exercise a root that does not equal its own canonical form"
        );

        assert_eq!(
            confine(
                &packages_root(home.path()),
                &home.path().join("packages/php/8.4/8.4.24")
            ),
            Confinement::Contained
        );

        // …and the relocation is not a blanket pass: a series directory that
        // leaves the RELOCATED root is refused exactly as it would be at home.
        outside_tree(elsewhere.path(), "8.5.0");
        std::os::unix::fs::symlink(elsewhere.path(), real_root.join("php/8.5")).expect("symlink");
        assert!(matches!(
            confine(
                &packages_root(home.path()),
                &home.path().join("packages/php/8.5/8.5.0")
            ),
            Confinement::Refused { .. }
        ));
    }

    /// SHAPE 3, and the reason the two above are not enough: a symlink at the
    /// leaf that RESOLVES INSIDE the packages root.
    ///
    /// Both shapes above are refused because what they resolve to is outside
    /// the root — i.e. by the comparison, not by being links. This one passes
    /// that comparison, and it is where `confine` and the call it authorises
    /// stop talking about the same object: `canonicalize` follows the leaf and
    /// answers about the TARGET, while `remove_dir_all` does not follow it and
    /// acts on the LINK. Measured on this toolchain (security audit, fix wave):
    /// the old predicate said `Contained` here, and the delete then unlinked a
    /// directory entry the predicate had never looked at.
    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_leaf_is_refused_even_when_it_resolves_inside_the_packages_root() {
        let home = tempfile::tempdir().expect("tempdir");
        let major = home.path().join("packages/php/8.4");
        std::fs::create_dir_all(major.join("real-8.4.24/bin")).expect("mkdir");
        std::fs::write(major.join("real-8.4.24/bin/php-fpm"), b"the program files").expect("write");
        std::os::unix::fs::symlink(major.join("real-8.4.24"), major.join("8.4.24"))
            .expect("symlink");

        let target = major.join("8.4.24");
        // The fixture must reach the DISAGREEMENT, or this proves nothing: what
        // the resolving half sees really is inside the root, so every check
        // made after `canonicalize` passes on this path.
        assert!(
            std::fs::canonicalize(&target)
                .expect("resolves")
                .starts_with(std::fs::canonicalize(packages_root(home.path())).expect("resolves")),
            "the fixture must resolve INSIDE the root, or it is just SHAPE 2 again"
        );

        assert!(matches!(
            confine(&packages_root(home.path()), &target),
            Confinement::Refused { .. }
        ));
    }

    /// A dangling symlink where the version directory belongs is SOMETHING,
    /// and reading it as "already gone" would leave it on disk for discovery
    /// to trip over. It cannot be resolved, so it is refused.
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_where_the_version_directory_belongs_is_refused_not_absent() {
        let home = tempfile::tempdir().expect("tempdir");
        let major = home.path().join("packages/php/8.4");
        std::fs::create_dir_all(&major).expect("mkdir");
        std::os::unix::fs::symlink(home.path().join("gone"), major.join("8.4.24"))
            .expect("symlink");

        assert!(matches!(
            confine(&packages_root(home.path()), &major.join("8.4.24")),
            Confinement::Refused { .. }
        ));
    }

    // ---- the executor, end to end ----------------------------------------

    /// The whole packaged path: the tree goes, `current` goes with it,
    /// everything the brew path already keeps is kept, and Homebrew is never
    /// asked.
    ///
    /// The last assertion is the one that makes the `current` cleanup matter
    /// rather than being tidiness: discovery counts ANY entry left in the major
    /// directory as an install it could not identify, so a leftover link would
    /// make a successful uninstall render as a broken install.
    ///
    /// VACUITY: deleting the `clear_dangling_current` call left every other
    /// assertion passing and failed the two about `current` and
    /// `is_complete()`. Measured; see the task report.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_successful_packaged_php_uninstall_removes_the_tree_and_the_current_link() {
        let home = packaged_php_home("8.4.24");
        let before = untouched_fingerprint(home.path());
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();

        let packaged = php("8.4").packaged(home.path()).expect("a packaged 8.4");
        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            packaged,
        ))
        .await
        .unwrap();

        assert_eq!(outcome, UninstallOutcome::Done);
        assert!(
            !home.path().join("packages/php/8.4/8.4.24").exists(),
            "the version directory must be gone"
        );
        assert!(
            std::fs::symlink_metadata(home.path().join("packages/php/8.4/current")).is_err(),
            "`current` must not be left pointing at a tree that is gone"
        );
        assert!(
            !home
                .path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists(),
            "the generated pool config must be gone"
        );
        assert!(sup.snapshot().is_empty(), "the service row must be gone");
        assert_untouched(&before);
        // Never Homebrew (D2): a packaged inventory has no `BrewFormula` step,
        // so this runner — which WOULD record one — must have recorded nothing.
        assert!(
            runner.spawns().is_empty(),
            "a packaged uninstall must spawn NOTHING, recorded {:?}",
            runner.spawns()
        );
        // The user-visible consequence, asserted through the same discovery
        // that built the row they pressed Uninstall on.
        let found = openvhost_core::discover_php(
            &openvhost_core::PackagesRoot::from_home(home.path()),
            &[],
            &|_| None,
        );
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(
            found.is_complete(),
            "an uninstalled major must not be reported as an install we cannot identify: {found:?}"
        );
    }

    /// SHAPE 1, end to end. The escape the guard exists for: a symlinked
    /// SERIES directory, where `remove_dir_all` follows the intermediate
    /// component and deletes the real contents behind it — returning `Ok(())`,
    /// so nothing after the fact could tell.
    ///
    /// Asserted on the outside tree STILL EXISTING, not on the call having
    /// returned an error: with this call an error is not evidence and a success
    /// is not evidence either.
    ///
    /// VACUITY: with `confine`'s result ignored (`remove_dir_all` called
    /// unconditionally), this test failed on `precious` being destroyed —
    /// i.e. the escape is real and this fixture reaches it. Measured; see the
    /// task report.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_series_directory_refuses_the_removal_and_the_outside_tree_survives() {
        let home = provisioned_home();
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let real = outside_tree(elsewhere.path(), "8.4.24");
        let php_tree = home.path().join("packages/php");
        std::fs::create_dir_all(&php_tree).expect("mkdir");
        std::os::unix::fs::symlink(elsewhere.path(), php_tree.join("8.4")).expect("symlink");
        point_current(home.path(), "8.4", "8.4.24");

        // The resolver still finds it — every check it makes is satisfied —
        // so the path really does reach the executor.
        let packaged = php("8.4")
            .packaged(home.path())
            .expect("the lexical checks pass on this tree");
        assert_eq!(
            packaged.version_dir,
            home.path().join("packages/php/8.4/8.4.24")
        );
        assert!(packaged.version_dir.starts_with(packages_root(home.path())));

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let runner = RecordingRunner::ok();
        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            runner.clone(),
            packaged,
        ))
        .await;

        // FIRST, and deliberately before the `Result` is even looked at: with
        // this call an error is not evidence and a success is not evidence
        // either, so what the outside tree looks like afterwards is the only
        // thing that can be. Asserting the refusal first would make the
        // reported failure "expected Err, got Ok" for a run that had just
        // deleted somebody else's directory.
        assert_outside_survived(&real);

        let text = format!("{}", outcome.expect_err("the removal must be refused"));
        assert!(
            text.contains("PHP 8.4 was not removed") && text.contains("Nothing was removed"),
            "the refusal must say the version is still installed: {text}"
        );
        // And nothing local moved either: the refusal returns before the pool
        // config and the service row are touched.
        assert!(
            home.path()
                .join("config/generated/php/8.4/php-fpm.conf")
                .exists(),
            "a refusal must not delete the pool config"
        );
        assert_eq!(sup.snapshot().len(), 1, "nor the service row");
        assert!(runner.spawns().is_empty(), "recorded {:?}", runner.spawns());
    }

    /// SHAPE 2, end to end: a symlinked VERSION directory.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_version_directory_refuses_the_removal_and_the_outside_tree_survives() {
        let home = provisioned_home();
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let real = outside_tree(elsewhere.path(), "realtree");
        let major = home.path().join("packages/php/8.4");
        std::fs::create_dir_all(&major).expect("mkdir");
        std::os::unix::fs::symlink(&real, major.join("8.4.24")).expect("symlink");
        point_current(home.path(), "8.4", "8.4.24");

        let packaged = php("8.4")
            .packaged(home.path())
            .expect("the lexical checks pass on this tree");
        assert_eq!(packaged.version_dir, major.join("8.4.24"));

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            packaged,
        ))
        .await;

        // Disk first, `Result` second — see the SHAPE 1 test for why. Here the
        // link itself surviving is the discriminating half: `remove_dir_all`
        // would UNLINK this leaf and report success, leaving the outside tree
        // intact and the program files still installed under a name the
        // uninstall claimed to have removed.
        assert_outside_survived(&real);
        assert!(
            std::fs::symlink_metadata(major.join("8.4.24"))
                .expect("the link survives")
                .file_type()
                .is_symlink(),
            "this refused; it did not half-remove"
        );
        assert!(std::fs::read_link(major.join("current")).is_ok());

        let err = outcome.expect_err("the removal must be refused");
        assert!(
            format!("{err}").contains("PHP 8.4 was not removed"),
            "{err}"
        );
    }

    /// SHAPE 3, end to end, and THE escape the security audit reproduced live:
    /// `confine` judged the object `canonicalize` reached, `remove_dir_all`
    /// acted on a different one, and the entry it unlinked was OUTSIDE the
    /// packages root.
    ///
    /// The series directory leaves the tree and the leaf inside it points back
    /// in, so the resolved path is a real directory under `<home>/packages` —
    /// which is exactly what the old predicate answered about — while the
    /// directory entry `remove_dir_all` removes lives in someone else's
    /// directory. Nothing reports either half: the call returns `Ok(())`.
    ///
    /// VACUITY: this is the pre-fix behaviour, measured. With the leaf-symlink
    /// refusal removed from `confine`, this test fails on the outside entry
    /// having been unlinked — i.e. the fixture really reaches the escape.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_leaf_symlink_resolving_back_into_the_tree_does_not_unlink_outside_it() {
        let home = provisioned_home();
        let elsewhere = tempfile::tempdir().expect("tempdir");
        // A real version directory INSIDE the root for the leaf to resolve to,
        // so the resolving half of the guard sees a path it is happy with…
        install_packaged_php(home.path(), "8.3", "8.3.0");
        let php_tree = home.path().join("packages/php");
        std::fs::create_dir_all(&php_tree).expect("mkdir");
        // …reached through a series directory that leaves the tree…
        std::os::unix::fs::symlink(elsewhere.path(), php_tree.join("8.4")).expect("symlink");
        // …and a leaf, outside, that points back in.
        let outside_entry = elsewhere.path().join("8.4.24");
        std::os::unix::fs::symlink(home.path().join("packages/php/8.3/8.3.0"), &outside_entry)
            .expect("symlink");
        point_current(home.path(), "8.4", "8.4.24");

        // Resolved the way production resolves it: every check the resolver
        // makes is satisfied, so this really does reach the executor.
        let packaged = php("8.4")
            .packaged(home.path())
            .expect("the lexical checks pass on this tree");
        assert_eq!(
            packaged.version_dir,
            home.path().join("packages/php/8.4/8.4.24")
        );
        assert!(
            std::fs::canonicalize(&packaged.version_dir)
                .expect("resolves")
                .starts_with(std::fs::canonicalize(packages_root(home.path())).expect("resolves")),
            "the fixture must resolve INSIDE the root, or the escape it reproduces is not this one"
        );

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            packaged,
        ))
        .await;

        // Disk first, `Result` second — see the SHAPE 1 test for why. A
        // redirected delete returns `Ok(())`, so the `Result` is not evidence
        // about what is left on disk.
        assert!(
            std::fs::symlink_metadata(&outside_entry).is_ok(),
            "{} was unlinked: the removal reached outside the packages root",
            outside_entry.display()
        );
        assert!(
            home.path()
                .join("packages/php/8.3/8.3.0/bin/php-fpm")
                .is_file(),
            "nor may the version the leaf resolved to be touched"
        );

        let err = outcome.expect_err("the removal must be refused");
        assert!(
            format!("{err}").contains("PHP 8.4 was not removed"),
            "{err}"
        );
    }

    /// The same shape with no attacker in it at all, and the reason this is a
    /// correctness fix as well as a containment one: a version directory that
    /// is a symlink used to be "removed" by unlinking the link, leaving the
    /// program files installed while the uninstall reported [`Done`].
    ///
    /// VACUITY: with the leaf-symlink refusal removed from `confine`, this
    /// fails on the LINK being gone — `NotFound` where a symlink should be —
    /// which is the pre-fix behaviour exactly. Measured.
    ///
    /// [`Done`]: UninstallOutcome::Done
    #[cfg(unix)]
    #[tokio::test]
    async fn a_version_directory_that_is_a_symlink_is_refused_rather_than_reported_done() {
        let home = provisioned_home();
        let major = home.path().join("packages/php/8.4");
        // The program files, under a name `current` does not select…
        install_packaged_php(home.path(), "8.4", "8.4.24-real");
        // …and the name it does select, a link to them.
        std::os::unix::fs::symlink(major.join("8.4.24-real"), major.join("8.4.24"))
            .expect("symlink");
        point_current(home.path(), "8.4", "8.4.24");

        let packaged = php("8.4")
            .packaged(home.path())
            .expect("the lexical checks pass on this tree");
        assert_eq!(packaged.version_dir, major.join("8.4.24"));

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            packaged,
        ))
        .await;

        // Disk first. The discriminating half is the LINK surviving: the
        // program files survive either way, which is the whole complaint — a
        // run that unlinked the link and reported success left them installed
        // under a name it had just claimed to remove.
        assert!(
            std::fs::symlink_metadata(major.join("8.4.24"))
                .expect("the link survives")
                .file_type()
                .is_symlink(),
            "this refused; it did not half-remove"
        );
        assert!(
            major.join("8.4.24-real/bin/php-fpm").is_file(),
            "the program files must still be installed"
        );

        let err = outcome.expect_err(
            "an uninstall that leaves the program files installed must not report success",
        );
        assert!(
            format!("{err}").contains("PHP 8.4 was not removed"),
            "{err}"
        );
    }

    /// The discriminating half of the `current` handling: the link is removed
    /// because it DANGLES, not because an uninstall ran. One still pointing at
    /// a version directory that survived must be left exactly where it is —
    /// removing it would break a working install.
    ///
    /// The state is built by hand rather than resolved: `Target::packaged`
    /// reads the removal path OFF `current`, so a resolved fixture can never
    /// have the link naming a different version than the one being removed.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_current_link_pointing_at_a_surviving_version_is_left_alone() {
        let home = packaged_php_home("8.4.23");
        install_packaged_php(home.path(), "8.4", "8.4.24");
        // `current` selects 8.4.23; the executor is told to remove 8.4.24.
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            PackagedPhp {
                version_dir: home.path().join("packages/php/8.4/8.4.24"),
                brew_keg: None,
            },
        ))
        .await
        .unwrap();

        assert_eq!(outcome, UninstallOutcome::Done);
        assert!(!home.path().join("packages/php/8.4/8.4.24").exists());
        assert_eq!(
            std::fs::read_link(home.path().join("packages/php/8.4/current")).expect("still linked"),
            PathBuf::from("8.4.23"),
            "a `current` that still resolves must not be removed"
        );
        assert!(
            home.path()
                .join("packages/php/8.4/8.4.23/bin/php-fpm")
                .is_file(),
            "nor may the version it names be touched"
        );
    }

    /// A tree already removed by a previous attempt that then failed: the
    /// removal is `Absent`, and the link it left behind is still cleared.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_already_removed_tree_still_gets_its_dangling_current_cleared() {
        let home = packaged_php_home("8.4.24");
        std::fs::remove_dir_all(home.path().join("packages/php/8.4/8.4.24")).expect("remove");
        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();

        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            PackagedPhp {
                version_dir: home.path().join("packages/php/8.4/8.4.24"),
                brew_keg: None,
            },
        ))
        .await
        .unwrap();

        assert_eq!(outcome, UninstallOutcome::Done);
        assert!(
            std::fs::symlink_metadata(home.path().join("packages/php/8.4/current")).is_err(),
            "the leftover link must be cleared even when the tree was already gone"
        );
    }

    /// The `current` cleanup is the one filesystem call in this module that
    /// used to be unconfined, and this is the state that reaches outside with
    /// it: a series directory that is a symlink OUT of the packages root, with
    /// a dangling `current` beyond it.
    ///
    /// `remove_file` does not follow the final component — which is what makes
    /// it safe on the link itself — but it does follow every component above
    /// it, so the entry it unlinks here belongs to whatever directory the
    /// series link names. Reachable on the `Absent` arm precisely because
    /// nothing was refused: there is no version directory at all.
    ///
    /// VACUITY: with the containment check removed from
    /// `clear_dangling_current`, this test fails on the outside entry having
    /// been unlinked. Measured.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dangling_current_beyond_a_symlinked_series_directory_is_not_unlinked() {
        let home = provisioned_home();
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let php_tree = home.path().join("packages/php");
        std::fs::create_dir_all(&php_tree).expect("mkdir");
        std::os::unix::fs::symlink(elsewhere.path(), php_tree.join("8.4")).expect("symlink");
        // Someone else's `current`, dangling, outside the packages root.
        let outside_link = elsewhere.path().join("current");
        std::os::unix::fs::symlink(PathBuf::from("8.4.24"), &outside_link).expect("symlink");

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        // Built by hand rather than resolved: there is no version directory
        // here at all, so `Target::packaged` finds nothing — which is the point.
        // The removal is `Absent`, and the `current` cleanup runs anyway.
        let outcome = perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            PackagedPhp {
                version_dir: home.path().join("packages/php/8.4/8.4.24"),
                brew_keg: None,
            },
        ))
        .await;

        // Disk first: `remove_current_link` returns `Ok(())` whether it removed
        // the right link, the wrong one, or none at all.
        assert!(
            std::fs::symlink_metadata(&outside_link).is_ok(),
            "{} was unlinked: the `current` cleanup reached outside the packages root",
            outside_link.display()
        );

        // …and it is REPORTED rather than swallowed: a `current` left in place
        // is the difference between a clean uninstall and a major discovery
        // reads as an install it cannot identify.
        match outcome.expect("the run itself does not fail") {
            UninstallOutcome::Incomplete(problems) => assert!(
                problems.iter().any(|p| p.contains("current")),
                "got {problems:?}"
            ),
            UninstallOutcome::Done => {
                panic!("a `current` that could not be cleared must not report success")
            }
        }
    }

    /// The promise the brew path already makes, checked on the packaged path:
    /// logs, pool overrides and every site's saved PHP version survive.
    ///
    /// The site row is read back from state.db rather than trusted from the
    /// `sites` vector the executor was handed — a repo call that silently
    /// dropped it would return `Ok` all the same. Asserted on `updated_at` as
    /// well as on the version, for the reason
    /// `the_stored_mariadb_root_password_survives_the_uninstall` gives: a row
    /// deleted and rewritten with identical content carries a fresh timestamp,
    /// which a content-only comparison cannot see.
    #[cfg(unix)]
    #[tokio::test]
    async fn logs_overrides_and_every_sites_saved_php_version_survive_a_packaged_uninstall() {
        let home = packaged_php_home("8.4.24");
        let before = untouched_fingerprint(home.path());
        let db = Db::open(home.path()).await.expect("state.db");
        let repo = SqliteSiteRepository::new(&db);
        // Pinned to a DIFFERENT major: a site pinned to 8.4 would block the
        // uninstall outright (`Blocker::SitesPinned`), so the surviving-setting
        // promise is only observable on a site that does not block.
        let pinned = site("shop", "shop.localhost", "8.3");
        let saved = repo
            .create(openvhost_core::NewSite {
                name: pinned.name,
                domain: pinned.domain,
                docroot: pinned.docroot,
                web_server: pinned.web_server,
                php_version: pinned.php_version,
                enabled: pinned.enabled,
            })
            .await
            .expect("create site");

        let sup = supervisor_with("php-fpm-8.4");
        let lock = InstallLock::default();
        let packaged = php("8.4").packaged(home.path()).expect("a packaged 8.4");
        perform_uninstall(run_for_packaged(
            php("8.4"),
            home.path(),
            &sup,
            &lock,
            RecordingRunner::ok(),
            packaged,
        ))
        .await
        .unwrap();

        assert_untouched(&before);
        let after = repo.list().await.expect("list");
        assert_eq!(after.len(), 1, "got {after:?}");
        assert_eq!(after[0].php_version.as_str(), "8.3");
        assert_eq!(
            after[0].updated_at, saved.updated_at,
            "the row was rewritten even though its content came back the same"
        );
    }
}
