// SPDX-License-Identifier: GPL-3.0-or-later
//! Installing MariaDB from OpenVHost's own package tree, over IPC (P1
//! MariaDB UI design D3/D4/D7). A sibling of `mysql_pkg` — mirrors its shape
//! deliberately (design D5's "consistency beats novelty"): the DTOs that
//! carry a `Progress` event to the webview, the exhaustive mapping from
//! `openvhost-core`'s typed errors onto states the UI can render, and two
//! commands (`install_mariadb`, `cancel_mariadb_install`) that hold no logic
//! of their own beyond wiring.
//!
//! **No Homebrew, ever.** Unlike MySQL's migration off brew, MariaDB never
//! had a brew half to migrate away from (the off-Homebrew decision,
//! 2026-08-01) — `openvhost_core::mariadb::discover_mariadb` looks only at
//! this app's own package tree. There is therefore no
//! `MariadbRuntimeSourceDto` here: `mysql_pkg::MysqlRuntimeSourceDto` exists
//! to answer "packaged or Homebrew", and that question has exactly one
//! answer for this engine.
//!
//! Two facts this module exists to make visible, mirroring `mysql_pkg`'s own
//! two (its third — "which source" — does not apply here, per the paragraph
//! above):
//!
//! 1. **That verification happened** — [`MariadbInstallProgressDto::Verified`]
//!    is its own variant, distinct from `Extracted`, for the identical reason
//!    `mysql_pkg`'s copy is.
//! 2. **That the user can cancel** — [`cancel_mariadb_install`]. The install
//!    permit `openvhost-pkg` holds is process-wide and shared with every
//!    OTHER package this pipeline installs (PHP, MySQL, MariaDB), so a stalled
//!    MariaDB download would starve a later PHP install just as readily as a
//!    later MySQL one — dropping the install future is the cancel, and this
//!    module holds the handle that drops it.
//!
//! **A third state MySQL's offer never needed.**
//! [`MariadbPackageOfferDto::AwaitingRelease`] exists because slice A's
//! catalogue pin can be genuinely correct — audited, signature-traced,
//! artifact-contract-passing — while the GitHub release that would SERVE it
//! does not exist yet (`openvhost_core::mariadb::Availability`, design D2).
//! Collapsing that into `Unavailable` would tell an Apple Silicon owner their
//! machine is unsupported when the truth is "nobody can have this yet".

use std::path::Path;
use std::sync::{Arc, RwLock};

use openvhost_proc::Supervisor;
use tauri_specta::Event;

use crate::commands::{
    IpcError, MARIADB_INSTALL_RUN, RunningInstallGuard, now_ms, rescan_mariadb_into_state,
    stack_paths,
};
use crate::db_state::DbHandle;
pub(crate) use crate::mysql_pkg::ProgressThrottle;
use crate::stack::StackPaths;

/// Whether this build can install MariaDB on THIS host, and what it would
/// install — the MariaDB counterpart of `mysql_pkg::MysqlPackageOfferDto`,
/// extended with the THIRD state design D2 requires (this module's own doc
/// comment explains why): a build exists and is pinned, but the release that
/// would serve it has not been published yet.
///
/// Matched exhaustively wherever it is consumed, with **no wildcard arm**: a
/// fourth state must be decided about here rather than silently folded into
/// one of the first three.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbPackageOfferDto {
    Available {
        version: String,
    },
    /// The pinned build exists and was audited, but the GitHub release that
    /// would serve it has not been published — the next action belongs to
    /// the maintainer, not the user. `tag` is the release to publish, e.g.
    /// `"mariadb-11.4.9"`.
    AwaitingRelease {
        tag: String,
    },
    Unavailable {
        target: String,
    },
}

/// What this build would install for MariaDB on `target`.
///
/// `target` is an explicit `Option`, mirroring `mysql_pkg::package_offer_for`
/// exactly and for the identical reason: both branches must be reachable
/// from a test on any one machine.
pub(crate) fn package_offer_for(
    target: Option<openvhost_core::PackageTarget>,
) -> MariadbPackageOfferDto {
    let named = match target {
        Some(t) => t.as_str().to_string(),
        None => "this host".to_string(),
    };
    match openvhost_core::mariadb_package_for_target(target) {
        // Exhaustive on `Availability`, no wildcard: a third state would have
        // to be decided about here too, not silently treated as an offer.
        Ok(entry) => match entry.availability {
            openvhost_core::Availability::Published => MariadbPackageOfferDto::Available {
                version: entry.version.to_string(),
            },
            openvhost_core::Availability::AwaitingRelease { tag } => {
                MariadbPackageOfferDto::AwaitingRelease {
                    tag: tag.to_string(),
                }
            }
        },
        Err(_) => MariadbPackageOfferDto::Unavailable { target: named },
    }
}

/// What this build would install for MariaDB on the host it was compiled
/// for.
pub(crate) fn package_offer() -> MariadbPackageOfferDto {
    package_offer_for(openvhost_core::PackageTarget::host())
}

/// One step of the install pipeline, as the user watches it — the wire copy
/// of `openvhost_pkg::Progress`, identical in shape to
/// `mysql_pkg::MysqlInstallProgressDto` (the pipeline itself is shared — see
/// `install_mariadb_package`'s own doc comment). Kept as its own type rather
/// than reused, mirroring every other MariaDB DTO in this slice: the two
/// engines' wire shapes must be able to diverge without an edit to one
/// silently reaching the other.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbInstallProgressDto {
    Started { total: Option<u64> },
    Downloaded { bytes: u64 },
    Verified,
    Extracted,
    Linked,
}

impl From<openvhost_core::Progress> for MariadbInstallProgressDto {
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
/// No `major`/`series` field, unlike [`crate::mysql_pkg::MysqlInstallProgressEvent`]:
/// this build ships exactly one series, so a field nothing can vary would be
/// pure overhead — the same reasoning `MariadbInstance` gives for leaving
/// `major` off its own struct.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MariadbInstallProgressEvent {
    pub ts_ms: u64,
    pub progress: MariadbInstallProgressDto,
}

/// Whether the install was also recorded in `state.db`'s package ledger —
/// the MariaDB mirror of `mysql_pkg::MysqlLedgerWriteDto`. Reused from the
/// same `openvhost_core::mysql::LedgerWrite` the MySQL path returns (the
/// ledger is package-agnostic; see `MariadbPackageInstall`'s own doc
/// comment), so only the WIRE type is duplicated, not the underlying model.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbLedgerWriteDto {
    Recorded,
    Failed { reason: String },
}

impl From<openvhost_core::mysql::LedgerWrite> for MariadbLedgerWriteDto {
    fn from(w: openvhost_core::mysql::LedgerWrite) -> Self {
        use openvhost_core::mysql::LedgerWrite as L;
        match w {
            L::Recorded { .. } => Self::Recorded,
            L::Failed { reason } => Self::Failed { reason },
        }
    }
}

/// How one `install_mariadb` call ended — the MariaDB mirror of
/// `mysql_pkg::MysqlInstallResultDto`, with `Unavailable` renamed nowhere and
/// one addition: `AwaitingRelease`, the release-not-published refusal
/// `install_mariadb_package` raises before any network or filesystem work
/// (design D2/D5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbInstallResultDto {
    Installed {
        version: String,
        detected: bool,
        ledger: MariadbLedgerWriteDto,
    },
    AlreadyInstalled {
        version: String,
    },
    Cancelled,
    VerificationFailed {
        expected: String,
        actual: String,
    },
    Stalled {
        detail: String,
    },
    /// The pinned build exists but the release that would serve it has not
    /// been published — see [`MariadbPackageOfferDto::AwaitingRelease`].
    /// `tag` is the release a human has to create.
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

/// `install_mariadb`'s return: whether it installed, and how.
///
/// No `major`/`series` field, unlike `mysql_pkg::MysqlInstallOutcomeDto` —
/// same reasoning as [`MariadbInstallProgressEvent`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MariadbInstallOutcomeDto {
    pub result: MariadbInstallResultDto,
}

/// Map a failed install onto a renderable state.
///
/// Exhaustive over `PkgError` with **no wildcard arm**, mirroring
/// `mysql_pkg::pkg_failure` exactly.
fn pkg_failure(e: openvhost_core::PkgError) -> MariadbInstallResultDto {
    use openvhost_core::PkgError as E;
    match e {
        E::HashMismatch { expected, actual } => {
            MariadbInstallResultDto::VerificationFailed { expected, actual }
        }
        stalled @ E::DownloadStalled { .. } => MariadbInstallResultDto::Stalled {
            detail: stalled.to_string(),
        },
        E::AlreadyInstalled { version, .. } => {
            MariadbInstallResultDto::AlreadyInstalled { version }
        }
        other @ (E::InvalidComponent { .. }
        | E::InvalidUrl(_)
        | E::InvalidSha256
        | E::InvalidWarmupPath { .. }
        | E::Network(_)
        | E::TooLarge { .. }
        | E::UnsafeArchive(_)
        | E::Io { .. }
        | E::Internal(_)
        | E::Unsupported(_)) => MariadbInstallResultDto::Failed {
            reason: other.to_string(),
        },
    }
}

/// The same mapping one level up, for the error type
/// `install_mariadb_package` actually returns. `PackageNotPublished` is the
/// state this build is in TODAY (design D2/D5) and is given its own arm
/// rather than falling into the generic `other` tail, so the Databases page
/// can render "waiting on a release", not a red banner.
pub(crate) fn install_failure(e: openvhost_core::CoreError) -> MariadbInstallResultDto {
    match e {
        openvhost_core::CoreError::NoPackageForTarget { target, .. } => {
            MariadbInstallResultDto::Unavailable {
                target: target.to_string(),
            }
        }
        openvhost_core::CoreError::PackageNotPublished { tag, .. } => {
            MariadbInstallResultDto::AwaitingRelease {
                tag: tag.to_string(),
            }
        }
        openvhost_core::CoreError::Package(pkg) => pkg_failure(pkg),
        // `CoreError` is a wide, crate-wide enum whose other variants are not
        // install-pipeline states; reported verbatim rather than enumerated
        // here, mirroring `mysql_pkg::install_failure`'s identical reasoning.
        other => MariadbInstallResultDto::Failed {
            reason: other.to_string(),
        },
    }
}

/// Install MariaDB's pinned 11.4.9 build from the compiled-in catalogue,
/// streaming [`MariadbInstallProgressEvent`] as the pipeline advances, then
/// rescan so a freshly installed runtime is picked up.
///
/// Takes no arguments at all: unlike `install_mysql`, there is no major to
/// parse — `openvhost_core::install_mariadb_package` resolves the ONE
/// catalogued series itself (design D7: "none takes a series argument").
///
/// The run is **spawned**, not awaited inline, so its `AbortHandle` can be
/// recorded — mirrors `install_mysql`'s identical reasoning.
#[tauri::command]
#[specta::specta]
// DEGRADE (optional-state.db design D2), for the same reason and with the same
// cost as `mysql_pkg::install_mysql`: a store that never opened costs the
// ledger row, never the install, and `MariadbInstallResultDto` reports that as
// `ledger: Failed { reason }`.
pub async fn install_mariadb(
    app: tauri::AppHandle,
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, crate::commands::InstallLock>,
) -> Result<MariadbInstallOutcomeDto, IpcError> {
    // One at a time, shared with `install_php`/`install_mysql`/
    // `uninstall_package`. `try_lock` rather than `lock`: a second press is
    // refused with an explanation, never silently queued behind a download
    // that has no wall-clock bound.
    let Ok(_guard) = lock.inner().guard.try_lock() else {
        return Err(IpcError::Core {
            message: "an install is already running".into(),
        });
    };

    let p = stack_paths(&paths)?;
    let outcome = run_install(
        &app,
        &p.home,
        db.inner(),
        lock.inner(),
        runtimes.inner(),
        sup.inner(),
    )
    .await?;
    Ok(outcome)
}

/// The body of [`install_mariadb`], minus `tauri::State` extraction — mirrors
/// `mysql_pkg::run_install`'s split and its ordering discipline (spawn,
/// record the abort handle, THEN await).
async fn run_install(
    app: &tauri::AppHandle,
    home: &Path,
    db: &DbHandle,
    lock: &crate::commands::InstallLock,
    runtimes: &RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>,
    sup: &Supervisor,
) -> Result<MariadbInstallOutcomeDto, IpcError> {
    // `None` when the store never opened, through the one named seam
    // `mysql_pkg::run_install` uses — see `DbHandle::install_ledger`.
    // `install_mariadb_package` owns the reason it reports for the missing row;
    // nothing is retyped here.
    let ledger = db.install_ledger();
    let emitter = app.clone();
    let spawn_root = openvhost_core::PackagesRoot::from_home(home);

    let task = tokio::spawn(async move {
        // Same audit-F2-taught throttle `mysql_pkg::run_install` applies,
        // reused directly rather than a second copy — see this module's own
        // `use` of `ProgressThrottle`.
        let mut throttle = ProgressThrottle::new();
        openvhost_core::install_mariadb_package(&spawn_root, ledger.as_ref(), move |progress| {
            for progress in throttle.admit(progress, std::time::Instant::now()) {
                let _ = MariadbInstallProgressEvent {
                    ts_ms: now_ms(),
                    progress: progress.into(),
                }
                .emit(&emitter);
            }
        })
        .await
    });

    let abort_handle = task.abort_handle();
    let (kind, operation) = MARIADB_INSTALL_RUN;
    lock.set_running(
        kind,
        operation,
        format!("MariaDB {}", openvhost_core::MARIADB_SERIES),
        abort_handle.clone(),
    );
    let _running_guard = RunningInstallGuard {
        lock,
        abort: abort_handle,
    };

    let result = match task.await {
        Ok(Ok(install)) => {
            // No seed: unlike a brew probe, nothing here needs one — see
            // `rescan_mariadb_into_state`'s own doc comment for why.
            let discovery = rescan_mariadb_into_state(runtimes, sup, home).await?;
            let detected = discovery
                .iter()
                .any(|rt| rt.version == install.package.version);
            MariadbInstallResultDto::Installed {
                version: install.package.version,
                detected,
                ledger: install.ledger.into(),
            }
        }
        Ok(Err(e)) => install_failure(e),
        // The cancel path — `cancel_mariadb_install` or `perform_quit` fired
        // the handle. Staging unwound with the future, so this is a plain
        // "nothing happened", not a failure to explain away.
        Err(join_err) if join_err.is_cancelled() => MariadbInstallResultDto::Cancelled,
        Err(join_err) => MariadbInstallResultDto::Failed {
            reason: format!("the install task ended unexpectedly: {join_err}"),
        },
    };

    Ok(MariadbInstallOutcomeDto { result })
}

/// Cancel an in-flight MariaDB install, if one is running.
///
/// **Kind- and operation-checked**, exactly like `cancel_mysql_install`: the
/// check and the abort happen under one `InstallLock::abort_running_if`
/// acquisition, so the slot cannot change in between, and both discriminators
/// (`InstallKind::Mariadb`, `PackageOperation::Install`) must match — a
/// MariaDB install sharing MySQL's `(kind, operation)` pair would let this
/// command fire on MySQL's run, which is audit F1 with a second engine
/// (D4).
#[tauri::command]
#[specta::specta]
pub async fn cancel_mariadb_install(
    lock: tauri::State<'_, crate::commands::InstallLock>,
) -> Result<bool, IpcError> {
    let (kind, operation) = MARIADB_INSTALL_RUN;
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
    // ------------------------------------------------------------------

    /// The state this build is in TODAY: the release is not published, so
    /// this build offers `AwaitingRelease`, never `Available` and never
    /// `Unavailable` (which would tell an Apple Silicon owner the wrong
    /// thing — see this module's own doc comment).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn apple_silicon_is_offered_awaiting_release_while_the_pin_is_unpublished() {
        let offer = package_offer_for(Some(openvhost_core::PackageTarget::MacosArm64));
        match offer {
            MariadbPackageOfferDto::AwaitingRelease { tag } => {
                assert_eq!(tag, "mariadb-11.4.9");
            }
            other => {
                panic!("expected AwaitingRelease while the release is unpublished, got {other:?}")
            }
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_offer_agrees_with_the_explicit_arm64_offer_on_this_machine() {
        assert_eq!(
            openvhost_core::PackageTarget::host(),
            Some(openvhost_core::PackageTarget::MacosArm64)
        );
        assert_eq!(
            package_offer(),
            package_offer_for(Some(openvhost_core::PackageTarget::MacosArm64))
        );
    }

    /// The Intel story: no signature-checked x86_64 pin exists at all (out of
    /// scope per the design), so Intel is offered nothing and the absence
    /// names the target — never `AwaitingRelease`, which would wrongly
    /// suggest a build is coming.
    #[test]
    fn an_intel_host_is_offered_nothing_and_the_absence_names_the_target() {
        assert_eq!(
            package_offer_for(Some(openvhost_core::PackageTarget::MacosX86_64)),
            MariadbPackageOfferDto::Unavailable {
                target: "macos-x86_64".into()
            }
        );
    }

    #[test]
    fn a_host_this_programme_publishes_nothing_for_says_so_without_naming_an_arch() {
        assert_eq!(
            package_offer_for(None),
            MariadbPackageOfferDto::Unavailable {
                target: "this host".into()
            }
        );
    }

    /// The three states must not be confusable on the wire: distinct tags,
    /// distinct shapes.
    #[test]
    fn the_three_offer_states_serialize_distinctly() {
        let all = [
            MariadbPackageOfferDto::Available {
                version: "11.4.9".into(),
            },
            MariadbPackageOfferDto::AwaitingRelease {
                tag: "mariadb-11.4.9".into(),
            },
            MariadbPackageOfferDto::Unavailable {
                target: "macos-x86_64".into(),
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

    // ------------------------------------------------------------------
    // Group 2 — progress crosses the wire as five DISTINCT states, exactly
    // like mysql_pkg's own (the pipeline events are shared).
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
            .map(|p| serde_json::to_value(MariadbInstallProgressDto::from(p)).unwrap())
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
        assert_ne!(
            MariadbInstallProgressDto::from(openvhost_core::Progress::Verified),
            MariadbInstallProgressDto::from(openvhost_core::Progress::Extracted)
        );
    }

    /// The event carries no `major`/`series` field at all — pinned directly
    /// against the wire shape, so a future edit that adds one back (to
    /// "match" MySQL's) fails here rather than silently reintroducing the
    /// dictionary key design D6 exists to avoid.
    #[test]
    fn the_progress_event_carries_no_series_field() {
        let wire = serde_json::to_value(MariadbInstallProgressEvent {
            ts_ms: 1,
            progress: MariadbInstallProgressDto::Verified,
        })
        .unwrap();
        assert!(wire.get("major").is_none(), "got {wire:?}");
        assert!(wire.get("series").is_none(), "got {wire:?}");
    }

    // ------------------------------------------------------------------
    // Group 3 — failures are classified, not flattened into prose.
    // ------------------------------------------------------------------

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

    /// The state this build is in TODAY: `install_mariadb_package` refuses
    /// with `PackageNotPublished` before any network work, and that must map
    /// to its own renderable state — never a generic `Failed` banner.
    #[test]
    fn a_package_not_yet_published_is_reported_as_awaiting_release_not_a_generic_failure() {
        let result = install_failure(openvhost_core::CoreError::PackageNotPublished {
            name: "mariadb",
            version: "11.4.9",
            tag: "mariadb-11.4.9",
            url: "https://example.invalid/mariadb-11.4.9.tar.gz",
        });
        assert_eq!(
            result,
            MariadbInstallResultDto::AwaitingRelease {
                tag: "mariadb-11.4.9".into()
            }
        );
        assert_ne!(tag_of(&result), "failed");
    }

    #[test]
    fn no_package_for_this_target_is_an_absence_that_names_the_target() {
        let result = install_failure(openvhost_core::CoreError::NoPackageForTarget {
            name: "mariadb",
            version: "11.4".into(),
            target: "macos-x86_64",
        });
        assert_eq!(
            result,
            MariadbInstallResultDto::Unavailable {
                target: "macos-x86_64".into()
            }
        );
        assert_ne!(tag_of(&result), "failed");
    }

    #[test]
    fn every_install_result_state_serializes_distinctly() {
        let all = [
            MariadbInstallResultDto::Installed {
                version: "11.4.9".into(),
                detected: true,
                ledger: MariadbLedgerWriteDto::Recorded,
            },
            MariadbInstallResultDto::AlreadyInstalled {
                version: "11.4.9".into(),
            },
            MariadbInstallResultDto::Cancelled,
            MariadbInstallResultDto::VerificationFailed {
                expected: "aa".into(),
                actual: "bb".into(),
            },
            MariadbInstallResultDto::Stalled {
                detail: "stalled".into(),
            },
            MariadbInstallResultDto::AwaitingRelease {
                tag: "mariadb-11.4.9".into(),
            },
            MariadbInstallResultDto::Unavailable {
                target: "macos-x86_64".into(),
            },
            MariadbInstallResultDto::Failed {
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
    fn the_outcome_carries_no_series_field_either() {
        let wire = serde_json::to_value(MariadbInstallOutcomeDto {
            result: MariadbInstallResultDto::Cancelled,
        })
        .unwrap();
        assert!(wire.get("major").is_none(), "got {wire:?}");
        assert!(wire.get("series").is_none(), "got {wire:?}");
    }
}
