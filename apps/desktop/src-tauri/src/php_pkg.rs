// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP's own package tree, on the wire (off-Homebrew slice 5C, design D1/D3/D4).
//!
//! A sibling of `mysql_pkg` and `mariadb_pkg` rather than more of `commands.rs`
//! (already ~10 000 lines), holding what the Languages page needs now that a PHP
//! can arrive from somewhere other than Homebrew:
//!
//! 1. **Where a listed runtime came from** — [`PhpRuntimeSourceDto`]. Slice 5B
//!    made `openvhost_core::PhpRuntimeSource` real and deliberately did NOT add
//!    its wire copy, because nothing rendered it yet. This is that copy.
//! 2. **Whether this build publishes a package for a major on this host** —
//!    [`PhpPackageOfferDto`].
//! 3. **Which install route a major takes, and how that install ended** —
//!    [`route_for`] and [`PhpInstallResultDto`] (design D4). `install_php` is
//!    ONE command that routes; the frontend does not re-derive the rule, and the
//!    two routes' outcomes are two arms of one tagged result rather than two
//!    shapes a caller has to tell apart.
//!
//! **PHP is in MariaDB's situation, not MySQL's.** `MysqlPackageOfferDto` has
//! two states because Oracle publishes its binaries directly, so a pinned entry
//! is fetchable the moment it exists. PHP's artifact is one *we* build and
//! publish (php-recipe design D5), so — exactly like MariaDB's — a pin can be
//! completely correct while the release that would serve it does not exist yet.
//! That is [`PhpPackageOfferDto::AwaitingRelease`], and collapsing it into
//! `Unavailable` would tell an Apple Silicon owner their machine is unsupported
//! when the truth is "nobody can have this yet".
//!
//! **Per major, not per app** (design D1). MariaDB ships one series and left
//! `major` off its own types; PHP's whole point is several majors side by side,
//! so the offer is answered per major and rides on the row
//! ([`crate::commands::PhpRuntimeDto::offer`]) rather than on the environment.
//!
//! **Today every offer this build can make is `AwaitingRelease` or
//! `Unavailable`** — `php-8.4.24` is pinned but unpublished, and no other major
//! has a built artifact at all (`openvhost_core::PHP_PACKAGES`). Nothing here
//! is therefore installable from our own tree, and [`route_for`] sends every
//! major to Homebrew on every real machine today.
//!
//! **The result cannot be brew-shaped, and that is why [`PhpInstallResultDto`]
//! exists.** The outcome `install_php` used to return carried
//! `exit_code: Option<i32>`, and `LanguageRow.svelte` derived
//! `installFailed = exitCode !== 0` from it. That
//! is right for a child process — `None` means "killed by a signal", which is
//! not a clean exit — and meaningless for an installer that spawns none: a
//! SUCCESSFUL packaged install has no exit code at all, so it would have
//! rendered under `role="alert"` as "brew was killed before installing PHP 8.4
//! finished". A tagged result makes that misreading impossible, because
//! [`PhpInstallResultDto::Installed`] carries no `exit_code` field to compare.
//!
//! **The packaged install path merges UNPROVEN.** Every offer this build can
//! make is `AwaitingRelease` or `Unavailable`, so no test and no live run can
//! drive a real packaged PHP install end to end — the same position the MariaDB
//! UI slice merged in. What IS proven here is the routing, the classification
//! of each failure onto a renderable state, and that the Homebrew route is
//! untouched. Before any `availability` flips to `Published`, someone fetches
//! the served bytes once and confirms the SHA-256 by hand.

use std::sync::RwLock;

use openvhost_core::php::Availability;
use openvhost_core::{Db, InstalledRuntimes, PackageTarget};
use openvhost_proc::Supervisor;
use tauri::Manager;
use tauri_specta::Event;

use crate::commands::{
    InstallLock, IpcError, PHP_INSTALL_RUN, RunningInstallGuard, now_ms, rescan_into_state,
};
use crate::mysql_pkg::ProgressThrottle;
use crate::stack::StackPaths;

/// Where a listed PHP runtime's binaries came from — the wire copy of
/// `openvhost_core::PhpRuntimeSource` (PHP-discovery design D1, slice 5B).
///
/// Transcribed from `NginxRuntimeSourceDto`/`MysqlRuntimeSourceDto` rather than
/// reinvented: all three ask the identical question — "which install put these
/// bytes here" — and nothing about PHP's answer needs a different shape.
/// `PhpRuntimeSource::as_str()` stays the one machine-facing spelling for each
/// source; `the_wire_tag_is_php_runtime_source_as_str` below pins this type's
/// serialized `kind` to it for every variant, so the two cannot drift into
/// different words for the same fact.
///
/// `Homebrew` carries **no version, on purpose**, and this is the field that
/// makes the asymmetry visible: a packaged runtime's exact version is a
/// directory name chosen at install time, so reporting it costs nothing, while
/// Homebrew's would have to be probed — and the only prober we have returns
/// `major.minor`, never a patch level. Reporting the major as though it were
/// the full version would be a lie no caller could detect.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhpRuntimeSourceDto {
    Packaged { version: String },
    Homebrew,
}

impl From<&openvhost_core::PhpRuntimeSource> for PhpRuntimeSourceDto {
    fn from(s: &openvhost_core::PhpRuntimeSource) -> Self {
        use openvhost_core::PhpRuntimeSource as S;
        match s {
            S::Packaged { version } => Self::Packaged {
                version: version.clone(),
            },
            S::Homebrew => Self::Homebrew,
        }
    }
}

/// Whether this build can install a given PHP major from its own package tree
/// on THIS host, and what it would install — the three states
/// `MariadbPackageOfferDto` spells (`mariadb_pkg.rs`), mirrored exactly.
///
/// Matched exhaustively wherever it is consumed, with **no wildcard arm**: a
/// fourth state must be decided about rather than silently folded into one of
/// the first three.
///
/// `AwaitingRelease`'s own meaning is the one that matters today: the next
/// action belongs to the **maintainer, not the user**, so a row in that state
/// must say what it is waiting for rather than offer a button that would 404.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhpPackageOfferDto {
    Available {
        version: String,
    },
    /// The pinned build exists and was audited, but the GitHub release that
    /// would serve it has not been published — the next action belongs to the
    /// maintainer, not the user. `tag` is the release to publish, e.g.
    /// `"php-8.4.24"`.
    AwaitingRelease {
        tag: String,
    },
    Unavailable {
        target: String,
    },
}

/// What this build would install for `major` on `target`.
///
/// `target` is an explicit `Option`, mirroring `mysql_pkg::package_offer_for`
/// and `mariadb_pkg::package_offer_for` exactly and for the identical reason:
/// **both refusal branches must be reachable from a test on any one machine.**
/// A mutation that returned an offer for every refusal once survived a whole
/// suite green on Apple Silicon because the Intel arm never executed there.
///
/// `major` is a `&str`, unlike `openvhost_core::php_package_for_target`'s
/// `&PhpMajor`, because the Languages page has rows this build does not manage
/// at all — a hand-installed 7.4, or a major a later catalogue drops (see
/// `PhpRuntimeDto::cataloged`). Such a major cannot be parsed into a
/// `PhpMajor` (that constructor is catalogue-gated, deliberately, because it
/// also guards a `brew` argv), and the honest answer for it is the same
/// absence a cataloged-but-unbuilt 8.1 gets: this build publishes no artifact
/// for it. Parsing is therefore done here and **any** failure to resolve is an
/// absence — this deliberately does not read an error payload to decide, so a
/// future refusal reason cannot accidentally become an offer.
///
/// Nothing here reaches a child process, a URL or a hash: the lookup is a
/// compiled-in table (`openvhost_core::PHP_PACKAGES`) keyed by a parsed major
/// and a `PackageTarget`.
pub(crate) fn package_offer_for(major: &str, target: Option<PackageTarget>) -> PhpPackageOfferDto {
    // `PackageTarget` is named through its own `as_str`; `None` is the host
    // this programme publishes nothing for at all.
    let named = match target {
        Some(t) => t.as_str().to_string(),
        None => "this host".to_string(),
    };
    let Ok(major) = openvhost_core::PhpMajor::parse(major) else {
        return PhpPackageOfferDto::Unavailable { target: named };
    };
    match openvhost_core::php_package_for_target(&major, target) {
        // Exhaustive on `php::Availability`, no wildcard: a third availability
        // state would have to be decided about here too, not silently treated
        // as an offer.
        Ok(entry) => match entry.availability {
            Availability::Published => PhpPackageOfferDto::Available {
                version: entry.version.to_string(),
            },
            Availability::AwaitingRelease { tag } => PhpPackageOfferDto::AwaitingRelease {
                tag: tag.to_string(),
            },
        },
        Err(_) => PhpPackageOfferDto::Unavailable { target: named },
    }
}

/// What this build would install for `major` on the host it was compiled for.
pub(crate) fn package_offer(major: &str) -> PhpPackageOfferDto {
    package_offer_for(major, PackageTarget::host())
}

/// Which of `install_php`'s two pipelines a major's offer selects (design D4).
///
/// Deliberately NOT on the wire. The webview never sends a route and never
/// derives one: `install_php` re-reads [`package_offer`] itself — the same
/// compiled-in table that filled the row's `offer` field — so a caller cannot
/// choose which pipeline runs, only which major to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhpInstallRoute {
    /// OpenVHost's own package tree: download → SHA-256 verify → extract →
    /// atomic install. No child process, and therefore no exit code.
    Package,
    /// `brew install php@<major>`, exactly as before this slice.
    Homebrew,
}

/// The routing rule, in one place (design D4: "the row's own offer decides;
/// the frontend does not re-derive the rule").
///
/// Matched exhaustively with **no wildcard arm**, and each state is decided
/// about separately rather than folded into an or-pattern: this is the
/// compile-time site that makes a fourth offer state a routing decision rather
/// than a silent inheritance of whatever the last arm happened to say.
///
/// **`Unavailable` is the ordinary path, not the failure path** (design D4).
/// Four of the five catalogued majors carry it today, every major carries it on
/// Intel, and it routes to Homebrew with no apology: that is a supported route,
/// not a fallback a user should feel they are on.
///
/// **`AwaitingRelease` routes to Homebrew too**, and getting this wrong would
/// have removed a working control. On this Apple Silicon machine 8.4 is
/// `AwaitingRelease` today *and* has a functioning Homebrew Install button
/// (spec §8.5, corrected). What `AwaitingRelease` withholds is the PACKAGED
/// route — sending it there would return [`PhpInstallResultDto::AwaitingRelease`]
/// and install nothing, on the one major most likely to be pressed.
pub(crate) fn route_for(offer: &PhpPackageOfferDto) -> PhpInstallRoute {
    match offer {
        PhpPackageOfferDto::Available { .. } => PhpInstallRoute::Package,
        PhpPackageOfferDto::AwaitingRelease { .. } => PhpInstallRoute::Homebrew,
        PhpPackageOfferDto::Unavailable { .. } => PhpInstallRoute::Homebrew,
    }
}

/// One step of the packaged install pipeline, as the user watches it — the wire
/// copy of `openvhost_pkg::Progress`, identical in shape to
/// `mysql_pkg::MysqlInstallProgressDto` and `mariadb_pkg`'s (the pipeline
/// itself is shared). Kept as its own type rather than reused, mirroring every
/// other per-engine DTO in this app: the wire shapes must be able to diverge
/// without an edit to one silently reaching the others.
///
/// Emitted only on the [`PhpInstallRoute::Package`] route. The Homebrew route
/// still streams `commands::PhpInstallLogEvent` — brew's own output — and this
/// slice does not touch it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhpInstallProgressDto {
    Started {
        total: Option<u64>,
    },
    Downloaded {
        bytes: u64,
    },
    /// Its own variant, never folded into `Extracted`: golden rule 6 makes
    /// SHA-256 verification a requirement, and a requirement the UI cannot
    /// distinguish structurally is one a substring match on prose would
    /// "prove".
    Verified,
    Extracted,
    Linked,
}

impl From<openvhost_core::Progress> for PhpInstallProgressDto {
    fn from(p: openvhost_core::Progress) -> Self {
        use openvhost_core::Progress as P;
        match p {
            P::Started { total } => Self::Started { total },
            P::Downloaded { bytes } => Self::Downloaded { bytes },
            P::Verified => Self::Verified,
            P::Extracted => Self::Extracted,
            P::Linked => Self::Linked,
        }
    }
}

/// One pipeline step, forwarded live while a packaged install runs.
///
/// Carries `major`, unlike [`crate::mariadb_pkg::MariadbInstallProgressEvent`]
/// and exactly like `mysql_pkg::MysqlInstallProgressEvent`: MariaDB ships one
/// series so a field nothing can vary would be overhead, while PHP's whole
/// point is several majors side by side and a progress bar has to know which
/// row it belongs to.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct PhpInstallProgressEvent {
    pub major: String,
    pub ts_ms: u64,
    pub progress: PhpInstallProgressDto,
}

/// Whether a packaged install was also recorded in `state.db`'s package ledger
/// — the PHP mirror of `mariadb_pkg::MariadbLedgerWriteDto`, over the same
/// `openvhost_core::mysql::LedgerWrite` (the ledger is package-agnostic), so
/// only the WIRE type is duplicated and not the underlying model.
///
/// A state rather than a boolean, for the reason its Rust counterpart is one:
/// the failure carries a reason worth showing. The package is installed either
/// way — the tree is the inventory — so a failed row costs provenance, never
/// correctness.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhpLedgerWriteDto {
    Recorded,
    Failed { reason: String },
}

impl From<openvhost_core::mysql::LedgerWrite> for PhpLedgerWriteDto {
    fn from(w: openvhost_core::mysql::LedgerWrite) -> Self {
        use openvhost_core::mysql::LedgerWrite as L;
        match w {
            L::Recorded { .. } => Self::Recorded,
            L::Failed { reason } => Self::Failed { reason },
        }
    }
}

/// How one `install_php` call ended — **both routes, one union** (design D4).
///
/// The two families are deliberately not interchangeable:
///
/// * [`Self::Brew`] is the only arm carrying an `exit_code`, because it is the
///   only arm with a child process. Everything the Homebrew route reported
///   before this slice is here, unchanged and under one tag.
/// * Every other arm is the package pipeline's. A verification failure, a
///   stall, an unpublished release and an unsupported host are **states of the
///   result**, never thrown errors: throwing them would discard exactly the
///   distinction `PhpPackageOfferDto`'s three states exist to make.
///
/// Note what [`Self::Installed`] does NOT have: an exit code. That absence is
/// the fix — see this module's own doc comment for the render it prevents.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhpInstallResultDto {
    /// `brew install php@<major>` ran to completion (cleanly or not).
    ///
    /// **The per-variant `rename_all` is load-bearing, not decoration.** On an
    /// enum, serde's container-level `rename_all` renames the VARIANTS and not
    /// their fields, so `exit_code` reached the webview as `exit_code` while
    /// every hand-written consumer read `exitCode` — the same snake_case seam
    /// that made `fieldErrors` mark nothing. Every sibling result enum in this
    /// app happens to have single-word fields only, so this is the first place
    /// it could bite. `the_wire_uses_camel_case_keys_everywhere` is what keeps
    /// it fixed; deleting this attribute reddens it.
    ///
    /// `exit_code` is brew's own, and `None` means it was killed by a signal
    /// rather than exiting — "not a clean exit", which is what makes
    /// `exitCode !== 0` the right test HERE and the wrong test anywhere else.
    ///
    /// `detected` answers the silent-failure case this project keeps catching:
    /// brew reporting success while no `php-fpm` appears afterwards. It comes
    /// from a stat of the formula directory brew was asked to create, never
    /// from a version probe — deriving it from a probe is what made a
    /// successful `brew install mysql@8.4` report failure.
    #[serde(rename_all = "camelCase")]
    Brew {
        exit_code: Option<i32>,
        detected: bool,
    },
    /// The packaged route finished: the tree is on disk and `current` points at
    /// it. `version` is the exact patch level the catalogue pinned.
    Installed {
        version: String,
        detected: bool,
        ledger: PhpLedgerWriteDto,
    },
    AlreadyInstalled {
        version: String,
    },
    /// The run's future was dropped — `cancel_php_install`, or `perform_quit`
    /// on the way out. Staging unwound with it, so this is a plain "nothing
    /// happened", not a failure to explain away.
    Cancelled,
    VerificationFailed {
        expected: String,
        actual: String,
    },
    Stalled {
        detail: String,
    },
    /// The pinned build exists but the release that would serve it has not been
    /// published — see [`PhpPackageOfferDto::AwaitingRelease`]. `tag` is the
    /// release a human has to create.
    ///
    /// [`route_for`] sends an `AwaitingRelease` OFFER to Homebrew, so reaching
    /// this arm means the catalogue changed under a run in flight. It is
    /// classified rather than flattened anyway: `install_php_package` refuses
    /// with `CoreError::PackageNotPublished` before any network or filesystem
    /// work, and that refusal must survive the trip to the webview as itself.
    AwaitingRelease {
        tag: String,
    },
    Unavailable {
        target: String,
    },
    Failed {
        reason: String,
    },
}

/// `install_php`'s return: the major it was for, and how it ended.
///
/// `major` sits OUTSIDE the result union because every consumer needs it to
/// attribute the outcome to a row, on every branch — the same reasoning
/// `mysql_pkg::MysqlInstallOutcomeDto` states for its own copy, and the reason
/// PHP follows MySQL here rather than MariaDB (which ships one series and so
/// has nothing to attribute).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PhpInstallOutcomeDto {
    pub major: String,
    pub result: PhpInstallResultDto,
}

/// Map a failed package install onto a renderable state.
///
/// Exhaustive over `PkgError` with **no wildcard arm**, mirroring
/// `mysql_pkg::pkg_failure` and `mariadb_pkg::pkg_failure` exactly.
fn pkg_failure(e: openvhost_core::PkgError) -> PhpInstallResultDto {
    use openvhost_core::PkgError as E;
    match e {
        E::HashMismatch { expected, actual } => {
            PhpInstallResultDto::VerificationFailed { expected, actual }
        }
        stalled @ E::DownloadStalled { .. } => PhpInstallResultDto::Stalled {
            detail: stalled.to_string(),
        },
        E::AlreadyInstalled { version, .. } => PhpInstallResultDto::AlreadyInstalled { version },
        other @ (E::InvalidComponent { .. }
        | E::InvalidUrl(_)
        | E::InvalidSha256
        | E::InvalidWarmupPath { .. }
        | E::Network(_)
        | E::TooLarge { .. }
        | E::UnsafeArchive(_)
        | E::Io { .. }
        | E::Internal(_)
        | E::Unsupported(_)) => PhpInstallResultDto::Failed {
            reason: other.to_string(),
        },
    }
}

/// The same mapping one level up, for the error type `install_php_package`
/// actually returns.
///
/// `PackageNotPublished` and `NoPackageForTarget` get their own arms rather
/// than falling into the generic tail: both are refusals raised BEFORE any
/// network or filesystem work, and both name the one thing a reader needs (the
/// tag a maintainer must publish; the target no artifact exists for). Flattening
/// either into `Failed { reason }` would turn a fully-understood state into
/// prose and hand the page a red banner where it should render an explanation.
pub(crate) fn install_failure(e: openvhost_core::CoreError) -> PhpInstallResultDto {
    match e {
        openvhost_core::CoreError::NoPackageForTarget { target, .. } => {
            PhpInstallResultDto::Unavailable {
                target: target.to_string(),
            }
        }
        openvhost_core::CoreError::PackageNotPublished { tag, .. } => {
            PhpInstallResultDto::AwaitingRelease {
                tag: tag.to_string(),
            }
        }
        openvhost_core::CoreError::Package(pkg) => pkg_failure(pkg),
        // `CoreError` is a wide, crate-wide enum whose other variants are not
        // install-pipeline states; reported verbatim rather than enumerated
        // here, mirroring `mariadb_pkg::install_failure`'s identical reasoning.
        other => PhpInstallResultDto::Failed {
            reason: other.to_string(),
        },
    }
}

/// Install `major` from OpenVHost's own package tree, streaming
/// [`PhpInstallProgressEvent`] as the pipeline advances, then rescan so the
/// freshly installed runtime gets a supervisor row.
///
/// Reached only when [`route_for`] answered [`PhpInstallRoute::Package`], which
/// no machine can produce today — see this module's "merges unproven" note.
///
/// The run is **spawned**, not awaited inline, so its `AbortHandle` can be
/// recorded before the first await: `perform_quit` and [`cancel_php_install`]
/// both need something to abort, and a handle recorded after the await would
/// leave a window where neither finds one. Same ordering discipline as
/// `mysql_pkg::run_install` and `mariadb_pkg::run_install`.
pub(crate) async fn run_package_install(
    app: &tauri::AppHandle,
    major: &openvhost_core::PhpMajor,
    paths: &StackPaths,
    lock: &InstallLock,
    runtimes: &RwLock<Option<InstalledRuntimes>>,
    sup: &Supervisor,
) -> Result<PhpInstallResultDto, IpcError> {
    // `try_state`, not a `tauri::State<Db>` parameter on `install_php` — and
    // this is a §8.6 requirement, not a style choice. `state.db` is managed
    // only when it opened successfully at startup (`lib.rs`: a missing or
    // unreadable one is logged and the app carries on). A `State<Db>` argument
    // on `install_php` would make Tauri refuse the WHOLE command when it is
    // absent, including the Homebrew route, which today is every real machine's
    // only route — turning a degraded state.db into "PHP cannot be installed at
    // all". Read here instead, on the one route that needs a ledger.
    //
    // AND A MISSING ONE DOES NOT REFUSE THIS ROUTE EITHER (audit LOW-4). This
    // used to return `Failed { reason: "state.db is unavailable…" }` before
    // spawning anything, which conceded on the packaged route the exact point
    // the paragraph above wins on the Homebrew one — and contradicted
    // `PhpLedgerWriteDto`'s own contract, that the package is installed either
    // way and a failed row costs provenance, never correctness. So the install
    // runs with `None` and reports `ledger: Failed`, which is the state that
    // type was built for. `openvhost_core::install_php_package` owns the
    // reason; nothing is retyped here.
    let ledger = app
        .try_state::<Db>()
        .map(|db| openvhost_core::mysql::InstallLedger::new(&db));
    let emitter = app.clone();
    let for_event = major.as_str().to_string();
    let spawn_major = major.clone();
    let spawn_root = openvhost_core::PackagesRoot::from_home(&paths.home);

    let task = tokio::spawn(async move {
        // The same audit-F2-taught throttle the MySQL and MariaDB installs
        // apply, reused directly rather than copied a third time: `openvhost-pkg`
        // emits one `Downloaded` per stream chunk, unthrottled.
        let mut throttle = ProgressThrottle::new();
        openvhost_core::install_php_package(
            &spawn_major,
            &spawn_root,
            ledger.as_ref(),
            move |progress| {
                for progress in throttle.admit(progress, std::time::Instant::now()) {
                    let _ = PhpInstallProgressEvent {
                        major: for_event.clone(),
                        ts_ms: now_ms(),
                        progress: progress.into(),
                    }
                    .emit(&emitter);
                }
            },
        )
        .await
    });

    let abort_handle = task.abort_handle();
    let (kind, operation) = PHP_INSTALL_RUN;
    lock.set_running(
        kind,
        operation,
        major.as_str().to_string(),
        abort_handle.clone(),
    );
    let _running_guard = RunningInstallGuard {
        lock,
        abort: abort_handle,
    };

    Ok(match task.await {
        Ok(Ok(install)) => {
            // No seed: a seed exists to stand in for a brew formula directory
            // this app just asked brew to create. Our own installer wrote the
            // tree itself, and `discover_php` reads it directly.
            let discovery = rescan_into_state(runtimes, sup, paths, None).await?;
            // Both halves matter. The major alone would be satisfied by a
            // Homebrew keg that was already there; the version alone could match
            // a different major's tree. `source.version()` is `None` for every
            // Homebrew row, so this can only ever be answered by a packaged one.
            let detected = discovery.runtimes.iter().any(|rt| {
                rt.major == install.package.major
                    && rt.source.version() == Some(install.package.version.as_str())
            });
            PhpInstallResultDto::Installed {
                version: install.package.version,
                detected,
                ledger: install.ledger.into(),
            }
        }
        Ok(Err(e)) => install_failure(e),
        Err(join_err) if join_err.is_cancelled() => PhpInstallResultDto::Cancelled,
        Err(join_err) => PhpInstallResultDto::Failed {
            reason: format!("the install task ended unexpectedly: {join_err}"),
        },
    })
}

/// Cancel an in-flight PHP install, if one is running.
///
/// **Kind- and operation-checked**, exactly like `cancel_mariadb_install` and
/// `cancel_mysql_install`: the check and the abort happen under one
/// `InstallLock::abort_running_if` acquisition, so the slot cannot change in
/// between, and both discriminators must match. That is the audit F1 guarantee
/// — a run tagged with another engine's pair would be abortable from the wrong
/// button — and [`PHP_INSTALL_RUN`] is what makes PHP's pair a genuinely
/// different value rather than an identical one wearing a different label.
///
/// Cancels **either** route: a PHP install occupies one slot whichever pipeline
/// it took. Dropping the future is the cancel in both cases — for the packaged
/// route the staging directory unwinds with it, and for the Homebrew route
/// `run_task`'s `KillOnDrop` takes brew's whole process group down.
#[tauri::command]
#[specta::specta]
pub async fn cancel_php_install(lock: tauri::State<'_, InstallLock>) -> Result<bool, IpcError> {
    let (kind, operation) = PHP_INSTALL_RUN;
    Ok(lock.inner().abort_running_if(kind, operation))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tag_of(value: &impl serde::Serialize) -> String {
        serde_json::to_value(value).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_string()
    }

    // ------------------------------------------------------------------
    // Group 1 — the offer, and its third state.
    //
    // VACUITY: returning `Unavailable` unconditionally from
    // `package_offer_for` reddens
    // `apple_silicon_is_offered_awaiting_release_while_the_pin_is_unpublished`
    // alone; returning `AwaitingRelease` unconditionally reddens the three
    // absence tests below. Both were run.
    // ------------------------------------------------------------------

    /// The state this build is in TODAY for the one pinned major: the release
    /// is not published, so the offer is `AwaitingRelease` — never `Available`
    /// (which would send the user at a 404) and never `Unavailable` (which
    /// would tell an Apple Silicon owner their machine is unsupported).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn apple_silicon_is_offered_awaiting_release_while_the_pin_is_unpublished() {
        let offer = package_offer_for("8.4", Some(PackageTarget::MacosArm64));
        match offer {
            PhpPackageOfferDto::AwaitingRelease { tag } => assert_eq!(tag, "php-8.4.24"),
            other => {
                panic!("expected AwaitingRelease while the release is unpublished, got {other:?}")
            }
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_offer_agrees_with_the_explicit_arm64_offer_on_this_machine() {
        assert_eq!(PackageTarget::host(), Some(PackageTarget::MacosArm64));
        assert_eq!(
            package_offer("8.4"),
            package_offer_for("8.4", Some(PackageTarget::MacosArm64))
        );
    }

    /// **The single most load-bearing fact in this slice.** `AwaitingRelease`
    /// is what EVERY offer this build can make resolves to — there is exactly
    /// one pinned major, and its release does not exist — so it is the only
    /// non-absence state any row can carry today, and the state whose render
    /// the page must get right. What the row carries in it is a tag a human
    /// has to publish, and nothing a user can act on.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn awaiting_release_is_the_only_non_absence_offer_this_build_can_make_today() {
        let offered: Vec<(&str, PhpPackageOfferDto)> = openvhost_core::CATALOGUE
            .iter()
            .map(|major| (*major, package_offer(major)))
            .collect();
        for (major, offer) in &offered {
            match offer {
                // No `Available` anywhere: nothing is installable from the
                // package tree until a release is published. When one is, THIS
                // assertion is the tripwire that says the slice's untested
                // install path is now reachable.
                PhpPackageOfferDto::Available { version } => panic!(
                    "PHP {major} reports an installable {version}: the release was published, so \
                     the packaged install path is now reachable and is no longer unproven"
                ),
                PhpPackageOfferDto::AwaitingRelease { tag } => {
                    assert_eq!(*major, "8.4", "only 8.4 has a pinned build today");
                    assert_eq!(tag, "php-8.4.24");
                }
                PhpPackageOfferDto::Unavailable { target } => {
                    assert_eq!(target, "macos-arm64");
                }
            }
        }
        assert_eq!(
            offered
                .iter()
                .filter(|(_, o)| matches!(o, PhpPackageOfferDto::AwaitingRelease { .. }))
                .count(),
            1,
            "exactly one major is pinned-but-unpublished today"
        );
    }

    /// A major this build manages for Homebrew but has never built an artifact
    /// for. The absence is real and names the target, exactly as an
    /// unsupported architecture's does — a pinned catalogue entry is per-major
    /// work, not a URL template.
    #[test]
    fn a_cataloged_major_with_no_pinned_build_is_offered_nothing() {
        assert_eq!(
            package_offer_for("8.1", Some(PackageTarget::MacosArm64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into()
            }
        );
    }

    /// The Intel story: no signature-checked x86_64 artifact exists, so Intel
    /// is offered nothing and the absence names the target — never
    /// `AwaitingRelease`, which would wrongly suggest a build is coming.
    #[test]
    fn an_intel_host_is_offered_nothing_and_the_absence_names_the_target() {
        assert_eq!(
            package_offer_for("8.4", Some(PackageTarget::MacosX86_64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-x86_64".into()
            }
        );
    }

    #[test]
    fn a_host_this_programme_publishes_nothing_for_says_so_without_naming_an_arch() {
        assert_eq!(
            package_offer_for("8.4", None),
            PhpPackageOfferDto::Unavailable {
                target: "this host".into()
            }
        );
    }

    /// A hand-installed major outside the catalogue still gets an answer
    /// rather than a panic or a parse error: this build publishes nothing for
    /// it, on any target. The row's own `cataloged: false` is what tells the
    /// page this is a version it does not manage; the offer only says there
    /// are no bytes.
    #[test]
    fn a_major_outside_the_catalogue_is_offered_nothing() {
        assert_eq!(
            package_offer_for("7.4", Some(PackageTarget::MacosArm64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into()
            }
        );
        // And a value that is not even a version — the walk feeds row majors
        // read off a disk — is an absence too, never a panic.
        assert_eq!(
            package_offer_for("--build-from-source", Some(PackageTarget::MacosArm64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into()
            }
        );
    }

    /// The three states must not be confusable on the wire: distinct tags,
    /// distinct shapes.
    ///
    /// The `match` is exhaustive with no wildcard on purpose — this is the
    /// compile-time site that makes a fourth variant a decision rather than a
    /// silent addition.
    #[test]
    fn the_three_offer_states_serialize_distinctly() {
        let all = [
            PhpPackageOfferDto::Available {
                version: "8.4.24".into(),
            },
            PhpPackageOfferDto::AwaitingRelease {
                tag: "php-8.4.24".into(),
            },
            PhpPackageOfferDto::Unavailable {
                target: "macos-x86_64".into(),
            },
        ];
        for offer in &all {
            // Every state carries exactly one payload field beside its tag,
            // and it is the one a user is shown.
            let wire = serde_json::to_value(offer).unwrap();
            let payload = match offer {
                PhpPackageOfferDto::Available { version } => ("version", version),
                PhpPackageOfferDto::AwaitingRelease { tag } => ("tag", tag),
                PhpPackageOfferDto::Unavailable { target } => ("target", target),
            };
            assert_eq!(wire[payload.0].as_str(), Some(payload.1.as_str()));
        }
        let tags: Vec<String> = all.iter().map(tag_of).collect();
        for (i, a) in tags.iter().enumerate() {
            for (j, b) in tags.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{:?} and {:?} share a tag", all[i], all[j]);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Group 2 — the source on the wire.
    //
    // VACUITY: mapping `Packaged` to `Self::Homebrew` in the `From` impl
    // reddens both tests below; dropping `version` from the `Packaged` arm
    // reddens `a_packaged_source_carries_the_exact_version_it_was_installed_at`.
    // Both were run.
    // ------------------------------------------------------------------

    /// ONE spelling for each source. `PhpRuntimeSource::as_str()` is the
    /// definition; this pins the wire tag to it for every variant, matched
    /// exhaustively so a third source cannot be added without deciding what it
    /// is called here.
    #[test]
    fn the_wire_tag_is_php_runtime_source_as_str() {
        let all = [
            openvhost_core::PhpRuntimeSource::Packaged {
                version: "8.4.24".into(),
            },
            openvhost_core::PhpRuntimeSource::Homebrew,
        ];
        for source in &all {
            let dto = PhpRuntimeSourceDto::from(source);
            assert_eq!(tag_of(&dto), source.as_str(), "{source:?}");
        }
    }

    #[test]
    fn a_packaged_source_carries_the_exact_version_it_was_installed_at() {
        let dto = PhpRuntimeSourceDto::from(&openvhost_core::PhpRuntimeSource::Packaged {
            version: "8.4.24".into(),
        });
        assert_eq!(
            dto,
            PhpRuntimeSourceDto::Packaged {
                version: "8.4.24".into()
            }
        );
        // And Homebrew's answer is a different shape entirely, not a version
        // string that happens to be missing: the two must not be confusable.
        let brew = PhpRuntimeSourceDto::from(&openvhost_core::PhpRuntimeSource::Homebrew);
        assert_eq!(brew, PhpRuntimeSourceDto::Homebrew);
        assert_ne!(
            serde_json::to_value(&dto).unwrap(),
            serde_json::to_value(&brew).unwrap()
        );
    }

    /// Each state carries exactly the payload its tag promises — and a
    /// Homebrew row carries **no version key at all**, rather than one that
    /// happens to be null, so a consumer cannot read an absent patch level as
    /// an empty one.
    ///
    /// Exhaustive over this DTO with **no wildcard**, and that is the point of
    /// writing it as a `match`: a variant added to the wire type has to be
    /// given a tag and a payload here rather than reaching the webview as a
    /// shape nothing has described. Measured: adding a throwaway variant to
    /// `PhpRuntimeSourceDto` failed to compile at exactly this site.
    #[test]
    fn every_source_state_carries_exactly_the_payload_its_tag_promises() {
        for dto in [
            PhpRuntimeSourceDto::Packaged {
                version: "8.4.24".into(),
            },
            PhpRuntimeSourceDto::Homebrew,
        ] {
            let wire = serde_json::to_value(&dto).unwrap();
            let keys = wire.as_object().unwrap().len();
            match &dto {
                PhpRuntimeSourceDto::Packaged { version } => {
                    assert_eq!(wire["kind"], "packaged");
                    assert_eq!(wire["version"], version.as_str());
                    assert_eq!(keys, 2, "got {wire:?}");
                }
                PhpRuntimeSourceDto::Homebrew => {
                    assert_eq!(wire["kind"], "homebrew");
                    assert_eq!(keys, 1, "a Homebrew source must carry no payload: {wire:?}");
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Group 3 — the routing rule (design D4).
    //
    // What is proven here is the DECISION, not the packaged install: with
    // every offer `AwaitingRelease` or `Unavailable`, nothing in this crate
    // or on this machine can drive a real packaged install end to end. See
    // this module's own "merges unproven" note.
    //
    // VACUITY: three separate mutations of `route_for`, each run and each
    // reverted.
    //   * `Available => Homebrew` reddens
    //     `an_available_offer_is_the_only_thing_that_routes_to_the_package_tree`
    //     alone — every other test in this group still passes, which is the
    //     point: it is the only test that can see that arm today.
    //   * `AwaitingRelease => Package` reddens
    //     `an_awaiting_release_offer_still_installs_through_homebrew` AND
    //     `every_major_this_build_offers_today_installs_through_homebrew`.
    //   * `Unavailable => Package` reddens
    //     `an_unavailable_offer_is_the_ordinary_homebrew_path` AND
    //     `every_major_this_build_offers_today_installs_through_homebrew`.
    // ------------------------------------------------------------------

    /// The one state that routes away from Homebrew — and the only test in
    /// this file that touches the `Available` arm at all, because no catalogue
    /// entry can produce one today.
    #[test]
    fn an_available_offer_is_the_only_thing_that_routes_to_the_package_tree() {
        assert_eq!(
            route_for(&PhpPackageOfferDto::Available {
                version: "8.4.24".into()
            }),
            PhpInstallRoute::Package
        );
        for other in [
            PhpPackageOfferDto::AwaitingRelease {
                tag: "php-8.4.24".into(),
            },
            PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into(),
            },
        ] {
            assert_eq!(
                route_for(&other),
                PhpInstallRoute::Homebrew,
                "{other:?} must not reach the package tree"
            );
        }
    }

    /// Spec §8.5, as CORRECTED after T1 found the first draft contradicted
    /// §8.6. On this machine 8.4 is `AwaitingRelease` today *and* has a working
    /// Homebrew Install button; routing it to the package tree would return
    /// `AwaitingRelease` and install nothing, removing a working control on the
    /// one major a user is most likely to press.
    ///
    /// What `AwaitingRelease` withholds is the PACKAGED route, not the button.
    #[test]
    fn an_awaiting_release_offer_still_installs_through_homebrew() {
        assert_eq!(
            route_for(&PhpPackageOfferDto::AwaitingRelease {
                tag: "php-8.4.24".into()
            }),
            PhpInstallRoute::Homebrew
        );
    }

    /// `Unavailable` is the ORDINARY path, not the failure path (design D4):
    /// four of the five catalogued majors carry it today and every major
    /// carries it on Intel. It installs through Homebrew exactly as it did
    /// before this slice.
    #[test]
    fn an_unavailable_offer_is_the_ordinary_homebrew_path() {
        assert_eq!(
            route_for(&PhpPackageOfferDto::Unavailable {
                target: "macos-x86_64".into()
            }),
            PhpInstallRoute::Homebrew
        );
    }

    /// **Spec §8.6, established rather than asserted.** Every major this build
    /// catalogues, offered against this host, routes to Homebrew — so
    /// `install_php` runs exactly the code it ran before this slice, on every
    /// real machine today including the developer's.
    ///
    /// Paired with `awaiting_release_is_the_only_non_absence_offer_this_build_can_make_today`
    /// above, which is the tripwire on the other half: the day a release is
    /// published, that test panics and this one starts exercising the package
    /// route for real.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn every_major_this_build_offers_today_installs_through_homebrew() {
        for major in openvhost_core::CATALOGUE {
            let offer = package_offer(major);
            assert_eq!(
                route_for(&offer),
                PhpInstallRoute::Homebrew,
                "PHP {major} routed away from Homebrew on {offer:?}"
            );
        }
        // And the same for a major this build does not manage at all — the
        // walk feeds row majors read off a disk, so a hand-installed 7.4 must
        // get an answer, not a panic.
        assert_eq!(route_for(&package_offer("7.4")), PhpInstallRoute::Homebrew);
    }

    // ------------------------------------------------------------------
    // Group 4 — progress crosses the wire as five DISTINCT states.
    //
    // VACUITY: mapping `P::Verified` to `Self::Extracted` in the `From` impl
    // reddens both tests below. Run and reverted.
    // ------------------------------------------------------------------

    #[test]
    fn every_progress_variant_crosses_the_wire_distinctly() {
        use openvhost_core::Progress as P;
        let all = [
            P::Started { total: Some(10) },
            P::Downloaded { bytes: 5 },
            P::Verified,
            P::Extracted,
            P::Linked,
        ];
        let wire: Vec<serde_json::Value> = all
            .iter()
            .cloned()
            .map(|p| serde_json::to_value(PhpInstallProgressDto::from(p)).unwrap())
            .collect();
        for (i, a) in wire.iter().enumerate() {
            for (j, b) in wire.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{:?} and {:?} serialize identically", all[i], all[j]);
                }
            }
        }
    }

    /// Golden rule 6 makes SHA-256 verification a requirement; a requirement
    /// the UI cannot tell from "the bytes arrived" is one a substring match on
    /// prose would "prove".
    #[test]
    fn a_verified_download_is_not_the_same_event_as_an_extracted_one() {
        assert_ne!(
            PhpInstallProgressDto::from(openvhost_core::Progress::Verified),
            PhpInstallProgressDto::from(openvhost_core::Progress::Extracted)
        );
    }

    /// PHP's progress event carries `major`, unlike MariaDB's — several majors
    /// sit side by side, so a bar with no subject would attach to whichever row
    /// the page guessed. Pinned against the wire shape so an edit that "matches"
    /// MariaDB's fails here.
    #[test]
    fn the_progress_event_names_the_major_it_belongs_to() {
        let wire = serde_json::to_value(PhpInstallProgressEvent {
            major: "8.4".into(),
            ts_ms: 1,
            progress: PhpInstallProgressDto::Verified,
        })
        .unwrap();
        assert_eq!(wire["major"], "8.4");
        assert_eq!(wire["tsMs"], 1);
    }

    // ------------------------------------------------------------------
    // Group 5 — failures are classified, not flattened into prose.
    //
    // VACUITY: replacing `install_failure`'s `PackageNotPublished` arm with a
    // fall-through to `Failed { reason }` reddens
    // `a_package_not_yet_published_is_reported_as_awaiting_release_not_a_generic_failure`;
    // doing the same to `NoPackageForTarget` reddens
    // `no_package_for_this_target_is_an_absence_that_names_the_target`;
    // mapping `HashMismatch` to `Failed` reddens the hash-mismatch test. All
    // three were run and reverted.
    // ------------------------------------------------------------------

    /// The refusal `install_php_package` raises BEFORE any network or
    /// filesystem work, preserved across the command surface as itself. It
    /// carries the tag a maintainer has to publish — the next action belongs to
    /// them, not the user — and a red `Failed` banner would say the opposite.
    #[test]
    fn a_package_not_yet_published_is_reported_as_awaiting_release_not_a_generic_failure() {
        let result = install_failure(openvhost_core::CoreError::PackageNotPublished {
            name: "php",
            version: "8.4.24",
            tag: "php-8.4.24",
            url: "https://example.invalid/php-8.4.24-macos-arm64.tar.gz",
        });
        assert_eq!(
            result,
            PhpInstallResultDto::AwaitingRelease {
                tag: "php-8.4.24".into()
            }
        );
        assert_ne!(tag_of(&result), "failed");
    }

    /// The other pre-network refusal: no artifact for this host, or none for
    /// this major at all. An absence that names what it looked for.
    #[test]
    fn no_package_for_this_target_is_an_absence_that_names_the_target() {
        let result = install_failure(openvhost_core::CoreError::NoPackageForTarget {
            name: "php",
            version: "8.1".into(),
            target: "macos-x86_64",
        });
        assert_eq!(
            result,
            PhpInstallResultDto::Unavailable {
                target: "macos-x86_64".into()
            }
        );
        assert_ne!(tag_of(&result), "failed");
    }

    /// A payload whose bytes did not hash to the pin is not "a network
    /// problem": it is the one failure golden rule 6 exists to catch, and it
    /// must be distinguishable from every other way a download can end.
    #[test]
    fn a_hash_mismatch_is_reported_as_a_verification_failure_not_a_network_error() {
        let result = pkg_failure(openvhost_core::PkgError::HashMismatch {
            expected: "aa".repeat(32),
            actual: "bb".repeat(32),
        });
        assert_eq!(tag_of(&result), "verificationFailed");
        assert_ne!(
            tag_of(&pkg_failure(openvhost_core::PkgError::Network(
                "connection reset".into()
            ))),
            "verificationFailed"
        );
    }

    /// A download that stopped making progress is its own state, with the
    /// detail that says how long it waited — not a generic failure and not a
    /// hash mismatch.
    #[test]
    fn a_stalled_download_keeps_its_own_state_and_its_detail() {
        let stalled = openvhost_core::PkgError::DownloadStalled {
            received: 1024,
            expected: Some(4096),
            elapsed_secs: 30.0,
            stall_secs: 20.0,
        };
        let detail = stalled.to_string();
        assert_eq!(
            pkg_failure(stalled),
            PhpInstallResultDto::Stalled { detail }
        );
    }

    // ------------------------------------------------------------------
    // Group 6 — the tagged result itself, and the misread it exists to
    // prevent (design D4).
    //
    // VACUITY: adding an `exit_code: Option<i32>` field to
    // `PhpInstallResultDto::Installed` reddens
    // `a_packaged_install_carries_no_exit_code_because_it_spawns_nothing`;
    // dropping the per-variant `rename_all` on `Brew` reddens
    // `the_wire_uses_camel_case_keys_everywhere`. Both were run and reverted.
    // ------------------------------------------------------------------

    /// **The reason this type exists.** `LanguageRow.svelte` derives
    /// `installFailed = exitCode !== 0`, and `null !== 0` is true — so a
    /// SUCCESSFUL packaged install, which has no exit code because no child
    /// process runs, rendered under `role="alert"` as "brew was killed before
    /// installing PHP 8.4 finished".
    ///
    /// The fix is structural: `Installed` carries no `exitCode` key at all, so
    /// there is nothing to compare. Only `Brew` has one, and there it is right.
    #[test]
    fn a_packaged_install_carries_no_exit_code_because_it_spawns_nothing() {
        let installed = serde_json::to_value(PhpInstallResultDto::Installed {
            version: "8.4.24".into(),
            detected: true,
            ledger: PhpLedgerWriteDto::Recorded,
        })
        .unwrap();
        assert!(
            installed.get("exitCode").is_none(),
            "a packaged install must not carry an exit code: {installed:?}"
        );
        // Not merely absent-because-null either: no key, so a consumer cannot
        // read "no code" as "killed by a signal".
        assert!(installed.get("exit_code").is_none(), "{installed:?}");

        // And the arm where the comparison IS right keeps it.
        let brew = serde_json::to_value(PhpInstallResultDto::Brew {
            exit_code: Some(0),
            detected: true,
        })
        .unwrap();
        assert_eq!(brew["exitCode"], 0);
    }

    /// A brew run killed by a signal reports no code at all, and that is what
    /// makes `exitCode !== 0` the right test on THIS arm: `null` there means
    /// "not a clean exit", which is true of a killed process.
    #[test]
    fn a_killed_brew_run_reports_no_exit_code_on_the_brew_arm() {
        let wire = serde_json::to_value(PhpInstallResultDto::Brew {
            exit_code: None,
            detected: false,
        })
        .unwrap();
        assert_eq!(wire["kind"], "brew");
        // `get`, not indexing: `wire["exitCode"]` on an ABSENT key yields
        // `Value::Null` too, so `is_null()` alone would pass just as happily
        // for a key renamed out of existence. Measured — dropping the
        // per-variant `rename_all` left this test green until it was written
        // this way.
        assert_eq!(
            wire.get("exitCode"),
            Some(&serde_json::Value::Null),
            "a killed brew run must carry the key with no value, not no key: {wire:?}"
        );
    }

    /// Every key the webview reads is camelCase. Serde's container-level
    /// `rename_all` on an ENUM renames variants and NOT their fields, so
    /// `exit_code` crossed the wire in snake_case until the per-variant
    /// attribute was added — the same seam that made `fieldErrors` mark
    /// nothing. Checked over every state of every new wire type here rather
    /// than only the one that bit, because the next multi-word field will not
    /// announce itself.
    ///
    /// **At every depth, not only the top level** (audit LOW-1). This walked
    /// `value.as_object().keys()` and stopped there, so a nested type's keys
    /// were never inspected — and [`PhpLedgerWriteDto`] is nested by
    /// construction: it is never returned on its own, only ever as
    /// `Installed.ledger`. The auditor proved the gap rather than inferring it,
    /// by renaming that type's `reason` to `failure_reason` and watching this
    /// test stay green while the regenerated bindings correctly declared the
    /// snake_case key. Both halves of the fix are load-bearing: the walk
    /// recurses, AND the ledger's own states are in the value list, since the
    /// only ledger this list held before was field-less `Recorded`.
    #[test]
    fn the_wire_uses_camel_case_keys_everywhere() {
        let mut values = vec![
            serde_json::to_value(PhpInstallProgressEvent {
                major: "8.4".into(),
                ts_ms: 1,
                progress: PhpInstallProgressDto::Started { total: Some(1) },
            })
            .unwrap(),
        ];
        for progress in [
            PhpInstallProgressDto::Started { total: Some(1) },
            PhpInstallProgressDto::Downloaded { bytes: 1 },
            PhpInstallProgressDto::Verified,
            PhpInstallProgressDto::Extracted,
            PhpInstallProgressDto::Linked,
        ] {
            values.push(serde_json::to_value(progress).unwrap());
        }
        for result in every_install_result_state() {
            values.push(serde_json::to_value(&result).unwrap());
            values.push(
                serde_json::to_value(PhpInstallOutcomeDto {
                    major: "8.4".into(),
                    result,
                })
                .unwrap(),
            );
        }
        for ledger in every_ledger_write_state() {
            // Standalone AND nested. Nested is the shape that actually crosses
            // the wire; standalone keeps the check meaningful if a later
            // `Installed` stops carrying one.
            values.push(serde_json::to_value(&ledger).unwrap());
            values.push(
                serde_json::to_value(PhpInstallResultDto::Installed {
                    version: "8.4.24".into(),
                    detected: true,
                    ledger,
                })
                .unwrap(),
            );
        }
        for value in &values {
            // Kept from the `as_object().unwrap()` this replaced: every value in
            // the list is a struct or a tagged enum, so one that serialized to a
            // bare scalar would have nothing to walk and would pass silently.
            assert!(value.is_object(), "not an object on the wire: {value:?}");
            assert_camel_case_keys(value);
        }
    }

    /// Assert `value` and everything under it uses camelCase keys.
    ///
    /// Recurses through objects and arrays; scalars carry no keys. Arrays are
    /// walked even though no wire type here holds one — a `Vec<T>` field is one
    /// edit away, and a walker that stopped at the first array would go quiet
    /// exactly then.
    fn assert_camel_case_keys(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    assert!(
                        !key.contains('_'),
                        "{key} is snake_case on the wire: {value:?}"
                    );
                    assert_camel_case_keys(child);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    assert_camel_case_keys(item);
                }
            }
            _ => {}
        }
    }

    /// Every state of the union, in one place so the tests above cannot drift
    /// out of step with the type. Written as an explicit list rather than
    /// derived, so a new variant is added here deliberately.
    fn every_install_result_state() -> Vec<PhpInstallResultDto> {
        vec![
            PhpInstallResultDto::Brew {
                exit_code: Some(0),
                detected: true,
            },
            PhpInstallResultDto::Installed {
                version: "8.4.24".into(),
                detected: true,
                ledger: PhpLedgerWriteDto::Recorded,
            },
            PhpInstallResultDto::AlreadyInstalled {
                version: "8.4.24".into(),
            },
            PhpInstallResultDto::Cancelled,
            PhpInstallResultDto::VerificationFailed {
                expected: "aa".into(),
                actual: "bb".into(),
            },
            PhpInstallResultDto::Stalled {
                detail: "stalled".into(),
            },
            PhpInstallResultDto::AwaitingRelease {
                tag: "php-8.4.24".into(),
            },
            PhpInstallResultDto::Unavailable {
                target: "macos-x86_64".into(),
            },
            PhpInstallResultDto::Failed {
                reason: "boom".into(),
            },
        ]
    }

    /// Every state of the ledger write, for the same reason
    /// [`every_install_result_state`] exists: the walk above needs a value for
    /// each, and an explicit list means a third state is added here
    /// deliberately. Pinned to the type by
    /// `every_ledger_write_state_carries_its_own_tag`.
    fn every_ledger_write_state() -> Vec<PhpLedgerWriteDto> {
        vec![
            PhpLedgerWriteDto::Recorded,
            PhpLedgerWriteDto::Failed {
                reason: "database is locked".into(),
            },
        ]
    }

    /// The compile-time site that keeps the list above in step with the type —
    /// exhaustive, no wildcard — and the reason it matters here rather than
    /// being ceremony: a third ledger state absent from that list would never
    /// have its keys walked by `the_wire_uses_camel_case_keys_everywhere`, which
    /// is precisely the hole that test was just fixed for.
    #[test]
    fn every_ledger_write_state_carries_its_own_tag() {
        for state in every_ledger_write_state() {
            let carried = match state {
                PhpLedgerWriteDto::Recorded => "recorded",
                PhpLedgerWriteDto::Failed { .. } => "failed",
            };
            assert_eq!(tag_of(&state), carried, "{state:?}");
        }
    }

    /// The nine states must not be confusable on the wire. `Brew` in
    /// particular must not share a tag with `Installed`: the two mean opposite
    /// things about whether a child process ran.
    #[test]
    fn every_install_result_state_serializes_distinctly() {
        let all = every_install_result_state();
        let tags: Vec<String> = all.iter().map(tag_of).collect();
        for (i, a) in tags.iter().enumerate() {
            for (j, b) in tags.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{:?} and {:?} share a tag", all[i], all[j]);
                }
            }
        }
        // Exhaustive with no wildcard: this is the compile-time site that makes
        // a tenth state a decision rather than a silent addition, and it is
        // also what pins the list above to the type. Measured: adding a
        // throwaway variant to `PhpInstallResultDto` failed to compile here.
        for state in &all {
            let carried = match state {
                PhpInstallResultDto::Brew { .. } => "brew",
                PhpInstallResultDto::Installed { .. } => "installed",
                PhpInstallResultDto::AlreadyInstalled { .. } => "alreadyInstalled",
                PhpInstallResultDto::Cancelled => "cancelled",
                PhpInstallResultDto::VerificationFailed { .. } => "verificationFailed",
                PhpInstallResultDto::Stalled { .. } => "stalled",
                PhpInstallResultDto::AwaitingRelease { .. } => "awaitingRelease",
                PhpInstallResultDto::Unavailable { .. } => "unavailable",
                PhpInstallResultDto::Failed { .. } => "failed",
            };
            assert_eq!(tag_of(state), carried, "{state:?}");
        }
    }

    /// `major` sits OUTSIDE the union, on every branch, because every consumer
    /// needs it to attribute the outcome to a row — including the branches that
    /// carry no version of their own (`Cancelled`, `AwaitingRelease`). Pinned
    /// so a later edit that pushes it into the arms fails here.
    #[test]
    fn the_outcome_names_its_major_on_every_branch() {
        for result in every_install_result_state() {
            let wire = serde_json::to_value(PhpInstallOutcomeDto {
                major: "8.4".into(),
                result,
            })
            .unwrap();
            assert_eq!(wire["major"], "8.4", "got {wire:?}");
            assert!(wire["result"]["kind"].is_string(), "got {wire:?}");
        }
    }

    /// The ledger is provenance, not correctness: a failed write costs the
    /// `state.db` row, never the install, and it carries the reason rather than
    /// collapsing to a bool.
    #[test]
    fn a_failed_ledger_write_keeps_its_reason() {
        let dto = PhpLedgerWriteDto::from(openvhost_core::mysql::LedgerWrite::Failed {
            reason: "database is locked".into(),
        });
        assert_eq!(
            dto,
            PhpLedgerWriteDto::Failed {
                reason: "database is locked".into()
            }
        );
        assert_ne!(
            tag_of(&dto),
            tag_of(&PhpLedgerWriteDto::from(
                openvhost_core::mysql::LedgerWrite::Recorded {
                    installed_at: 1_786_000_000
                }
            ))
        );
    }
}
