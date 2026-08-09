// SPDX-License-Identifier: GPL-3.0-or-later
//! Installing MySQL from OpenVHost's own package tree, over IPC
//! (MySQL-from-tarball design D2/D3/D4, plan Task 3).
//!
//! A sibling module rather than more of `commands.rs`, which is already ~8 200
//! lines. Everything here is thin: the DTOs that carry a `Progress` event and a
//! runtime's provenance to the webview, the exhaustive mapping from
//! `openvhost-pkg`'s typed errors onto states the UI can render, and two
//! commands (`install_mysql`, `cancel_mysql_install`) that hold no logic of
//! their own beyond wiring.
//!
//! **`brew install` is gone from this path.** Installing MySQL now means
//! download → SHA-256 verify → extract, and a machine with no Homebrew at all
//! can do it. Homebrew survives only as a *discovery* source for a keg the user
//! installed themselves (design D3/D7).
//!
//! Three facts this module exists to make visible, because all three were
//! invisible before and all three matter during the migration:
//!
//! 1. **Which source a runtime came from** — [`MysqlRuntimeSourceDto`]. The
//!    owner will be running a brew-installed 8.4 and a packaged 8.4 at the same
//!    time; "which mysqld am I actually running" needs an answer that is not a
//!    guess.
//! 2. **That verification happened** — [`MysqlInstallProgressDto::Verified`] is
//!    its own variant, distinct from `Extracted`. A download that was checked
//!    and one that was not must not look identical; that distinction is what
//!    golden rule 6 buys, and it is worth nothing if the UI collapses it.
//! 3. **That the user can cancel** — [`cancel_mysql_install`]. Not a nicety:
//!    the install permit inside `openvhost-pkg` is process-wide and taken
//!    *before* staging, and nothing bounds a download by wall clock any more
//!    (only a 30 s idle window), so a server dribbling one byte every 29 s
//!    would hold that permit effectively forever and starve every later
//!    install. Dropping the install future is the cancel; this module holds the
//!    handle that drops it.

use std::path::Path;
use std::sync::{Arc, RwLock};

use openvhost_proc::Supervisor;
use tauri_specta::Event;

use crate::commands::{
    InstallLock, IpcError, MYSQL_INSTALL_RUN, RunningInstallGuard, now_ms, rescan_mysql_into_state,
    stack_paths,
};
use crate::db_state::DbHandle;
use crate::stack::StackPaths;

/// Where a listed runtime's binaries came from — the wire copy of
/// `openvhost_core::mysql::MysqlRuntimeSource`.
///
/// **The tag is not a second spelling.** `MysqlRuntimeSource::as_str()` is the
/// one definition of the machine-facing word for each source (Task 2 made it so
/// deliberately), and [`the_wire_tag_is_mysql_runtime_source_as_str`] pins this
/// type's serialized `kind` to it for every variant, so the two cannot drift
/// into different words for the same fact.
///
/// `Homebrew` carries **no version, on purpose.** Brew's exact version would
/// have to be probed — the measurement that put design D4 in the spec — and
/// reporting the *major* as though it were the full version would be a lie no
/// caller could detect. The UI renders `8.4.11` for a packaged runtime and a
/// bare `8.4` for a brew one, and invents nothing in between.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlRuntimeSourceDto {
    Packaged { version: String },
    Homebrew,
}

impl From<&openvhost_core::mysql::MysqlRuntimeSource> for MysqlRuntimeSourceDto {
    fn from(s: &openvhost_core::mysql::MysqlRuntimeSource) -> Self {
        use openvhost_core::mysql::MysqlRuntimeSource as S;
        match s {
            S::Packaged { version } => Self::Packaged {
                version: version.clone(),
            },
            S::Homebrew => Self::Homebrew,
        }
    }
}

/// Whether this build can install a given major on THIS host, and what it
/// would install.
///
/// Modelled as a state rather than a `bool` because the two answers carry
/// different payloads and different copy: `Available` names the exact version
/// the user is about to get, `Unavailable` names the target we publish nothing
/// for. An Intel Mac lands on the second one — Oracle does publish an x86_64
/// build, but its bytes never went through the signature check the catalogue's
/// pin rests on, so this build offers nothing for it and says so. That is an
/// honest **absence**, not a failure: Homebrew remains that machine's only
/// source today, and the row renders no Install button at all rather than one
/// that throws.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlPackageOfferDto {
    Available { version: String },
    Unavailable { target: String },
}

/// What this build would install for `major` on `target`.
///
/// `target` is an explicit `Option` rather than read from the host inside, for
/// exactly the reason `mysql_package_for_target` takes one: **both branches
/// have to be reachable from a test on any one machine.** They were not before
/// — a mutation that returned `Available` for every refusal survived the whole
/// suite green on Apple Silicon, because the refusal arm never executed there,
/// and the refusal arm is the entire Intel story this slice promises.
///
/// Any refusal is an absence. This deliberately does not parse an error payload
/// to decide, so a future refusal reason cannot accidentally become an offer.
pub(crate) fn package_offer_for(
    major: &openvhost_core::mysql::MysqlMajor,
    target: Option<openvhost_core::mysql::PackageTarget>,
) -> MysqlPackageOfferDto {
    // `PackageTarget` is matched exhaustively via its own `as_str`; `None` is
    // the host this programme publishes nothing for at all.
    let named = match target {
        Some(t) => t.as_str().to_string(),
        None => "this host".to_string(),
    };
    match openvhost_core::mysql::mysql_package_for_target(major, target) {
        Ok(entry) => MysqlPackageOfferDto::Available {
            version: entry.version.to_string(),
        },
        Err(_) => MysqlPackageOfferDto::Unavailable { target: named },
    }
}

/// What this build would install for `major` on the host it was compiled for.
pub(crate) fn package_offer(major: &openvhost_core::mysql::MysqlMajor) -> MysqlPackageOfferDto {
    package_offer_for(major, openvhost_core::mysql::PackageTarget::host())
}

/// One step of the install pipeline, as the user watches it — the wire copy of
/// `openvhost_pkg::Progress`, which carries no serde/specta derives of its own.
///
/// Variants map 1:1 and the [`From`] impl below is exhaustive, so a sixth
/// pipeline stage cannot silently arrive as one of these five.
///
/// `Verified` and `Extracted` are SEPARATE variants and must render as separate
/// sentences. That is the entire point: a download whose SHA-256 was checked
/// against the compiled-in pin and one that merely arrived are different events,
/// and collapsing them would make the guarantee golden rule 6 exists to provide
/// unobservable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlInstallProgressDto {
    /// The transfer started; `total` is the server's declared length, absent
    /// when it declared none.
    Started { total: Option<u64> },
    /// Cumulative bytes received so far.
    Downloaded { bytes: u64 },
    /// The received bytes hash to the pinned SHA-256. Nothing has parsed the
    /// archive at this point — verification happens first, by design.
    Verified,
    /// The archive was unpacked into a private staging directory.
    Extracted,
    /// The tree was renamed into place and this major's `current` link now
    /// points at it.
    Linked,
}

impl From<openvhost_core::Progress> for MysqlInstallProgressDto {
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

/// One pipeline step, forwarded live while an install runs.
///
/// A typed payload rather than a log line (which is what the brew era streamed
/// through `MysqlInstallLogEvent`): the UI has to tell `Verified` from
/// `Extracted` structurally, and a substring match on prose is exactly the kind
/// of check that passes while the two render identically.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MysqlInstallProgressEvent {
    pub major: String,
    pub ts_ms: u64,
    pub progress: MysqlInstallProgressDto,
}

/// Whether the install was also recorded in `state.db`'s package ledger.
///
/// The wire copy of `openvhost_core::mysql::LedgerWrite`, and a state rather
/// than a boolean for the reason its Rust counterpart is one: the failure
/// carries a reason worth showing. The package is installed either way — the
/// tree is the inventory — so a failed row costs provenance, never correctness,
/// and the UI says exactly that rather than calling a demonstrably installed
/// MySQL a failure.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlLedgerWriteDto {
    Recorded,
    Failed { reason: String },
}

impl From<openvhost_core::mysql::LedgerWrite> for MysqlLedgerWriteDto {
    fn from(w: openvhost_core::mysql::LedgerWrite) -> Self {
        use openvhost_core::mysql::LedgerWrite as L;
        match w {
            L::Recorded { .. } => Self::Recorded,
            L::Failed { reason } => Self::Failed { reason },
        }
    }
}

/// How one `install_mysql` call ended.
///
/// Outcome-shaped, never an `IpcError`, for the same reason
/// `MysqlConnectionProofDto` is: every one of these is something the row must
/// RENDER, with its own copy and its own next step, and a thrown error collapses
/// all of them into one red banner. In particular a **verification failure is
/// its own variant** — it is not a network error, and telling a user "network
/// error" when the bytes arrived intact but hashed wrong would hide the one
/// event the SHA-256 pin exists to catch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlInstallResultDto {
    /// Downloaded, verified, extracted, linked. `detected` is whether the
    /// packaged walk then found the runtime — no probe, no spawn, just a
    /// `read_link` and three `is_file` calls, so a `false` here means the
    /// archive genuinely did not contain the binaries this app drives.
    Installed {
        version: String,
        detected: bool,
        ledger: MysqlLedgerWriteDto,
    },
    /// That exact version directory already exists. Refused before any network
    /// or staging work — a fact, not a failure.
    AlreadyInstalled { version: String },
    /// The user cancelled. Staging is an RAII temporary removed as the future
    /// unwinds, so nothing was installed and nothing was left behind.
    Cancelled,
    /// The bytes arrived but did not hash to the pinned value. The install
    /// stopped **before** anything parsed the archive.
    VerificationFailed { expected: String, actual: String },
    /// The transfer stopped making progress for longer than the idle window.
    /// `detail` is `PkgError::DownloadStalled`'s own message, which already
    /// names how far it got, how fast it was going and how long it was silent.
    Stalled { detail: String },
    /// This build publishes no verified package for this major on `target`.
    Unavailable { target: String },
    /// Anything else, verbatim.
    Failed { reason: String },
}

/// `install_mysql`'s return: the major it was for, and how it ended.
///
/// `major` sits OUTSIDE the result union because every consumer needs it to
/// attribute the outcome to a row, on every branch — the same "tag it with what
/// it is for" fix `MysqlInitFailure` applies on the frontend to a DTO that does
/// not carry its own subject.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MysqlInstallOutcomeDto {
    pub major: String,
    pub result: MysqlInstallResultDto,
}

/// Map a failed install onto a renderable state.
///
/// Exhaustive over `PkgError` with **no wildcard arm**: a new package-pipeline
/// failure mode must be classified here deliberately rather than arriving as
/// generic prose. The `other @ (…)` binding keeps that exhaustiveness while
/// still routing the uninteresting variants to one place.
fn pkg_failure(e: openvhost_core::PkgError) -> MysqlInstallResultDto {
    use openvhost_core::PkgError as E;
    match e {
        E::HashMismatch { expected, actual } => {
            MysqlInstallResultDto::VerificationFailed { expected, actual }
        }
        stalled @ E::DownloadStalled { .. } => MysqlInstallResultDto::Stalled {
            detail: stalled.to_string(),
        },
        E::AlreadyInstalled { version, .. } => MysqlInstallResultDto::AlreadyInstalled { version },
        other @ (E::InvalidComponent { .. }
        | E::InvalidUrl(_)
        | E::InvalidSha256
        | E::InvalidWarmupPath { .. }
        | E::Network(_)
        | E::TooLarge { .. }
        | E::UnsafeArchive(_)
        | E::Io { .. }
        | E::Internal(_)
        | E::Unsupported(_)) => MysqlInstallResultDto::Failed {
            reason: other.to_string(),
        },
    }
}

/// The same mapping one level up, for the error type `install_mysql_package`
/// actually returns.
pub(crate) fn install_failure(e: openvhost_core::CoreError) -> MysqlInstallResultDto {
    match e {
        openvhost_core::CoreError::NoPackageForTarget { target, .. } => {
            MysqlInstallResultDto::Unavailable {
                target: target.to_string(),
            }
        }
        openvhost_core::CoreError::Package(pkg) => pkg_failure(pkg),
        // `CoreError` is a wide, crate-wide enum whose other variants are not
        // install-pipeline states; they are reported verbatim rather than
        // enumerated here, which would pin this module to every future core
        // error. The two that ARE install states are matched by name above.
        other => MysqlInstallResultDto::Failed {
            reason: other.to_string(),
        },
    }
}

/// Forward a `Downloaded` at most this often.
///
/// ~30 events across the measured 6.4-second transfer, which is smoother than
/// any progress bar needs and three orders of magnitude below what the
/// downloader produces.
const DOWNLOAD_EVENT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// …and additionally whenever this much of the declared total has arrived
/// since the last one, so a transfer far faster than
/// [`DOWNLOAD_EVENT_INTERVAL`] still animates. Caps the whole download at ~100
/// byte-count events.
const DOWNLOAD_EVENT_FRACTION: u64 = 100;

/// Decides which of `openvhost-pkg`'s progress events actually cross to the
/// webview (audit F2).
///
/// **What is thrown away and what is not.** `Downloaded` is a running byte
/// count: dropping one costs nothing, because the next one carries the same
/// fact brought up to date. Every other variant is a *pipeline transition* —
/// `Started`, `Verified`, `Extracted`, `Linked` each happen exactly once and
/// each is the only announcement of itself. `Verified` in particular is the
/// event golden rule 6 exists to make observable, so it is forwarded
/// unconditionally, no timer consulted.
///
/// A withheld byte count is not simply lost either: the next transition
/// flushes it first, so the bar reaches the total before "Verified" replaces
/// it. Without that, a download whose last chunk landed 20 ms after the
/// previous emit would visibly finish at 98%.
///
/// `now` is a parameter rather than read inside, for the reason
/// `package_offer_for` takes an explicit target: a clock read internally is a
/// clock a test cannot move, and "does this actually suppress anything" is the
/// only question worth asking of a throttle.
#[derive(Debug)]
pub(crate) struct ProgressThrottle {
    /// When the last `Downloaded` was forwarded; `None` until the first one,
    /// which always is.
    last_emit: Option<std::time::Instant>,
    /// The byte count that last crossed the wire.
    last_bytes: u64,
    /// The most recent byte count that did NOT, awaiting a flush.
    withheld: Option<u64>,
    /// The server's declared length, from `Started` — absent when it declared
    /// none, in which case only the interval rule applies.
    total: Option<u64>,
}

impl ProgressThrottle {
    pub(crate) fn new() -> Self {
        Self {
            last_emit: None,
            last_bytes: 0,
            withheld: None,
            total: None,
        }
    }

    /// What should cross the wire for `progress`, in order. Usually nothing.
    pub(crate) fn admit(
        &mut self,
        progress: openvhost_core::Progress,
        now: std::time::Instant,
    ) -> Vec<openvhost_core::Progress> {
        use openvhost_core::Progress as P;
        // Exhaustive, no wildcard: a sixth pipeline stage must be classified as
        // "throttleable" or "always" deliberately, and the safe default for a
        // one-shot transition is `always`, not silence.
        match progress {
            P::Downloaded { bytes } => {
                if self.should_emit(bytes, now) {
                    self.mark_emitted(bytes, now);
                    vec![P::Downloaded { bytes }]
                } else {
                    self.withheld = Some(bytes);
                    Vec::new()
                }
            }
            transition @ (P::Started { .. } | P::Verified | P::Extracted | P::Linked) => {
                if let P::Started { total } = &transition {
                    self.total = *total;
                }
                match self.withheld.take() {
                    Some(bytes) => {
                        self.mark_emitted(bytes, now);
                        vec![P::Downloaded { bytes }, transition]
                    }
                    None => vec![transition],
                }
            }
        }
    }

    fn should_emit(&self, bytes: u64, now: std::time::Instant) -> bool {
        // The first byte count always crosses: it is what replaces
        // "Preparing the download…" on the row.
        let Some(last) = self.last_emit else {
            return true;
        };
        if now.duration_since(last) >= DOWNLOAD_EVENT_INTERVAL {
            return true;
        }
        match self.total {
            // `.max(1)` so a total under 100 bytes gives a 1-byte step rather
            // than a 0-byte one, which would make this rule always true and
            // the throttle a no-op for small payloads.
            Some(total) => {
                bytes.saturating_sub(self.last_bytes) >= (total / DOWNLOAD_EVENT_FRACTION).max(1)
            }
            None => false,
        }
    }

    fn mark_emitted(&mut self, bytes: u64, now: std::time::Instant) {
        self.last_emit = Some(now);
        self.last_bytes = bytes;
        self.withheld = None;
    }
}

/// Install a catalogued MySQL major from the pinned upstream tarball, streaming
/// [`MysqlInstallProgressEvent`] as the pipeline advances, then rescan so the
/// freshly installed runtime is picked up.
///
/// **No Homebrew.** `major` is parsed through the catalogue-gated
/// `MysqlMajor::parse` and nothing else a caller supplies reaches the pipeline:
/// the URL and the SHA-256 come only from the compiled-in catalogue, so there
/// is no argument over IPC that can change which bytes are fetched or what they
/// must hash to.
///
/// The run is **spawned**, not awaited inline, so its `AbortHandle` can be
/// recorded — that handle is what [`cancel_mysql_install`] and `perform_quit`
/// both fire. Dropping the install future removes the RAII staging directory
/// and releases `openvhost-pkg`'s process-wide install permit; without a handle
/// there would be no way to make either happen.
#[tauri::command]
#[specta::specta]
// DEGRADE (optional-state.db design D2): a store that never opened costs the
// ledger row, never the install — `MysqlInstallResultDto` reports that as
// `ledger: Failed { reason }`, which is the state that type was built for. This
// is `php_pkg::run_package_install`'s §8.6 argument and its audit-LOW-4 note,
// applied to the engine those were written against.
pub async fn install_mysql(
    app: tauri::AppHandle,
    major: String,
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<MysqlInstallOutcomeDto, IpcError> {
    // The catalogue gate, before anything else happens.
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;

    // One at a time, shared with `install_php`/`uninstall_package`. `try_lock`
    // rather than `lock`: a second press is refused with an explanation, never
    // silently queued behind a download that has no wall-clock bound.
    let Ok(_guard) = lock.inner().guard.try_lock() else {
        return Err(IpcError::Core {
            message: "an install is already running".into(),
        });
    };

    let p = stack_paths(&paths)?;
    let outcome = run_install(
        &app,
        &major,
        &p.home,
        db.inner(),
        lock.inner(),
        runtimes.inner(),
        sup.inner(),
    )
    .await?;
    Ok(outcome)
}

/// The body of [`install_mysql`], minus `tauri::State` extraction.
///
/// Split out so the state-threading stays readable and so the ordering — spawn,
/// record the abort handle, THEN await — is stated once. The guard must be
/// created before the first `await` on the task, or a quit (or a cancel)
/// arriving in that window would find an empty slot.
#[allow(clippy::too_many_arguments)]
async fn run_install(
    app: &tauri::AppHandle,
    major: &openvhost_core::mysql::MysqlMajor,
    home: &Path,
    db: &DbHandle,
    lock: &InstallLock,
    runtimes: &RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>,
    sup: &Supervisor,
) -> Result<MysqlInstallOutcomeDto, IpcError> {
    // `None` when the store never opened. `DbHandle::install_ledger` is the one
    // named place that decision is made — nothing here may reach for `require`
    // — and `install_mysql_package` owns the reason it reports for the missing
    // row, so nothing is retyped here either.
    let ledger = db.install_ledger();
    let emitter = app.clone();
    let for_event = major.as_str().to_string();
    let spawn_major = major.clone();
    let spawn_root = openvhost_core::PackagesRoot::from_home(home);

    let task = tokio::spawn(async move {
        // Audit F2: `openvhost-pkg` emits one `Downloaded` per stream chunk,
        // unthrottled — order 10³–10⁴ of them at roughly 500–2000/s against the
        // measured 167,977,240-byte payload. That cost nothing while nothing
        // consumed `Progress`; this is its first consumer, and every event
        // forwarded here becomes a Tauri event and a `$state` write on the
        // webview's main thread. The throttle is app-side deliberately: the
        // pipeline's job is to report the truth as it happens, and deciding how
        // often a *user interface* needs to hear it is this layer's decision,
        // not the downloader's.
        let mut throttle = ProgressThrottle::new();
        openvhost_core::mysql::install_mysql_package(
            &spawn_major,
            &spawn_root,
            ledger.as_ref(),
            move |progress| {
                for progress in throttle.admit(progress, std::time::Instant::now()) {
                    let _ = MysqlInstallProgressEvent {
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
    let (kind, operation) = MYSQL_INSTALL_RUN;
    lock.set_running(
        kind,
        operation,
        format!("MySQL {}", major.as_str()),
        abort_handle.clone(),
    );
    let _running_guard = RunningInstallGuard {
        lock,
        abort: abort_handle,
    };

    let result = match task.await {
        Ok(Ok(install)) => {
            // Design D3/D4: the packaged walk finds it with no probe at all —
            // the version is a directory name we chose, not something to
            // interrogate a 55 MB `mysqld` for. No seed is passed: unlike the
            // brew path, there is nothing a seed would tell the rescan that the
            // tree does not already say.
            let discovery = rescan_mysql_into_state(runtimes, sup, home, None).await?;
            let detected = discovery.runtimes.iter().any(|rt| {
                rt.major == *major
                    && matches!(
                        rt.source,
                        openvhost_core::mysql::MysqlRuntimeSource::Packaged { .. }
                    )
            });
            MysqlInstallResultDto::Installed {
                version: install.package.version,
                detected,
                ledger: install.ledger.into(),
            }
        }
        Ok(Err(e)) => install_failure(e),
        // The cancel path — `cancel_mysql_install` or `perform_quit` fired the
        // handle. Staging unwound with the future, so this is a plain "nothing
        // happened", not a failure to explain away.
        Err(join_err) if join_err.is_cancelled() => MysqlInstallResultDto::Cancelled,
        Err(join_err) => MysqlInstallResultDto::Failed {
            reason: format!("the install task ended unexpectedly: {join_err}"),
        },
    };

    Ok(MysqlInstallOutcomeDto {
        major: major.as_str().to_string(),
        result,
    })
}

/// Cancel an in-flight MySQL install, if one is running.
///
/// Returns whether anything was actually cancelled — `false` when the slot is
/// empty or holds a different run, so the UI can say "it had already finished"
/// instead of implying it stopped something.
///
/// **Kind- and operation-checked.** `InstallLock`'s slot is shared with
/// `install_php`, `uninstall_package` and `initialize_mysql`, and a Cancel
/// button on the Databases page must never abort somebody else's run just
/// because it happens to hold the lock. The check and the abort happen under
/// one lock acquisition (`InstallLock::abort_running_if`) so the slot cannot
/// change in between.
///
/// SECURITY (audit F1): that guarantee was *stated* here before it was
/// *enforced*. `initialize_mysql` tagged its own run with the identical
/// `(Mysql, Install)` pair — the labels differed, the discriminators did not,
/// and `abort_running_if` compares only the discriminators — so this command
/// aborted datadir initializations. Unreachable through the shipped UI, but
/// every Tauri command is reachable from the webview, which is this project's
/// standing assumption. The fix is `PackageOperation::Initialize` plus the two
/// named pairs in `commands.rs`; this command fires on [`MYSQL_INSTALL_RUN`]
/// and nothing else.
#[tauri::command]
#[specta::specta]
pub async fn cancel_mysql_install(lock: tauri::State<'_, InstallLock>) -> Result<bool, IpcError> {
    let (kind, operation) = MYSQL_INSTALL_RUN;
    Ok(lock.inner().abort_running_if(kind, operation))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use openvhost_core::mysql::MysqlRuntimeSource;

    fn tag_of(value: &impl serde::Serialize) -> String {
        serde_json::to_value(value).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_string()
    }

    // ------------------------------------------------------------------
    // Group 1 — the source tag is `MysqlRuntimeSource::as_str()`, not a
    // second spelling of it.
    // ------------------------------------------------------------------

    /// The one that stops this DTO drifting: for EVERY source, the wire tag is
    /// literally what `as_str()` says and the wire version is literally what
    /// `version()` says. The match is exhaustive, so a third source has to be
    /// added here too.
    #[test]
    fn the_wire_tag_is_mysql_runtime_source_as_str() {
        let sources = [
            MysqlRuntimeSource::Packaged {
                version: "8.4.11".to_string(),
            },
            MysqlRuntimeSource::Homebrew,
        ];
        for source in &sources {
            let dto = MysqlRuntimeSourceDto::from(source);
            assert_eq!(tag_of(&dto), source.as_str(), "tag drifted for {source:?}");
            let wire = serde_json::to_value(&dto).unwrap();
            let wire_version = wire.get("version").and_then(|v| v.as_str());
            assert_eq!(
                wire_version,
                source.version(),
                "version drifted for {source:?}"
            );
        }
    }

    /// A Homebrew runtime carries NO version over the wire — deliberately, so
    /// the UI cannot render an invented patch number. If this ever starts
    /// carrying one it will be because someone decided a probe is acceptable,
    /// and that decision should break this test first.
    #[test]
    fn a_homebrew_runtime_carries_no_version_over_the_wire() {
        let wire = serde_json::to_value(MysqlRuntimeSourceDto::from(&MysqlRuntimeSource::Homebrew))
            .unwrap();
        assert_eq!(wire.get("version"), None);
        assert_eq!(wire["kind"], "homebrew");
    }

    #[test]
    fn a_packaged_runtime_carries_its_exact_version() {
        let wire =
            serde_json::to_value(MysqlRuntimeSourceDto::from(&MysqlRuntimeSource::Packaged {
                version: "8.4.11".to_string(),
            }))
            .unwrap();
        assert_eq!(wire["kind"], "packaged");
        assert_eq!(wire["version"], "8.4.11");
    }

    // ------------------------------------------------------------------
    // Group 2 — progress crosses the wire as five DISTINCT states.
    // ------------------------------------------------------------------

    /// Pairwise, not "each is non-empty": the failure this rules out is a
    /// mapping that collapses `Verified` into `Extracted`, which every
    /// per-variant existence check passes.
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
            .map(|p| serde_json::to_value(MysqlInstallProgressDto::from(p)).unwrap())
            .collect();
        for (i, a) in wire.iter().enumerate() {
            for (j, b) in wire.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{:?} and {:?} serialize identically", all[i], all[j]);
                }
            }
        }
    }

    #[test]
    fn a_verified_download_is_not_the_same_event_as_an_extracted_one() {
        // Stated on its own as well as pairwise, because this specific pair is
        // the one that carries golden rule 6's guarantee.
        assert_ne!(
            MysqlInstallProgressDto::from(openvhost_core::Progress::Verified),
            MysqlInstallProgressDto::from(openvhost_core::Progress::Extracted)
        );
        assert_eq!(
            tag_of(&MysqlInstallProgressDto::from(
                openvhost_core::Progress::Verified
            )),
            "verified"
        );
        assert_eq!(
            tag_of(&MysqlInstallProgressDto::from(
                openvhost_core::Progress::Extracted
            )),
            "extracted"
        );
    }

    #[test]
    fn a_declared_total_survives_the_crossing_and_an_undeclared_one_stays_absent() {
        let with = serde_json::to_value(MysqlInstallProgressDto::from(
            openvhost_core::Progress::Started {
                total: Some(167_977_240),
            },
        ))
        .unwrap();
        assert_eq!(with["total"], 167_977_240u64);
        let without = serde_json::to_value(MysqlInstallProgressDto::from(
            openvhost_core::Progress::Started { total: None },
        ))
        .unwrap();
        assert!(without["total"].is_null());
    }

    // ------------------------------------------------------------------
    // Group 3 — failures are classified, not flattened into prose.
    // ------------------------------------------------------------------

    /// The mandatory one: a hash mismatch must arrive as a VERIFICATION
    /// failure. Reporting it as a network error would hide the single event the
    /// SHA-256 pin exists to catch, and "network error" even invites the exact
    /// wrong response (retry until it works).
    #[test]
    fn a_hash_mismatch_is_reported_as_a_verification_failure_not_a_network_error() {
        let result = pkg_failure(openvhost_core::PkgError::HashMismatch {
            expected: "aa".repeat(32),
            actual: "bb".repeat(32),
        });
        match &result {
            MysqlInstallResultDto::VerificationFailed { expected, actual } => {
                assert_eq!(expected, &"aa".repeat(32));
                assert_eq!(actual, &"bb".repeat(32));
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(tag_of(&result), "verificationFailed");
        // And specifically NOT the shape a genuine network failure takes.
        assert_ne!(
            tag_of(&pkg_failure(openvhost_core::PkgError::Network(
                "connection reset".into()
            ))),
            "verificationFailed"
        );
    }

    #[test]
    fn a_stall_is_its_own_state_and_keeps_the_measurement_that_explains_it() {
        let result = pkg_failure(openvhost_core::PkgError::DownloadStalled {
            received: 1024,
            expected: Some(2048),
            elapsed_secs: 10.0,
            stall_secs: 30.0,
        });
        match &result {
            MysqlInstallResultDto::Stalled { detail } => {
                assert!(detail.contains("1024"), "got {detail}");
                assert!(detail.contains("30.0"), "got {detail}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_ne!(tag_of(&result), "failed");
    }

    #[test]
    fn an_already_installed_version_is_a_fact_and_not_a_failure() {
        let result = pkg_failure(openvhost_core::PkgError::AlreadyInstalled {
            name: "mysql".into(),
            version: "8.4.11".into(),
        });
        assert_eq!(
            result,
            MysqlInstallResultDto::AlreadyInstalled {
                version: "8.4.11".into()
            }
        );
    }

    /// The Intel case, end to end through the mapping the command uses.
    #[test]
    fn no_package_for_this_target_is_an_absence_that_names_the_target() {
        let result = install_failure(openvhost_core::CoreError::NoPackageForTarget {
            name: "mysql",
            version: "8.4".into(),
            target: "macos-x86_64",
        });
        assert_eq!(
            result,
            MysqlInstallResultDto::Unavailable {
                target: "macos-x86_64".into()
            }
        );
        assert_ne!(tag_of(&result), "failed");
    }

    #[test]
    fn every_install_result_state_serializes_distinctly() {
        let all = [
            MysqlInstallResultDto::Installed {
                version: "8.4.11".into(),
                detected: true,
                ledger: MysqlLedgerWriteDto::Recorded,
            },
            MysqlInstallResultDto::AlreadyInstalled {
                version: "8.4.11".into(),
            },
            MysqlInstallResultDto::Cancelled,
            MysqlInstallResultDto::VerificationFailed {
                expected: "aa".into(),
                actual: "bb".into(),
            },
            MysqlInstallResultDto::Stalled {
                detail: "stalled".into(),
            },
            MysqlInstallResultDto::Unavailable {
                target: "macos-x86_64".into(),
            },
            MysqlInstallResultDto::Failed {
                reason: "boom".into(),
            },
        ];
        let tags: Vec<String> = all.iter().map(tag_of).collect();
        for (i, a) in tags.iter().enumerate() {
            for (j, b) in tags.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{:?} and {:?} share a tag", all[i], all[j]);
                }
            }
        }
    }

    #[test]
    fn a_failed_ledger_write_keeps_its_reason_and_never_fails_the_install() {
        let dto = MysqlLedgerWriteDto::from(openvhost_core::mysql::LedgerWrite::Failed {
            reason: "database is locked".into(),
        });
        assert_eq!(
            dto,
            MysqlLedgerWriteDto::Failed {
                reason: "database is locked".into()
            }
        );
        assert_eq!(
            MysqlLedgerWriteDto::from(openvhost_core::mysql::LedgerWrite::Recorded {
                installed_at: 1
            }),
            MysqlLedgerWriteDto::Recorded
        );
    }

    // ------------------------------------------------------------------
    // Group 4 — the offer, on this host.
    // ------------------------------------------------------------------

    #[test]
    fn a_catalogued_major_is_offered_with_the_exact_version_it_would_install() {
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        let offer = package_offer(&major);
        // On the platforms this slice ships for the catalogue has an entry; on
        // any other host the honest answer is an absence naming the target. Both
        // are correct — what must never happen is an offer with no version.
        match offer {
            MysqlPackageOfferDto::Available { version } => {
                assert!(version.starts_with("8.4."), "got {version}");
            }
            MysqlPackageOfferDto::Unavailable { target } => {
                assert!(!target.is_empty());
            }
        }
    }

    #[test]
    fn apple_silicon_is_offered_the_pinned_build() {
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        assert_eq!(
            package_offer_for(
                &major,
                Some(openvhost_core::mysql::PackageTarget::MacosArm64)
            ),
            MysqlPackageOfferDto::Available {
                version: "8.4.11".into()
            }
        );
    }

    /// The Intel story, reachable on the Apple Silicon machine this is
    /// developed on — which is the whole reason `package_offer_for` takes an
    /// explicit target. Before it did, a mutation returning `Available` for
    /// every refusal survived the entire suite green here, because this arm
    /// never ran.
    #[test]
    fn an_intel_host_is_offered_nothing_and_the_absence_names_the_target() {
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        assert_eq!(
            package_offer_for(
                &major,
                Some(openvhost_core::mysql::PackageTarget::MacosX86_64)
            ),
            MysqlPackageOfferDto::Unavailable {
                target: "macos-x86_64".into()
            }
        );
    }

    #[test]
    fn a_host_this_programme_publishes_nothing_for_says_so_without_naming_an_arch() {
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        assert_eq!(
            package_offer_for(&major, None),
            MysqlPackageOfferDto::Unavailable {
                target: "this host".into()
            }
        );
    }

    /// The two answers must not be confusable: an offer always carries a
    /// version, an absence always carries a target, and neither is the other.
    #[test]
    fn an_offer_and_an_absence_serialize_distinctly() {
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        let arm = package_offer_for(
            &major,
            Some(openvhost_core::mysql::PackageTarget::MacosArm64),
        );
        let intel = package_offer_for(
            &major,
            Some(openvhost_core::mysql::PackageTarget::MacosX86_64),
        );
        assert_ne!(arm, intel);
        assert_ne!(tag_of(&arm), tag_of(&intel));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_offer_agrees_with_the_explicit_arm64_offer_on_this_machine() {
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        assert_eq!(
            package_offer(&major),
            package_offer_for(
                &major,
                Some(openvhost_core::mysql::PackageTarget::MacosArm64)
            )
        );
    }

    // ------------------------------------------------------------------
    // Group 5 — the progress throttle (audit F2). The download emits one
    // `Downloaded` per stream chunk; the webview must not see them all.
    // ------------------------------------------------------------------

    use openvhost_core::Progress as P;
    use std::time::{Duration, Instant};

    /// Feed a whole event sequence through one throttle and collect what
    /// crossed the wire, with `step` of wall clock between events.
    fn forwarded(events: Vec<P>, step: Duration) -> Vec<P> {
        let mut throttle = ProgressThrottle::new();
        let start = Instant::now();
        let mut out = Vec::new();
        for (i, ev) in events.into_iter().enumerate() {
            out.extend(throttle.admit(ev, start + step * (i as u32)));
        }
        out
    }

    /// The measured shape: 167,977,240 bytes in ~6.4 s. At 64 KiB per chunk
    /// that is ~2 560 events; a 200 ms interval over 6.4 s allows ~32, and the
    /// 1 % rule allows ~100, so the ceiling is ~100 either way.
    #[test]
    fn a_real_sized_download_does_not_flood_the_webview() {
        const TOTAL: u64 = 167_977_240;
        const CHUNK: u64 = 64 * 1024;
        let chunks = TOTAL.div_ceil(CHUNK);
        let mut events = vec![P::Started { total: Some(TOTAL) }];
        for i in 1..=chunks {
            events.push(P::Downloaded {
                bytes: (i * CHUNK).min(TOTAL),
            });
        }
        events.push(P::Verified);

        // 6.4 s spread across the ~2 561 events.
        let step = Duration::from_micros(6_400_000 / (chunks + 2));
        let out = forwarded(events, step);
        let downloaded = out
            .iter()
            .filter(|p| matches!(p, P::Downloaded { .. }))
            .count();

        assert!(
            downloaded <= 120,
            "forwarded {downloaded} byte-count events for a {chunks}-chunk download"
        );
        // ...and it is a throttle, not a mute button: the bar must still move.
        assert!(downloaded >= 20, "only {downloaded} byte-count events");
    }

    /// The one that must never be throttled. Suppressing `Verified` would make
    /// the SHA-256 check unobservable, which is the entire guarantee golden
    /// rule 6 buys.
    #[test]
    fn every_pipeline_transition_crosses_however_fast_they_arrive() {
        // A WITHHELD byte count sits in front of every transition here — a
        // 10 000-byte total makes the fraction rule a 100-byte step and no wall
        // clock passes — so each transition is taken through the flush path
        // rather than the trivial one. Without that, a throttle that swallowed
        // a transition whenever it had a count in hand would pass this test;
        // measured, it did.
        let total = Some(10_000);
        let out = forwarded(
            vec![
                P::Started { total },
                P::Downloaded { bytes: 1 }, // first — always forwarded
                P::Downloaded { bytes: 2 }, // withheld
                P::Verified,
                P::Downloaded { bytes: 3 }, // withheld
                P::Extracted,
                P::Downloaded { bytes: 4 }, // withheld
                P::Linked,
            ],
            Duration::ZERO,
        );
        for expected in [P::Started { total }, P::Verified, P::Extracted, P::Linked] {
            assert!(
                out.contains(&expected),
                "{expected:?} was swallowed: {out:?}"
            );
        }
    }

    /// A byte count withheld by the throttle must still reach the UI before
    /// the next transition, or the bar finishes short of 100 % and stays there
    /// under a "Verified" caption.
    #[test]
    fn the_final_byte_count_is_flushed_before_the_transition_that_follows_it() {
        // The real shape of the end of a download: a 10 000-byte total makes
        // the fraction rule a 100-byte step, no wall clock passes, and the
        // LAST chunk is small — so nothing but the flush can release it. This
        // is the "finishes at 98% under a Verified caption" case.
        let out = forwarded(
            vec![
                P::Started {
                    total: Some(10_000),
                },
                P::Downloaded { bytes: 1 },     // first — always forwarded
                P::Downloaded { bytes: 2 },     // withheld, and superseded below
                P::Downloaded { bytes: 9_999 }, // crosses on the fraction rule
                P::Downloaded { bytes: 10_000 }, // a 1-byte step: withheld
                P::Verified,
            ],
            Duration::ZERO,
        );

        assert_eq!(
            out,
            vec![
                P::Started {
                    total: Some(10_000)
                },
                P::Downloaded { bytes: 1 },
                // No `bytes: 2` — a withheld count is REPLACED by the next one,
                // never queued, or the flush would replay stale numbers.
                P::Downloaded { bytes: 9_999 },
                P::Downloaded { bytes: 10_000 },
                P::Verified,
            ],
            "the total must arrive, and arrive before Verified"
        );
    }

    /// The flush specifically, with no rule able to release the count on its
    /// own: a total the fraction rule cannot trigger on, and no time passing.
    #[test]
    fn a_count_no_rule_would_release_is_still_flushed_by_the_next_transition() {
        let out = forwarded(
            vec![
                P::Started { total: None },
                P::Downloaded { bytes: 1 }, // first — forwarded
                P::Downloaded { bytes: 2 }, // no total, no time: withheld
                P::Verified,
            ],
            Duration::ZERO,
        );
        assert_eq!(
            out,
            vec![
                P::Started { total: None },
                P::Downloaded { bytes: 1 },
                P::Downloaded { bytes: 2 },
                P::Verified,
            ]
        );
    }

    /// Non-vacuity, stated directly: without a total and without wall clock,
    /// intermediate counts really are suppressed. If this ever passes with
    /// every count present, the throttle has stopped throttling.
    #[test]
    fn byte_counts_are_actually_suppressed_between_the_first_and_the_flush() {
        let mut events = vec![P::Started { total: None }];
        for bytes in 1..=50 {
            events.push(P::Downloaded { bytes });
        }
        let out = forwarded(events, Duration::ZERO);
        let downloaded = out
            .iter()
            .filter(|p| matches!(p, P::Downloaded { .. }))
            .count();
        assert_eq!(downloaded, 1, "expected only the first count: {out:?}");
    }

    /// Time alone releases a count when the server declared no length — the
    /// only rule available on that path.
    #[test]
    fn the_interval_alone_releases_a_count_when_no_total_was_declared() {
        let out = forwarded(
            vec![
                P::Started { total: None },
                P::Downloaded { bytes: 1 },
                P::Downloaded { bytes: 2 },
            ],
            DOWNLOAD_EVENT_INTERVAL,
        );
        assert_eq!(
            out,
            vec![
                P::Started { total: None },
                P::Downloaded { bytes: 1 },
                P::Downloaded { bytes: 2 },
            ]
        );
    }

    // A discovered-but-uncatalogued major (a user's own 9.x) cannot be
    // constructed from outside `openvhost-core` — `MysqlMajor::from_probe` is
    // `pub(crate)` and `parse` is catalogue-gated — which is itself the
    // guarantee that such a major never reaches `package_offer` over IPC. The
    // "no pinned build for a shape-valid major" case is covered where the value
    // can actually be minted: `mysql/package/catalogue.rs`'s
    // `a_major_with_no_pinned_build_is_refused_on_a_supported_target`.
}
