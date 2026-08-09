// SPDX-License-Identifier: GPL-3.0-or-later
//! Tauri command surface — thin validation + delegation to openvhost-core
//! (business logic never lives here; master plan §5).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

// `WebServerAdapter` is imported for its `supports_hot_reload` method: it is a
// trait method, so the trait must be in scope even though the call site names
// the concrete `NginxAdapter`.
use openvhost_conf::WebServerAdapter;

use openvhost_core::{
    ApplyError, ApplyInput, ChangeKind, CoreInfo, Db, Docroot, Domain, InstalledRuntimes, NewSite,
    PhpSettingsRepository, PhpVersion, Site, SiteId, SiteName, SiteRepository, SqlitePhpSettings,
    SqliteSiteRepository, SqliteWebServerSettings, WebServer, WebServerSettingsRepository,
};
// Not re-exported at the crate root like the flat types above: `scaffold`'s
// home is the `site` submodule (Tasks 2-3), and it stays that way rather than
// growing `lib.rs`'s re-export list for a type only this one command needs.
use openvhost_core::site::scaffold::{ScaffoldOutcome, ScaffoldStep, scaffold, scaffold_path};
// The managed store wrapper (optional-state.db design D1). Commands take
// `State<'_, DbHandle>` and resolve through `require()`/`optional()`; the bare
// `Db` above is still named here for the `&Db` those accessors hand back, but
// it is never a managed type and never a command parameter.
use crate::db_state::DbHandle;
// The MySQL package surface lives in its own sibling module (this file is
// already ~8 200 lines); only the two DTOs a `MysqlInstanceDto` embeds are
// named here.
use crate::mysql_pkg::{MysqlPackageOfferDto, MysqlRuntimeSourceDto};
// PHP's package surface likewise lives in its own sibling module; the two DTOs
// a `PhpRuntimeDto` embeds and the tagged outcome `install_php` returns for
// BOTH of its routes (design D4) are named here.
use crate::php_pkg::{PhpInstallOutcomeDto, PhpPackageOfferDto, PhpRuntimeSourceDto};

use crate::stack::StackPaths;

/// Serializable command error (spec §7.2). Establishes the pattern:
/// every command returns `Result<_, IpcError>` and the UI renders failures.
#[derive(Debug, Clone, serde::Serialize, thiserror::Error, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum IpcError {
    /// Dev-only simulated failure used to exercise the UI error path.
    #[error("simulated failure (dev only)")]
    Simulated,
    /// An error bubbled up from openvhost-core.
    #[error("{message}")]
    Core { message: String },
    /// An error bubbled up from the process supervisor.
    #[error("{message}")]
    Proc { message: String },
    /// A domain value failed validation; `field` names the offending input so
    /// the UI can mark it instead of showing a generic banner.
    #[error("{message}")]
    Validation { field: String, message: String },
}

impl From<openvhost_conf::ConfError> for IpcError {
    /// A failed `parse` on one of the nginx settings newtypes becomes a
    /// field-shaped error the form can mark; everything else (render failures,
    /// IO, validator problems) becomes a banner.
    ///
    /// `field` is the newtype's own `&'static str`, which is deliberately the
    /// **snake_case DTO field name** (`gzip_comp_level`, `client_max_body_size`)
    /// — the existing `fieldErrors` seam is keyed by the backend's snake_case
    /// names, not the camelCase wire names (see `SiteDrawer.svelte`, which
    /// reads `fieldErrors.web_server` and `fieldErrors.php_version`). Inventing
    /// a second convention here would mean the settings form silently marked
    /// nothing.
    fn from(e: openvhost_conf::ConfError) -> Self {
        match e {
            openvhost_conf::ConfError::InvalidField {
                field,
                value,
                reason,
            } => IpcError::Validation {
                field: field.to_string(),
                message: format!("{value:?} {reason}"),
            },
            other => IpcError::Core {
                message: other.to_string(),
            },
        }
    }
}

impl From<openvhost_core::CoreError> for IpcError {
    fn from(e: openvhost_core::CoreError) -> Self {
        match e {
            openvhost_core::CoreError::Validation { field, reason } => IpcError::Validation {
                field: field.to_string(),
                message: reason,
            },
            other => IpcError::Core {
                message: other.to_string(),
            },
        }
    }
}

/// Manage the store on a mock app the way `lib.rs` does: as a [`DbHandle`],
/// never a bare `Db`.
///
/// ONE helper rather than the ~21 `app.manage(...)` calls it replaces, so what
/// a test app manages is decided in a single place — and so a test app cannot
/// drift from production into managing something `lib.rs` does not.
///
/// A bare `Db` is managed nowhere now, in production or in tests, which is what
/// makes design D6's property hold: a new `db: State<'_, Db>` parameter fails
/// on every machine, including this harness, rather than only on one whose
/// store is broken.
#[cfg(test)]
fn manage_db(app: &tauri::App<tauri::test::MockRuntime>, db: Db) {
    use tauri::Manager;
    app.manage(DbHandle::Ready(db));
}

/// The reason a test's store is down.
///
/// A sentinel no other string in the tree can collide with, so an assertion
/// that finds it has found THIS refusal — not some other `IpcError::Core` that
/// happens to be in flight. `os error 14` is sqlite's real "unable to open
/// database file", the shape a genuine `Db::open` failure carries.
#[cfg(test)]
pub(crate) const STORE_DOWN_REASON: &str =
    "openvhost-test-sentinel: unable to open database file (os error 14)";

/// A handle for a store that failed to open — the arm `lib.rs` takes when
/// `Db::open` returns `Err`.
#[cfg(test)]
pub(crate) fn store_down() -> DbHandle {
    DbHandle::Unavailable {
        reason: STORE_DOWN_REASON.to_string(),
    }
}

/// Manage a store that is DOWN, exactly as `lib.rs` does on the failed arm:
/// the handle, and **no** bare `Db` at all.
#[cfg(test)]
fn manage_store_down(app: &tauri::App<tauri::test::MockRuntime>) {
    use tauri::Manager;
    app.manage(store_down());
}

/// Assert `err` is a REFUSE command's typed store refusal.
///
/// Three claims, and all three are the point of the slice rather than
/// incidental: it is the typed `IpcError` a page can render; it CARRIES THE
/// REASON, so the user is told *unable to open database file* and not merely
/// "unavailable"; and it never contains the string a user was previously shown
/// — Tauri's own "you must call `.manage()` before using this command".
///
/// `what` names the command, so a group test that walks several of them says
/// which one broke instead of pointing at a line number.
///
/// VACUITY, measured once here for every caller rather than restated at each
/// of them. The group is the **13** tests `cargo test store_is_down` selects,
/// which is deliberately not the same set as this function's callers — 10 of
/// the 13 call it, both gate tests among them. Of the other three, two assert
/// the same claims inline because they check a `Failed` DTO rather than an
/// `IpcError`; the third reads `unavailable_reason()` and asserts only the
/// reason, as the mutation below records. Three mutations, each run against
/// all 13:
///
/// - `unavailable_message` reduced to `format!("{STORE_UNAVAILABLE}")` — the
///   reason dropped: **12 of the 13 failed** on the "carries the reason" claim.
///   The one that did not is `db_state`'s `state_store_status_reports_the_
///   reason_when_the_store_is_down`, which reads `unavailable_reason()` and so
///   never passes through `unavailable_message` — it guards the banner's
///   sentence, not this one.
/// - `unavailable_message` with ``". You must call `.manage()` before using
///   this command."`` appended: **the same 12 failed**, on the third claim,
///   with the first two still passing — so that claim is doing its own work.
/// - `store_down()` changed to hand back a real `DbHandle::Ready` over an
///   in-memory store: **all 13 failed**. This is the neuter that stands in for
///   "the guard was removed" wherever removing it is not expressible —
///   `require` is the only way to obtain a `&Db`, so for several of these
///   commands there is no way to write the degraded version at all. That is
///   design D6 holding, and it is why this mutation exists.
///
/// Each was reverted and re-run green.
#[cfg(test)]
pub(crate) fn assert_store_refusal(err: &IpcError, what: &str) {
    let IpcError::Core { message } = err else {
        panic!("{what}: expected IpcError::Core, got {err:?}");
    };
    assert!(
        message.contains(STORE_DOWN_REASON),
        "{what}: the refusal must carry the reason the store is unavailable, got {message:?}"
    );
    assert!(
        message.contains("state.db"),
        "{what}: the refusal must name what is unavailable, got {message:?}"
    );
    assert!(
        !message.contains(".manage()"),
        "{what}: a user must never be told to call a Rust API, got {message:?}"
    );
}

#[tauri::command]
#[specta::specta] // registers this command's types for TS binding generation (spec §7.3)
pub fn core_info(simulate_error: Option<bool>) -> Result<CoreInfo, IpcError> {
    // Dev-only demo affordance (spec §7.1): ignored in release builds.
    if cfg!(debug_assertions) && simulate_error.unwrap_or(false) {
        return Err(IpcError::Simulated);
    }
    Ok(openvhost_core::core_info(env!("CARGO_PKG_VERSION"))?)
}

use std::sync::Arc;

use openvhost_proc::{LogLevel, LogLine, ProcError, ServiceState, ServiceStatus, Supervisor};
use tauri_specta::Event;

impl From<ProcError> for IpcError {
    fn from(e: ProcError) -> Self {
        IpcError::Proc {
            message: e.to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStateEvent {
    pub id: String,
    pub state: ServiceState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServiceLogEvent {
    pub id: String,
    pub ts_ms: u64,
    pub level: LogLevel,
    pub line: String,
}

/// Emitted when [`openvhost_proc::SupervisorEvent::Registered`] fires — a
/// service row was added, including after startup (tray/menu-bar design
/// `2026-07-31-p1-tray-design.md` D2: a PHP major installed at runtime, or a
/// freshly initialized MySQL major, used to reach no observer at all until
/// the next full [`list_services`] round trip). Carries the full
/// [`ServiceStatus`] rather than a delta, unlike [`ServiceStateEvent`]: the
/// receiving side may be seeing this id for the first time, so there is no
/// existing row to patch.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRegisteredEvent {
    pub status: ServiceStatus,
}

/// Emitted when [`openvhost_proc::SupervisorEvent::Unregistered`] fires — a
/// service row was REMOVED (package-uninstall design
/// `2026-07-31-p1-pkg-uninstall-design.md` D4: uninstalling a PHP or MySQL
/// major must make its row leave the Services page and the tray without a
/// restart, not leave it behind failing).
///
/// Carries the id alone, the mirror of [`ServiceRegisteredEvent`]'s full
/// status: the row is gone, so there is no state left to describe, and every
/// receiver's job is the same subtraction.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUnregisteredEvent {
    pub id: String,
}

// These four commands must stay `async fn`: Tauri dispatches async commands
// onto its own tokio runtime, which is what gives `Supervisor::start`'s
// internal `tokio::spawn` a valid reactor to spawn onto. A sync `#[tauri::
// command]` runs on a plain threadpool with no tokio context, so
// `tokio::spawn` inside it panics ("must be called from the context of a
// Tokio 1.x runtime"). The bodies stay thin sync calls — no `.await` is
// needed, `async fn` alone is what matters here.
#[tauri::command]
#[specta::specta]
pub async fn list_services(
    sup: tauri::State<'_, Arc<Supervisor>>,
) -> Result<Vec<ServiceStatus>, IpcError> {
    Ok(sup.snapshot())
}

#[tauri::command]
#[specta::specta]
pub async fn start_service(
    sup: tauri::State<'_, Arc<Supervisor>>,
    id: String,
) -> Result<(), IpcError> {
    sup.start(&id).map_err(IpcError::from)
}

#[tauri::command]
#[specta::specta]
pub async fn stop_service(
    sup: tauri::State<'_, Arc<Supervisor>>,
    id: String,
) -> Result<(), IpcError> {
    sup.stop(&id).map_err(IpcError::from)
}

/// Tell the Rust side that the quit dialog's listener is registered.
///
/// Until this lands, a close request is NOT prevented — see `quit::UiReady` for
/// why an emit's return value cannot stand in for this. Idempotent and
/// unauthenticated by design: the only thing a caller can achieve is enabling a
/// confirmation dialog on their own window.
#[tauri::command]
#[specta::specta]
pub async fn quit_dialog_ready(app: tauri::AppHandle) -> Result<(), IpcError> {
    use tauri::Manager;
    if let Some(ready) = app.try_state::<crate::quit::UiReady>() {
        ready.mark();
    }
    Ok(())
}

/// Quit after the UI has confirmed it: stop every pending service, then destroy
/// the window.
///
/// The mechanics and the reasoning live in `crate::quit` — this is only the IPC
/// boundary. It takes no arguments on purpose: there is nothing to validate, and
/// the command can therefore do exactly one thing no matter who calls it. It is
/// NOT the thing that decides to quit; `quit::request_quit` has already asked the
/// user, and a caller reaching this directly could only do what the close button
/// already does.
#[tauri::command]
#[specta::specta]
pub async fn confirm_quit(app: tauri::AppHandle) -> Result<(), IpcError> {
    // `Proc`: every way this can fail is about tearing down supervised processes
    // or the window that hosts them, and the message is rendered verbatim in the
    // quit dialog.
    crate::quit::perform_quit(&app)
        .await
        .map_err(|message| IpcError::Proc { message })
}

#[tauri::command]
#[specta::specta]
pub async fn service_log_tail(
    sup: tauri::State<'_, Arc<Supervisor>>,
    id: String,
    n: u32,
) -> Result<Vec<LogLine>, IpcError> {
    sup.log_tail(&id, n as usize).map_err(IpcError::from)
}

/// Summed resident memory of the supervised services.
///
/// `bytes` and `process_count` are both u64/u32 crossing a
/// `.dangerously_cast_bigints_to_number()` boundary — see `lib.rs`'s standing
/// warning, which names "byte totals" as the case requiring a conscious check.
/// `2^53` bytes is 9 petabytes; a resident set is many orders of magnitude
/// below it. Checked, not assumed.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServicesMemoryDto {
    pub bytes: u64,
    /// How many pids actually produced a figure — NOT how many services are
    /// running. See `sum_readings`.
    pub process_count: u32,
}

/// Total bytes under the OpenVHost home. Same bigint check as
/// [`ServicesMemoryDto`]: a home directory is nowhere near 9 PB.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HomeUsageDto {
    pub bytes: u64,
}

/// Sum the readings that produced a figure, and count them.
///
/// Extracted from `services_memory` so the rule is testable without a live
/// `AppHandle` and a real supervisor: `None` readings (a pid that exited between
/// the snapshot and the read) drop out of the sum AND the count together, so the
/// figure and its "N processes" label can never contradict each other.
/// `saturating_add` so an absurd reading cannot wrap the total into a small,
/// plausible-looking number.
fn sum_readings(readings: impl Iterator<Item = Option<u64>>) -> (u64, u32) {
    let mut bytes: u64 = 0;
    let mut count: u32 = 0;
    for r in readings.flatten() {
        bytes = bytes.saturating_add(r);
        count = count.saturating_add(1);
    }
    (bytes, count)
}

/// Read one resident-memory figure per pid, aborting the WHOLE collection on
/// the first `Err`.
///
/// This is split out of `services_memory` because the abort-on-`Err` rule is a
/// spec requirement (§4.1: an unreadable pid means measurement is impossible
/// on this platform, so a partial sum must never be presented to the user as
/// if it were complete) that used to live ENTIRELY inside that command's body,
/// behind a `.map_err(...)?` that needs a live `AppHandle` and a real
/// `Supervisor` to reach at all. Nothing could exercise it in a unit test, so
/// a later "simplification" — swapping that `?` for `.ok()`, pushing a reading
/// before mapping its error, or folding the loop into a `filter_map` over the
/// `Result` — would silently start reporting a partial sum as if it were
/// complete, and no test would fail.
///
/// Injecting `read` turns the rule into a pure function of its inputs, so it
/// is reachable from a test the same way `sum_readings`'s rule already is. The
/// short-circuit itself is `Result`'s `FromIterator` impl doing its job:
/// `.collect::<io::Result<Vec<_>>>()` stops pulling from `pids.map(read)` at
/// the first `Err`, so the abort is a property of `collect` rather than of
/// hand-written control flow a future editor could quietly reorder.
fn collect_readings(
    pids: impl Iterator<Item = u32>,
    read: impl Fn(u32) -> std::io::Result<Option<u64>>,
) -> std::io::Result<Vec<Option<u64>>> {
    pids.map(read).collect()
}

/// Resident memory of everything the supervisor is running.
///
/// `try_state` rather than `State<'_, Arc<Supervisor>>`: the supervisor is only
/// managed when the setup bootstrap succeeded, and an unmanaged one must give a
/// clean error the strip renders as "—" rather than Tauri's raw state panic
/// message. Same precedent as the quit path.
///
/// The abort-on-`Err` rule (spec §4.1) lives in `collect_readings`, not here —
/// see its doc comment for why the read loop was extracted instead of kept
/// inline.
#[tauri::command]
#[specta::specta]
pub async fn services_memory(app: tauri::AppHandle) -> Result<ServicesMemoryDto, IpcError> {
    use tauri::Manager;
    let Some(sup) = app.try_state::<Arc<Supervisor>>() else {
        return Err(IpcError::Proc {
            message: "the supervisor is not running".to_string(),
        });
    };
    let pids = sup.snapshot().into_iter().filter_map(|status| status.pid);
    let readings = collect_readings(pids, openvhost_proc::platform::process_rss).map_err(|e| {
        IpcError::Proc {
            message: e.to_string(),
        }
    })?;
    let (bytes, process_count) = sum_readings(readings.into_iter());
    Ok(ServicesMemoryDto {
        bytes,
        process_count,
    })
}

/// Total size of the OpenVHost home.
///
/// The walk runs on `spawn_blocking`, not inline: it measured 40 ms over 6,470
/// files (spec §3.2), which is long enough to matter on a runtime thread, and
/// the figure is not urgent.
#[tauri::command]
#[specta::specta]
pub async fn home_disk_usage() -> Result<HomeUsageDto, IpcError> {
    let bytes = tauri::async_runtime::spawn_blocking(openvhost_core::home_disk_usage)
        .await
        .map_err(|e| IpcError::Core {
            message: format!("the disk-usage task failed to run: {e}"),
        })?
        .map_err(IpcError::from)?;
    Ok(HomeUsageDto { bytes })
}

/// A site as it crosses IPC. `Site`'s fields are opaque validated newtypes
/// (deliberately not serializable), so the wire form is plain strings.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SiteDto {
    pub id: String,
    pub name: String,
    pub domain: String,
    pub docroot: String,
    pub web_server: String,
    pub php_version: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Site> for SiteDto {
    fn from(s: Site) -> Self {
        SiteDto {
            id: s.id.as_str().to_string(),
            name: s.name.as_str().to_string(),
            domain: s.domain.as_str().to_string(),
            docroot: s.docroot.as_str().to_string(),
            web_server: s.web_server.as_str().to_string(),
            php_version: s.php_version.as_str().to_string(),
            enabled: s.enabled,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

/// Client-supplied site fields. Note there is no `id`/`created_at`/
/// `updated_at`: those are server-owned and never taken from the client.
#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SiteInput {
    pub name: String,
    pub domain: String,
    pub docroot: String,
    pub web_server: String,
    pub php_version: String,
    pub enabled: bool,
}

impl TryFrom<SiteInput> for NewSite {
    type Error = IpcError;

    /// THE IPC INGRESS GUARD: every field goes through its domain newtype's
    /// `parse`, so no unvalidated string can reach `state.db`. `?` maps
    /// `CoreError::Validation` to `IpcError::Validation { field, .. }`.
    fn try_from(i: SiteInput) -> Result<NewSite, IpcError> {
        Ok(NewSite {
            name: SiteName::parse(&i.name)?,
            domain: Domain::parse(&i.domain)?,
            docroot: Docroot::parse(&i.docroot)?,
            web_server: WebServer::parse(&i.web_server)?,
            php_version: PhpVersion::parse(&i.php_version)?,
            enabled: i.enabled,
        })
    }
}

/// Scaffold outcome crossing IPC. Mirrors `openvhost_core::site::scaffold`'s
/// `ScaffoldOutcome` — the core crate stays serde/specta-free by design, so
/// this DTO is the serialization layer, the same seam as `Site` → `SiteDto`.
/// `kind` is the discriminator the UI switches on exhaustively.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScaffoldOutcomeDto {
    Created,
    KeptExisting {
        existing: String,
    },
    Failed {
        step: ScaffoldStepDto,
        reason: String,
    },
}

impl From<ScaffoldOutcome> for ScaffoldOutcomeDto {
    fn from(o: ScaffoldOutcome) -> Self {
        match o {
            ScaffoldOutcome::Created => ScaffoldOutcomeDto::Created,
            ScaffoldOutcome::KeptExisting { existing } => {
                ScaffoldOutcomeDto::KeptExisting { existing }
            }
            ScaffoldOutcome::Failed { step, reason } => ScaffoldOutcomeDto::Failed {
                step: step.into(),
                reason,
            },
        }
    }
}

/// Which step of a [`ScaffoldOutcomeDto::Failed`] failed. Mirrors
/// `openvhost_core::site::scaffold::ScaffoldStep`; never parse English out of
/// `reason` in the UI — this is the stable discriminator for that.
#[derive(Debug, Clone, Copy, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ScaffoldStepDto {
    CreateDir,
    Inspect,
    WritePlaceholder,
}

impl From<ScaffoldStep> for ScaffoldStepDto {
    fn from(s: ScaffoldStep) -> Self {
        match s {
            ScaffoldStep::CreateDir => ScaffoldStepDto::CreateDir,
            ScaffoldStep::Inspect => ScaffoldStepDto::Inspect,
            ScaffoldStep::WritePlaceholder => ScaffoldStepDto::WritePlaceholder,
        }
    }
}

/// The result of `create_site`: the persisted site, plus what scaffolding did
/// — if it was requested at all.
///
/// `scaffold: None` means "not requested" (the caller passed
/// `create_folder: false`) — it is NOT a fourth outcome alongside
/// `Created`/`KeptExisting`/`Failed`, and the UI must not conflate the two.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateSiteResult {
    pub site: SiteDto,
    pub scaffold: Option<ScaffoldOutcomeDto>,
}

// These commands build a repository per call from the managed store (cheap —
// cloning a pool handle) rather than managing a second type. Every site lives
// in state.db, so with no store there is no honest answer but a refusal:
// `DbHandle::require` returns one that names why (design D2, REFUSE). What it
// replaced was Tauri refusing the whole command with "you must call
// `.manage()`", rendered verbatim to the user.
#[tauri::command]
#[specta::specta]
pub async fn list_sites(db: tauri::State<'_, DbHandle>) -> Result<Vec<SiteDto>, IpcError> {
    let db = db.require()?;
    let repo = SqliteSiteRepository::new(db);
    Ok(repo.list().await?.into_iter().map(SiteDto::from).collect())
}

/// Create a site, optionally scaffolding its docroot folder and a starter
/// page (spec: docs/superpowers/specs/2026-07-29-p1-site-scaffold-design.md,
/// D2).
///
/// Order is load-bearing: ingress guard → join (if `create_folder`) → DB
/// insert → THEN scaffold. Scaffolding runs only once `repo.create` has
/// actually returned `Ok` — a UNIQUE violation on `name`/`domain` returns
/// `Err` from the `?` below and this function stops right there, before the
/// `scaffold` line is ever reached, so a rejected create can never leave a
/// folder behind.
#[tauri::command]
#[specta::specta]
pub async fn create_site(
    db: tauri::State<'_, DbHandle>,
    input: SiteInput,
    create_folder: bool,
) -> Result<CreateSiteResult, IpcError> {
    // FIRST, ahead of the ingress guard and the join: the order comment above
    // is about not leaving a folder behind for a row that was never written,
    // and "there is nowhere to write the row at all" is that case at its most
    // extreme.
    let db = db.require()?;
    let mut new: NewSite = input.try_into()?;
    if create_folder {
        // Re-parse of the JOINED path: over-length or bad-charset joins fail
        // here as a docroot field error, before any row or folder exists.
        new.docroot = scaffold_path(&new.docroot, &new.name)?;
    }
    let repo = SqliteSiteRepository::new(db);
    let site = repo.create(new).await?;
    let scaffold = create_folder.then(|| scaffold(&site.docroot, &site.name, &site.domain));
    Ok(CreateSiteResult {
        site: SiteDto::from(site),
        scaffold: scaffold.map(Into::into),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn update_site(
    db: tauri::State<'_, DbHandle>,
    id: String,
    input: SiteInput,
) -> Result<SiteDto, IpcError> {
    let db = db.require()?;
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db);
    let existing = repo.get(&site_id).await?.ok_or_else(|| IpcError::Core {
        message: format!("site {id} not found"),
    })?;
    let new: NewSite = input.try_into()?;
    // `id` and `created_at` come from the stored row, never the client.
    // `updated_at` is bumped by the repository.
    let updated = Site {
        id: existing.id,
        name: new.name,
        domain: new.domain,
        docroot: new.docroot,
        web_server: new.web_server,
        php_version: new.php_version,
        enabled: new.enabled,
        created_at: existing.created_at,
        updated_at: existing.updated_at,
    };
    Ok(SiteDto::from(repo.update(&updated).await?))
}

/// Open a site in the user's default browser.
///
/// **The URL is built HERE, in Rust, and the webview never supplies one.** The
/// obvious alternative — granting the frontend `opener:allow-open-url` and letting
/// it call `openUrl(...)` — would hand the renderer a general "open any URL"
/// primitive. This command narrows that to "open the site with this id": the only
/// thing a caller can influence is which stored row is used, and the scheme is
/// fixed. No capability grant is added to `capabilities/default.json` at all,
/// because the ACL gates the JS-to-plugin path and this calls the plugin's Rust
/// API instead.
///
/// The domain also already passed `Domain`'s charset guard on its way into
/// state.db, so it cannot carry a scheme, a path, whitespace or a quote. That
/// guard is a charset check and NOT a policy check, though — it does not decide
/// which hosts are ours — so this deliberately hardcodes `http://` rather than
/// letting a stored value choose the scheme.
#[tauri::command]
#[specta::specta]
pub async fn open_site(
    db: tauri::State<'_, DbHandle>,
    app: tauri::AppHandle,
    id: String,
) -> Result<(), IpcError> {
    use tauri_plugin_opener::OpenerExt;
    let url = open_site_url(&db, &id).await?;
    // `None` for `with`: let the OS pick the default handler rather than naming a
    // browser we would then have to keep a list of.
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| IpcError::Core {
            message: e.to_string(),
        })
}

/// Everything [`open_site`] decides before it opens anything: the store, the
/// id, the stored row, and the URL built from it.
///
/// A named function rather than four lines inlined above, for the same reason
/// [`initialize_mysql_gate`] exists — `open_site` takes an `AppHandle<Wry>`,
/// which `tauri::test::mock_builder` cannot produce, so its body is
/// unreachable from a test and a refusal written there is a decision **no test
/// can see**. Every way this can say no now lives where a test can call it.
async fn open_site_url(db: &DbHandle, id: &str) -> Result<String, IpcError> {
    let db = db.require()?;
    let site_id = SiteId::parse(id)?;
    let repo = SqliteSiteRepository::new(db);
    let site = repo.get(&site_id).await?.ok_or_else(|| IpcError::Core {
        message: format!("site {id} not found"),
    })?;
    Ok(site_url(site.domain.as_str()))
}

/// Open Homebrew's install page in the user's browser.
///
/// Zero parameters, and the URL is a Rust literal: the webview cannot choose
/// where this goes. Same reasoning as `open_site` above — granting the
/// renderer a general "open any URL" primitive is the thing being avoided,
/// not the act of opening a URL. No capability grant is added to
/// `capabilities/default.json` for the same reason `open_site` needs none:
/// this calls the plugin's Rust API, not its JS path, so the ACL (which
/// gates the JS-to-plugin path) has nothing to do with it.
///
/// This exists because a plain `<a target="_blank">` is inert in this
/// webview: Tauri only handles a new-window request when the app registers
/// `.on_new_window(...)` on the webview builder, which this app does not, so
/// WebKit's `WKUIDelegate` is told not to create a window and the click
/// silently does nothing — no tab, no error, no console warning.
#[tauri::command]
#[specta::specta]
pub async fn open_homebrew_site(app: tauri::AppHandle) -> Result<(), IpcError> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url("https://brew.sh", None::<&str>)
        .map_err(|e| IpcError::Core {
            message: e.to_string(),
        })
}

/// `http://<domain>:<LISTEN_PORT>`. Extracted so the one thing worth pinning —
/// that the scheme is fixed and prepended, never taken from the stored value —
/// is testable without a live `AppHandle` and a real database.
///
/// The port is `site::apply::LISTEN_PORT` (8080): every applied site listens
/// there, not on 80 (spec — port 80 needs the privileged helper, Phase 3), so
/// a scheme-only URL sends the browser to a port nothing is bound to.
fn site_url(domain: &str) -> String {
    format!(
        "http://{domain}:{}",
        openvhost_core::site::apply::LISTEN_PORT
    )
}

#[tauri::command]
#[specta::specta]
pub async fn delete_site(db: tauri::State<'_, DbHandle>, id: String) -> Result<bool, IpcError> {
    let db = db.require()?;
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db);
    Ok(repo.delete(&site_id).await?)
}

impl From<ApplyError> for IpcError {
    fn from(e: ApplyError) -> Self {
        // Every variant's Display already names the site, the versions or the
        // stranded paths, so one arm is enough and none of that detail is lost.
        IpcError::Core {
            message: e.to_string(),
        }
    }
}

fn change_kind_str(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Added => "added",
        ChangeKind::Modified => "modified",
        ChangeKind::Removed => "removed",
    }
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeDto {
    pub path: String,
    /// "added" | "modified" | "removed"
    pub kind: String,
    pub diff: String,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPlanDto {
    pub changes: Vec<FileChangeDto>,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServiceProblemDto {
    pub id: String,
    /// A sentence the UI can show as-is, telling the user what happened and
    /// what is left for them to do.
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcomeDto {
    /// `u32`, not `usize`: specta rejects pointer-sized ints (see lib.rs).
    pub applied: u32,
    /// Stopped and successfully started again on the new config.
    pub restarted: Vec<String>,
    /// Services whose config changed but which were not running, so the new
    /// config takes effect the next time they start.
    pub not_started: Vec<String>,
    /// Were running and could NOT be brought back — the user has to act.
    /// Never empty-and-ignored: a service this pipeline stopped and failed to
    /// restart is the one outcome the UI must not present as success.
    pub needs_attention: Vec<ServiceProblemDto>,
}

/// Serializes `apply_config` end to end (A2): plan -> commit -> validate ->
/// restart all run while this lock is held, so two overlapping Apply calls
/// cannot interleave their commit/rollback (one rollback restoring its own
/// snapshot over the other's writes) or their stop/start of the same
/// services. `Default` gives `lib.rs` a plain `ApplyLock::default()` to
/// manage, matching `quit::UiReady`'s pattern next to it.
///
/// `plan_config_apply` deliberately does NOT take this lock — it is read-only
/// and is called after every site mutation to drive the pending-changes
/// banner, so serializing it against Apply would make that banner block on
/// an in-flight apply for no safety benefit.
#[derive(Default)]
pub struct ApplyLock(pub(crate) tokio::sync::Mutex<()>);

/// Build the apply input from state.db plus the runtimes probed at startup.
///
/// The nginx settings and the default-PHP preference are read here, alongside
/// the sites, so BOTH entry points
/// to the pipeline (`plan_config_apply` for the pending-changes banner and
/// `apply_config` for the apply itself) see the same stored values. Reading them
/// per call rather than caching is deliberate: `apply_config` recomputes its plan
/// from state.db under the apply lock, and a cached copy would let a settings
/// save land between the diff the user saw and the config that got written.
///
/// A consequence to expect rather than treat as a bug: saving a setting makes
/// the Sites page's pending-changes banner light up too. It is the same pending
/// change — one config set, one plan.
async fn apply_input(
    db: &Db,
    runtimes: &Option<InstalledRuntimes>,
    paths: &Option<StackPaths>,
) -> Result<ApplyInput, IpcError> {
    let (Some(runtimes), Some(paths)) = (runtimes.as_ref(), paths.as_ref()) else {
        return Err(IpcError::Core {
            message: "no web server stack is configured for this platform".into(),
        });
    };
    let repo = SqliteSiteRepository::new(db);
    let settings = SqliteWebServerSettings::new(db);
    let php_settings = SqlitePhpSettings::new(db);
    Ok(ApplyInput {
        home: paths.home.clone(),
        sites: repo.list().await?,
        runtimes: runtimes.clone(),
        // Absent row => documented defaults, and nothing is written. See
        // `WebServerSettingsRepository::get`.
        settings: settings.get().await?,
        // Likewise absent row => `None` => nobody has chosen a default PHP, so
        // the catch-all keeps serving the first discovered runtime exactly as
        // it did before this field existed. The PREFERENCE is what travels;
        // `render_set` resolves it against `runtimes` itself.
        default_php: php_settings.get().await?.default_major,
    })
}

/// What Apply would change across the WHOLE generated config — the sites and
/// the editable nginx settings both feed `apply_input`, so this is one plan
/// over one config set, not a site-only view. (That is the rename: the old
/// `plan_site_apply` would have told the reader this covered only sites.)
///
/// Read-only and process-free — the pending-changes banner calls this after
/// every site mutation and after every settings save.
#[tauri::command]
#[specta::specta]
pub async fn plan_config_apply(
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<ApplyPlanDto, IpcError> {
    // Same fail-closed refusal `apply_config` makes, for the same reason: this
    // is the SAME plan over the SAME config set, and with no store `apply_input`
    // would build it from an empty site list — a plan whose changes are "remove
    // every vhost". A pending-changes banner is not a safe place to show that.
    let db = db.require()?;
    // Clone out of the guard and drop it before the `.await` below: holding a
    // `std::sync::RwLockReadGuard` across an await point makes this command's
    // future non-`Send`, which fails to compile.
    let runtimes = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .clone();
    let input = apply_input(db, &runtimes, paths.inner()).await?;
    let p = openvhost_core::plan(&input)?;
    Ok(ApplyPlanDto {
        changes: p
            .changes
            .into_iter()
            .map(|c| FileChangeDto {
                path: c.path.display().to_string(),
                kind: change_kind_str(c.kind).to_string(),
                diff: c.diff,
            })
            .collect(),
    })
}

/// Write the generated config — sites AND the editable nginx settings — then
/// restart whichever affected services are running.
///
/// The restart is the app's job, not core's: `openvhost-core` has no supervisor
/// and must stay usable from the CLI.
#[tauri::command]
#[specta::specta]
pub async fn apply_config(
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, ApplyLock>,
) -> Result<ApplyOutcomeDto, IpcError> {
    // FAIL CLOSED, and first — before the apply lock is even taken (design D2).
    //
    // This is the one REFUSE that is not merely about honesty. `apply_input`
    // reads the sites FROM state.db; with no store, a degraded read would hand
    // `plan` an EMPTY site list, which renders a valid config containing no
    // vhosts at all — so the commit below would DELETE every vhost the user
    // has, pass `nginx -t` (an empty config is valid), and never roll back.
    // The refusal must therefore happen where no code path can reach the
    // render, not inside `apply_input` where an `Ok(empty)` is still expressible.
    let db = db.require()?;

    // A2: held across the whole plan -> commit -> validate -> restart
    // sequence below. The plan is recomputed HERE, from state.db, by
    // design — the frontend never supplies a plan for this command to run,
    // so anything that changed state.db between the dialog rendering and the
    // click is included on purpose. The lock therefore bounds the window
    // between "user saw a diff" and "that diff is what applied", rather than
    // eliminating it; a full plan-digest re-check is a larger design change
    // and out of scope here.
    let _apply_guard = lock.inner().0.lock().await;

    // Clone out of the guard and drop it before the `.await` below: holding a
    // `std::sync::RwLockReadGuard` across an await point makes this command's
    // future non-`Send`, which fails to compile.
    let runtimes = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .clone();

    let input = apply_input(db, &runtimes, paths.inner()).await?;
    let Some(stack) = paths.inner().as_ref() else {
        return Err(IpcError::Core {
            message: "no web server stack is configured for this platform".into(),
        });
    };
    let p = openvhost_core::plan(&input)?;

    // A3: nothing to write means nothing to restart. Without this, Apply
    // fell through to the restart block unconditionally, so an empty-plan
    // click (or a double-click firing the command twice) still stopped and
    // started nginx/php-fpm — an unbounded stop/start primitive against the
    // user's own stack, and a needless connection drop for every site it
    // fronts.
    if p.changes.is_empty() {
        return Ok(ApplyOutcomeDto {
            applied: 0,
            restarted: Vec::new(),
            not_started: Vec::new(),
            needs_attention: Vec::new(),
        });
    }

    // Design D3: `nginx_bin` is `None` when discovery found neither a
    // packaged nor a Homebrew nginx. An honest refusal here, rather than
    // handing `NginxValidator` a path to a binary that does not exist —
    // Apply always validates through nginx, so there is nothing this command
    // can do without one.
    let Some(nginx_bin) = stack.nginx_bin.clone() else {
        return Err(IpcError::Core {
            message: "no nginx binary was found on this machine".into(),
        });
    };
    let validator = openvhost_core::NginxValidator {
        bin: nginx_bin,
        err_log: openvhost_core::LogPaths::new(&stack.home).nginx_error(),
        // `-p`'s target, never `stack.home` itself — see
        // `NginxValidator::home`'s own doc comment (4B fix-wave, item 1).
        home: openvhost_core::nginx_prefix_dir(&stack.home),
    };
    let outcome = openvhost_core::apply(&p, &validator).await?;

    // php-fpm before nginx: nginx connects to the pool socket, so the pool has
    // to be listening first.
    let mut ids: Vec<String> = input
        .runtimes
        .php
        .iter()
        .map(|r| format!("php-fpm-{}", r.major))
        .collect();
    ids.push("nginx".to_string());

    // Only restart what is actually running. A stopped service keeps its state;
    // the new config takes effect when the user starts it.
    let snapshot = sup.snapshot();
    let running: Vec<String> = ids
        .iter()
        .filter(|id| {
            snapshot
                .iter()
                .any(|s| s.id == **id && matches!(s.state, ServiceState::Running))
        })
        .cloned()
        .collect();
    let not_started: Vec<String> = ids
        .iter()
        .filter(|id| !running.contains(id))
        .cloned()
        .collect();

    // Wait for a real Stopped rather than assuming `stop` took effect — the same
    // reason quit.rs polls instead of firing and hoping.
    let for_pending = Arc::clone(sup.inner());
    let watched = running.clone();
    let for_stop = Arc::clone(sup.inner());
    let stragglers = crate::quit::stop_all_with(
        move || {
            for_pending
                .snapshot()
                .into_iter()
                .filter(|s| watched.contains(&s.id))
                .filter(|s| !matches!(s.state, ServiceState::Stopped | ServiceState::Failed { .. }))
                .map(|s| s.id)
                .collect()
        },
        move |id| {
            let _ = for_stop.stop(id);
        },
        std::time::Duration::from_secs(10),
        std::time::Duration::from_millis(50),
    )
    .await;

    // The stop result — not just "we asked it to stop" — decides what happens
    // next. A straggler that is still shutting down gets a no-op `start` (the
    // supervisor treats Starting/Running as already-in-progress) and then
    // finishes stopping moments later, on its own schedule: exactly how a
    // green Apply leaves a site down. So a straggler is never started; it is
    // reported instead.
    let (restarted, needs_attention) = restart_outcome(&running, &stragglers, |id| {
        sup.start(id).map_err(|e| e.to_string())
    });

    Ok(ApplyOutcomeDto {
        // `u32`, not `usize`: specta rejects pointer-sized ints.
        applied: u32::try_from(outcome.applied).unwrap_or(u32::MAX),
        restarted,
        not_started,
        needs_attention,
    })
}

/// Decide, for each service that was running, what the restart achieved.
/// Split out from `apply_config` so the straggler logic is testable without
/// spawning anything.
///
/// Deliberately does not use `?`/early-return on a failed `start`: one
/// service's failure must not hide the fate of the others, so every id in
/// `running` is visited regardless of what happened to the ones before it.
fn restart_outcome(
    running: &[String],
    stragglers: &[String],
    start: impl Fn(&str) -> Result<(), String>,
) -> (Vec<String>, Vec<ServiceProblemDto>) {
    let mut restarted = Vec::new();
    let mut needs_attention = Vec::new();
    for id in running {
        if stragglers.contains(id) {
            needs_attention.push(ServiceProblemDto {
                id: id.clone(),
                reason: "did not stop within 10s, so it was left alone — stop it and start it \
                         again to pick up the new config"
                    .to_string(),
            });
            continue;
        }
        match start(id) {
            Ok(()) => restarted.push(id.clone()),
            Err(e) => needs_attention.push(ServiceProblemDto {
                id: id.clone(),
                reason: format!("stopped, but could not be started again: {e}"),
            }),
        }
    }
    (restarted, needs_attention)
}

/// The web servers OpenVHost knows about. A CLOSED list: the client sends only
/// this id — never a path, filename or argument — and every path used by the
/// commands below is derived server-side from managed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebServerBrand {
    Nginx,
    Apache,
}

/// A validator invocation, resolved as ONE unit from the parsed brand: which
/// binary, which config file, and which flags belong together.
///
/// Why a type instead of two lookups at the call site. `validate_web_server_config`
/// used to derive the config from the parsed brand but pass `&p.nginx_bin`
/// **unconditionally**, so the binary and the validator were not brand-keyed at
/// all: a future editor who flips Apache to supported and adds an `apache_conf`
/// arm would get `nginx -t -c apache.conf`, with every test still green. Adding a
/// brand now cannot compile without adding a variant HERE and an arm at the single
/// `match` in `validate_web_server_config`.
///
/// **Not a security fix, and it must not be described as one.** A mismatched
/// binary/config pair yields a parse error and a red row: nothing is written,
/// nothing is disclosed, no privilege is crossed. It is a correctness footgun,
/// closed structurally because that costs a couple of dozen lines now instead of a
/// prose guardrail the next editor has to remember — the same move this repo
/// already made at `89471df`.
enum ValidationTarget<'a> {
    /// `<bin> -e <err_log> -p <prefix> -t -c <conf>`. Only ever constructed
    /// for `Nginx`. `-p` (nginx discovery design D4) rides alongside `-e` for
    /// a related but stronger reason (4B fix-wave, item 1): `home` is
    /// carrying `state.db`, and a relative path in a user-authored custom
    /// nginx file resolved under `-p`'s target would serve it verbatim, so
    /// `home` FIELD is [`openvhost_core::nginx_prefix_dir`]'s answer — a
    /// dedicated, empty, provisioned directory — never `paths.home` itself.
    /// `PathBuf`, not `&'a Path`, because unlike `conf`/`bin` it is not
    /// borrowed out of managed state: it is derived fresh, the same way
    /// `err_log` already is.
    NginxT {
        bin: &'a Path,
        conf: &'a Path,
        err_log: PathBuf,
        home: PathBuf,
    },
}

impl WebServerBrand {
    /// Exact match only. An unknown id is a validation error rather than a
    /// fallback: silently treating a typo as nginx would operate on a server
    /// the caller did not name.
    fn parse(s: &str) -> Result<Self, IpcError> {
        match s {
            "nginx" => Ok(Self::Nginx),
            "apache" => Ok(Self::Apache),
            _ => Err(IpcError::Validation {
                field: "id".into(),
                message: format!("unknown web server {s:?}"),
            }),
        }
    }

    /// The brand's name as the product spells it. Exists so a message can name the
    /// brand the caller actually asked for instead of hardcoding one.
    fn display_name(self) -> &'static str {
        match self {
            Self::Nginx => "nginx",
            Self::Apache => "Apache",
        }
    }

    /// Whether OpenVHost can actually operate this brand end to end: adapter,
    /// template, real paths in `StackPaths` and a validator that speaks its
    /// config. Exhaustive rather than `matches!`, so a third variant has to be
    /// classified here instead of silently defaulting to unsupported.
    fn supported(self) -> bool {
        match self {
            Self::Nginx => true,
            // Flipping this to `true` is NOT on its own enough to support Apache.
            // It also needs its own paths on `StackPaths`, its own arm in
            // `live_config_path` below, and its own arm in `validation_target` —
            // which is where a brand gets a validator that speaks its config
            // rather than being handed `nginx -t`. Until all of those exist, those
            // two accessors are what stop a supported-but-pathless brand from
            // being handed nginx's config, or nginx's binary, under an Apache
            // heading.
            Self::Apache => false,
        }
    }

    /// Reject an unsupported brand BEFORE deriving any path, so the failure is
    /// "OpenVHost cannot do this" rather than an empty read that reads as "this
    /// server has no configuration".
    fn require_supported(self) -> Result<(), IpcError> {
        if self.supported() {
            return Ok(());
        }
        Err(IpcError::Validation {
            field: "id".into(),
            // Keyed off `self`, NOT hardcoded. `supported()`'s exhaustive match
            // forces a new variant to be classified, but nothing forces this
            // sentence to be updated — so a hardcoded "Apache" here would answer a
            // request for Caddy with "OpenVHost cannot serve Apache sites yet", a
            // wrong statement on the one surface whose entire job is telling the
            // truth about the machine.
            message: format!(
                "OpenVHost cannot serve {} sites yet — it only generates nginx config",
                self.display_name()
            ),
        })
    }

    /// The live config file for THIS brand — and the only way the commands below
    /// can obtain a config path at all.
    ///
    /// The brand KEYS the path rather than merely gating it: the `match` is
    /// exhaustive, so a future brand is a compile error here instead of silently
    /// inheriting nginx's. `require_supported` runs first and inside, so a caller
    /// cannot reach a path before the gate no matter how the statements are
    /// ordered, and the user-facing message is unchanged.
    ///
    /// `self` is `Copy` and taken by value, so the elided output lifetime binds to
    /// `paths` — the returned path always borrows from managed state.
    fn live_config_path(self, paths: &StackPaths) -> Result<&Path, IpcError> {
        self.require_supported()?;
        match self {
            Self::Nginx => Ok(&paths.nginx_conf),
            // Unreachable while `Nginx` is the only `supported()` brand: the gate
            // above already returned. Deliberately an error and not
            // `unreachable!()` — marking Apache supported without giving it a
            // path here must degrade to an honest failure, never a panic. Adding
            // a path is also not sufficient by itself; see `supported()`.
            Self::Apache => Err(IpcError::Core {
                message: "no live configuration path is known for Apache".into(),
            }),
        }
    }

    /// The WHOLE validator invocation for this brand — binary, config and flags
    /// together. See [`ValidationTarget`] for why this is a type rather than a
    /// second path lookup beside an unconditional `&p.nginx_bin`.
    ///
    /// `live_config_path` above is deliberately left as it was, for
    /// `read_web_server_config`: that command needs a config path and nothing else,
    /// and it is already correctly brand-keyed.
    ///
    /// `self` is `Copy` and taken by value, so the elided output lifetime binds to
    /// `paths` — the borrowed halves always come from managed state.
    fn validation_target(self, paths: &StackPaths) -> Result<ValidationTarget<'_>, IpcError> {
        self.require_supported()?;
        match self {
            // Design D3: an absent `nginx_bin` is refused here, honestly,
            // rather than borrowed as a path to a binary that does not exist.
            Self::Nginx => {
                let Some(bin) = paths.nginx_bin.as_deref() else {
                    return Err(IpcError::Core {
                        message: "no nginx binary was found on this machine".into(),
                    });
                };
                Ok(ValidationTarget::NginxT {
                    bin,
                    conf: &paths.nginx_conf,
                    err_log: openvhost_core::LogPaths::new(&paths.home).nginx_error(),
                    home: openvhost_core::nginx_prefix_dir(&paths.home),
                })
            }
            // Unreachable while `Nginx` is the only `supported()` brand: the gate
            // above already returned. Deliberately an error and not
            // `unreachable!()`, for the same reason as `live_config_path` — marking
            // a brand supported without giving it a validator here must degrade to
            // an honest failure, never a panic.
            Self::Apache => Err(IpcError::Core {
                message: "no validator invocation is known for Apache".into(),
            }),
        }
    }
}

/// Where a listed nginx runtime's binary came from — the wire copy of
/// `openvhost_core::nginx::NginxRuntimeSource` (nginx source design D1).
///
/// Transcribed from `MysqlRuntimeSourceDto` (`mysql_pkg.rs`) rather than
/// reinvented: the two ask the identical question — "which install put these
/// bytes here" — and nothing about nginx's answer needs a different shape.
/// `NginxRuntimeSource::as_str()` stays the one machine-facing spelling for
/// each source; `the_wire_tag_is_nginx_runtime_source_as_str` below pins this
/// type's serialized `kind` to it for every variant, so the two cannot drift
/// into different words for the same fact.
///
/// `Homebrew` carries **no version, on purpose** (design D2): nginx has no
/// `--version` flag, only `-v`, and finding out means executing the
/// binary — the exact cost design D2 exists to remove from the packaged
/// path. Reporting the packaged series as though it were Homebrew's exact
/// patch release would be a lie no caller could detect.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NginxRuntimeSourceDto {
    Packaged { version: String },
    Homebrew,
}

impl From<&openvhost_core::nginx::NginxRuntimeSource> for NginxRuntimeSourceDto {
    fn from(s: &openvhost_core::nginx::NginxRuntimeSource) -> Self {
        use openvhost_core::nginx::NginxRuntimeSource as S;
        match s {
            S::Packaged { version } => Self::Packaged {
                version: version.clone(),
            },
            S::Homebrew => Self::Homebrew,
        }
    }
}

/// One row on the Web Server page.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WebServerDto {
    pub id: String,
    pub display_name: String,
    pub supported: bool,
    /// Correlates with the shared services store for live status; `None` when
    /// the brand is not a supervised service.
    pub service_id: Option<String>,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    /// Where `binary_path` came from (nginx source design D1) — `None` both
    /// when no nginx was found on this machine AND for the Apache row, which
    /// has no runtime at all. The row's own `supported` flag already tells
    /// those two apart for any consumer that cares; this field carries
    /// provenance only, and adds no second discriminator for "is there a
    /// server here".
    pub source: Option<NginxRuntimeSourceDto>,
    pub supports_hot_reload: bool,
    pub config_path: Option<String>,
    /// Whether a file exists at `config_path` right now — a TRI-STATE, because
    /// a filesystem stat has three honest outcomes, not two.
    ///
    /// `Some(true)`: confirmed present. `Some(false)`: confirmed absent — and
    /// on a fresh install this is the common case, because `provision_home`
    /// seeds directories and the welcome page but writes no config (pinned by
    /// `provisioning_no_longer_writes_any_config`). `None`: the stat could not
    /// be performed at all (permission denied on a parent directory, a
    /// dangling symlink from an interrupted atomic write, ...) — this is
    /// EXISTENCE UNKNOWN, and it must never collapse into `Some(false)`.
    /// Doing so would tell the user "no config generated yet — apply your
    /// changes first" when the real cause has nothing to do with Apply and
    /// re-running it cannot fix it.
    ///
    /// EXISTENCE, NOT VALIDITY, in every non-`None` case. nginx is registered
    /// to spawn with `-c <config_path>`, so a confirmed-missing file means
    /// Start exits immediately, and the page disables Start on `Some(false)`
    /// and says why, rather than letting the user find out by pressing it. On
    /// `None` the page instead leaves Start enabled with no claim about the
    /// cause — see `startStopFor` — because nginx's own stderr on a genuine
    /// failure names the real problem, which this DTO cannot. A config that
    /// exists can still be refused by nginx; that case is the row's stderr
    /// block, not this flag.
    pub config_exists: Option<bool>,
}

impl WebServerDto {
    /// Listed so the UI can say plainly that it is not available, rather than
    /// hiding it and leaving the site editor's Apache option unexplained.
    fn apache() -> Self {
        Self {
            id: "apache".into(),
            display_name: "Apache".into(),
            supported: false,
            service_id: None,
            binary_path: None,
            version: None,
            // No runtime at all, so no provenance to report — see this field's
            // own doc comment for why `None` here is not confusable with "no
            // nginx was found": `supported: false` above is what tells the two
            // apart.
            source: None,
            supports_hot_reload: false,
            config_path: None,
            // `Some(false)`, not `None`. `None` means "a stat was attempted and
            // could not be performed" — but no stat is ever attempted here at
            // all, because there is no `config_path` to stat. "a file exists at
            // config_path" is false for every value of "a file" when
            // `config_path` is `None`, so this is a confirmed absence we can
            // state outright, not an unresolved one. It is also moot either way:
            // `WebServerRow.svelte` gates the whole service control on
            // `server.serviceId === null` before `config_exists` is ever
            // consulted, and Apache's `service_id` is `None` too.
            config_exists: Some(false),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReportDto {
    pub ok: bool,
    pub stderr: String,
}

/// The managed stack paths, or a rendered error when nothing was managed.
///
/// The managed type is `Option<StackPaths>`, not `StackPaths`: tauri implements
/// `CommandArg` only for `State<'r, T>`, so a command cannot take an
/// optionally-managed state. Task 2 therefore manages the `Option` itself.
///
/// `None` means **no web server stack was managed**. On every target except macOS
/// that is simply "there is no stack builder yet": the OpenVHost home resolved
/// perfectly well. macOS can reach it too, though only barely — `stack::macos_stack`
/// calls `resolve_home()` a SECOND time and returns `paths: None` when that fails.
/// Practically unreachable, since `resolve_home` is pure over two inputs, does no
/// IO, and nothing in-process mutates them, so a success at startup followed by a
/// failure moments later is not a state to design for. It is still why the message
/// stays platform-agnostic and names neither the platform nor home resolution: on
/// macOS this outcome IS a home problem, and blaming the platform would send the
/// reader somewhere there is nothing to find.
///
/// This is NOT how a failed startup surfaces. `lib.rs` manages the value inside
/// the `resolve_home()` + `InstanceLock::acquire` success arm, so the two failure
/// paths next to it manage **nothing at all**: `State` extraction itself fails
/// and the user sees Tauri's raw error rather than this text. Of those two, an
/// unresolvable home is the rare one — `InstanceLock::acquire` returning
/// `Ok(None)`, i.e. the app was double-launched, is far more likely.
///
/// Making those two friendly would take one `app.manage` after the match, with
/// every arm yielding an `Option<StackPaths>`. Do NOT approximate it by managing
/// `None` early and the real value later: `Manager::manage` does not overwrite an
/// existing value (its own doc example asserts `assert!(!app.manage(MyInt(1)))`),
/// so the second call is a silent no-op that would pin every user to `None`.
pub(crate) fn stack_paths<'a>(
    paths: &'a tauri::State<'_, Option<StackPaths>>,
) -> Result<&'a StackPaths, IpcError> {
    paths.inner().as_ref().ok_or_else(|| IpcError::Core {
        message: "no web server stack is configured for this platform".into(),
    })
}

/// The page's rows, built from the paths the supervisor actually registered.
///
/// Split out of `list_web_servers` so the listing is testable without Tauri or a
/// live version probe: everything process- or state-dependent arrives as an
/// argument. A test can therefore pin that Apache is LISTED, not merely
/// constructible.
fn web_server_rows(
    p: &StackPaths,
    version: Option<String>,
    config_exists: Option<bool>,
    source: Option<NginxRuntimeSourceDto>,
) -> Vec<WebServerDto> {
    vec![
        WebServerDto {
            id: "nginx".into(),
            display_name: "nginx".into(),
            supported: true,
            service_id: Some("nginx".into()),
            // Design D3: `None` is an honest "no nginx was found", not a
            // path this row should invent.
            binary_path: p.nginx_bin.as_ref().map(|b| b.display().to_string()),
            version,
            source,
            supports_hot_reload: openvhost_conf::NginxAdapter.supports_hot_reload(),
            config_path: Some(p.nginx_conf.display().to_string()),
            config_exists,
        },
        WebServerDto::apache(),
    ]
}

#[tauri::command]
#[specta::specta]
pub async fn list_web_servers(
    paths: tauri::State<'_, Option<StackPaths>>,
    nginx_source: tauri::State<'_, Option<openvhost_core::nginx::NginxRuntimeSource>>,
) -> Result<Vec<WebServerDto>, IpcError> {
    let p = stack_paths(&paths)?;
    // `nginx_bin` and `nginx_source` are two projections of the ONE
    // `Option<NginxRuntime>` that `macos_stack` resolved, and this is the only
    // place both are read together. Nothing structural keeps them in step —
    // "one fact, two managed projections" is the shape `StackPaths.nginx_bin`
    // and `InstalledRuntimes.nginx_bin` already have, so this is a convention
    // here, not a type. That is fine while both are written once at startup
    // and never rescanned; it stops being fine the day nginx gains an install
    // or rescan flow that updates one side. Assert now, in dev and test, so
    // that day announces itself instead of rendering a row that reports a
    // source for a binary it does not have (branch review, 4C, MEDIUM).
    debug_assert_eq!(
        p.nginx_bin.is_some(),
        nginx_source.inner().is_some(),
        "nginx_bin and nginx_source disagree — both come from one discovery at startup"
    );
    let err_log = openvhost_core::LogPaths::new(&p.home).nginx_error();
    // Nginx source design D2: a packaged nginx's version comes for free from
    // the tree discovery already resolved — no process runs to learn it.
    // Probing (which SPAWNS `nginx -v`, so merely opening this page used to
    // start a process for EVERY source) now happens only for `Homebrew`,
    // whose exact patch release genuinely cannot be known any other way.
    // Exhaustive over `NginxRuntimeSource`, no wildcard arm: a third source
    // must be decided about here rather than silently probed or silently
    // trusted.
    //
    // `-p`'s target is `nginx_prefix_dir(&p.home)`, never `p.home` itself
    // (4B fix-wave, item 1) — see that function's own doc comment for the
    // credential-exposure finding this closes.
    let version = match nginx_source.inner() {
        Some(openvhost_core::nginx::NginxRuntimeSource::Packaged { version }) => {
            Some(version.clone())
        }
        Some(openvhost_core::nginx::NginxRuntimeSource::Homebrew) => match p.nginx_bin.as_deref() {
            Some(bin) => {
                openvhost_conf::probe_nginx_version(
                    bin,
                    &err_log,
                    &openvhost_core::nginx_prefix_dir(&p.home),
                )
                .await
            }
            None => None,
        },
        // Design D3: no source, no probe — `None` reaches the row honestly
        // rather than spawning against a path that does not exist.
        None => None,
    };
    let source = nginx_source
        .inner()
        .as_ref()
        .map(NginxRuntimeSourceDto::from);
    // `tokio::fs`, not `Path::exists()`: a sync stat pins a tokio WORKER, and an
    // OPENVHOST_HOME on a stalled network mount would take the supervisor event
    // pump down with it — the same hazard `read_web_server_config` documents
    // below. `.ok()`, not `.unwrap_or(false)`: a stat that ERRORS (permission
    // denied on a parent directory, a dangling symlink from an interrupted
    // atomic write, ...) is not evidence the file is ABSENT either — it is no
    // evidence at all, and `unwrap_or(false)` used to collapse that unknown
    // into the same value as a confirmed absence, which sent the user to
    // re-Apply for a problem Apply cannot fix. `.ok()` keeps the three
    // outcomes distinct: `Some(true)`, `Some(false)`, or `None` for "could not
    // tell" — see `WebServerDto::config_exists`'s doc comment for what each one
    // means to the page.
    let config_exists = tokio::fs::try_exists(&p.nginx_conf).await.ok();
    Ok(web_server_rows(p, version, config_exists, source))
}

#[tauri::command]
#[specta::specta]
pub async fn read_web_server_config(
    paths: tauri::State<'_, Option<StackPaths>>,
    id: String,
) -> Result<String, IpcError> {
    let brand = WebServerBrand::parse(&id)?;
    let p = stack_paths(&paths)?;
    // NOT a general file reader: the path is looked up in managed state BY the
    // parsed brand, so it can neither be aimed at an arbitrary file nor return
    // one brand's config under another's heading. An unsupported brand yields no
    // path at all — `live_config_path` gates before it matches.
    let conf = brand.live_config_path(p)?;
    // Async read: the file is small, but an `OPENVHOST_HOME` on a stalled network
    // mount would hang for as long as the mount does. `tokio::fs` hands the read to
    // the BLOCKING POOL, so the tokio worker is released at the `.await` — the
    // hazard is pool exhaustion, not a wedged worker, and a blocking-pool task
    // cannot be cancelled, so a stalled read holds its thread until the mount
    // answers. That pool is shared with every other blocking operation the app
    // makes. A plain `std::fs::read_to_string` here would be strictly worse: it
    // pins a tokio WORKER, and on a desktop-sized worker pool that stalls the
    // supervisor event pump and every other in-flight command with it. Bounding
    // concurrent calls and capping the response size are recorded follow-ups.
    tokio::fs::read_to_string(conf)
        .await
        .map_err(|e| IpcError::Core {
            message: format!("cannot read {}: {e}", conf.display()),
        })
}

#[tauri::command]
#[specta::specta]
pub async fn validate_web_server_config(
    paths: tauri::State<'_, Option<StackPaths>>,
    id: String,
) -> Result<ValidationReportDto, IpcError> {
    let brand = WebServerBrand::parse(&id)?;
    let p = stack_paths(&paths)?;
    // The BINARY, the config and the flags are resolved together from the parsed
    // brand — never a brand-keyed config beside an unconditional nginx binary. A
    // brand `validation_target` rejects stops here rather than getting a green
    // "valid" badge for a config nginx never examined.
    //
    // Read-only: `validate_live` runs `-t` against the config in place and never
    // calls `materialize`. `-e` keeps nginx's OWN error log inside our home instead
    // of its compiled-in prefix; no config file is written. `nginx -t` is not
    // read-only with respect to the FILESYSTEM, though — it creates whatever
    // `error_log`/`access_log`/`*_temp_path` the config declares. See spec §3.2.
    let report = match brand.validation_target(p)? {
        ValidationTarget::NginxT {
            bin,
            conf,
            err_log,
            home,
        } => openvhost_conf::validate_live(bin, conf, &err_log, &home).await,
    }
    .map_err(|e| IpcError::Core {
        message: e.to_string(),
    })?;
    Ok(ValidationReportDto {
        ok: report.ok,
        stderr: report.stderr,
    })
}

// ---------------------------------------------------------------------------
// Editable nginx settings (Web server page)
// ---------------------------------------------------------------------------

/// The editable nginx settings as they cross IPC.
///
/// `WebServerSettings`' fields are opaque validated newtypes that carry no
/// `specta::Type` (and cannot get one without making `openvhost-conf` an IPC
/// crate), so the wire form is plain primitives. `u32`, never `usize`: specta
/// rejects pointer-sized ints — see the standing note in `lib.rs`.
///
/// `camelCase` on the wire like every other DTO here, so TypeScript sees
/// `fastcgiReadTimeout`/`clientMaxBodySize`. Note that the *validation* field
/// names in [`IpcError::Validation`] are snake_case, because that is what the
/// existing `fieldErrors` seam is keyed by (see `From<ConfError> for IpcError`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WebServerSettingsDto {
    pub worker_connections: u32,
    pub client_max_body_size: String,
    pub keepalive_timeout: u32,
    pub tcp_nodelay: bool,
    pub fastcgi_connect_timeout: u32,
    pub fastcgi_send_timeout: u32,
    pub fastcgi_read_timeout: u32,
    pub gzip: bool,
    pub gzip_comp_level: u32,
    pub gzip_types: String,
}

impl Default for WebServerSettingsDto {
    /// Derived from [`openvhost_conf::WebServerSettings::default`], never from
    /// literals repeated here. A DTO that hardcoded its own defaults would be a
    /// second source of truth, and the two would drift the first time spec §5's
    /// table changed on one side only.
    fn default() -> Self {
        Self::from(openvhost_conf::WebServerSettings::default())
    }
}

/// Parse a `Seconds` field, reporting the DTO field name rather than
/// `Seconds::parse`'s own `"seconds"`.
///
/// Four settings are `Seconds` and `Seconds::parse` takes only a value, so
/// untouched it names a field called `"seconds"` — which exists on no form. The
/// UI would highlight nothing and the user would be told only that something
/// is wrong. See `ConfError::with_field` for why the relabel lives on the error
/// rather than in `Seconds::parse`'s signature.
fn seconds_field(v: u32, field: &'static str) -> Result<openvhost_conf::Seconds, IpcError> {
    openvhost_conf::Seconds::parse(v).map_err(|e| IpcError::from(e.with_field(field)))
}

impl TryFrom<WebServerSettingsDto> for openvhost_conf::WebServerSettings {
    type Error = IpcError;

    /// THE IPC INGRESS GUARD for the settings, the same shape as
    /// `TryFrom<SiteInput>` above: every field goes through its newtype's
    /// `parse`, so nothing unvalidated can reach `state.db` — and from there a
    /// generated nginx config. The repository's re-validate-on-read is the
    /// second line of defence against a hand-edited row, not the first against
    /// a hostile caller.
    ///
    /// `?` short-circuits on the first bad field, exactly like the site guard,
    /// and each error names its own field so the form marks that input. Nothing
    /// is written when any field is rejected, so the other fields keep the
    /// values already stored.
    fn try_from(d: WebServerSettingsDto) -> Result<Self, IpcError> {
        Ok(openvhost_conf::WebServerSettings {
            worker_connections: openvhost_conf::WorkerConnections::parse(d.worker_connections)?,
            client_max_body_size: openvhost_conf::BodySize::parse(&d.client_max_body_size)?,
            keepalive_timeout: seconds_field(d.keepalive_timeout, "keepalive_timeout")?,
            tcp_nodelay: openvhost_conf::OnOff::new(d.tcp_nodelay),
            fastcgi_connect_timeout: seconds_field(
                d.fastcgi_connect_timeout,
                "fastcgi_connect_timeout",
            )?,
            fastcgi_send_timeout: seconds_field(d.fastcgi_send_timeout, "fastcgi_send_timeout")?,
            fastcgi_read_timeout: seconds_field(d.fastcgi_read_timeout, "fastcgi_read_timeout")?,
            gzip: openvhost_conf::OnOff::new(d.gzip),
            gzip_comp_level: openvhost_conf::GzipLevel::parse(d.gzip_comp_level)?,
            gzip_types: openvhost_conf::GzipTypes::parse(&d.gzip_types)?,
        })
    }
}

impl From<openvhost_conf::WebServerSettings> for WebServerSettingsDto {
    fn from(s: openvhost_conf::WebServerSettings) -> Self {
        WebServerSettingsDto {
            worker_connections: s.worker_connections.get(),
            client_max_body_size: s.client_max_body_size.as_str().to_string(),
            keepalive_timeout: s.keepalive_timeout.get(),
            tcp_nodelay: s.tcp_nodelay.is_on(),
            fastcgi_connect_timeout: s.fastcgi_connect_timeout.get(),
            fastcgi_send_timeout: s.fastcgi_send_timeout.get(),
            fastcgi_read_timeout: s.fastcgi_read_timeout.get(),
            gzip: s.gzip.is_on(),
            gzip_comp_level: s.gzip_comp_level.get(),
            gzip_types: s.gzip_types.as_directive(),
        }
    }
}

/// The read and save bodies, over a plain `&Db` rather than `tauri::State`.
///
/// Split out of the two commands below so the boundary's actual behaviour —
/// that a rejected field is named, and that nothing is written when it is — is
/// reachable from a test with an in-memory database, instead of needing a mock
/// Tauri app to obtain a `State`.
async fn read_settings(db: &Db) -> Result<WebServerSettingsDto, IpcError> {
    let repo = SqliteWebServerSettings::new(db);
    // Absent row => documented defaults, and nothing is written. See
    // `WebServerSettingsRepository::get`.
    Ok(WebServerSettingsDto::from(repo.get().await?))
}

/// Asks nginx whether a candidate settings struct is acceptable.
///
/// A trait rather than a direct call so the save path's behaviour — that a
/// rejection names a field and writes nothing — is testable without nginx
/// installed. The production implementation is [`NginxSettingsChecker`].
#[async_trait::async_trait]
pub trait SettingsChecker: Send + Sync {
    async fn check(
        &self,
        settings: &openvhost_conf::WebServerSettings,
    ) -> Result<openvhost_conf::SettingsCheck, openvhost_conf::ConfError>;
}

/// The real check: `nginx -t` over a candidate render (`check_settings`).
pub struct NginxSettingsChecker {
    pub bin: PathBuf,
    /// Where the throwaway candidate config is written — inside the app's own
    /// home, never `/tmp`.
    pub scratch_root: PathBuf,
}

#[async_trait::async_trait]
impl SettingsChecker for NginxSettingsChecker {
    async fn check(
        &self,
        settings: &openvhost_conf::WebServerSettings,
    ) -> Result<openvhost_conf::SettingsCheck, openvhost_conf::ConfError> {
        openvhost_conf::check_settings(&self.bin, &self.scratch_root, settings).await
    }
}

/// The part of nginx's stderr worth putting beside a form field.
///
/// nginx's `[emerg]` line ends with ` in <path>:<line>`, where the path is a
/// throwaway directory the user has never seen and the line number belongs to
/// a file they cannot open. The field marking already says WHERE, so that tail
/// is dropped and the reason kept. Anything unrecognised is passed through
/// whole rather than swallowed — a message we failed to parse is still the
/// only diagnostic there is.
fn rejection_message(stderr: &str) -> String {
    let Some(line) = stderr.lines().find(|l| l.contains("[emerg]")) else {
        return stderr.trim().to_string();
    };
    let reason = line.split("[emerg]").nth(1).unwrap_or(line).trim();
    let trimmed = match reason.rfind(" in ") {
        Some(i) if reason[i..].ends_with(char::is_numeric) => &reason[..i],
        _ => reason,
    };
    format!("nginx rejected this value: {trimmed}")
}

/// Validate, ASK NGINX, then store.
///
/// The nginx step is what stops a value that passes the newtypes but that
/// nginx refuses from being written. Such a value is not a one-off failure:
/// `render_set` regenerates `nginx.conf` from the stored settings on every
/// plan, so once stored it makes EVERY later apply fail validation and roll
/// back — including one triggered from the Sites page, where the error names
/// an nginx internal and points at no field. See `openvhost_conf::settings`'s
/// check module for why `WebServerAdapter::validate` cannot serve this.
///
/// `checker` is `None` when nginx is not installed. The save then proceeds
/// unchecked, deliberately: the Web server page stays editable on a machine
/// that has not installed nginx yet (the Languages page guides that), and with
/// no nginx there is no apply to trap. The guarantee is "checked whenever
/// nginx is present", which is exactly when it can matter.
async fn write_settings(
    db: &Db,
    input: WebServerSettingsDto,
    checker: Option<&dyn SettingsChecker>,
) -> Result<(), IpcError> {
    // Cheap, precise guard first: these errors name their own field exactly,
    // and cost no process spawn.
    let settings: openvhost_conf::WebServerSettings = input.try_into()?;

    if let Some(checker) = checker {
        let verdict = match checker.check(&settings).await {
            Ok(v) => v,
            // The binary recorded at launch will not run — nginx was removed
            // or moved since. That is the SAME situation as `checker: None`,
            // so it degrades the same way rather than making the page
            // unsavable: with no runnable nginx there is no apply to trap.
            // A TIMEOUT is deliberately not included: that means nginx exists
            // and hung, which is a real failure and must not pass as checked.
            Err(openvhost_conf::ConfError::ValidatorSpawn { .. }) => {
                return Ok(SqliteWebServerSettings::new(db).save(&settings).await?);
            }
            Err(e) => return Err(e.into()),
        };
        match verdict {
            openvhost_conf::SettingsCheck::Accepted { .. } => {}
            openvhost_conf::SettingsCheck::Rejected { field, stderr } => {
                let message = rejection_message(&stderr);
                // A rejection nginx could not be traced to one field is a
                // banner, NOT a silent pass: either way nothing is stored.
                return Err(match field {
                    Some(field) => IpcError::Validation {
                        field: field.to_string(),
                        message,
                    },
                    None => IpcError::Core { message },
                });
            }
        }
    }

    SqliteWebServerSettings::new(db).save(&settings).await?;
    Ok(())
}

/// The stored nginx settings, or the documented defaults when the user has
/// never saved any.
// Not a doc comment, deliberately: tauri-specta copies those into
// `bindings.ts`, and this note is for a Rust reader, not a TS caller.
//
// REFUSES with no store rather than serving the defaults (design D3). Both are
// defensible; the tiebreak is which side fails QUIETLY, and a populated,
// editable form whose Save always fails is the quiet one.
#[tauri::command]
#[specta::specta]
pub async fn web_server_settings(
    db: tauri::State<'_, DbHandle>,
) -> Result<WebServerSettingsDto, IpcError> {
    read_settings(db.require()?).await
}

/// Validate and store the nginx settings. Does **not** apply them.
///
/// Applying is the user's next, explicit step through `plan_config_apply` /
/// `apply_config` — the same pipeline the sites go through, which is why there
/// is no settings-only apply command. A second apply path would mean two ways
/// for the live config to change, only one of which shows a diff first.
///
/// It DOES run `nginx -t` first, over a candidate render of the submitted
/// values (`write_settings`). That check renders the user's own values, which
/// is why it cannot be `WebServerAdapter::validate` — that call renders with
/// *defaults* on purpose, answers "is the shape valid?", and would wave
/// through a combination nginx rejects. Measured cost of the spawn is well
/// under a frame, against a failure that otherwise surfaces at an unrelated
/// later apply.
#[tauri::command]
#[specta::specta]
pub async fn save_web_server_settings(
    db: tauri::State<'_, DbHandle>,
    paths: tauri::State<'_, Option<StackPaths>>,
    input: WebServerSettingsDto,
) -> Result<(), IpcError> {
    // Before the `nginx -t` spawn below: there is nowhere to store the result,
    // so checking first would burn a child process to reach the same refusal.
    let db = db.require()?;
    // No stack, or a stack with no nginx binary resolved (design D3: neither
    // a packaged nor a Homebrew nginx was found) => no checker; see
    // `write_settings`.
    let checker = paths.inner().as_ref().and_then(|p| {
        p.nginx_bin.as_ref().map(|bin| NginxSettingsChecker {
            bin: bin.clone(),
            scratch_root: p.home.join("run"),
        })
    });
    write_settings(
        db,
        input,
        checker.as_ref().map(|c| c as &dyn SettingsChecker),
    )
    .await
}

// ---------------------------------------------------------------------------
// PHP versions (Languages page)
// ---------------------------------------------------------------------------

/// One row on the Languages page: a catalogue version (installed or not), or
/// an installed version outside the catalogue (spec §6.1's "still listed"
/// requirement — an install made by hand, or one a later catalogue drops,
/// must not vanish from the page while it keeps serving sites).
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PhpRuntimeDto {
    pub major: String,
    pub installed: bool,
    /// Whether this build OFFERS this major — i.e. whether it is in
    /// `openvhost_core::CATALOGUE`.
    ///
    /// The mirror of `MysqlInstanceDto::cataloged`, and it exists for the same
    /// reason: `false` means the page must render the row (it is installed and
    /// serving sites) while offering neither Install nor Uninstall for it.
    /// Both spec builders refuse an out-of-catalogue major outright
    /// (`php::brew::cataloged`), so an affordance the row cannot honour would
    /// be a button whose only outcome is a validation error.
    ///
    /// Not derivable on the frontend: the catalogue is a Rust constant, and a
    /// second copy of it in TypeScript would be a list to forget to update.
    pub cataloged: bool,
    pub recommended: bool,
    /// A more precise version string than `major` (e.g. a patch level), when
    /// one is known. `None` does NOT mean anything is wrong with this row —
    /// it means we do not know the patch level.
    ///
    /// **A packaged row knows it; a Homebrew row still does not**, and that
    /// asymmetry is the point (PHP-discovery design D1, off-Homebrew slice
    /// 5B). OpenVHost's own install writes the exact version down as a
    /// directory name — `packages/php/8.4/8.4.24/` — so a packaged row reports
    /// `8.4.24` with **nothing executed** to find out. Homebrew's would have to
    /// be probed, and the only prober we have,
    /// `openvhost_conf::probe_php_fpm_version`, returns `major.minor` and never
    /// a patch level, so a Homebrew row stays `None` — which is what every row
    /// carried before there was a package tree to read.
    ///
    /// **Never echo `major` back into this field.** It would render "8.3" twice
    /// next to each other and imply a patch level was fetched when it was not.
    /// A packaged 8.4 row shows `8.4` and `8.4.24`; a brew 8.5 row shows `8.5`
    /// and nothing. If those look wrong side by side the fix is the layout, not
    /// an invented patch level.
    pub full_version: Option<String>,
    pub path: Option<String>,
    /// Where this version's pool listens. `None` until installed.
    pub socket_path: Option<String>,
    /// The supervisor id for this version's pool, so the UI can drive
    /// start/stop from the row without inventing the id itself.
    pub service_id: Option<String>,
    /// Where this row's binaries came from — OpenVHost's own package tree or a
    /// Homebrew keg (off-Homebrew slice 5C design D3). `None` when nothing is
    /// installed for this major, which is the ONLY reason it is optional: an
    /// installed runtime always knows its own source.
    ///
    /// Two install sources coexist by design during the migration, so "which
    /// php-fpm am I actually running" has to be answerable without the user
    /// reading a path — the same question `MysqlInstanceDto::source` answers
    /// for mysqld.
    pub source: Option<PhpRuntimeSourceDto>,
    /// Whether THIS BUILD publishes a verified package for this major on THIS
    /// host, and which version it would install (design D1).
    ///
    /// Distinct from `cataloged`: that says "this build manages the major", and
    /// is what gates the Homebrew Install/Uninstall affordances; this says "and
    /// there are bytes of our own for your architecture". The two disagree on
    /// every real machine today — 8.4 is pinned but its release is unpublished
    /// (`AwaitingRelease`) and no other major has a built artifact at all
    /// (`Unavailable`) — which is exactly why this is a state and not a bool.
    ///
    /// Not an `Option`: "no package for this major" is `Unavailable`, which
    /// names the target it looked for. A `None` beside it would be a second
    /// spelling of the same absence.
    pub offer: PhpPackageOfferDto,
}

/// Which PHP major the catch-all serves and **why** — the wire copy of
/// [`openvhost_core::DefaultPhp`] (default-PHP design D2).
///
/// Four variants because there are four distinct outcomes, and the whole point
/// of the type is that they stay distinct: "nobody chose" and "your choice is
/// no longer installed" agree on what gets served and must never be rendered
/// the same way. Collapsing them is the defect shape this project has now
/// shipped four times (a boolean that could not express `Failed`; an offer
/// union that could not express `awaitingRelease`; a `fallback_brew()` that
/// invented a path; a `brewFound` bool answering a per-major question).
///
/// [`From<&DefaultPhp>`] below is a full match with **no wildcard arm**, so a
/// fifth core outcome fails to compile here rather than arriving on the wire
/// as whichever variant happened to come last.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DefaultPhpDto {
    /// No preference, and no PHP installed: the catch-all gets no PHP block.
    NothingInstalled,
    /// No preference stored, so the historical first-discovered rule applies.
    /// **This is every machine that predates the preference**, and the state
    /// the Languages page must render exactly as it did before this slice.
    Unset { serving: String },
    /// A preference is stored and that major is installed.
    Preferred { major: String },
    /// A preference is stored and that major is **not** installed — uninstalled
    /// since, or a keg that disappeared. Carries BOTH what was asked for and
    /// what is being served instead, which is what lets the page say "your
    /// default was 8.4, which is no longer installed" (spec claim 4) rather
    /// than silently showing 8.1.
    ///
    /// `serving` is `None` in the one case where the fallback has nothing to
    /// fall back to: a stored preference with nothing installed at all.
    PreferredMissing {
        requested: String,
        serving: Option<String>,
    },
}

impl From<&openvhost_core::DefaultPhp> for DefaultPhpDto {
    fn from(d: &openvhost_core::DefaultPhp) -> Self {
        use openvhost_core::DefaultPhp as D;
        match d {
            D::NothingInstalled => Self::NothingInstalled,
            D::Unset { serving } => Self::Unset {
                serving: serving.clone(),
            },
            D::Preferred { major } => Self::Preferred {
                major: major.clone(),
            },
            D::PreferredMissing { requested, serving } => Self::PreferredMissing {
                requested: requested.clone(),
                serving: serving.clone(),
            },
        }
    }
}

/// What the Languages page needs to decide which state to show (spec §6.1).
///
/// `brew_found` means exactly one thing — **we looked for Homebrew and did not
/// find it** — and `brew_searched` lists the paths verbatim so a user can check
/// the right place on their own machine. That is honest and it stays.
///
/// What it is NOT is the page's first and highest-priority state (off-Homebrew
/// slice 5C design D5). A machine with no Homebrew is not a machine with no
/// PHP: it may already have a packaged runtime listed in `runtimes`, or a major
/// whose `offer` is installable from our own tree. Whether the page has a route
/// to any PHP at all is a question about the ROWS, and answering it from this
/// bool alone is what made the page tell a user it could not install PHP while
/// simultaneously not listing the PHP they already had.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PhpEnvironmentDto {
    pub brew_found: bool,
    pub brew_searched: Vec<String>,
    pub runtimes: Vec<PhpRuntimeDto>,
    /// Which major the catch-all serves, resolved from the stored preference
    /// against `runtimes` in the same pass that built them (default-PHP design
    /// D2/D6).
    ///
    /// Rides on the ENVIRONMENT rather than on a row, unlike
    /// [`PhpRuntimeDto::offer`], and the reason is [`DefaultPhpDto::PreferredMissing`]:
    /// the major it names may have no row at all (a hand-installed `php@7.4`
    /// that was then removed leaves neither a catalogue entry nor an installed
    /// runtime). A per-row field could not carry that state anywhere, which is
    /// exactly the fact the user most needs told.
    pub default_php: DefaultPhpDto,
}

/// One line of `brew install`'s output, forwarded live while an install runs.
/// Same shape and reasoning as [`ServiceLogEvent`] — see its declaration —
/// except `major` names which install this line belongs to, and `stream` is
/// a plain "stdout"/"stderr" string rather than `LogLevel`: brew's output has
/// no severity for the supervisor's classifier to assign, only a stream.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct PhpInstallLogEvent {
    pub major: String,
    pub ts_ms: u64,
    pub stream: String,
    pub line: String,
}

/// Milliseconds since the epoch, for [`PhpInstallLogEvent::ts_ms`].
/// `openvhost_proc` has an identical helper, but it is `pub(crate)` there —
/// this command builds its own event rather than relaying one off the
/// supervisor's broadcast, so it needs its own clock read.
///
/// `pub(crate)`: `uninstall.rs` streams `brew uninstall`'s output through
/// these very same events (package-uninstall design D1 — the user who watched
/// it arrive watches it leave, on one surface), so it stamps them the same way.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build one row per catalogue entry, in catalogue order, plus one row for
/// every installed runtime that falls outside the catalogue. Pure and
/// Tauri-free so the row-building logic is testable without a live
/// `AppHandle` or a real supervisor.
///
/// `full_versions` maps a major to a more precise string (e.g. a patch level)
/// the page can show next to it, when one is actually known. **It is the
/// HOMEBREW half of that question, and in production it is still empty**: the
/// only prober we have, `openvhost_conf::probe_php_fpm_version`, returns
/// `major.minor` and never a patch level, so there is nothing more precise to
/// hand in for a keg today — and echoing `major` back in as if it were that
/// string would render "8.3" twice and imply a patch level had been fetched
/// when it had not. Wiring a true patch-level prober is future work; keeping
/// this a separate parameter means that upgrade will not have to touch this
/// function's callers beyond what they pass in.
///
/// A **packaged** runtime needs no such map and never consults it: its exact
/// version is already written down as the directory name our own installer
/// chose (`PhpRuntimeSource::Packaged { version }`, PHP-discovery design D1),
/// so `full_version` is filled from the runtime itself and **nothing is
/// executed** to learn it. The runtime's own source wins over the map for the
/// same reason: it describes THIS install, while a map keyed only by major
/// could carry a different install's answer.
///
/// Spawns nothing, opens nothing, and takes no probe closure — the whole
/// function is a fold over data the caller already has.
fn php_rows(
    home: &Path,
    installed: &[openvhost_core::PhpRuntime],
    full_versions: &[(&str, &str)],
) -> Vec<PhpRuntimeDto> {
    let newest = openvhost_core::CATALOGUE.last().copied();
    let build = |major: &str, found: Option<&openvhost_core::PhpRuntime>| {
        let spec = found.map(|rt| crate::stack::php_fpm_spec(home, rt));
        PhpRuntimeDto {
            major: major.to_string(),
            installed: found.is_some(),
            // The SAME expression the out-of-catalogue loop below uses to
            // decide a row needs adding at all — one reading of the catalogue,
            // so a row can never be appended as out-of-catalogue while
            // reporting itself as offered.
            cataloged: openvhost_core::CATALOGUE.contains(&major),
            recommended: Some(major) == newest,
            full_version: found.and_then(|rt| {
                // The packaged answer first, and it costs a `clone`: the tree
                // recorded this runtime's exact version at install time, so
                // `PhpRuntimeSource::version()` is a read of something already
                // written down. Only a Homebrew row falls through to the map,
                // which is empty in production — see this function's doc.
                rt.source.version().map(str::to_string).or_else(|| {
                    full_versions
                        .iter()
                        .find(|(m, _)| *m == major)
                        .map(|(_, v)| (*v).to_string())
                })
            }),
            path: found.map(|rt| rt.fpm_bin.display().to_string()),
            socket_path: spec.as_ref().and_then(|s| s.endpoint.clone()),
            service_id: spec.map(|s| s.id),
            source: found.map(|rt| PhpRuntimeSourceDto::from(&rt.source)),
            // Per major, on the row (design D1) — PHP's headline feature is
            // several majors side by side, so one answer for the whole page
            // would be wrong for every row but one.
            offer: crate::php_pkg::package_offer(major),
        }
    };

    let mut rows: Vec<PhpRuntimeDto> = openvhost_core::CATALOGUE
        .iter()
        .map(|major| build(major, installed.iter().find(|rt| rt.major == *major)))
        .collect();

    for rt in installed {
        if !openvhost_core::CATALOGUE.contains(&rt.major.as_str()) {
            rows.push(build(&rt.major, Some(rt)));
        }
    }

    rows
}

/// Scan BOTH PHP install sources — OpenVHost's own `<home>/packages/php/`
/// tree and every known Homebrew prefix (PHP-discovery design D2).
///
/// `home` is what makes the packaged tree visible; the [`PackagesRoot`] is
/// minted from it and from nothing a caller supplies. This is the exact
/// parameter, and the exact reason, `discover_all_mysql` below already has: a
/// rescan that read only Homebrew would make a freshly installed packaged
/// runtime vanish from the Languages page the moment the user pressed Check
/// again — and, worse, would disagree with what startup found, which is the
/// C2 class of bug (`stack.rs`) all over again.
///
/// `openvhost_core::discover_php` takes a SYNCHRONOUS probe closure, but
/// `openvhost_conf::probe_php_fpm_version` is async. Resolved by running the
/// whole directory walk on `spawn_blocking` and calling the async prober via
/// `Handle::block_on` from INSIDE that blocking closure: `spawn_blocking`
/// hands the closure its own blocking-pool thread, not one of the async
/// worker threads, so blocking there to wait on a future cannot deadlock the
/// runtime the way calling `block_on` directly inside an async command would.
///
/// The other option the task allowed — pre-building a `path -> version` map
/// by probing candidates asynchronously first, then handing the walk a closure
/// that only reads that map — was passed over because the set of candidate
/// paths is exactly what the walk's own (private) directory traversal already
/// computes. Re-deriving that candidate list here first would duplicate
/// discovery logic that already exists and is already tested, which is the
/// kind of copy-paste drift the project's own coding-style rules warn against;
/// this approach reuses the walk untouched instead.
///
/// The packaged half spawns nothing at all (design D1: its version is a
/// directory name chosen at install time), so this bridge is only ever paid
/// for the Homebrew candidates.
async fn discover_all_php(
    home: &Path,
) -> Result<openvhost_core::Discovery<openvhost_core::PhpRuntime>, IpcError> {
    let packages = openvhost_core::PackagesRoot::from_home(home);
    tauri::async_runtime::spawn_blocking(move || {
        let handle = tokio::runtime::Handle::current();
        let prefixes: Vec<&Path> = brew_prefixes();
        openvhost_core::discover_php(&packages, &prefixes, &|bin| {
            handle.block_on(openvhost_conf::probe_php_fpm_version(bin))
        })
    })
    .await
    .map_err(|e| IpcError::Core {
        message: format!("the PHP discovery task failed to run: {e}"),
    })
}

/// `openvhost_core::BREW_PREFIXES` as paths, in order. One expression, because
/// discovery, the install-time path resolver and the uninstall's keg check must
/// all look in the same places and in the same order or they disagree about
/// which installation the app is talking about.
fn brew_prefixes() -> Vec<&'static Path> {
    openvhost_core::BREW_PREFIXES
        .iter()
        .map(Path::new)
        .collect()
}

/// Report candidates a discovery pass could not identify.
///
/// Never propagated — a rescan is a read-mostly refresh and must not fail
/// because one directory was unreadable — but never silently dropped either:
/// this is the difference between "nothing is installed" and "I could not
/// tell", and the whole reason `Discovery` is not a bare `Vec`. A candidate
/// listed here holds the binaries the app needs; only its VERSION is unknown.
fn report_unidentified(kind: &str, unidentified: &[PathBuf]) {
    for dir in unidentified {
        eprintln!(
            "rescan: {kind} is installed at {} but its version could not be read — \
             neither Homebrew's keg path nor the version probe answered; it is not listed",
            dir.display()
        );
    }
}

/// Which majors in `found` were not already in `before` — the ones a rescan
/// should hand to `Supervisor::register`.
///
/// Extracted so the "which majors are new" decision is a pure function,
/// testable without a `Supervisor`. It matters because `Supervisor::register`
/// only no-ops against a `Starting`/`Running` entry — a `Failed { exit,
/// stderr_tail }` row, or a `Stopped` one with accumulated log lines, is
/// REPLACED, which wipes that row's `RingBuffer`, its `stderr_tail` and its
/// exit code. That is real diagnostic state, readable through
/// `service_log_tail`, not bookkeeping: it is the answer to "why did this
/// pool fail to start" for a bystander PHP version that had nothing to do
/// with whatever prompted this rescan. So only a major that is genuinely new
/// gets registered; one already known, in whatever state, is left alone.
///
/// A major present in `before` but missing from `found` (uninstalled outside
/// the app) is likewise not returned here — that is the SUBTRACTION half of a
/// rescan, and it belongs to [`vanished_service_ids`]/[`unregister_vanished`]
/// below (package-uninstall design D5), not to this function.
fn newly_installed_majors(before: &[String], found: &[String]) -> Vec<String> {
    found
        .iter()
        .filter(|major| !before.contains(major))
        .cloned()
        .collect()
}

/// Which of `registered`'s ids belong to this package family (by `prefix`) and
/// name a major that is NOT in `found` — i.e. rows for a version that has
/// disappeared from disk.
///
/// Pure and separately testable, which is the point: the dangerous mistake
/// here is over-matching. `nginx`, `demo-ticker` and every MySQL row must be
/// invisible to a PHP rescan (and vice versa), because the consequence of a
/// wrong match is the supervisor forgetting a service the user is running.
/// `strip_prefix` — not `contains`/`starts_with`-plus-slicing — is what makes
/// the major exact rather than merely present.
fn vanished_service_ids(registered: &[String], prefix: &str, found: &[String]) -> Vec<String> {
    registered
        .iter()
        .filter(|id| {
            id.strip_prefix(prefix)
                .is_some_and(|major| !found.iter().any(|f| f == major))
        })
        .cloned()
        .collect()
}

/// Forget every supervisor row whose package is no longer installed — the
/// reconciliation half of a rescan (package-uninstall design D5).
///
/// This is what makes an in-app uninstall and a `brew uninstall` run behind the
/// app's back converge on the same observable state, and it fixes the
/// pre-existing stale-row bug (a row pointing at a deleted binary, which used
/// to survive forever and "simply fail honestly the next time it is started")
/// as a side effect rather than leaving two divergent behaviours.
///
/// Every outcome is LOGGED, never propagated: a rescan is a read-mostly
/// refresh and must not fail because one row could not be tidied.
/// `NotTerminal` in particular is expected, not impossible — a pool whose
/// binary was deleted out from under a running process keeps running (unix
/// unlinks the path, not the inode), and `unregister` deliberately refuses to
/// forget a child the supervisor is still supervising. Such a row is picked up
/// by the next rescan after the user stops it.
///
/// Exhaustive over [`ProcError`] with no wildcard arm: a new failure mode must
/// be classified here on purpose, not swallowed by a `_`.
fn unregister_vanished(sup: &Supervisor, prefix: &str, found: &[String]) {
    let registered: Vec<String> = sup.snapshot().into_iter().map(|s| s.id).collect();
    for id in vanished_service_ids(&registered, prefix, found) {
        match sup.unregister(&id) {
            Ok(()) => eprintln!("rescan: {id} is no longer installed; removed its service row"),
            // Started again between the snapshot and here, or still running
            // from a deleted binary. Left alone on purpose; the next rescan
            // after it stops will remove it.
            Err(ProcError::NotTerminal { id, state }) => eprintln!(
                "rescan: {id} is no longer installed but is {state}; leaving its row until it stops"
            ),
            // Raced by another rescan or by an in-app uninstall that got there
            // first. Nothing to do and nothing wrong.
            Err(ProcError::NotFound(id)) => {
                eprintln!("rescan: {id} was already gone before it could be removed")
            }
            Err(ProcError::Io(e)) => {
                eprintln!("rescan: failed to remove the service row for {id}: {e}")
            }
        }
    }
}

/// Probe for installed PHP runtimes, write the result into the managed
/// `RwLock`, and register a supervisor row for every NEWLY discovered major.
///
/// Only new majors are registered — see `newly_installed_majors` for why
/// re-registering an already-known major (even an unchanged one) is not
/// idempotent in the way it looks: it can silently erase a `Failed` row's
/// stderr and exit code.
///
/// Majors that VANISHED are unregistered (package-uninstall design D5), so an
/// in-app uninstall and a `brew uninstall` behind the app's back converge on
/// the same observable state — see [`unregister_vanished`].
///
/// Shared by `rescan_php_runtimes`, `install_php` and `uninstall_package` so
/// they cannot register two different service shapes for the same version.
///
/// `seed` is `Some` on exactly one path: an `install_php` that has just run
/// `brew install php@<major>` itself. See [`seeded_php`].
pub(crate) async fn rescan_into_state(
    runtimes: &RwLock<Option<InstalledRuntimes>>,
    sup: &Supervisor,
    paths: &StackPaths,
    seed: Option<openvhost_core::PhpRuntime>,
) -> Result<openvhost_core::Discovery<openvhost_core::PhpRuntime>, IpcError> {
    // `paths.home` — the resolved home `macos_stack` built these paths from,
    // never a caller-supplied path — is what makes OpenVHost's own package
    // tree visible to a rescan, so a rescan and a cold start see the same
    // machine (PHP-discovery design D2).
    let discovered = seeded_php(discover_all_php(&paths.home).await?, seed);
    report_unidentified("PHP", &discovered.unidentified);
    reconcile_php(runtimes, sup, paths, discovered)
}

/// Fold a runtime this app just installed ITSELF into a discovery result.
///
/// The seed is not another probe result to be merged on equal terms: it is the
/// formula directory brew created in response to a `brew install php@<major>`
/// this process ran, located by path. It exists because interrogating that
/// binary afterwards is a lie waiting to happen — a freshly extracted binary's
/// first execution can stall past the probe's bound (see
/// `openvhost_core::keg`'s module docs for the measurement), and a killed probe
/// used to read as "nothing installed".
///
/// Discovery still WINS when it found the major independently: it applies the
/// prefix-priority and alias rules, and the seed is a naive first-hit. The seed
/// only fills a gap.
fn seeded_php(
    mut discovered: openvhost_core::Discovery<openvhost_core::PhpRuntime>,
    seed: Option<openvhost_core::PhpRuntime>,
) -> openvhost_core::Discovery<openvhost_core::PhpRuntime> {
    let Some(rt) = seed else {
        return discovered;
    };
    // This candidate is identified now — by the install that produced it.
    discovered
        .unidentified
        .retain(|dir| !rt.fpm_bin.starts_with(dir));
    if !discovered.runtimes.iter().any(|r| r.major == rt.major) {
        discovered.runtimes.push(rt);
        discovered.runtimes.sort_by(|a, b| a.major.cmp(&b.major));
    }
    discovered
}

/// Everything a PHP rescan does with the discovery result: write the managed
/// state, register what is new, forget what is gone.
///
/// Split from the probe above so this — the part with the actual reconciliation
/// RULES in it — is testable against a synthetic runtime list, with no
/// Homebrew on the machine and no process spawned. Without the split, the D5
/// unregister step could be deleted from the rescan path and no test in this
/// crate would notice, because the only way to reach it would be through a
/// probe of the developer's own installed PHP versions.
fn reconcile_php(
    runtimes: &RwLock<Option<InstalledRuntimes>>,
    sup: &Supervisor,
    paths: &StackPaths,
    discovered: openvhost_core::Discovery<openvhost_core::PhpRuntime>,
) -> Result<openvhost_core::Discovery<openvhost_core::PhpRuntime>, IpcError> {
    let php = discovered.runtimes.clone();
    let before: Vec<String> = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .as_ref()
        .map(|r| r.php.iter().map(|rt| rt.major.clone()).collect())
        .unwrap_or_default();

    let found: Vec<String> = php.iter().map(|rt| rt.major.clone()).collect();
    let new_majors = newly_installed_majors(&before, &found);

    // State write BEFORE the supervisor registration below. `apply_config` and
    // `plan_config_apply` read the `RwLock`, not the supervisor's row set, to
    // decide which PHP versions exist — so registering a row first would open
    // a window where `php-fpm-8.4` is visible and startable in the Services
    // panel while the apply pipeline still answers `MissingRuntime` for 8.4,
    // because the state write had not landed yet.
    *runtimes.write().map_err(|_| IpcError::Core {
        message: "runtime list is poisoned".into(),
    })? = Some(InstalledRuntimes {
        nginx_bin: paths.nginx_bin.clone(),
        php: php.clone(),
    });

    for major in new_majors {
        if let Some(rt) = php.iter().find(|rt| rt.major == major) {
            sup.register(crate::stack::php_fpm_spec(&paths.home, rt));
        }
    }
    // AFTER the registrations, not before: the two touch disjoint sets (a
    // major cannot be both newly found and vanished), but ordering the
    // subtraction last keeps the window in which a row is missing from the
    // snapshot as short as possible for an observer watching the events.
    unregister_vanished(sup, crate::stack::PHP_FPM_ID_PREFIX, &found);

    Ok(discovered)
}

/// `openvhost_core::BREW_PREFIXES` joined with `bin/brew`, so the UI can say
/// exactly where Homebrew was looked for.
fn brew_searched_paths() -> Vec<String> {
    openvhost_core::BREW_PREFIXES
        .iter()
        .map(|prefix| Path::new(prefix).join("bin/brew").display().to_string())
        .collect()
}

/// Resolve the stored default-PHP preference against `installed`.
///
/// Over a plain `&Db` and a slice rather than `tauri::State`, the same split
/// `read_settings`/`write_settings` use above and for the same reason: the
/// behaviour that matters — that an uninstalled preference resolves to
/// `PreferredMissing` and not to `Unset` — is then reachable from a test with
/// an in-memory database instead of needing a mock Tauri app.
///
/// Reads a fresh row every call. `php_environment` is the Languages page's own
/// mount/refresh read, so caching would mean a default set in one place and a
/// page rendered in another could disagree about which major is served.
/// **Takes `Option<&Db>`, and neither absence is an error on this path.**
/// `state.db` is opened best-effort (`lib.rs`: "a missing/unreadable state.db
/// must never stop the supervisor from starting"), and this is the Languages
/// page's own mount read. A hard dependency here would take the whole page
/// down — no rows, no Install, no Homebrew guidance — on a machine where it
/// used to work fine, which is the failure `install_php` was deliberately
/// written to avoid. No database means no stored preference, which is exactly
/// what `None` already means.
///
/// An **unreadable** stored value is treated the same way, and that is a
/// judgement rather than laziness: the only in-app route to clear a bad
/// preference is the control on this very page, so failing here would brick
/// the one surface that can fix it. The apply pipeline still fails closed on
/// the same value (`apply_input`), so nothing is generated from a value we
/// could not parse — the fallback is for *rendering*, not for serving.
async fn read_default_php(
    db: Option<&Db>,
    installed: &[openvhost_core::PhpRuntime],
) -> Result<DefaultPhpDto, IpcError> {
    let Some(db) = db else {
        return Ok(DefaultPhpDto::from(&openvhost_core::DefaultPhp::resolve(
            None, installed,
        )));
    };
    // Absent row => `None` => nobody has chosen. See `PhpSettingsRepository::get`:
    // reading never writes one.
    let stored = match SqlitePhpSettings::new(db).get().await {
        Ok(stored) => stored,
        Err(e) => {
            // Deliberately not surfaced as a resolution state: naming it would
            // need a fifth `DefaultPhp` variant, and the value is unbounded
            // text that would then have to reach a user-facing sentence. The
            // gap this leaves — the user is not TOLD why their choice stopped
            // applying, only that it is not in effect — is recorded in the
            // spec as owed.
            eprintln!(
                "openvhost: stored default PHP could not be read ({e}); \
                 falling back to no preference"
            );
            return Ok(DefaultPhpDto::from(&openvhost_core::DefaultPhp::resolve(
                None, installed,
            )));
        }
    };
    Ok(DefaultPhpDto::from(&openvhost_core::DefaultPhp::resolve(
        stored.default_major.as_ref(),
        installed,
    )))
}

/// Store the default-PHP preference (`Some`) or clear it (`None`).
///
/// **Two guards, and they answer different questions.**
///
///  1. [`PhpVersion::parse`] is THE IPC ingress guard, the same shape
///     `TryFrom<SiteInput>` and `TryFrom<WebServerSettingsDto>` are: the value
///     reaches `state.db`, and from there a php-fpm socket filename, so nothing
///     unvalidated may pass. The error names `default_major`, matching the
///     column and the repository's own re-validate-on-read relabel.
///  2. **The major must be installed.** A preference for something absent is a
///     state you ARRIVE at — by uninstalling the major you chose — never one
///     you choose; the app has no reason to help a caller manufacture it, and
///     the Languages page only ever offers installed rows. Refusing it here
///     means the only route to `PreferredMissing` is the real one, so the state
///     the page renders always describes something that actually happened.
///
/// Storing is deliberately NOT applying. The generated config changes only
/// through `plan_config_apply` / `apply_config` — diff preview, `nginx -t`,
/// rollback — exactly like `save_web_server_settings` (default-PHP spec claim
/// 6). A second write path to the live config would be a change the user never
/// saw a diff for.
///
/// No `nginx -t` pre-check, unlike `write_settings`, and the asymmetry is
/// structural rather than an omission: that check exists because a stored nginx
/// value nginx refuses makes EVERY later apply fail and roll back. A stored
/// major cannot do that. `DefaultPhp::resolve` takes the served major from the
/// **installed runtime** it matched and falls back to the first installed one
/// when it matches nothing, so the stored string never reaches
/// `socket_path` — there is no value here that could poison a later apply.
async fn write_default_php(
    db: &Db,
    major: Option<String>,
    installed: &[openvhost_core::PhpRuntime],
) -> Result<(), IpcError> {
    let default_major = match major {
        None => None,
        Some(raw) => {
            let parsed = PhpVersion::parse(&raw).map_err(|e| match e {
                openvhost_core::CoreError::Validation { reason, .. } => IpcError::Validation {
                    field: "default_major".to_string(),
                    message: reason,
                },
                other => IpcError::from(other),
            })?;
            if !installed.iter().any(|rt| rt.major == parsed.as_str()) {
                return Err(IpcError::Validation {
                    field: "default_major".to_string(),
                    message: format!(
                        "PHP {} is not installed, so it cannot be made the default",
                        parsed.as_str()
                    ),
                });
            }
            Some(parsed)
        }
    };
    SqlitePhpSettings::new(db)
        .save(&openvhost_core::PhpSettings { default_major })
        .await?;
    Ok(())
}

/// Choose which PHP major the catch-all (`localhost:8080`) serves, or clear the
/// choice with `null`.
///
/// **Stores only.** The live config changes on the user's next explicit Apply,
/// through the same `plan_config_apply` / `apply_config` pipeline the sites and
/// the nginx settings go through — so the change is previewed as a diff,
/// validated by `nginx -t`, and rolled back if that fails. The Languages page
/// opens that dialog itself the moment this resolves, the same way the Web
/// server page does after a settings save.
///
/// A major that is not installed is refused, naming `default_major` — see
/// [`write_default_php`].
#[tauri::command]
#[specta::specta]
pub async fn set_default_php(
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    major: Option<String>,
) -> Result<(), IpcError> {
    // Storing IS the whole command (see the doc above — it applies nothing), so
    // with no store there is nothing left of it to degrade to.
    let db = db.require()?;
    // Cloned out of the guard and dropped before the `.await`, like every other
    // command reading this lock: holding a `std::sync::RwLockReadGuard` across
    // an await point makes the future non-`Send`, which fails to compile.
    let installed = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .as_ref()
        .map(|r| r.php.clone())
        .unwrap_or_default();
    write_default_php(db, major, &installed).await
}

/// Read-only environment summary for the Languages page: whether Homebrew was
/// found, where it looked, and one row per PHP version (spec §6.1).
///
/// Deliberately spawns NOTHING — it reads the managed `RwLock` and calls
/// `find_brew()` (a filesystem check, not a process). It is called on page
/// mount and after every install, and the discipline that keeps
/// `plan_config_apply` cheap (Task 4's managed `RwLock`, read then cloned and
/// dropped before anything else runs) applies here too. `rescan_php_runtimes`
/// is the one that actually probes.
#[tauri::command]
#[specta::specta]
// DEGRADE (optional-state.db design D2): the rows, the Homebrew probe and the
// search paths are all filesystem facts, so a store that never opened costs
// this command exactly one field — a STORED default reads as "no preference"
// (`read_default_php`'s own `None` arm). Refusing instead would empty a page
// whose every other value is still true. D5's banner is what stops that one
// silently-wrong field being dishonest.
pub async fn php_environment(
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<PhpEnvironmentDto, IpcError> {
    let p = stack_paths(&paths)?;
    let installed = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .as_ref()
        .map(|r| r.php.clone())
        .unwrap_or_default();
    // Empty, and still deliberately so: this map is the HOMEBREW patch level
    // (see `php_rows`'s doc), there is no prober for one, and a
    // `(major, major)` echo would make `full_version` read as a fetched patch
    // level instead of "unknown". A PACKAGED row does not come through here at
    // all — it fills `full_version` from the version its own install recorded.
    Ok(PhpEnvironmentDto {
        brew_found: openvhost_core::find_brew().is_some(),
        brew_searched: brew_searched_paths(),
        // Resolved against THE SAME `installed` slice the rows are built from,
        // so the major the page marks as default is always one of the rows it
        // rendered in the same response — the seam `render_set` keeps on its
        // own side by resolving against the very list its pool loop iterates.
        default_php: read_default_php(db.optional(), &installed).await?,
        runtimes: php_rows(&p.home, &installed, &[]),
    })
}

/// The explicit, user-initiated re-probe behind Languages' "Check again"
/// button: the user left to install Homebrew (or a PHP version) in a
/// terminal and came back. Unlike `php_environment`, this DOES spawn — once
/// per candidate binary, to read its version — so it is never called
/// implicitly.
///
/// Takes `InstallLock` — the same lock `install_php` holds for its whole
/// run — across the entire `rescan_into_state` call. Without it, a rescan is
/// a read-modify-write over the managed `RwLock` with nothing serializing it
/// against a concurrent install: the rescan can read the OLD set, block on
/// probing every candidate binary, and only write its (now stale) result
/// back AFTER an in-flight install finished and wrote the new one — silently
/// reverting a completed install. `install_php` still returns
/// `detected: true` for that install, so the row would say "Installed" while
/// the apply pipeline no longer knows the version exists.
#[tauri::command]
#[specta::specta]
// DEGRADE, for the same reason and with the same one-field cost as
// `php_environment` above — the probing this command adds needs no store at
// all.
pub async fn rescan_php_runtimes(
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<PhpEnvironmentDto, IpcError> {
    let p = stack_paths(&paths)?;
    // Blocks until any in-flight install has finished, rather than
    // `try_lock`-and-refuse like `install_php` does: a rescan is cheap and
    // idempotent, so waiting behind an install and then reading the
    // now-current state is correct, not merely tolerable — refusing it
    // outright would trade a wrong answer for no answer.
    let _guard = lock.inner().guard.lock().await;
    // No seed: nothing was installed here, and a seed is only ever a record of
    // what THIS app just asked brew for.
    let installed = rescan_into_state(runtimes.inner(), sup.inner(), p, None).await?;
    // Empty for the same reason `php_environment`'s is — see the comment
    // there: this map is the Homebrew patch level, and there is still no
    // prober that returns one.
    Ok(PhpEnvironmentDto {
        brew_found: openvhost_core::find_brew().is_some(),
        brew_searched: brew_searched_paths(),
        // Re-resolved against the NEWLY discovered set, not the pre-rescan one:
        // a rescan is exactly when a major can appear or disappear, so this is
        // the call that turns "your default is gone" into "your default is back"
        // (and the reverse) without a relaunch. Spec claim 5 — the preference
        // survives a rescan — is this line plus the fact that nothing on the
        // rescan path writes to `php_settings`.
        default_php: read_default_php(db.optional(), &installed.runtimes).await?,
        runtimes: php_rows(&p.home, &installed.runtimes, &[]),
    })
}

/// Serializes EVERY long-running, abortable background install/init run:
/// `install_php`, and (P1 MySQL lifecycle design, spec D7/plan Task 5)
/// `install_mysql` and `initialize_mysql`. Only one runs at a time,
/// REGARDLESS of kind — the mandatory cross-kind test below is the
/// generalization proof. The call site uses `try_lock`, not `lock` — a
/// second press while one is running should be refused with an explanation,
/// not silently queued behind a build that can take twenty minutes. Mirrors
/// `ApplyLock`'s shape.
///
/// Also holds the one thing `perform_quit` needs to make the C1 audit finding
/// stop being true: the in-flight run's `AbortHandle` (see `running`). The
/// containment `openvhost_proc::run_task` provides — killing the whole
/// process group — is `KillOnDrop`, which only fires when the run's future is
/// actually DROPPED. Before this, nothing in production ever dropped it:
/// `install_php` awaited `run_task` inline, so the future lived exactly as
/// long as the command handler did, and quitting mid-install went straight to
/// `window.destroy()` and then `process::exit` with no unwinding at all. Now
/// every one of these commands spawns its run so it has a handle to abort,
/// and `perform_quit` aborts-and-waits on it BEFORE destroying the window —
/// see `quit.rs`.
#[derive(Default)]
pub struct InstallLock {
    pub(crate) guard: tokio::sync::Mutex<()>,
    running: std::sync::Mutex<Option<RunningInstall>>,
}

/// Which of the commands sharing [`InstallLock`] currently occupies its
/// slot.
///
/// Review fix wave, Important 1: this used to gate only a PHP-specific
/// query (`pending_php_install`/`running_php_major`, both replaced by
/// [`InstallLock::running_install`]/`pending_install` below) whose own doc
/// comment deferred generalizing the quit dialog's copy to "the Databases UI
/// slice (Task 6)" — until this fix, a MySQL install or initialization in
/// flight was therefore invisible to the quit-confirmation dialog entirely
/// (`+layout.svelte`'s `pendingInstall`, rendered by `QuitDialog.svelte` as
/// either `PHP {label} is still installing.` or
/// `MySQL {label} is still installing.`), the same class of bug as a
/// service quitting mid-work with no warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallKind {
    Php,
    Mysql,
    Mariadb,
}

/// Wire-safe copy of [`InstallKind`] for [`PendingInstallDto`] — `InstallKind`
/// itself carries no `specta::Type`/`Serialize`; it is purely an internal
/// discriminator for `InstallLock`'s slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum InstallKindDto {
    Php,
    Mysql,
    Mariadb,
}

impl From<InstallKind> for InstallKindDto {
    fn from(kind: InstallKind) -> Self {
        match kind {
            InstallKind::Php => Self::Php,
            InstallKind::Mysql => Self::Mysql,
            InstallKind::Mariadb => Self::Mariadb,
        }
    }
}

/// WHAT the run occupying `InstallLock`'s slot is doing: putting a package on
/// the machine, taking one off it, or setting one up (package-uninstall design
/// D1 — an uninstall shares the lock, the abort handle and the live-output
/// surface with an install; a MySQL datadir initialization shares all three
/// too).
///
/// A separate value rather than more `InstallKind` variants, and rather than a
/// boolean: `kind` answers "PHP or MySQL" and this answers "arriving, leaving
/// or being set up". Collapsing the second question into the first would give
/// six variants that have to be kept in sync with two orthogonal facts, and
/// collapsing it into a `bool` is the same shape this codebase has already had
/// to unpick three times (a boolean where a state belongs).
///
/// SECURITY (audit F1, MySQL-from-tarball fix wave). [`Initialize`] exists
/// because `initialize_mysql` used to tag its run `(Mysql, Install)` — the
/// exact pair [`cancel_mysql_install`] fires on. The two runs differed only in
/// their `label`, which [`InstallLock::abort_running_if`] does not and must not
/// consult, so a `cancel_mysql_install` call — reachable from the webview even
/// though the shipped UI offers no such button during an init — aborted a
/// datadir initialization it was never meant to reach. **Every distinct run
/// that can hold this slot must be distinguishable HERE, in the discriminators,
/// not merely in prose a human reads.**
///
/// [`Initialize`]: PackageOperation::Initialize
/// [`cancel_mysql_install`]: crate::mysql_pkg::cancel_mysql_install
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackageOperation {
    Install,
    Uninstall,
    Initialize,
}

/// Wire-safe copy of [`PackageOperation`] — same relationship
/// [`InstallKindDto`] has to [`InstallKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum PackageOperationDto {
    Install,
    Uninstall,
    Initialize,
}

impl From<PackageOperation> for PackageOperationDto {
    fn from(op: PackageOperation) -> Self {
        match op {
            PackageOperation::Install => Self::Install,
            PackageOperation::Uninstall => Self::Uninstall,
            PackageOperation::Initialize => Self::Initialize,
        }
    }
}

/// The `(kind, operation)` pair a PHP **install** run is tagged with — either
/// route — and the same pair [`crate::php_pkg::cancel_php_install`] fires on.
///
/// One definition rather than the two inline spellings this used to have, for
/// the audit F1 reason [`MYSQL_INSTALL_RUN`] gives at length: the button and the
/// run it is meant to stop cannot drift apart if they read the same value.
/// `InstallKind::Php` is what keeps a PHP install out of `cancel_mysql_install`'s
/// and `cancel_mariadb_install`'s reach and vice versa.
pub(crate) const PHP_INSTALL_RUN: (InstallKind, PackageOperation) =
    (InstallKind::Php, PackageOperation::Install);

/// The `(kind, operation)` pair a MySQL **install** run is tagged with, and the
/// same pair the Databases page's Cancel button
/// ([`crate::mysql_pkg::cancel_mysql_install`]) fires on.
///
/// One definition rather than two spellings at two call sites: the button and
/// the run it is meant to stop cannot drift apart if they read the same value,
/// and — the audit F1 lesson — a run that is NOT an install is then visibly a
/// *different value* rather than an identical pair wearing different prose.
pub(crate) const MYSQL_INSTALL_RUN: (InstallKind, PackageOperation) =
    (InstallKind::Mysql, PackageOperation::Install);

/// The `(kind, operation)` pair a MySQL **datadir initialization** run is
/// tagged with ([`initialize_mysql`]).
///
/// Deliberately distinct from [`MYSQL_INSTALL_RUN`]; that distinctness is the
/// audit F1 fix, and `a_mysql_init_is_not_tagged_as_an_install` is what says so
/// — retagging an init back to `PackageOperation::Install` fails that test
/// rather than silently re-arming the cancel.
pub(crate) const MYSQL_INIT_RUN: (InstallKind, PackageOperation) =
    (InstallKind::Mysql, PackageOperation::Initialize);

/// The `(kind, operation)` pair a MariaDB **install** run is tagged with, and
/// the pair the Databases page's Cancel button
/// ([`crate::mariadb_pkg::cancel_mariadb_install`]) fires on — the MariaDB
/// mirror of [`MYSQL_INSTALL_RUN`], for the identical reason.
///
/// SECURITY (P1 MariaDB UI design D4/F1). This is not a symmetry nicety: the
/// audit F1 finding was exactly a run sharing another run's `(kind,
/// operation)` pair and differing only in a `label` that
/// [`InstallLock::abort_running_if`] does not and must not consult. Had
/// MariaDB's install shared [`MYSQL_INSTALL_RUN`] here, `cancel_mysql_install`
/// would abort a MariaDB install (and vice versa via `cancel_mariadb_install`)
/// — F1 again, with a second engine. `InstallKind::Mariadb` is what makes this
/// pair a genuinely different value rather than an identical one wearing new
/// prose.
pub(crate) const MARIADB_INSTALL_RUN: (InstallKind, PackageOperation) =
    (InstallKind::Mariadb, PackageOperation::Install);

/// The `(kind, operation)` pair a MariaDB **datadir initialization** run is
/// tagged with ([`initialize_mariadb`]) — the MariaDB mirror of
/// [`MYSQL_INIT_RUN`].
///
/// Deliberately distinct from [`MARIADB_INSTALL_RUN`], for the same audit F1
/// reason `MYSQL_INIT_RUN` is distinct from `MYSQL_INSTALL_RUN`:
/// `a_mariadb_init_is_not_tagged_as_an_install` is what says so.
pub(crate) const MARIADB_INIT_RUN: (InstallKind, PackageOperation) =
    (InstallKind::Mariadb, PackageOperation::Initialize);

/// What [`pending_install`] reports: which kind of run occupies
/// `InstallLock`'s shared slot, what it is doing, and its label — e.g. `"8.4"`
/// for a PHP install or uninstall, `"MySQL 8.4"` for a MySQL install,
/// uninstall or initialization (see the `set_running` calls in
/// `install_php`/`install_mysql`/`initialize_mysql`/`uninstall_package` for the
/// exact shapes).
///
/// `operation` exists because the quit dialog's copy — "… is still installing.
/// Quitting stops it immediately and discards the download/build in progress"
/// — is simply false for a removal, where what is at risk is a
/// half-uninstalled formula rather than a discarded download, and false again
/// for an initialization, where nothing has been downloaded and nothing is
/// half-removed. The label no longer carries that fact in prose (it used to
/// read `"MySQL 8.4 initialization"`): `operation` carries it, which is the
/// only place [`InstallLock::abort_running_if`] can see it — audit F1.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PendingInstallDto {
    pub kind: InstallKindDto,
    pub operation: PackageOperationDto,
    pub label: String,
}

/// The kind, direction, label, and abort handle of the one install/uninstall/
/// init run `InstallLock` may have in flight, so `perform_quit` can abort it
/// and `pending_install` can tell the user what they are about to lose,
/// regardless of kind.
struct RunningInstall {
    kind: InstallKind,
    operation: PackageOperation,
    label: String,
    abort: tokio::task::AbortHandle,
}

impl InstallLock {
    /// `pub(crate)`, not private: the C1 regression test drives this directly
    /// to reproduce what `install_php` does — spawn a run, record its abort
    /// handle — without going through the full IPC command.
    pub(crate) fn set_running(
        &self,
        kind: InstallKind,
        operation: PackageOperation,
        label: String,
        abort: tokio::task::AbortHandle,
    ) {
        let mut slot = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(RunningInstall {
            kind,
            operation,
            label,
            abort,
        });
    }

    fn clear_running(&self) {
        let mut slot = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = None;
    }

    /// The kind, direction and label of whatever run currently occupies the
    /// slot, if any — `None` only when nothing is running. The generalization
    /// (review fix wave Important 1) of the old PHP-only `running_php_major`,
    /// which used to `.filter(|r| r.kind == InstallKind::Php)` here and
    /// silently returned `None` for a MySQL occupant. The quit dialog's copy
    /// reads this through the [`pending_install`] command.
    pub(crate) fn running_install(&self) -> Option<(InstallKind, PackageOperation, String)> {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|r| (r.kind, r.operation, r.label.clone()))
    }

    /// A clone of the in-flight run's abort handle, if any, REGARDLESS of
    /// kind — `perform_quit` must abort a MySQL install/init exactly as
    /// eagerly as a PHP one. `AbortHandle` is cheap to clone (it is a
    /// handle, not the task), so `perform_quit` can hold its own copy and
    /// call `.abort()`/`.is_finished()` on it without disturbing whatever
    /// the owning command itself is doing with the run.
    pub(crate) fn running_abort_handle(&self) -> Option<tokio::task::AbortHandle> {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|r| r.abort.clone())
    }

    /// Abort the in-flight run **only if** it is the given kind and direction,
    /// reporting whether anything was actually aborted.
    ///
    /// The user-facing cancel (`cancel_mysql_install`) needs this rather than a
    /// bare `running_abort_handle()`: the slot is shared across PHP, MySQL,
    /// install and uninstall, and a Cancel button on the Databases page must
    /// never abort somebody else's run just because it happens to hold the
    /// lock. Check and abort happen under ONE acquisition of `running`, so the
    /// occupant cannot change between the two — reading the kind and then
    /// fetching the handle would be a real race, however narrow.
    ///
    /// Both discriminators must match — deliberately full equality on the two
    /// enums rather than "is it MySQL", so a third `InstallKind` or a third
    /// `PackageOperation` cannot start matching a cancel that was never meant
    /// for it.
    ///
    /// **`label` is deliberately NOT consulted**, and that is why audit F1 was
    /// a real hole rather than a cosmetic one: `initialize_mysql` used to tag
    /// its run with the same `(Mysql, Install)` pair `cancel_mysql_install`
    /// fires on and rely on a different label to tell them apart, so the
    /// guarantee stated above was false for exactly one caller. Runs are now
    /// tagged from the named [`MYSQL_INSTALL_RUN`]/[`MYSQL_INIT_RUN`] pairs;
    /// the tests immediately following `pending_install`'s own cover every
    /// occupant this call must refuse.
    pub(crate) fn abort_running_if(&self, kind: InstallKind, operation: PackageOperation) -> bool {
        let slot = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(running) = slot.as_ref() else {
            return false;
        };
        let is_target = running.kind == kind && running.operation == operation;
        if is_target {
            running.abort.abort();
        }
        is_target
    }
}

/// Clears `InstallLock`'s running slot when dropped, no matter which of
/// `install_php`'s several return points (normal completion, a `?` on a
/// failed rescan, an aborted/panicked task) is the one that fires. Mirrors
/// `openvhost_proc::task::KillOnDrop`'s own reasoning: a fallible sequence of
/// early returns is exactly the shape where "remember to clear this" rots the
/// first time a new early return is added, so the guarantee is a `Drop` impl
/// instead of a set of matching calls scattered through the function body.
///
/// A2 audit finding: also owns the run's own `AbortHandle` and aborts it on
/// drop, rather than only clearing the slot. There is no live bug today —
/// Tauri never cancels a command's future on its own, which is exactly why
/// `perform_quit` needs its own explicit abort-before-`window.destroy()` call
/// (see `install_php`'s doc comment and `quit.rs`) — but a bare `clear_running`
/// here inverted the OLD failure mode instead of merely lacking a new one.
/// Before this branch, dropping the command future killed brew via
/// `run_task`'s `KillOnDrop` because nothing spawned it — the future WAS the
/// run. After `install_php` started spawning the run (the C1 fix), the
/// spawned task keeps executing independent of this guard; if this guard were
/// ever dropped WITHOUT `install_task.await` above having already run it to
/// completion (a `?` on an early return added later, a panic unwinding
/// through this scope), the task would leak — AND the slot `perform_quit`
/// reads to find something to abort would already be empty, so quit would no
/// longer find it either. Aborting here closes both: `AbortHandle::abort` is
/// a no-op on an already-finished task, so this costs nothing on the normal
/// path where `install_task.await` already completed it.
///
/// `pub(crate)`: `uninstall.rs` spawns its `brew uninstall` run exactly the
/// same way and needs the identical guarantee — see its own use.
pub(crate) struct RunningInstallGuard<'a> {
    pub(crate) lock: &'a InstallLock,
    pub(crate) abort: tokio::task::AbortHandle,
}

impl Drop for RunningInstallGuard<'_> {
    fn drop(&mut self) {
        self.abort.abort();
        self.lock.clear_running();
    }
}

/// Whatever is currently installing or initializing, if anything — for the
/// quit dialog: a build/init in progress is invisible to
/// `pending_service_ids` (it is not a supervised service), so without this
/// the confirmation would silently discard it. Kind-agnostic (review fix
/// wave Important 1 — see `InstallKind`'s doc comment): PHP and MySQL both
/// surface here, and `QuitDialog` renders the sentence matching `kind`.
#[tauri::command]
#[specta::specta]
pub async fn pending_install(
    lock: tauri::State<'_, InstallLock>,
) -> Result<Option<PendingInstallDto>, IpcError> {
    Ok(lock
        .inner()
        .running_install()
        .map(|(kind, operation, label)| PendingInstallDto {
            kind: kind.into(),
            operation: operation.into(),
            label,
        }))
}

/// Install a PHP major — from OpenVHost's own package tree when this build
/// publishes one for that major on this host, and via Homebrew otherwise
/// (off-Homebrew slice 5C design D4).
///
/// **One command, one routing rule.** The alternative — two commands with the
/// page dispatching on the row's `offer` — was rejected because it puts the rule
/// in two places, which is the cross-file constant-pair shape this project has
/// been bitten by, and because it makes D4's own sentence ("the frontend does
/// not re-derive the rule") false. The route is decided HERE, by re-reading
/// `php_pkg::package_offer` — the same compiled-in table that filled the row's
/// `offer` field — so no argument a caller can supply chooses a pipeline.
///
/// **On every real machine today this is the Homebrew route**, unchanged: every
/// offer this build can make is `AwaitingRelease` or `Unavailable`, and
/// `php_pkg::route_for` sends both to Homebrew (spec §8.5 corrected, §8.6).
///
/// Every argument that reaches `brew`'s argv is validated or derived from
/// managed state before this function does anything observable: `major` is
/// parsed and checked against the catalogue allowlist, `brew` is located by
/// absolute path (never `PATH`), and `brew_install_spec` itself refuses a
/// non-absolute `brew` path. The packaged route reaches no argv at all — its URL
/// and hash come only from `openvhost_core::PHP_PACKAGES`.
#[tauri::command]
#[specta::specta]
pub async fn install_php(
    app: tauri::AppHandle,
    major: String,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<PhpInstallOutcomeDto, IpcError> {
    // Both guard layers, before anything else happens.
    let major = openvhost_core::PhpMajor::parse(&major)?;

    // One at a time. `try_lock` rather than `lock`: a second press should be
    // refused with an explanation, not silently queued behind a 20-minute
    // build.
    let Ok(_guard) = lock.inner().guard.try_lock() else {
        return Err(IpcError::Core {
            message: "an install is already running".into(),
        });
    };

    let p = stack_paths(&paths)?;

    // A compiled-in table lookup — nothing spawned, nothing fetched — so its
    // position ahead of the Homebrew route's own checks changes nothing
    // observable about that route. Matched exhaustively; a third route would
    // have to be handled here rather than inherited.
    let result = match crate::php_pkg::route_for(&crate::php_pkg::package_offer(major.as_str())) {
        crate::php_pkg::PhpInstallRoute::Package => {
            crate::php_pkg::run_package_install(
                &app,
                &major,
                p,
                lock.inner(),
                runtimes.inner(),
                sup.inner(),
            )
            .await?
        }
        crate::php_pkg::PhpInstallRoute::Homebrew => {
            run_brew_install(&app, &major, p, lock.inner(), runtimes.inner(), sup.inner()).await?
        }
    };

    Ok(PhpInstallOutcomeDto {
        major: major.as_str().to_string(),
        result,
    })
}

/// The Homebrew half of [`install_php`], moved out of it verbatim when the
/// routing arrived and otherwise untouched (spec §8.6: nothing changes on a
/// machine with Homebrew and no package tree).
///
/// Everything below — the already-installed refusal, the `find_brew` message and
/// the paths it lists, `brew_install_spec`'s own refusal, the live
/// `PhpInstallLogEvent` pump, the spawn-then-record-then-await ordering, the
/// seeded `detected`, and the rescan on a non-zero exit — is the code that was
/// here before, with the same errors on the same conditions.
///
/// The one deliberate change is the cancelled arm; see it for why.
async fn run_brew_install(
    app: &tauri::AppHandle,
    major: &openvhost_core::PhpMajor,
    p: &StackPaths,
    lock: &InstallLock,
    runtimes: &RwLock<Option<InstalledRuntimes>>,
    sup: &Supervisor,
) -> Result<crate::php_pkg::PhpInstallResultDto, IpcError> {
    let before: Vec<String> = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .as_ref()
        .map(|r| r.php.iter().map(|rt| rt.major.clone()).collect())
        .unwrap_or_default();

    if before.iter().any(|m| m == major.as_str()) {
        return Err(IpcError::Core {
            message: format!("PHP {} is already installed", major.as_str()),
        });
    }

    let brew = openvhost_core::find_brew().ok_or_else(|| IpcError::Core {
        message: format!(
            "Homebrew was not found. Looked for bin/brew under: {}",
            openvhost_core::BREW_PREFIXES.join(", ")
        ),
    })?;

    // Returns Result: it refuses a non-absolute brew path, because composing
    // PATH from one yields an empty leading component and exec resolves that
    // as the working directory.
    let spec = openvhost_core::brew_install_spec(&brew, major)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);

    // Forward brew's output as it arrives, so a long install is visibly
    // working rather than apparently hung.
    let emitter = app.clone();
    let for_event = major.as_str().to_string();
    let pump = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let openvhost_proc::TaskEvent::Line { stream, text } = ev {
                let _ = PhpInstallLogEvent {
                    major: for_event.clone(),
                    ts_ms: now_ms(),
                    stream: match stream {
                        openvhost_proc::TaskStream::Stdout => "stdout".into(),
                        openvhost_proc::TaskStream::Stderr => "stderr".into(),
                    },
                    line: text,
                }
                .emit(&emitter);
            }
        }
    });

    // Spawned rather than awaited inline — the C1 audit fix. Awaiting
    // `run_task` directly (as this used to) makes ITS future identical to
    // this command handler's own future, and Tauri never cancels an
    // in-flight command: a quit mid-install would go straight from
    // `window.destroy()` to `process::exit`, and `run_task`'s `KillOnDrop`
    // containment — the whole reason a one-shot task runner exists instead of
    // a bare `Command::spawn` — would never fire. Spawning gives an
    // `AbortHandle`, stashed on `InstallLock` for `perform_quit` to use
    // BEFORE the window goes away, so aborting here genuinely drops the
    // future and runs `KillOnDrop` for real.
    let install_task = tokio::spawn(openvhost_proc::run_task(
        openvhost_proc::default_driver(),
        spec,
        tx,
    ));
    let abort_handle = install_task.abort_handle();
    let (kind, operation) = PHP_INSTALL_RUN;
    lock.set_running(
        kind,
        operation,
        major.as_str().to_string(),
        abort_handle.clone(),
    );
    // Cleared AND aborted on every return path below via `Drop`, including
    // the two `?`s still to come — see `RunningInstallGuard`'s doc comment
    // for why that is a `Drop` impl and not a matching call at each return
    // point, and why it aborts rather than merely clearing the slot.
    let _running_guard = RunningInstallGuard {
        lock,
        abort: abort_handle,
    };

    let exit_code = match install_task.await {
        Ok(result) => result?,
        // The task's future was genuinely dropped, so `KillOnDrop` ran and
        // brew's process group is gone.
        //
        // THE ONE DELIBERATE CHANGE to this route (spec §8.6). This used to
        // return `Err(IpcError::Proc("… because the app is quitting"))`, which
        // was true when `perform_quit` was the only thing that could abort a PHP
        // run. `cancel_php_install` is a second cause as of this slice, and that
        // message would be a plain lie for it. `Cancelled` is true of both
        // causes, and it is what the MySQL and MariaDB installs already return
        // for the identical event. Nothing observable moves during a quit — the
        // window is being destroyed as this resolves — so the change is confined
        // to the cause that did not exist before.
        Err(join_err) if join_err.is_cancelled() => {
            return Ok(crate::php_pkg::PhpInstallResultDto::Cancelled);
        }
        // Any other join failure (a panic inside `run_task`) is not this
        // command's fault to hide. Left as a thrown error, unchanged: no new
        // cause reaches it, so nothing justifies moving it.
        Err(join_err) => {
            return Err(IpcError::Proc {
                message: format!("the install task ended unexpectedly: {join_err}"),
            });
        }
    };
    let _ = pump.await;

    // Rescan even on a non-zero exit: brew can fail late having already
    // linked the formula, and the truth is on disk either way.
    //
    // `detected` comes from the SEED, not from the rescan's probe. We asked
    // brew for `php@<major>`, so the question is decidable by looking at the
    // formula directory brew was supposed to create — a stat, with no third
    // "I could not tell" state hiding inside the boolean. Deriving it from a
    // version probe instead is what made a successful `brew install mysql@8.4`
    // report failure: the probe was killed at its 5 s bound during macOS's
    // ~11.5 s first-run scan of the new binary, every single time.
    let seed = openvhost_core::php_runtime_for_major(&brew_prefixes(), major);
    let detected = seed.is_some();
    // Seeded so the MANAGED state and the supervisor row are right too, not
    // just the answer: the apply pipeline reads that list, so a version missing
    // from it is a version sites cannot be applied against.
    rescan_into_state(runtimes, sup, p, seed).await?;

    Ok(crate::php_pkg::PhpInstallResultDto::Brew {
        exit_code,
        detected,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod php_ipc_tests {
    use tauri::Manager;

    use super::*;

    /// A complete discovery pass: every candidate was identified. The
    /// reconcilers care only about `runtimes`, so tests about reconciliation
    /// say so explicitly rather than leaving `unidentified` implied.
    fn found<T>(runtimes: Vec<T>) -> openvhost_core::Discovery<T> {
        openvhost_core::Discovery {
            runtimes,
            unidentified: vec![],
        }
    }

    #[test]
    fn every_catalogue_entry_is_listed_with_its_installed_state() {
        // `PhpRuntime.source` arrived with PHP discovery (off-Homebrew slice
        // 5B). Every fixture in this file already described a Homebrew keg —
        // a `…/opt/php@<major>/sbin/php-fpm` path, or a synthetic stand-in
        // for one — so `Homebrew` is the TRUTHFUL value here rather than a
        // placeholder, and no test's meaning changes: nothing below reads the
        // field. The packaged variant enters this file when the Languages
        // page learns to display the distinction (5C).
        let installed = vec![openvhost_core::PhpRuntime {
            major: "8.3".into(),
            fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Homebrew,
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[("8.3", "8.3.14")]);
        assert_eq!(rows.len(), openvhost_core::CATALOGUE.len());
        let three = rows.iter().find(|r| r.major == "8.3").unwrap();
        assert!(three.installed);
        assert_eq!(three.full_version.as_deref(), Some("8.3.14"));
        assert_eq!(three.service_id.as_deref(), Some("php-fpm-8.3"));
        assert!(
            three
                .socket_path
                .as_deref()
                .is_some_and(|s| s.ends_with("php-fpm-8.3.sock"))
        );
        let one = rows.iter().find(|r| r.major == "8.1").unwrap();
        assert!(!one.installed);
        assert!(one.path.is_none());
        assert!(
            one.service_id.is_none(),
            "a version that is not installed has no pool"
        );
    }

    #[test]
    fn the_patch_level_is_absent_rather_than_a_repeat_of_the_major() {
        // The HOMEBREW half of `full_version` (off-Homebrew slice 5C design
        // D3): our only prober returns major.minor, so a keg's patch level is
        // still unknown. Echoing the major into `full_version` would render
        // "8.3" twice and imply a patch level we never fetched.
        let installed = vec![openvhost_core::PhpRuntime {
            major: "8.3".into(),
            fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Homebrew,
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[]);
        let three = rows.iter().find(|r| r.major == "8.3").unwrap();
        assert!(three.installed);
        assert!(
            three.full_version.is_none(),
            "got {:?} — an unknown patch level must be None, not a copy of the major",
            three.full_version
        );
    }

    // ------------------------------------------------------------------
    // `full_version`, finally carrying something — for packaged rows only
    // (off-Homebrew slice 5C design D3).
    //
    // VACUITY: filling `full_version` from `major` (the trap the field's own
    // doc names) reddens
    // `a_packaged_row_reports_the_patch_version_the_tree_recorded_not_its_major`
    // and `a_packaged_rows_own_version_wins_over_a_map_keyed_only_by_major`;
    // dropping the `rt.source.version()` half back to the old
    // `found.and_then(|_| full_versions…)` reddens all four tests in this
    // group except the Homebrew one. Both were run.
    // ------------------------------------------------------------------

    /// One packaged runtime, from a tree laid out the way our own installer
    /// lays it out. The field that was `None` on every row since it was added
    /// now answers `8.4.24` — and that answer is neither the major echoed back
    /// nor a guess: it is the version directory the install chose.
    #[test]
    fn a_packaged_row_reports_the_patch_version_the_tree_recorded_not_its_major() {
        let installed = vec![openvhost_core::PhpRuntime {
            major: "8.4".into(),
            fpm_bin: PathBuf::from("/Users/x/.openvhost/packages/php/8.4/8.4.24/bin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Packaged {
                version: "8.4.24".into(),
            },
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[]);
        let four = rows.iter().find(|r| r.major == "8.4").unwrap();
        assert!(four.installed);
        assert_eq!(four.full_version.as_deref(), Some("8.4.24"));
        // The trap this field's own comment names, asserted rather than
        // assumed: a value that merely repeats `major` implies a patch level
        // was fetched when it was not.
        assert_ne!(
            four.full_version.as_deref(),
            Some(four.major.as_str()),
            "full_version must not be the major echoed back"
        );
        // And it agrees with the tree it came from, so a row cannot report a
        // version that no directory on disk has.
        assert!(
            four.path
                .as_deref()
                .is_some_and(|p| p.contains("/packages/php/8.4/8.4.24/")),
            "got {:?}",
            four.path
        );
    }

    /// **No spawn.** Slice 5B made this a compiler guarantee one layer down —
    /// `discover_packaged` takes no probe argument at all — and this is the DTO
    /// layer's half of the same claim: `php_rows` takes no probe either, and
    /// the version below is reported for a binary that does not exist on this
    /// machine, under a home that does not exist either. Anything that tried
    /// to execute `fpm_bin` to learn the patch level would fail or hang here
    /// rather than pass.
    #[test]
    fn nothing_is_executed_to_learn_a_packaged_rows_version() {
        let installed = vec![openvhost_core::PhpRuntime {
            major: "8.4".into(),
            fpm_bin: PathBuf::from(
                "/nonexistent-openvhost-test/packages/php/8.4/8.4.24/bin/php-fpm",
            ),
            source: openvhost_core::PhpRuntimeSource::Packaged {
                version: "8.4.24".into(),
            },
        }];
        assert!(
            !installed[0].fpm_bin.exists(),
            "this test is only meaningful while the binary genuinely does not exist"
        );
        let rows = php_rows(Path::new("/nonexistent-openvhost-test"), &installed, &[]);
        let four = rows.iter().find(|r| r.major == "8.4").unwrap();
        assert_eq!(four.full_version.as_deref(), Some("8.4.24"));
    }

    /// The runtime's own recorded version wins over the probe map, which is
    /// keyed only by major and so could carry a DIFFERENT install's answer.
    /// The map exists for Homebrew rows; a packaged row never needs it.
    #[test]
    fn a_packaged_rows_own_version_wins_over_a_map_keyed_only_by_major() {
        let installed = vec![openvhost_core::PhpRuntime {
            major: "8.4".into(),
            fpm_bin: PathBuf::from("/Users/x/.openvhost/packages/php/8.4/8.4.24/bin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Packaged {
                version: "8.4.24".into(),
            },
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[("8.4", "8.4.99")]);
        let four = rows.iter().find(|r| r.major == "8.4").unwrap();
        assert_eq!(four.full_version.as_deref(), Some("8.4.24"));
    }

    /// Side by side, which is the whole point of the page: the packaged row
    /// knows its patch level and the Homebrew row does not, and the second is
    /// not "broken" for saying so.
    #[test]
    fn a_homebrew_row_stays_unknown_beside_a_packaged_one_that_knows() {
        let installed = vec![
            openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            },
            openvhost_core::PhpRuntime {
                major: "8.4".into(),
                fpm_bin: PathBuf::from("/Users/x/.openvhost/packages/php/8.4/8.4.24/bin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Packaged {
                    version: "8.4.24".into(),
                },
            },
        ];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[]);
        let three = rows.iter().find(|r| r.major == "8.3").unwrap();
        let four = rows.iter().find(|r| r.major == "8.4").unwrap();
        assert!(three.installed && four.installed);
        assert_eq!(three.full_version, None);
        assert_eq!(four.full_version.as_deref(), Some("8.4.24"));
    }

    // ------------------------------------------------------------------
    // The source, on the row (off-Homebrew slice 5C design D3).
    //
    // VACUITY: hard-coding `source: Some(PhpRuntimeSourceDto::Homebrew)` in
    // `php_rows`' build closure reddens the packaged assertion below;
    // hard-coding `None` reddens both installed assertions; filling it for
    // every row (`Some(...)` regardless of `found`) reddens the uninstalled
    // one. All three were run.
    // ------------------------------------------------------------------

    #[test]
    fn a_row_says_which_install_put_its_binaries_there_and_an_empty_row_says_nothing() {
        let installed = vec![
            openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            },
            openvhost_core::PhpRuntime {
                major: "8.4".into(),
                fpm_bin: PathBuf::from("/Users/x/.openvhost/packages/php/8.4/8.4.24/bin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Packaged {
                    version: "8.4.24".into(),
                },
            },
        ];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[]);
        assert_eq!(
            rows.iter().find(|r| r.major == "8.3").unwrap().source,
            Some(crate::php_pkg::PhpRuntimeSourceDto::Homebrew)
        );
        assert_eq!(
            rows.iter().find(|r| r.major == "8.4").unwrap().source,
            Some(crate::php_pkg::PhpRuntimeSourceDto::Packaged {
                version: "8.4.24".into()
            })
        );
        let uninstalled = rows.iter().find(|r| r.major == "8.1").unwrap();
        assert!(!uninstalled.installed);
        assert_eq!(
            uninstalled.source, None,
            "a row with nothing installed has no provenance to report"
        );
    }

    // ------------------------------------------------------------------
    // The offer, on the row and PER MAJOR (design D1).
    //
    // VACUITY: replacing `package_offer(major)` with `package_offer("8.4")`
    // reddens `the_offer_is_answered_per_major_not_once_for_the_whole_page`;
    // replacing it with a constant `Unavailable` reddens the AwaitingRelease
    // test. Both were run.
    // ------------------------------------------------------------------

    /// PHP's headline feature is several majors side by side, so one offer for
    /// the whole page would be wrong for every row but one. On this machine
    /// today 8.4 is pinned-but-unpublished while 8.1 has no artifact at all,
    /// and the rows say so separately.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_offer_is_answered_per_major_not_once_for_the_whole_page() {
        let rows = php_rows(Path::new("/tmp/ovh"), &[], &[]);
        let four = &rows.iter().find(|r| r.major == "8.4").unwrap().offer;
        let one = &rows.iter().find(|r| r.major == "8.1").unwrap().offer;
        assert_ne!(four, one, "two majors cannot share one page-wide answer");
    }

    /// **What an `AwaitingRelease` row actually carries**, which is what every
    /// non-absence offer resolves to today: a release tag a human has to
    /// publish, and no version a user could install. This is the state the
    /// page must render, and the reason this slice's packaged install path
    /// merges unexercised.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_one_pinned_major_carries_the_tag_it_is_waiting_on_and_offers_no_install() {
        let rows = php_rows(Path::new("/tmp/ovh"), &[], &[]);
        let four = rows.iter().find(|r| r.major == "8.4").unwrap();
        assert_eq!(
            four.offer,
            crate::php_pkg::PhpPackageOfferDto::AwaitingRelease {
                tag: "php-8.4.24".into()
            }
        );
        // The row is still cataloged — Homebrew can install this major today,
        // and `cataloged` is what gates that affordance. The offer is a
        // SEPARATE fact: there are no bytes of our own to fetch yet.
        assert!(four.cataloged);
    }

    /// A hand-installed major carries an offer too — an absence naming the
    /// target — rather than a missing field the page has to special-case.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn a_row_outside_the_catalogue_carries_an_absence_rather_than_no_offer() {
        let installed = vec![openvhost_core::PhpRuntime {
            major: "7.4".into(),
            fpm_bin: PathBuf::from("/opt/homebrew/opt/php@7.4/sbin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Homebrew,
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[]);
        let hand_installed = rows.iter().find(|r| r.major == "7.4").unwrap();
        assert!(!hand_installed.cataloged);
        assert_eq!(
            hand_installed.offer,
            crate::php_pkg::PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into()
            }
        );
    }

    #[test]
    fn only_newly_discovered_majors_are_registered() {
        // Re-registering an existing row replaces it, which wipes a Failed row's
        // stderr and exit code — the reason a user would be looking at it.
        let before = ["8.3".to_string(), "8.5".to_string()];
        let found = ["8.3".to_string(), "8.4".to_string(), "8.5".to_string()];
        assert_eq!(
            newly_installed_majors(&before, &found),
            vec!["8.4".to_string()]
        );
    }

    #[test]
    fn a_rescan_that_finds_nothing_new_registers_nothing() {
        let before = ["8.3".to_string()];
        let found = ["8.3".to_string()];
        assert!(newly_installed_majors(&before, &found).is_empty());
    }

    #[test]
    fn a_version_that_disappeared_is_not_treated_as_new() {
        // brew uninstall outside the app: 8.3 is gone from `found`. Nothing to
        // REGISTER — the subtraction half is `vanished_service_ids` below
        // (package-uninstall design D5), not this function's job.
        let before = ["8.3".to_string(), "8.5".to_string()];
        let found = ["8.5".to_string()];
        assert!(newly_installed_majors(&before, &found).is_empty());
    }

    // ---- D5: a rescan converges with an external `brew uninstall` --------
    //
    // VACUITY (RED first): written before `vanished_service_ids` existed —
    // they did not compile. Then neutered: replacing `strip_prefix` with
    // `starts_with` + a `contains` check made
    // `a_php_rescan_never_matches_a_mysql_or_nginx_row` fail by returning
    // `mysql-8.4`, and dropping the `unregister_vanished` call from
    // `rescan_into_state` made the supervisor-level test below fail.

    #[test]
    fn a_major_that_vanished_from_disk_is_the_one_that_gets_forgotten() {
        let registered = [
            "php-fpm-8.1".to_string(),
            "php-fpm-8.3".to_string(),
            "php-fpm-8.4".to_string(),
        ];
        let found = ["8.1".to_string(), "8.4".to_string()];
        assert_eq!(
            vanished_service_ids(&registered, crate::stack::PHP_FPM_ID_PREFIX, &found),
            vec!["php-fpm-8.3".to_string()]
        );
    }

    #[test]
    fn a_rescan_that_finds_everything_forgets_nothing() {
        let registered = ["php-fpm-8.4".to_string()];
        let found = ["8.4".to_string()];
        assert!(
            vanished_service_ids(&registered, crate::stack::PHP_FPM_ID_PREFIX, &found).is_empty()
        );
    }

    #[test]
    fn a_php_rescan_never_matches_a_mysql_or_nginx_row() {
        // Over-matching here would let a PHP rescan unregister the user's
        // database — the worst possible outcome of a "tidy up" pass.
        let registered = [
            "nginx".to_string(),
            "demo-ticker".to_string(),
            "mysql-8.4".to_string(),
            "php-fpm-8.4".to_string(),
        ];
        assert!(
            vanished_service_ids(
                &registered,
                crate::stack::PHP_FPM_ID_PREFIX,
                &["8.4".into()]
            )
            .is_empty()
        );
        // ...and with NOTHING installed, still only this family's rows.
        assert_eq!(
            vanished_service_ids(&registered, crate::stack::PHP_FPM_ID_PREFIX, &[]),
            vec!["php-fpm-8.4".to_string()]
        );
        assert_eq!(
            vanished_service_ids(&registered, crate::stack::MYSQL_ID_PREFIX, &[]),
            vec!["mysql-8.4".to_string()]
        );
    }

    /// The WIRING, not just the helper: a rescan whose discovery no longer
    /// returns 8.3 must leave no `php-fpm-8.3` row behind — the observable
    /// state an external `brew uninstall` has to converge on (D5).
    ///
    /// Drives `reconcile_php`, the half of `rescan_into_state` that does not
    /// probe. Going through `rescan_into_state` itself would probe the
    /// developer's own Homebrew prefixes and assert nothing reproducible.
    #[test]
    fn a_rescan_registers_what_appeared_and_forgets_what_vanished() {
        let home = tempfile::tempdir().unwrap();
        let paths = StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        };
        let sup = Supervisor::new(openvhost_proc::default_driver());
        let rt = |major: &str| openvhost_core::PhpRuntime {
            major: major.to_string(),
            fpm_bin: home.path().join(format!("php-fpm-{major}")),
            source: openvhost_core::PhpRuntimeSource::Homebrew,
        };

        // Before: 8.3 installed and registered.
        let runtimes = RwLock::new(None);
        reconcile_php(&runtimes, &sup, &paths, found(vec![rt("8.3")])).unwrap();
        assert_eq!(
            sup.snapshot().iter().map(|s| &s.id).collect::<Vec<_>>(),
            vec!["php-fpm-8.3"]
        );

        // After: 8.3 gone from disk, 8.4 appeared.
        reconcile_php(&runtimes, &sup, &paths, found(vec![rt("8.4")])).unwrap();

        assert_eq!(
            sup.snapshot().iter().map(|s| &s.id).collect::<Vec<_>>(),
            vec!["php-fpm-8.4"],
            "the vanished major's row must be gone and the new one present"
        );
        let listed: Vec<String> = runtimes
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .php
            .iter()
            .map(|r| r.major.clone())
            .collect();
        assert_eq!(listed, vec!["8.4".to_string()]);
    }

    // ---- the RESCAN seam reads the package tree (D2) ---------------------
    //
    // The symmetrical twin of `stack.rs`'s startup-seam pair, and the reason
    // it has to exist separately: `openvhost-core` owns the merge rules and
    // tests them thoroughly, `stack.rs` proves STARTUP reads
    // `<home>/packages/php/`, and neither of those says anything about the
    // OTHER seam. `rescan_into_state` hands `paths.home` to `discover_all_php`,
    // which mints the `PackagesRoot` from it and from nothing else. A refactor
    // that gave that call site a stale or empty home would compile, pass every
    // other test in the workspace, and make a freshly installed packaged
    // runtime vanish from the Languages page the moment the user pressed Check
    // again — while startup still listed it. That disagreement between two
    // views of the same machine is the C2 class of bug exactly.
    //
    // Driven through `rescan_into_state` rather than `discover_all_php`
    // directly, because the argument under test is the one the RESCAN supplies,
    // not one a test supplies. The sibling tests above avoid `rescan_into_state`
    // because it probes the developer's own Homebrew and asserts nothing
    // reproducible; that reasoning still holds for them, and these two work
    // around it by asserting only about the packaged entry — which is
    // machine-independent, because packaged wins per major, so whatever brew
    // contributes can neither remove our entry nor outrank it.

    /// `<home>/packages/php/<major>/<version>/bin/php-fpm` plus a relative
    /// `current` symlink, exactly as `openvhost-pkg` leaves it. Mirrors
    /// `stack.rs`'s `install_fake_php_package` + `point_current`, including
    /// `bin/` rather than brew's `sbin/` — the packaged walk refuses the brew
    /// shape, so a fixture written the other way would prove the wrong thing.
    #[cfg(unix)]
    fn install_fake_php_package(home: &Path, major: &str, version: &str, body: &str) {
        let root = openvhost_core::PackagesRoot::from_home(home);
        let bin = root
            .package_dir(openvhost_core::PHP_PACKAGE_NAME, major, version)
            .join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir package bin");
        std::fs::write(bin.join("php-fpm"), body.as_bytes()).expect("write fake php-fpm");
        let link = root.current_link(openvhost_core::PHP_PACKAGE_NAME, major);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(PathBuf::from(version), &link).expect("symlink current");
    }

    #[cfg(unix)]
    fn rescan_paths(home: &Path) -> StackPaths {
        StackPaths {
            home: home.to_path_buf(),
            nginx_bin: Some(home.join("nginx")),
            nginx_conf: home.join("nginx.conf"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_rescan_reads_the_package_tree_of_the_home_it_was_given() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        // A version string no Homebrew keg can produce, so "this came from the
        // fixture" is not an inference.
        install_fake_php_package(home, "8.4", "8.4.24", "8.4.24 fpm\n");

        let runtimes = RwLock::new(None);
        let sup = Supervisor::new(openvhost_proc::default_driver());
        let found = rescan_into_state(&runtimes, &sup, &rescan_paths(home), None)
            .await
            .unwrap();

        let ours: Vec<_> = found
            .runtimes
            .iter()
            .filter(|r| r.fpm_bin.starts_with(home))
            .collect();
        assert_eq!(ours.len(), 1, "got {:?}", found.runtimes);
        assert_eq!(ours[0].major, "8.4");
        assert_eq!(
            ours[0].source,
            openvhost_core::PhpRuntimeSource::Packaged {
                version: "8.4.24".to_string()
            },
            "the rescan must report where the runtime came from"
        );
        // D3 at this seam too: the concrete version directory, never `current`.
        assert!(
            !ours[0]
                .fpm_bin
                .components()
                .any(|c| c.as_os_str() == "current"),
            "the rescan handed out a path through the current link: {:?}",
            ours[0].fpm_bin
        );

        // The reconcile half ran on it: a supervisor row exists for the major,
        // and the spec it would spawn is the packaged binary — not a brew one.
        let row = sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == "php-fpm-8.4")
            .expect("the packaged major must be registered");
        assert_eq!(row.id, "php-fpm-8.4");
        let listed = runtimes.read().unwrap().clone().unwrap().php;
        assert!(
            listed
                .iter()
                .any(|r| r.fpm_bin == ours[0].fpm_bin && r.major == "8.4"),
            "the managed runtime list does not carry the packaged entry: {listed:?}"
        );
    }

    /// The other half of the same claim, and what makes the test above about
    /// the ARGUMENT rather than about this machine: the identical call against
    /// a home with no package tree reports nothing packaged. Without it, a
    /// `discover_all_php` that ignored its `home` and read some ambient
    /// location would still have to fail — but only if that ambient location
    /// happened to be empty, which is not something a test should rely on.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_same_rescan_against_a_home_with_no_package_tree_finds_nothing_packaged() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        assert!(
            !openvhost_core::PackagesRoot::from_home(home)
                .as_path()
                .exists(),
            "this test's whole point is a home with no package tree"
        );

        let runtimes = RwLock::new(None);
        let sup = Supervisor::new(openvhost_proc::default_driver());
        let found = rescan_into_state(&runtimes, &sup, &rescan_paths(home), None)
            .await
            .unwrap();

        assert!(
            found
                .runtimes
                .iter()
                .all(|r| r.source == openvhost_core::PhpRuntimeSource::Homebrew),
            "a packaged runtime appeared from somewhere other than this home: {:?}",
            found.runtimes
        );
        assert!(
            !found.runtimes.iter().any(|r| r.fpm_bin.starts_with(home)),
            "a runtime was reported under an empty home: {:?}",
            found.runtimes
        );
    }

    // ---- the install seed (fix R2, part 1) -------------------------------
    //
    // The seam between "the install knows what it asked brew for" and "the
    // rescan reconciles what it can see". Driven through `seeded_*` +
    // `reconcile_*` rather than the whole command, for the same reason the
    // rescan tests above are: `rescan_*_into_state` probes the developer's own
    // Homebrew and asserts nothing reproducible.

    #[test]
    fn a_seeded_install_is_registered_even_when_the_probe_told_us_nothing() {
        // THE R2 failure, reproduced as a unit: brew installed the version,
        // discovery could not identify it (probe killed at its 5 s bound during
        // macOS's ~11.5 s first-run scan), and the app used to conclude
        // "nothing installed" — leaving the managed runtime list empty, so the
        // apply pipeline answered `MissingRuntime` for a version that was
        // plainly there.
        //
        // VACUITY: replacing `seeded_php`'s body with `discovered` makes both
        // assertions fail — no row, and an empty runtime list.
        let home = tempfile::tempdir().unwrap();
        let paths = StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        };
        let sup = Supervisor::new(openvhost_proc::default_driver());
        let runtimes = RwLock::new(None);

        let candidate = home.path().join("opt/php@8.4");
        let could_not_tell = openvhost_core::Discovery {
            runtimes: vec![],
            unidentified: vec![candidate.clone()],
        };
        let seed = openvhost_core::PhpRuntime {
            major: "8.4".to_string(),
            fpm_bin: candidate.join("sbin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Homebrew,
        };

        let reconciled = reconcile_php(
            &runtimes,
            &sup,
            &paths,
            seeded_php(could_not_tell, Some(seed)),
        )
        .unwrap();

        assert_eq!(
            sup.snapshot().iter().map(|s| &s.id).collect::<Vec<_>>(),
            vec!["php-fpm-8.4"],
            "an install this app performed must get its row"
        );
        let listed: Vec<String> = runtimes
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .php
            .iter()
            .map(|r| r.major.clone())
            .collect();
        assert_eq!(listed, vec!["8.4".to_string()]);
        // And the candidate is no longer outstanding: the install identified it.
        assert!(
            reconciled.is_complete(),
            "got {:?}",
            reconciled.unidentified
        );
    }

    #[test]
    fn a_seed_never_displaces_what_discovery_found_for_itself() {
        // Discovery applies the prefix-priority and alias rules; the seed is a
        // naive first-prefix hit. Letting it win would move a runtime from a
        // native keg onto a Rosetta one.
        let discovered = openvhost_core::Discovery {
            runtimes: vec![openvhost_core::PhpRuntime {
                major: "8.4".to_string(),
                fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.4/sbin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            }],
            unidentified: vec![],
        };
        let seeded = seeded_php(
            discovered,
            Some(openvhost_core::PhpRuntime {
                major: "8.4".to_string(),
                fpm_bin: PathBuf::from("/usr/local/opt/php@8.4/sbin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            }),
        );
        assert_eq!(seeded.runtimes.len(), 1, "got {seeded:?}");
        assert_eq!(
            seeded.runtimes[0].fpm_bin,
            PathBuf::from("/opt/homebrew/opt/php@8.4/sbin/php-fpm")
        );
    }

    #[test]
    fn no_seed_leaves_a_discovery_exactly_as_it_was() {
        // Every path but an install passes `None`, and it must be inert there.
        let discovered = openvhost_core::Discovery {
            runtimes: vec![openvhost_core::PhpRuntime {
                major: "8.4".to_string(),
                fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.4/sbin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            }],
            unidentified: vec![PathBuf::from("/opt/homebrew/opt/php@8.1")],
        };
        assert_eq!(seeded_php(discovered.clone(), None), discovered);
    }

    #[test]
    fn a_mysql_seed_behaves_the_same_way() {
        // The family the failure was measured on. `MysqlRuntime` is built
        // through the real `discover_mysql` against a fake prefix — the same
        // discipline `mysql_rows_still_lists_an_out_of_catalogue_installed_major`
        // uses — rather than a private constructor.
        let prefix = tempfile::tempdir().unwrap();
        let bin_dir = prefix.path().join("opt").join("mysql@8.4").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        for name in ["mysqld", "mysql", "mysqladmin"] {
            std::fs::write(bin_dir.join(name), b"#!/bin/sh\n").unwrap();
        }
        let seed = openvhost_core::mysql::brew_mysql_runtime_for_major(
            &[prefix.path()],
            &openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap(),
        )
        .expect("the formula directory is right there");

        let could_not_tell = openvhost_core::Discovery {
            runtimes: vec![],
            unidentified: vec![prefix.path().join("opt/mysql@8.4")],
        };
        let seeded = seeded_mysql(could_not_tell, Some(seed));
        assert_eq!(seeded.runtimes.len(), 1, "got {seeded:?}");
        assert_eq!(seeded.runtimes[0].major.as_str(), "8.4");
        assert!(seeded.is_complete(), "got {:?}", seeded.unidentified);
    }

    /// The MySQL mirror. A row is only ever REGISTERED for an initialized
    /// datadir (spec D6), but forgetting one whose binaries vanished is
    /// unconditional — otherwise the Databases page keeps a row nothing can
    /// start, which is exactly the divergence D5 ends.
    #[test]
    fn a_mysql_rescan_forgets_a_row_whose_major_vanished() {
        let home = tempfile::tempdir().unwrap();
        let sup = Supervisor::new(openvhost_proc::default_driver());
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        sup.register(crate::stack::mysql_spec(
            home.path(),
            &openvhost_core::mysql::MysqlRuntime {
                major,
                mysqld: home.path().join("mysqld"),
                mysql: home.path().join("mysql"),
                mysqladmin: home.path().join("mysqladmin"),
                source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
            },
        ));
        assert_eq!(sup.snapshot().len(), 1);

        let runtimes = RwLock::new(None);
        reconcile_mysql(&runtimes, &sup, home.path(), found(vec![])).unwrap();

        assert!(sup.snapshot().is_empty(), "got {:?}", sup.snapshot());
    }

    #[test]
    fn a_terminal_row_for_a_vanished_major_is_removed_from_the_supervisor() {
        let sup = Supervisor::new(openvhost_proc::default_driver());
        sup.register(crate::stack::php_fpm_spec(
            Path::new("/tmp/ovh"),
            &openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: PathBuf::from("/nonexistent/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            },
        ));
        assert_eq!(sup.snapshot().len(), 1);

        unregister_vanished(&sup, crate::stack::PHP_FPM_ID_PREFIX, &["8.4".to_string()]);

        assert!(
            sup.snapshot().is_empty(),
            "a row whose major is gone must not survive a rescan: {:?}",
            sup.snapshot()
        );
    }

    /// D5's "expect a `NotTerminal` for a major whose pool is still running,
    /// and log it rather than treating it as impossible". A rescan must never
    /// fail — or forget a live child — because of one stubborn row.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_live_row_for_a_vanished_major_is_left_alone_rather_than_forgotten() {
        let sup = Supervisor::new(openvhost_proc::default_driver());
        sup.register(openvhost_proc::ServiceSpec {
            id: "php-fpm-8.3".into(),
            display_name: "PHP-FPM 8.3".into(),
            endpoint: None,
            spawn: openvhost_proc::SpawnSpec {
                program: PathBuf::from("/bin/sleep"),
                args: vec![OsString::from("30")],
                cwd: None,
                env: vec![],
            },
            readiness: openvhost_proc::ReadinessProbe::default(),
            grace: openvhost_proc::DEFAULT_GRACE,
        });
        sup.start("php-fpm-8.3").expect("start");
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

        unregister_vanished(&sup, crate::stack::PHP_FPM_ID_PREFIX, &[]);

        assert_eq!(
            sup.snapshot().len(),
            1,
            "the supervisor must never forget a child it is still supervising"
        );
        let _ = sup.stop("php-fpm-8.3");
    }

    #[test]
    fn exactly_one_catalogue_entry_is_recommended_and_it_is_the_newest() {
        // A first-time user should not have to know how 8.1 differs from 8.5.
        let rows = php_rows(Path::new("/tmp/ovh"), &[], &[]);
        let rec: Vec<&str> = rows
            .iter()
            .filter(|r| r.recommended)
            .map(|r| r.major.as_str())
            .collect();
        assert_eq!(rec, vec![*openvhost_core::CATALOGUE.last().unwrap()]);
    }

    #[test]
    fn an_installed_version_outside_the_catalogue_is_still_listed() {
        // Otherwise a version installed by hand — or dropped from a later
        // catalogue — vanishes from the page while still serving sites.
        let installed = vec![openvhost_core::PhpRuntime {
            major: "7.4".into(),
            fpm_bin: PathBuf::from("/opt/homebrew/opt/php@7.4/sbin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Homebrew,
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[("7.4", "7.4.33")]);
        assert!(rows.iter().any(|r| r.major == "7.4" && r.installed));
    }

    #[test]
    fn every_row_says_whether_this_build_offers_that_major() {
        // The row must carry this, because the CATALOGUE is a Rust constant
        // and the page has to hide both Install and Uninstall for a major
        // neither spec builder will compose a command for
        // (`php::brew::cataloged` refuses both). The MySQL page has had this
        // since its own slice; PHP simply never got told.
        //
        // VACUITY: hard-coding `cataloged: true` in `php_rows`' build closure
        // fails the 7.4 assertion; hard-coding `false` fails the 8.4 one.
        let installed = vec![openvhost_core::PhpRuntime {
            major: "7.4".into(),
            fpm_bin: PathBuf::from("/opt/homebrew/opt/php@7.4/sbin/php-fpm"),
            source: openvhost_core::PhpRuntimeSource::Homebrew,
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[]);
        let hand_installed = rows.iter().find(|r| r.major == "7.4").unwrap();
        assert!(hand_installed.installed);
        assert!(
            !hand_installed.cataloged,
            "a hand-installed 7.4 must render with no Install/Uninstall affordance"
        );
        // And every catalogue row says so — checked against the constant, not
        // against a copy of it, so adding a major cannot leave one row lying.
        for major in openvhost_core::CATALOGUE {
            let row = rows.iter().find(|r| r.major == major).unwrap();
            assert!(row.cataloged, "{major} is in the catalogue");
        }
    }

    #[test]
    fn a_rejected_version_names_the_field_so_the_ui_can_mark_it() {
        let e: IpcError = openvhost_core::PhpMajor::parse("--build-from-source")
            .unwrap_err()
            .into();
        match e {
            IpcError::Validation { field, .. } => assert_eq!(field, "php_version"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    /// `brew_searched_paths` is what the UI names as "looked here" — pinned
    /// against the exact prefixes rather than re-deriving the same join, so
    /// a typo in the join expression fails a test instead of only ever
    /// showing up in a manual read of the source.
    #[test]
    fn brew_searched_paths_names_bin_brew_under_every_prefix() {
        let searched = brew_searched_paths();
        assert_eq!(searched.len(), openvhost_core::BREW_PREFIXES.len());
        for prefix in openvhost_core::BREW_PREFIXES {
            assert!(
                searched.contains(&format!("{prefix}/bin/brew")),
                "expected {prefix}/bin/brew in {searched:?}"
            );
        }
    }

    /// H1 audit finding: `rescan_php_runtimes` must serialize against an
    /// in-flight `install_php` run rather than reading-modifying-writing the
    /// managed `RwLock` unguarded. Proven here by holding the SAME
    /// `InstallLock` guard `install_php` would hold, and asserting the
    /// spawned rescan cannot complete until that guard is released.
    ///
    /// Without the fix, this test does not fail loudly — it is a race the
    /// old code simply never lost in a single-threaded test, which is exactly
    /// how the bug survived review. What it proves instead is the mechanism:
    /// `rescan_php_runtimes` now genuinely blocks on `InstallLock`, which is
    /// what closes the "Check again races a completed install" window the
    /// audit describes.
    #[tokio::test]
    async fn rescan_blocks_while_an_install_holds_the_lock() {
        let home = tempfile::tempdir().expect("tempdir");
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        app.manage(RwLock::new(None::<InstalledRuntimes>));
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(Arc::new(Supervisor::new(openvhost_proc::default_driver())));
        app.manage(InstallLock::default());
        // Managed since the rescan started reporting the resolved default PHP
        // alongside the rows (default-PHP T2). A fresh in-memory database has
        // no preference row, which is what this test's machine looks like and
        // what every real machine looks like — the state under test here is
        // still the LOCK, not the preference.
        manage_db(&app, Db::open_in_memory().await.expect("in-memory db"));

        // Hold the guard the way `install_php`'s `try_lock` would while a
        // build is running.
        let lock = app.state::<InstallLock>();
        let held = lock.inner().guard.lock().await;

        let handle = app.handle().clone();
        let task = tokio::spawn(async move {
            rescan_php_runtimes(
                handle.state::<DbHandle>(),
                handle.state::<RwLock<Option<InstalledRuntimes>>>(),
                handle.state::<Option<StackPaths>>(),
                handle.state::<Arc<Supervisor>>(),
                handle.state::<InstallLock>(),
            )
            .await
        });

        // Give the spawned rescan every chance to (wrongly) finish anyway.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !task.is_finished(),
            "rescan_php_runtimes must not complete while InstallLock is held"
        );

        drop(held);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("rescan did not unblock after the install lock was released")
            .expect("rescan task panicked");
        assert!(result.is_ok(), "got {result:?}");
    }

    // ---- DEGRADE: the Languages page still has its data with no store ----
    //
    // Optional-state.db design D2. `php_environment` is what the Languages
    // page mounts on, and it is where the slice's motivating screenshot came
    // from: with the bare `Db` unmanaged, Tauri refused the whole command and
    // the page rendered "You must call `.manage()` before using this command."
    //
    // Vacuity for this pair: `read_default_php`'s `None` arm is the ONLY
    // difference between them, and the two tests disagree on `default_php`
    // while agreeing on every other field. Proven by mutation — swapping
    // `db.optional()` for `db.require()?` reddens the store-down test (an
    // `IpcError::Core`, no rows at all) and leaves the stored-default one
    // green; hardcoding `read_default_php(None, …)` does the reverse.

    #[tokio::test]
    async fn php_environment_still_lists_its_runtimes_with_the_store_down() {
        let home = tempfile::tempdir().expect("tempdir");
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        manage_store_down(&app);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: Some(home.path().join("nginx")),
            php: vec![openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: home.path().join("php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            }],
        })));

        let env = php_environment(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
        )
        .await
        .expect("a degraded store must not cost the page its rows");

        // The real work, all of which is a filesystem fact rather than a
        // stored one, and all of which survives.
        assert_eq!(
            env.runtimes.len(),
            openvhost_core::CATALOGUE.len(),
            "the page lost its rows: {:?}",
            env.runtimes
        );
        let three = env
            .runtimes
            .iter()
            .find(|r| r.major == "8.3")
            .expect("a row for the installed major");
        assert!(
            three.installed,
            "the installed runtime must still report as installed"
        );
        assert_eq!(
            three.path.as_deref(),
            Some(home.path().join("php-fpm").display().to_string().as_str())
        );
        assert!(
            !env.brew_searched.is_empty(),
            "the search paths are a probe"
        );

        // The one field a degraded store costs, and it costs it in the shape
        // `read_default_php`'s own `None` arm documents: no preference, so the
        // first installed major is what gets served. D5's banner is what makes
        // that honest rather than quiet.
        assert_eq!(
            env.default_php,
            DefaultPhpDto::Unset {
                serving: "8.3".to_string()
            }
        );
    }

    /// The discriminating twin: the SAME command against a store that holds a
    /// preference must report that preference, so the test above is measuring
    /// the `None` arm rather than a `default_php` that never depended on the
    /// store at all.
    #[tokio::test]
    async fn php_environment_reports_a_stored_default_when_the_store_is_up() {
        let home = tempfile::tempdir().expect("tempdir");
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        let db = Db::open_in_memory().await.expect("in-memory db");
        let php = vec![
            openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: home.path().join("php-fpm-8.3"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            },
            openvhost_core::PhpRuntime {
                major: "8.4".into(),
                fpm_bin: home.path().join("php-fpm-8.4"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            },
        ];
        write_default_php(&db, Some("8.4".into()), &php)
            .await
            .expect("store a preference");
        manage_db(&app, db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: Some(home.path().join("nginx")),
            php,
        })));

        let env = php_environment(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
        )
        .await
        .expect("a healthy store answers");

        assert_eq!(
            env.default_php,
            DefaultPhpDto::Preferred {
                major: "8.4".to_string()
            },
            "the stored preference must reach the page when there IS a store"
        );
    }

    /// The other one-line DEGRADE, and the one that actually probes. Asserts
    /// nothing about WHICH runtimes are found — this test's machine may have
    /// Homebrew PHP on it, which is why `rescan_blocks_while_an_install_holds_the_lock`
    /// above asserts only `is_ok()` too. What it does assert is
    /// machine-independent and is the whole claim: the command answers, it
    /// answers with the catalogue, and with no store to read a preference from
    /// it can never report one.
    #[tokio::test]
    async fn rescan_php_runtimes_still_answers_with_the_store_down() {
        let home = tempfile::tempdir().expect("tempdir");
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        manage_store_down(&app);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(RwLock::new(None::<InstalledRuntimes>));
        app.manage(Arc::new(Supervisor::new(openvhost_proc::default_driver())));
        app.manage(InstallLock::default());

        let env = rescan_php_runtimes(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            app.state::<Arc<Supervisor>>(),
            app.state::<InstallLock>(),
        )
        .await
        .expect("\"Check again\" must still work when the store is down");

        assert!(
            env.runtimes.len() >= openvhost_core::CATALOGUE.len(),
            "every catalogued major must still get a row: {:?}",
            env.runtimes
        );
        assert!(
            !matches!(env.default_php, DefaultPhpDto::Preferred { .. }),
            "with no store there is no preference to report, got {:?}",
            env.default_php
        );
    }

    /// A2 audit finding: `RunningInstallGuard::drop` used to clear
    /// `InstallLock`'s slot without aborting the run it was tracking. There is
    /// no real supervisor or brew process involved in reproducing this — an
    /// intentionally never-settling task stands in for "the run", and
    /// dropping the guard before that task's own future ever completes is
    /// exactly the shape the fix targets (in production this only happens via
    /// an early `?` return or a panic unwinding between `set_running` and
    /// `install_task.await`, but `Drop` cannot tell those apart from an
    /// explicit `drop(guard)`, so this test exercises the same code path
    /// directly). Before the fix, this task would still be running (and
    /// `result` would be `Ok(())` from a `.recv()`/timeout race, never
    /// `Err(cancelled)`) after the guard went out of scope.
    #[tokio::test]
    async fn dropping_the_running_install_guard_aborts_the_task_it_tracks() {
        let lock = InstallLock::default();
        let task = tokio::spawn(std::future::pending::<()>());
        let abort = task.abort_handle();
        lock.set_running(
            InstallKind::Php,
            PackageOperation::Install,
            "8.4".to_string(),
            abort.clone(),
        );

        {
            let _guard = RunningInstallGuard { lock: &lock, abort };
            // Dropped at the end of this block, same as `install_php`'s
            // `_running_guard` at the end of its own function body.
        }

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("task did not settle after the guard was dropped");
        match result {
            Err(join_err) => assert!(
                join_err.is_cancelled(),
                "expected the guard's Drop impl to cancel the task, got {join_err:?}"
            ),
            Ok(()) => {
                panic!("expected the guard's Drop impl to abort the task, but it ran to completion")
            }
        }
        assert!(
            lock.running_install().is_none(),
            "expected the guard's Drop impl to also clear the running slot"
        );
    }

    // -------------------------------------------------------------------
    // InstallLock generalization proof (P1 MySQL lifecycle design decision
    // 5): PHP and MySQL installs share ONE lock — a second install of
    // EITHER kind must be rejected while the other is running.
    // -------------------------------------------------------------------

    /// The same generalization, one axis further out (package-uninstall
    /// design D1: "one lock means an install and an uninstall can never
    /// interleave"). An uninstall that ran while an install was mid-build
    /// would have brew fighting itself over the same Cellar.
    ///
    /// Asserts the mechanism `uninstall_package` uses, the way the two tests
    /// below assert `install_php`'s and `install_mysql`'s — the commands
    /// themselves take a `tauri::AppHandle` (`Wry`), which
    /// `tauri::test::mock_builder` cannot produce, so none of the three can be
    /// invoked directly from a test.
    #[tokio::test]
    async fn an_uninstall_is_rejected_while_an_install_holds_the_lock() {
        let lock = InstallLock::default();
        let held = lock.guard.lock().await;
        lock.set_running(
            InstallKind::Php,
            PackageOperation::Install,
            "8.4".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );

        assert!(
            lock.guard.try_lock().is_err(),
            "uninstall_package's try_lock must fail while an install holds the guard"
        );

        drop(held);
        assert!(
            lock.guard.try_lock().is_ok(),
            "and must succeed once the install is done"
        );
    }

    /// The mirror image: an install must be refused while an uninstall is in
    /// flight, and the quit dialog must see the occupant tagged as a REMOVAL
    /// rather than as an install.
    #[tokio::test]
    async fn an_install_is_rejected_while_an_uninstall_holds_the_lock() {
        let lock = InstallLock::default();
        let held = lock.guard.lock().await;
        lock.set_running(
            InstallKind::Php,
            PackageOperation::Uninstall,
            "8.4".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );

        assert!(
            lock.guard.try_lock().is_err(),
            "install_php's try_lock must fail while an uninstall holds the guard"
        );
        assert_eq!(
            lock.running_install(),
            Some((
                InstallKind::Php,
                PackageOperation::Uninstall,
                "8.4".to_string()
            ))
        );
        assert!(lock.running_abort_handle().is_some());

        drop(held);
    }

    /// Mirrors `rescan_blocks_while_an_install_holds_the_lock` above, but
    /// holds the guard the way `install_mysql` would (a DIFFERENT kind) and
    /// asserts `install_php`'s own `try_lock` guard refuses to proceed —
    /// proving the generalization actually serializes across kinds, not just
    /// within PHP's own commands as before.
    #[tokio::test]
    async fn a_php_install_is_rejected_while_a_mysql_install_holds_the_lock() {
        let lock = InstallLock::default();
        let held = lock.guard.lock().await;
        lock.set_running(
            InstallKind::Mysql,
            PackageOperation::Install,
            "MySQL 8.4".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );

        assert!(
            lock.guard.try_lock().is_err(),
            "install_php's try_lock must fail while a MySQL install holds the guard"
        );
        // The kind-agnostic quit-dialog signal must see the MySQL occupant,
        // correctly tagged (review fix wave Important 1 — the old PHP-only
        // filter hid a MySQL label entirely instead of tagging it).
        assert_eq!(
            lock.running_install(),
            Some((
                InstallKind::Mysql,
                PackageOperation::Install,
                "MySQL 8.4".to_string()
            ))
        );
        // The kind-agnostic abort handle (what `perform_quit` uses) must
        // still find something to abort.
        assert!(lock.running_abort_handle().is_some());

        drop(held);
    }

    /// The mirror image: a MySQL install (`install_mysql`'s own `try_lock`)
    /// must be rejected while a PHP install holds the guard.
    #[tokio::test]
    async fn a_mysql_install_is_rejected_while_a_php_install_holds_the_lock() {
        let lock = InstallLock::default();
        let held = lock.guard.lock().await;
        lock.set_running(
            InstallKind::Php,
            PackageOperation::Install,
            "8.4".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );

        assert!(
            lock.guard.try_lock().is_err(),
            "install_mysql's try_lock must fail while a PHP install holds the guard"
        );
        assert_eq!(
            lock.running_install(),
            Some((
                InstallKind::Php,
                PackageOperation::Install,
                "8.4".to_string()
            ))
        );

        drop(held);
    }

    // -------------------------------------------------------------------
    // pending_install (review fix wave, Important 1): the kind-agnostic
    // quit-dialog query that replaced pending_php_install.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn pending_install_reports_a_mysql_occupant_with_its_kind() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let lock = InstallLock::default();
        lock.set_running(
            InstallKind::Mysql,
            PackageOperation::Install,
            "MySQL 8.4".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );
        app.manage(lock);

        let pending = pending_install(app.state::<InstallLock>()).await.unwrap();

        assert_eq!(
            pending,
            Some(PendingInstallDto {
                kind: InstallKindDto::Mysql,
                operation: PackageOperationDto::Install,
                label: "MySQL 8.4".to_string(),
            }),
            "a MySQL occupant must be visible, correctly tagged — the whole \
             point of the generalization"
        );
    }

    /// The quit dialog must be able to say an INITIALIZATION is in flight, not
    /// "MySQL 8.4 is still installing" — which is what it said before audit F1,
    /// because the fact lived in a label the dialog rendered as an install
    /// sentence. The operation now crosses the wire as its own value.
    #[tokio::test]
    async fn pending_install_reports_an_initialization_as_an_initialization() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let lock = InstallLock::default();
        let (kind, operation) = MYSQL_INIT_RUN;
        lock.set_running(
            kind,
            operation,
            "MySQL 8.4".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );
        app.manage(lock);

        let pending = pending_install(app.state::<InstallLock>()).await.unwrap();

        assert_eq!(
            pending,
            Some(PendingInstallDto {
                kind: InstallKindDto::Mysql,
                operation: PackageOperationDto::Initialize,
                label: "MySQL 8.4".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn pending_install_is_none_when_nothing_is_running() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(InstallLock::default());

        let pending = pending_install(app.state::<InstallLock>()).await.unwrap();

        assert!(pending.is_none());
    }

    // -------------------------------------------------------------------
    // abort_running_if — the user-facing cancel.
    //
    // AUDIT F1 + branch review: this function had NO test at all (grep
    // returned its definition and its one call site), and the guarantee its
    // doc comment stated — "a Cancel button on the Databases page must never
    // abort somebody else's run" — was FALSE, because `initialize_mysql`
    // tagged its run with the identical `(Mysql, Install)` pair
    // `cancel_mysql_install` fires on. Two gates landed on the same function
    // from opposite directions; these are the tests that were missing.
    // -------------------------------------------------------------------

    /// Whether a spawned run is still going. Deliberately not a bare
    /// `is_finished()` the instant after the call: `AbortHandle::abort` is
    /// asynchronous, so an already-aborted task would still report "alive"
    /// for a moment and every refusal assertion below would pass vacuously.
    /// Waiting on the handle instead means an abort that DID fire settles it
    /// well inside the window, and a run nobody touched never settles at all.
    async fn still_running(task: &mut tokio::task::JoinHandle<()>) -> bool {
        tokio::time::timeout(std::time::Duration::from_millis(200), task)
            .await
            .is_err()
    }

    /// The positive case, stated first so the refusals below cannot pass by
    /// aborting nothing whatsoever.
    #[tokio::test]
    async fn a_cancel_aborts_the_run_it_names_and_reports_that_it_did() {
        let lock = InstallLock::default();
        let task = tokio::spawn(std::future::pending::<()>());
        let (kind, operation) = MYSQL_INSTALL_RUN;
        lock.set_running(
            kind,
            operation,
            "MySQL 8.4".to_string(),
            task.abort_handle(),
        );

        assert!(
            lock.abort_running_if(kind, operation),
            "the cancel must report that it stopped the install it named"
        );

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the run did not settle after the cancel fired");
        match result {
            Err(join_err) => assert!(join_err.is_cancelled(), "got {join_err:?}"),
            Ok(()) => panic!("the cancel returned true but the run ran to completion"),
        }
    }

    /// The whole point of the pair check: every occupant that is NOT the one
    /// `cancel_mysql_install` names must survive it, and the call must say so.
    ///
    /// `(Mysql, Initialize)` is the audit F1 case. `(Php, Install)` differs
    /// only in kind, `(Mysql, Uninstall)` only in operation — so a check that
    /// dropped either discriminator fails here.
    #[tokio::test]
    async fn a_cancel_leaves_every_run_but_the_one_it_names_alone() {
        let (cancel_kind, cancel_operation) = MYSQL_INSTALL_RUN;
        for (kind, operation) in [
            (InstallKind::Php, PackageOperation::Install),
            (InstallKind::Php, PackageOperation::Uninstall),
            (InstallKind::Mysql, PackageOperation::Uninstall),
            MYSQL_INIT_RUN,
        ] {
            let lock = InstallLock::default();
            let mut task = tokio::spawn(std::future::pending::<()>());
            lock.set_running(kind, operation, "occupant".to_string(), task.abort_handle());

            assert!(
                !lock.abort_running_if(cancel_kind, cancel_operation),
                "a MySQL-install cancel claimed it stopped a {kind:?}/{operation:?} run"
            );
            assert!(
                still_running(&mut task).await,
                "a MySQL-install cancel ABORTED a {kind:?}/{operation:?} run"
            );
            task.abort();
        }
    }

    /// The audit F1 finding, stated as the value it turns on rather than only
    /// as behaviour: an initialization and an install must not be the same
    /// `(kind, operation)` pair. Retagging the init run back to
    /// `PackageOperation::Install` — the shape that shipped — fails here.
    #[test]
    fn a_mysql_init_is_not_tagged_as_an_install() {
        assert_ne!(
            MYSQL_INIT_RUN, MYSQL_INSTALL_RUN,
            "an initialization tagged as an install is cancellable by \
             cancel_mysql_install, whatever its label says"
        );
        assert_eq!(MYSQL_INIT_RUN.1, PackageOperation::Initialize);
    }

    #[tokio::test]
    async fn a_cancel_with_nothing_running_reports_that_it_stopped_nothing() {
        let lock = InstallLock::default();
        let (kind, operation) = MYSQL_INSTALL_RUN;
        assert!(!lock.abort_running_if(kind, operation));
    }

    /// The same audit F1 guarantee for PHP's own Cancel
    /// ([`crate::php_pkg::cancel_php_install`], off-Homebrew slice 5C): it
    /// aborts a PHP install and nothing else.
    ///
    /// PHP is the case where the pair check earns its keep twice over, because
    /// PHP is the one engine with TWO install routes — `install_php` tags both
    /// with [`PHP_INSTALL_RUN`], so one cancel covers a `brew install` and a
    /// packaged download alike, while still leaving a MySQL install, a MariaDB
    /// install, a MySQL init and PHP's own uninstall untouched.
    #[tokio::test]
    async fn a_php_install_cancel_aborts_only_a_php_install() {
        let (cancel_kind, cancel_operation) = PHP_INSTALL_RUN;

        // The positive case first, so the refusals below cannot pass by
        // aborting nothing whatsoever.
        let lock = InstallLock::default();
        let task = tokio::spawn(std::future::pending::<()>());
        lock.set_running(
            cancel_kind,
            cancel_operation,
            "8.4".to_string(),
            task.abort_handle(),
        );
        assert!(
            lock.abort_running_if(cancel_kind, cancel_operation),
            "the cancel must report that it stopped the PHP install it named"
        );
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the run did not settle after the cancel fired");
        match result {
            Err(join_err) => assert!(join_err.is_cancelled(), "got {join_err:?}"),
            Ok(()) => panic!("the cancel returned true but the run ran to completion"),
        }

        // `(Php, Uninstall)` differs only in operation and `(Mysql, Install)`
        // only in kind, so a check that dropped either discriminator fails
        // here.
        for (kind, operation) in [
            (InstallKind::Php, PackageOperation::Uninstall),
            MYSQL_INSTALL_RUN,
            MYSQL_INIT_RUN,
            MARIADB_INSTALL_RUN,
            MARIADB_INIT_RUN,
        ] {
            let lock = InstallLock::default();
            let mut task = tokio::spawn(std::future::pending::<()>());
            lock.set_running(kind, operation, "occupant".to_string(), task.abort_handle());

            assert!(
                !lock.abort_running_if(cancel_kind, cancel_operation),
                "a PHP-install cancel claimed it stopped a {kind:?}/{operation:?} run"
            );
            assert!(
                still_running(&mut task).await,
                "a PHP-install cancel ABORTED a {kind:?}/{operation:?} run"
            );
            task.abort();
        }
    }

    /// PHP's pair must be a genuinely different VALUE from every other engine's
    /// — the audit F1 lesson stated as the value it turns on, not as behaviour.
    /// Had `PHP_INSTALL_RUN` been spelled with another kind, the loop above
    /// would still pass while `cancel_mysql_install` silently gained the power
    /// to kill a PHP install.
    #[test]
    fn a_php_install_is_not_tagged_as_any_other_engines_run() {
        for other in [
            MYSQL_INSTALL_RUN,
            MYSQL_INIT_RUN,
            MARIADB_INSTALL_RUN,
            MARIADB_INIT_RUN,
        ] {
            assert_ne!(PHP_INSTALL_RUN, other, "PHP shares a pair with {other:?}");
        }
        assert_eq!(PHP_INSTALL_RUN.0, InstallKind::Php);
        assert_eq!(PHP_INSTALL_RUN.1, PackageOperation::Install);
    }
}

// ---------------------------------------------------------------------------
// MySQL (Databases page)
// spec docs/superpowers/specs/2026-07-29-p1-db-mysql-design.md
// ---------------------------------------------------------------------------

/// Mirrors `openvhost_core::mysql::DatadirState` 1:1 as a wire-safe copy
/// (that type carries no `specta::Type`/`Serialize`, since openvhost-core
/// does not depend on either) — read from disk every time, never a
/// state.db boolean (spec D2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlDatadirStateDto {
    NotInitialized,
    Initialized,
    Foreign { detail: String },
}

impl From<openvhost_core::mysql::DatadirState> for MysqlDatadirStateDto {
    fn from(s: openvhost_core::mysql::DatadirState) -> Self {
        match s {
            openvhost_core::mysql::DatadirState::NotInitialized => Self::NotInitialized,
            openvhost_core::mysql::DatadirState::Initialized => Self::Initialized,
            openvhost_core::mysql::DatadirState::Foreign { detail } => Self::Foreign { detail },
        }
    }
}

/// Classify `dir`'s datadir state for the wire, folding an `io::Error` (e.g.
/// permission denied) into `Foreign` rather than silently defaulting to
/// `NotInitialized` — the "never silently downgrade to the safe-looking
/// state" discipline (the Docroot lesson): a directory this process could
/// not actually inspect must never render as "safe to initialize into".
fn classify_datadir_dto(dir: &Path) -> MysqlDatadirStateDto {
    match openvhost_core::mysql::classify_datadir(dir) {
        Ok(state) => state.into(),
        Err(e) => MysqlDatadirStateDto::Foreign {
            detail: format!("could not inspect {}: {e}", dir.display()),
        },
    }
}

/// One row on the Databases page: a catalogue major (installed or not), or
/// an installed major outside the catalogue (spec D1 — "a user's 9.x
/// renders as a row without an Install button").
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MysqlInstanceDto {
    pub major: String,
    /// Whether THIS BUILD offers to install this major (`MYSQL_CATALOGUE`
    /// membership) — false means no Install affordance, never a broken one.
    pub cataloged: bool,
    pub installed: bool,
    pub path: Option<String>,
    /// `Some` ONLY once BOTH installed and the datadir is genuinely
    /// Initialized — exactly when `service_id` also names a real
    /// supervisor row (never merely "installed", unlike PHP's pool, which
    /// gets a row the moment it is installed: MySQL's row is gated on the
    /// datadir too, spec D6).
    pub socket_path: Option<String>,
    pub service_id: Option<String>,
    pub datadir_state: MysqlDatadirStateDto,
    /// WHERE the installed binaries came from — OpenVHost's own package tree or
    /// a Homebrew keg (MySQL-from-tarball design D3). `None` when nothing is
    /// installed for this major.
    ///
    /// Two install sources coexist by design during the migration, and the
    /// owner will be running a brew 8.4 and a packaged 8.4 at the same time, so
    /// "which mysqld am I actually running" is a question this page has to be
    /// able to answer without the user guessing from a path.
    pub source: Option<MysqlRuntimeSourceDto>,
    /// Whether THIS BUILD publishes a verified package for this major on THIS
    /// host, and which version it would install.
    ///
    /// Distinct from `cataloged`: that says "this build manages the major",
    /// this says "and there are bytes for your architecture". An Intel Mac gets
    /// `Unavailable` for a fully cataloged 8.4 — Oracle's x86_64 build exists
    /// but never went through the signature check the pin rests on — and the
    /// row renders that as an honest absence with Homebrew as the remaining
    /// route, never as a broken Install button.
    pub offer: MysqlPackageOfferDto,
}

/// What the Databases page needs to decide which state to show (spec D6).
/// `brew_found: false` means the page must guide, not list.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MysqlEnvironmentDto {
    pub brew_found: bool,
    pub brew_searched: Vec<String>,
    pub instances: Vec<MysqlInstanceDto>,
}

/// One line of a MySQL package operation's output, forwarded live while it
/// runs. Same shape and reasoning as [`PhpInstallLogEvent`].
///
/// Since the MySQL-from-tarball slice this carries **`brew uninstall`'s output
/// only**: installing no longer runs a child process at all, and its progress
/// arrives as typed [`crate::mysql_pkg::MysqlInstallProgressEvent`] states
/// instead of prose. The channel name is unchanged because the uninstall path
/// still uses it (package-uninstall design D1 — one lock, one output surface).
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MysqlInstallLogEvent {
    pub major: String,
    pub ts_ms: u64,
    pub stream: String,
    pub line: String,
}

/// One line of `initialize_mysql`'s staged-init sequence, streamed live —
/// same shape as [`MysqlInstallLogEvent`], a separate type so the frontend
/// can tell an install log from an init log without inspecting content.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MysqlInitLogEvent {
    pub major: String,
    pub ts_ms: u64,
    pub stream: String,
    pub line: String,
}

/// Mirrors `openvhost_core::mysql::MysqlInitStep` 1:1 as a wire-safe copy —
/// a stable discriminator for the UI, never parsed out of free text (the
/// `ScaffoldStep` precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum MysqlInitStepDto {
    Render,
    Validate,
    Initialize,
    StartTempServer,
    SetPassword,
    Shutdown,
    Finalize,
}

impl From<openvhost_core::mysql::MysqlInitStep> for MysqlInitStepDto {
    fn from(s: openvhost_core::mysql::MysqlInitStep) -> Self {
        use openvhost_core::mysql::MysqlInitStep as S;
        match s {
            S::Render => Self::Render,
            S::Validate => Self::Validate,
            S::Initialize => Self::Initialize,
            S::StartTempServer => Self::StartTempServer,
            S::SetPassword => Self::SetPassword,
            S::Shutdown => Self::Shutdown,
            S::Finalize => Self::Finalize,
        }
    }
}

/// Mirrors `openvhost_core::mysql::MysqlInitOutcome` 1:1 as a wire-safe copy
/// (spec D7's `initialize_mysql`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlInitOutcomeDto {
    Initialized,
    AlreadyInitialized,
    Foreign {
        detail: String,
    },
    Failed {
        step: MysqlInitStepDto,
        reason: String,
    },
}

impl From<openvhost_core::mysql::MysqlInitOutcome> for MysqlInitOutcomeDto {
    fn from(o: openvhost_core::mysql::MysqlInitOutcome) -> Self {
        use openvhost_core::mysql::MysqlInitOutcome as O;
        match o {
            O::Initialized => Self::Initialized,
            O::AlreadyInitialized => Self::AlreadyInitialized,
            O::Foreign { detail } => Self::Foreign { detail },
            O::Failed { step, reason } => Self::Failed {
                step: step.into(),
                reason,
            },
        }
    }
}

/// `reset_mysql_root_password`'s outcome (spec D7 + Deferred: "distinct
/// auth-failure state"). Auth failure is an EXPECTED, renderable outcome —
/// the stored password may be stale (a restored/hand-copied datadir) —
/// never thrown as an `IpcError`; a spawn/other failure still is.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlResetOutcomeDto {
    Reset,
    AuthFailed { detail: String },
}

/// `verify_mysql_connection`'s outcome (spec D7: "returns version/port or
/// failure detail" — the WHOLE contract is outcome-shaped, never an
/// `IpcError`, so the "Verify connection" button always has something to
/// render).
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MysqlConnectionProofDto {
    Ok { version: String, port: u32 },
    AuthFailed { detail: String },
    Failed { detail: String },
}

/// Build one row per catalogue entry, in catalogue order, plus one row for
/// every installed runtime that falls outside the catalogue. Mirrors
/// `php_rows` exactly, with MySQL's extra `datadir_state` (spec D2, read
/// from disk) and `cataloged` (spec D1) fields, and a stricter
/// `service_id`/`socket_path` gate — see [`MysqlInstanceDto`]'s doc comment.
fn mysql_rows(
    home: &Path,
    installed: &[openvhost_core::mysql::MysqlRuntime],
) -> Vec<MysqlInstanceDto> {
    let build = |major: &openvhost_core::mysql::MysqlMajor,
                 found: Option<&openvhost_core::mysql::MysqlRuntime>| {
        let mp = openvhost_core::mysql::mysql_paths(home, major);
        let datadir_state = classify_datadir_dto(&mp.datadir);
        let registered = found.is_some() && datadir_state == MysqlDatadirStateDto::Initialized;
        MysqlInstanceDto {
            major: major.as_str().to_string(),
            cataloged: major.is_cataloged(),
            installed: found.is_some(),
            source: found.map(|rt| MysqlRuntimeSourceDto::from(&rt.source)),
            offer: crate::mysql_pkg::package_offer(major),
            path: found.map(|rt| rt.mysqld.display().to_string()),
            socket_path: registered.then(|| mp.socket.display().to_string()),
            // Same `mysql-<major>` shape `crate::stack::mysql_spec` builds —
            // formatted directly rather than constructing a whole
            // `ServiceSpec` just to read its `id` back out.
            service_id: registered.then(|| format!("mysql-{}", major.as_str())),
            datadir_state,
        }
    };

    let mut rows: Vec<MysqlInstanceDto> = openvhost_core::mysql::MYSQL_CATALOGUE
        .iter()
        .filter_map(|m| openvhost_core::mysql::MysqlMajor::parse(m).ok())
        .map(|major| {
            let found = installed.iter().find(|rt| rt.major == major);
            build(&major, found)
        })
        .collect();

    for rt in installed {
        if !rt.major.is_cataloged() {
            rows.push(build(&rt.major, Some(rt)));
        }
    }

    rows
}

/// Scan BOTH MySQL install sources — OpenVHost's own `<home>/packages/mysql/`
/// tree and every known Homebrew prefix (MySQL-from-tarball design D3).
/// Mirrors `discover_all_php`'s `spawn_blocking` + `Handle::block_on` bridge
/// exactly — see its doc comment for why.
///
/// `home` is what makes the packaged tree visible; the [`PackagesRoot`] is
/// minted from it and from nothing a caller supplies. A rescan that read only
/// Homebrew would make a freshly installed packaged runtime vanish from the
/// Databases page the moment the user pressed Rescan.
async fn discover_all_mysql(
    home: &Path,
) -> Result<openvhost_core::Discovery<openvhost_core::mysql::MysqlRuntime>, IpcError> {
    let packages = openvhost_core::PackagesRoot::from_home(home);
    tauri::async_runtime::spawn_blocking(move || {
        let handle = tokio::runtime::Handle::current();
        let prefixes: Vec<&Path> = brew_prefixes();
        openvhost_core::mysql::discover_mysql(&packages, &prefixes, &|bin| {
            handle.block_on(openvhost_conf::probe_mysqld_version(bin))
        })
    })
    .await
    .map_err(|e| IpcError::Core {
        message: format!("the MySQL discovery task failed to run: {e}"),
    })
}

/// Probe for installed MySQL runtimes, write the result into the managed
/// `RwLock`, sweep abandoned staging directories (spec D2: "swept on
/// rescan"), and register a supervisor row for every major that is BOTH
/// not already registered AND has an Initialized datadir — mirrors
/// `rescan_into_state`'s "only register what is genuinely new" discipline
/// (a live `Failed` row's stderr/exit must survive a rescan), gated on
/// Initialized rather than merely "installed" (spec D6: nothing to start
/// without a datadir). Keyed against the SUPERVISOR's own registered ids
/// rather than a separately tracked "before" list: unlike PHP (installed ⇒
/// always registered), a MySQL major can be installed for a long time
/// before ever being initialized, so "was this major known before" is not
/// the same question as "does it already have a row".
///
/// Majors that VANISHED are unregistered, exactly as the PHP rescan does
/// (package-uninstall design D5). D5 names only `rescan_php_runtimes`, but the
/// principle it states — "an in-app uninstall and a `brew uninstall` run behind
/// the app's back must leave the same observable state" — is not PHP-specific,
/// and leaving MySQL out would preserve on the Databases page exactly the
/// divergent behaviour D5 exists to end.
pub(crate) async fn rescan_mysql_into_state(
    runtimes: &RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>,
    sup: &Supervisor,
    home: &Path,
    seed: Option<openvhost_core::mysql::MysqlRuntime>,
) -> Result<openvhost_core::Discovery<openvhost_core::mysql::MysqlRuntime>, IpcError> {
    if let Err(e) =
        openvhost_core::mysql::sweep_stale_staging(&openvhost_core::mysql::mysql_data_root(home))
    {
        eprintln!("mysql: failed to sweep abandoned staging directories: {e}");
    }

    let discovered = seeded_mysql(discover_all_mysql(home).await?, seed);
    report_unidentified("MySQL", &discovered.unidentified);
    reconcile_mysql(runtimes, sup, home, discovered)
}

/// The MySQL mirror of [`seeded_php`] — see it for why an install records what
/// it asked brew for instead of interrogating the binary afterwards. This is
/// the family the failure was actually MEASURED on: `mysqld` is 55 MB, its
/// first execution took 11.53 s under Gatekeeper's scan, and the 5 s probe
/// bound meant a real, successful `brew install mysql@8.4` reported
/// `detected: false` every time.
fn seeded_mysql(
    mut discovered: openvhost_core::Discovery<openvhost_core::mysql::MysqlRuntime>,
    seed: Option<openvhost_core::mysql::MysqlRuntime>,
) -> openvhost_core::Discovery<openvhost_core::mysql::MysqlRuntime> {
    let Some(rt) = seed else {
        return discovered;
    };
    discovered
        .unidentified
        .retain(|dir| !rt.mysqld.starts_with(dir));
    if !discovered.runtimes.iter().any(|r| r.major == rt.major) {
        discovered.runtimes.push(rt);
        discovered.runtimes.sort_by(|a, b| a.major.cmp(&b.major));
    }
    discovered
}

/// Everything a MySQL rescan does with the discovery result — the mirror of
/// [`reconcile_php`], split out for the identical reason: the reconciliation
/// rules must be testable without MySQL installed and without spawning a
/// version probe.
fn reconcile_mysql(
    runtimes: &RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>,
    sup: &Supervisor,
    home: &Path,
    discovered: openvhost_core::Discovery<openvhost_core::mysql::MysqlRuntime>,
) -> Result<openvhost_core::Discovery<openvhost_core::mysql::MysqlRuntime>, IpcError> {
    let found = discovered.runtimes.clone();
    *runtimes.write().map_err(|_| IpcError::Core {
        message: "mysql runtime list is poisoned".into(),
    })? = Some(found.clone());

    let already_registered: std::collections::HashSet<String> =
        sup.snapshot().into_iter().map(|s| s.id).collect();
    for rt in &found {
        let id = crate::stack::mysql_service_id(rt.major.as_str());
        if already_registered.contains(&id) {
            continue;
        }
        if crate::stack::mysql_datadir_is_initialized(home, rt) {
            sup.register(crate::stack::mysql_spec(home, rt));
        }
    }
    // See `rescan_into_state`'s matching call for why this comes last.
    let majors: Vec<String> = found
        .iter()
        .map(|rt| rt.major.as_str().to_string())
        .collect();
    unregister_vanished(sup, crate::stack::MYSQL_ID_PREFIX, &majors);

    Ok(discovered)
}

/// Look up a cached, already-discovered runtime by major — used by the
/// commands that need to SPAWN `mysql`/`mysqladmin` (reset, verify) but must
/// not themselves probe the filesystem/spawn a version check just to find a
/// path already known from the last environment read/rescan.
fn find_mysql_runtime(
    runtimes: &RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>,
    major: &openvhost_core::mysql::MysqlMajor,
) -> Result<openvhost_core::mysql::MysqlRuntime, IpcError> {
    runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "mysql runtime list is poisoned".into(),
        })?
        .as_ref()
        .and_then(|rts| rts.iter().find(|rt| &rt.major == major).cloned())
        .ok_or_else(|| IpcError::Core {
            message: format!("MySQL {} is not installed", major.as_str()),
        })
}

/// MySQL's canonical wording for ANY grant/authentication failure
/// (`ERROR 1045 (28000): Access denied for user ...`) — the one heuristic
/// available to distinguish "the stored credential is stale" (spec
/// Deferred's desync case) from every other failure, without parsing a
/// full SQL error-code table.
fn looks_like_auth_failure(stderr: &str) -> bool {
    stderr.contains("Access denied")
}

/// Pull `("8.4.11", 3306)` out of `mysql_exec_with_defaults_file`'s
/// `--batch --skip-column-names` output for `SELECT VERSION(), @@port` — one
/// line, tab-separated, no header.
fn parse_version_and_port(stdout: &str) -> Option<(String, u32)> {
    let line = stdout.lines().next()?;
    let mut cols = line.split('\t');
    let version = cols.next()?.trim().to_string();
    let port: u32 = cols.next()?.trim().parse().ok()?;
    if version.is_empty() {
        return None;
    }
    Some((version, port))
}

/// Replace every occurrence of `secret` with a fixed marker. Defense in
/// depth for `run_mysql_init`'s SetPassword/Shutdown steps (plan Global
/// Constraints SECRETS block): NEITHER child is expected to ever echo the
/// password back on stdout/stderr (it crosses only via stdin or the
/// ephemeral defaults-file), but an exotic error path — e.g. a real `mysql`
/// quoting the offending statement back in a syntax-error message — is not
/// something this function can rule out for every past and future MySQL
/// version. Scrubbing the one known secret value out of anything these two
/// steps produce, before it can reach a streamed event OR this command's own
/// return value, costs nothing on the (expected, tested) path where there
/// was nothing to redact.
fn redact(text: &str, secret: &str) -> String {
    text.replace(secret, "<redacted>")
}

/// [`redact`] against every secret in `secrets`, in order. Exists for
/// `reset_mysql_root_password`, which has TWO live secrets in play at
/// once — the CURRENT stored password (used to authenticate, via the
/// ephemeral defaults-file) and the freshly generated one the `ALTER USER`
/// is trying to set — and a failure detail could in principle echo back
/// either. Review fix wave finding 1 (CRITICAL): redacting only the new
/// password left the current, STILL-VALID one reachable through a failure
/// path.
fn redact_all(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .fold(text.to_string(), |acc, secret| redact(&acc, secret))
}

/// An ephemeral, 0600, RAII-deleted MySQL `--defaults-file` carrying a
/// credential (spec D2 step 5 / D3): written just before use, deleted on
/// EVERY path (success, error, panic-unwind) via `Drop`, never left on disk
/// past the single call it was created for.
struct EphemeralDefaultsFile {
    path: PathBuf,
}

impl EphemeralDefaultsFile {
    /// Write a `[client]` block authenticating as `root` with `password`
    /// against `socket`, at mode 0600 from the FIRST byte on disk (opened
    /// with the mode already set — `create_new` + `mode` together, never
    /// `write` then a separate `chmod`, so there is no window where the file
    /// is briefly group/world-readable). Lives alongside `socket` itself
    /// (same `<home>/run` directory every other MySQL runtime file uses).
    ///
    /// The password is embedded UNQUOTED, exactly like `my.cnf`'s own
    /// `[client]` section (`openvhost_conf::generate_my_cnf`'s doc comment):
    /// MySQL's option-file parser takes the rest of the line, verbatim, as
    /// the value. Defensible today because the ONLY generator
    /// (`generate_root_password`) emits pure lowercase hex — no `\n`, no
    /// leading/trailing space, nothing an option-file line could
    /// misinterpret — the same "hex charset makes this safe today" caveat
    /// `alter_user_sql` documents, for the identical deferred future
    /// (user-chosen passwords, spec D3).
    ///
    /// Audit finding M2: `protocol=SOCKET` pins the client to the unix
    /// socket regardless of the `socket=` line above. Without it, a missing
    /// or stale socket path is not a hard failure — the `mysql`/`mysqladmin`
    /// CLI silently falls back to TCP `127.0.0.1:3306`, which may be a
    /// DIFFERENT mysqld already listening there (spec Owner Caveat 1:
    /// Homebrew's own `brew services mysql@8.4` unit binds the identical
    /// port with no root password) — handing that unrelated server this
    /// app's stored credential over the wire instead of cleanly failing to
    /// connect.
    fn write(
        socket: &Path,
        password: &openvhost_core::mysql::RootPassword,
    ) -> std::io::Result<Self> {
        let run_dir = socket.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(run_dir)?;
        let name = format!(".mysql-defaults-{}", uuid::Uuid::new_v4().simple());
        let path = run_dir.join(name);
        let contents = format!(
            "[client]\nuser=root\npassword={}\nsocket={}\nprotocol=SOCKET\n",
            password.expose(),
            socket.display()
        );
        let f = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)?
            }
            #[cfg(not(unix))]
            {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?
            }
        };
        write_or_cleanup(&path, f, contents.as_bytes())?;
        Ok(Self { path })
    }
}

/// Write `contents` to the just-`create_new`'d `f` at `path`, removing
/// `path` if the write itself fails. Review fix wave finding 3: `create_new`
/// succeeding is not enough to make it safe to leave a file on disk —
/// without this, a write failure AFTER creation (a full disk, a quota, any
/// I/O error mid-write) left a leftover file behind with no
/// `EphemeralDefaultsFile` guard ever constructed to clean it up (the early
/// `?` returned before `Self { path }` was ever built). Split out of
/// [`EphemeralDefaultsFile::write`] so this specific failure path is
/// directly testable without needing to induce a REAL disk-full condition.
fn write_or_cleanup(path: &Path, mut f: std::fs::File, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Err(e) = f.write_all(contents) {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

impl Drop for EphemeralDefaultsFile {
    fn drop(&mut self) {
        // Best-effort: an already-gone file is not worth surfacing, matching
        // this codebase's other RAII-cleanup Drop impls (e.g.
        // `RunningInstallGuard` above).
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Where a log line came from, forwarded to the caller-supplied sink rather
/// than emitted directly — this is what lets `run_mysql_init` be driven from
/// a test with a `Vec`-collecting sink and no `AppHandle` at all (the
/// no-secret-in-events test below), while `initialize_mysql` supplies a
/// closure that actually calls `.emit`.
type InitLogSink = Arc<dyn Fn(&str, String) + Send + Sync>;

/// A secret that becomes known PARTWAY through `run_mysql_init` (the
/// generated root password does not exist until the SetPassword step), but
/// must be scrubbed from every line logged from that point on — including
/// lines from readers that were already spawned, and already hold their own
/// clone of the log sink, before the secret existed (the temp server's
/// stdout/stderr, drained from `StartTempServer` onward). Review fix wave
/// finding 2: wrapping `log` in a NEW redacting closure once the secret
/// exists does not reach those already-spawned readers — they hold their
/// OWN `Arc` clone of the OLD sink, made before the rebinding, and are
/// never told about a later one. A cell consulted BY ONE SINK, at CALL
/// TIME rather than at closure-construction time, closes that gap: every
/// clone of the sink, however early it was made, reads the SAME cell, so
/// setting it once is enough for every clone to start redacting.
type SecretCell = Arc<std::sync::Mutex<Option<String>>>;

/// Wrap `inner` in a sink that redacts against whatever `cell` currently
/// holds, checked on EVERY call rather than once at construction — see
/// [`SecretCell`]'s doc comment. `run_mysql_init` builds exactly one of
/// these, before spawning anything that might clone it, and every later
/// `.clone()` (the temp server's stdout/stderr readers, the `run_task`
/// pump for the Initialize step) shares the same cell transparently.
fn redacting_sink(inner: InitLogSink, cell: SecretCell) -> InitLogSink {
    Arc::new(move |stream: &str, line: String| {
        let scrubbed = match cell.lock().unwrap_or_else(|e| e.into_inner()).as_deref() {
            Some(secret) if !secret.is_empty() => redact(&line, secret),
            _ => line,
        };
        inner(stream, scrubbed);
    })
}

fn emit_init_log(app: &tauri::AppHandle, major: &str, stream: &str, line: String) {
    let _ = MysqlInitLogEvent {
        major: major.to_string(),
        ts_ms: now_ms(),
        stream: stream.to_string(),
        line,
    }
    .emit(app);
}

/// Drains one of the temp server's output streams, forwarding each line to
/// `log` — mirrors `openvhost_proc::service_task`'s own `spawn_reader`
/// (hands-off: ends naturally at EOF once the child exits, no explicit
/// abort needed).
async fn drain_and_forward(stream: openvhost_proc::OutputStream, log: InitLogSink, label: &str) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log(label, line);
    }
}

/// `mysqld --no-defaults --initialize-insecure --datadir=<staging>
/// --mysqlx=OFF` (spec D2 step 1) — built via `OsString`, not `format!` +
/// `.display()`, for the same non-lossy-path reason `MysqlValidator::validate`
/// gives.
///
/// `--no-defaults` is deliberate containment, kept on its own merits — NOT a
/// fix for a datadir-mismatch bug, which does not exist (an earlier fix wave
/// claimed combining `--defaults-file=<my_cnf>` with argv `--datadir=<staging>`
/// corrupted InnoDB's undo-tablespace bookkeeping; that diagnosis was WRONG,
/// a misdiagnosis of the leading-dot bug below, and is retracted — see spec
/// D2's dated correction note for the full, corrected history). A SEPARATE
/// earlier claim — that `--no-defaults` gains exclusion of machine-wide
/// option files (`/etc/my.cnf`, `~/.my.cnf`) — was ALSO wrong:
/// `--defaults-file=<path>` already excludes those on its own. The genuine
/// gain: the rendered my.cnf ends with `!includedir <custom_confd>`, so
/// under `--defaults-file` this step would read whatever the USER has
/// dropped into that directory — arbitrary user-controlled configuration
/// reaching the init sequence. `--no-defaults` removes all of it.
///
/// `--mysqlx=OFF`: this step starts no server at all (`--initialize-insecure`
/// writes the datadir and exits), so there is no listener of any kind here
/// regardless — added purely for symmetry with [`mysqld_temp_server_spec`]
/// below, where the identical flag is load-bearing, not decorative.
fn mysqld_init_spec(mysqld: &Path, staging: &Path) -> openvhost_proc::SpawnSpec {
    let mut datadir_arg = OsString::from("--datadir=");
    datadir_arg.push(staging.as_os_str());
    openvhost_proc::SpawnSpec {
        program: mysqld.to_path_buf(),
        args: vec![
            OsString::from("--no-defaults"),
            OsString::from("--initialize-insecure"),
            datadir_arg,
            OsString::from("--mysqlx=OFF"),
        ],
        cwd: None,
        env: vec![],
    }
}

/// `mysqld --no-defaults --datadir=<staging> --skip-networking
/// --socket=<init_socket> --mysqlx=OFF` (spec D2 step 2) — the network-less
/// temp server. Same deliberate-containment reasoning for `--no-defaults` as
/// [`mysqld_init_spec`]'s doc comment above (kept on its own merits, not as a
/// fix for the retracted datadir-mismatch misdiagnosis — see spec D2's dated
/// correction note). This step's real STARTUP failure mode, confirmed live
/// and unrelated to `--defaults-file`, was a datadir basename starting with
/// a dot — fixed in `openvhost_core::mysql::staging_dir_path`.
///
/// `--mysqlx=OFF` is LOAD-BEARING here, not decorative — do not "simplify"
/// it away. Measured directly against real mysql@8.4.11: with this flag
/// absent (mysqlx at its default), this exact invocation binds
/// `/tmp/mysqlx.sock` at mode `srwxrwxrwx` — world read/write, OUTSIDE the
/// 0700 home this app otherwise confines every socket to — AND `*:33060`.
/// `--skip-networking` suppresses the CLASSIC protocol's TCP listener only;
/// it does not touch the X Plugin's listener, unix socket or TCP, at all.
/// Both are live for the entire window between this server starting and
/// `SetPassword`'s `ALTER USER` succeeding — i.e. while `root@localhost`
/// still has the EMPTY password `--initialize-insecure` left it with. Any
/// local user (not just a network-adjacent one) could connect as root
/// through that world-writable socket during that window. Adding
/// `--mysqlx=OFF` was verified, in the same measurement session, to bind no
/// socket at all.
fn mysqld_temp_server_spec(
    mysqld: &Path,
    staging: &Path,
    init_socket: &Path,
) -> openvhost_proc::SpawnSpec {
    let mut datadir_arg = OsString::from("--datadir=");
    datadir_arg.push(staging.as_os_str());
    let mut socket_arg = OsString::from("--socket=");
    socket_arg.push(init_socket.as_os_str());
    openvhost_proc::SpawnSpec {
        program: mysqld.to_path_buf(),
        args: vec![
            OsString::from("--no-defaults"),
            datadir_arg,
            OsString::from("--skip-networking"),
            socket_arg,
            OsString::from("--mysqlx=OFF"),
        ],
        cwd: None,
        env: vec![],
    }
}

/// Kills the temp server's whole process group if `run_mysql_init`'s future
/// is ABANDONED (aborted — e.g. the app quits mid-init) before the server
/// was deliberately shut down. This child is spawned directly via
/// `ProcessDriver::spawn`, never through `run_task`/`Supervisor` — unlike
/// EVERY other child this app spawns, nothing else would ever kill or even
/// notice it if this guard did not exist, which is precisely the orphaned-
/// process bug class P0-8's containment work exists to prevent.
///
/// Mirrors `openvhost_proc::task`'s own private `KillOnDrop` guard exactly
/// (that one is not `pub`, so it cannot be reused here) and
/// `RunningInstallGuard`'s identical reasoning above this function:
/// `Drop::drop` cannot `.await`, so it can only SIGNAL the kill, never
/// confirm the exit — the explicit `kill` + `wait().await` pairs already at
/// every failure arm below remain the CONFIRMED-dead path for a normal
/// failure; this guard is the backstop for the one path (the future dropped
/// mid-`.await`, e.g. via `AbortHandle::abort()`) where none of them run at
/// all. `finished` is set once the server has ACTUALLY exited (the success
/// path, or after any explicit kill+wait already reaped it), so the normal
/// paths never attempt a second, redundant signal on the way out — harmless
/// either way (`kill` on an already-reaped pid is a silent no-op), but
/// setting it is the honest way to say "this guard already did its job".
struct TempServerGuard {
    driver: Arc<dyn openvhost_proc::ProcessDriver>,
    child: openvhost_proc::SpawnedChild,
    finished: bool,
}

impl Drop for TempServerGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.driver.kill(&mut self.child);
        }
    }
}

/// Poll `mysqladmin ping` against `socket` until it succeeds, the temp
/// server dies on its own, or `deadline` elapses (spec D2 step 3: 10s cap).
async fn poll_until_ready(
    mysqladmin: &Path,
    socket: &Path,
    server_child: &mut openvhost_proc::SpawnedChild,
    deadline: std::time::Duration,
) -> bool {
    let deadline_at = tokio::time::Instant::now() + deadline;
    loop {
        if crate::mysql_admin::mysqladmin_ping(mysqladmin, socket).await {
            return true;
        }
        if matches!(server_child.try_wait(), Ok(Some(_))) {
            return false; // the temp server died on its own — nothing left to poll
        }
        if tokio::time::Instant::now() >= deadline_at {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Everything one `initialize_mysql` run needs, gathered once so every step
/// shares identical values instead of each re-deriving them.
#[derive(Debug)]
struct MysqlInitCtx {
    major: openvhost_core::mysql::MysqlMajor,
    runtime: openvhost_core::mysql::MysqlRuntime,
    paths: openvhost_core::mysql::MysqlPaths,
}

/// Drives the staged-init sequence (spec D2) against an ALREADY-CLASSIFIED
/// `NotInitialized` datadir — the caller (`initialize_mysql`) handles
/// `AlreadyInitialized`/`Foreign` before this ever runs, and BEFORE `log` is
/// ever called, so a Foreign datadir is reported "without touching it"
/// (mandatory test) with zero side effects, not even a log line.
///
/// Every failure path removes ONLY the staging directory this attempt
/// created (`remove_staging_dir`) and, from `StartTempServer` onward, kills
/// the temp server if it might still be alive — the final datadir is never
/// created, adopted, or touched by a failed attempt (spec D2). Returns the
/// generated password alongside the outcome ONLY when that outcome is
/// `Initialized` — persisting it, and registering the service, is the
/// CALLER's job (`initialize_mysql` owns `Db`/`Supervisor` state this
/// pure-ish orchestration function does not take).
async fn run_mysql_init(
    ctx: MysqlInitCtx,
    log: InitLogSink,
) -> (
    openvhost_core::mysql::MysqlInitOutcome,
    Option<openvhost_core::mysql::RootPassword>,
) {
    use openvhost_core::mysql::MysqlInitStep as Step;
    use openvhost_core::mysql::{MysqlInitOutcome as Outcome, remove_staging_dir};

    macro_rules! fail {
        ($step:expr, $reason:expr) => {
            return (
                Outcome::Failed {
                    step: $step,
                    reason: $reason,
                },
                None,
            )
        };
    }

    // Wrapped ONCE, here, before anything is spawned or cloned — see
    // `SecretCell`/`redacting_sink`'s doc comments (review fix wave finding
    // 2). `secret_cell` starts empty; SetPassword populates it the moment
    // the password is generated, and every clone of `log` made BEFORE that
    // point (the temp server's stdout/stderr readers, spawned from
    // StartTempServer onward) starts redacting from then on too, because
    // they all consult this SAME cell rather than a value baked in at
    // clone time.
    let secret_cell: SecretCell = Arc::new(std::sync::Mutex::new(None));
    let log: InitLogSink = redacting_sink(log, Arc::clone(&secret_cell));

    // ---- Render ----
    log("stdout", "rendering my.cnf".to_string());
    // Spec D3 (2026-08-04 MariaDB service): the four runtime directories come
    // from THE RUNTIME WE ARE ABOUT TO SPAWN, never from a guess and never
    // from the server's compiled-in prefix. A discovery that cannot supply
    // them is a Render failure — writing a my.cnf that points at nothing, or
    // silently omitting the four and letting the prefix win, are the two
    // outcomes this refusal exists to prevent.
    let dirs = match openvhost_core::mysql::mysql_runtime_dirs(&ctx.runtime.mysqld) {
        Some(d) => d,
        None => fail!(
            Step::Render,
            format!(
                "{} does not look like a usable MySQL install: could not locate its \
                 plugin, charset and message directories",
                ctx.runtime.mysqld.display()
            )
        ),
    };
    let mysql_ctx = openvhost_conf::MysqlCtx {
        my_cnf: ctx.paths.my_cnf.clone(),
        datadir: ctx.paths.datadir.clone(),
        socket: ctx.paths.socket.clone(),
        pid_file: ctx.paths.pid_file.clone(),
        custom_confd: ctx.paths.custom_confd.clone(),
        basedir: dirs.basedir,
        plugin_dir: dirs.plugin_dir,
        character_sets_dir: dirs.character_sets_dir,
        lc_messages_dir: dirs.lc_messages_dir,
    };
    let generated = match openvhost_conf::generate_my_cnf(&mysql_ctx) {
        Ok(f) => f,
        Err(e) => fail!(Step::Render, e.to_string()),
    };
    // Review fix wave (Important 2), corrected post-live-run: the
    // `custom_confd` directory `!includedir` points at is now ensured INSIDE
    // `write_generated_config` itself (the chokepoint every producer of a
    // my.cnf writes through — see its doc comment), not by a separate call
    // here. A standalone `create_dir_all` at this call site alone covered
    // only THIS init sequence — it missed the live end-to-end test (which
    // calls `generate_my_cnf`/`write_generated_config` directly, never this
    // function) and an already-initialized instance whose directory is
    // deleted later (see `stack.rs::mysql_spec`'s own ensure for that case).
    if let Err(e) =
        openvhost_core::mysql::write_generated_config(&generated, &ctx.paths.custom_confd)
    {
        fail!(Step::Render, e.to_string());
    }

    // ---- Validate ----
    log("stdout", "validating my.cnf".to_string());
    let validator = openvhost_conf::MysqlValidator {
        mysqld: ctx.runtime.mysqld.clone(),
    };
    match validator.validate(&ctx.paths.my_cnf).await {
        Ok(report) if report.ok => {}
        Ok(report) => fail!(
            Step::Validate,
            if report.stderr.trim().is_empty() {
                "mysqld --validate-config rejected the generated my.cnf".to_string()
            } else {
                report.stderr
            }
        ),
        Err(e) => fail!(Step::Validate, e.to_string()),
    }

    // ---- Initialize ----
    let staging = openvhost_core::mysql::staging_dir_path(&ctx.paths.staging_parent, &ctx.major);
    log(
        "stdout",
        format!("initializing datadir at {}", staging.display()),
    );
    if let Err(e) = std::fs::create_dir_all(&staging) {
        fail!(
            Step::Initialize,
            format!(
                "failed to create staging directory {}: {e}",
                staging.display()
            )
        );
    }
    #[cfg(unix)]
    if let Err(e) = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))
    } {
        let _ = remove_staging_dir(&staging);
        fail!(
            Step::Initialize,
            format!("failed to lock down staging directory permissions: {e}")
        );
    }
    let init_spec = mysqld_init_spec(&ctx.runtime.mysqld, &staging);
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let pump = tokio::spawn({
        let log = log.clone();
        async move {
            while let Some(ev) = rx.recv().await {
                if let openvhost_proc::TaskEvent::Line { stream, text } = ev {
                    log(
                        match stream {
                            openvhost_proc::TaskStream::Stdout => "stdout",
                            openvhost_proc::TaskStream::Stderr => "stderr",
                        },
                        text,
                    );
                }
            }
        }
    });
    let init_result =
        openvhost_proc::run_task(openvhost_proc::default_driver(), init_spec, tx).await;
    let _ = pump.await;
    match init_result {
        Ok(Some(0)) => {}
        Ok(Some(code)) => {
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::Initialize,
                format!("mysqld --initialize-insecure exited {code}")
            );
        }
        Ok(None) => {
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::Initialize,
                "mysqld --initialize-insecure was terminated by a signal".to_string()
            );
        }
        Err(e) => {
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::Initialize,
                format!("failed to run mysqld --initialize-insecure: {e}")
            );
        }
    }

    // ---- StartTempServer ----
    log(
        "stdout",
        "starting the temporary server for password setup".to_string(),
    );
    // `<home>/run` normally already exists by the time a user can reach this
    // command at all (`provision_home` creates it, unconditionally, at app
    // startup) — but this sequence must not silently DEPEND on that having
    // happened. Without this, mysqld's `--socket=<home>/run/...` has nowhere
    // to bind, the fake/real server never gets that far, and every later
    // step (the readiness poll, then shutdown) fails or hangs having nothing
    // useful to report — a class of bug a fake-binary test caught directly.
    let run_dir = ctx.paths.init_socket.parent().unwrap_or(Path::new("."));
    if let Err(e) = std::fs::create_dir_all(run_dir) {
        let _ = remove_staging_dir(&staging);
        fail!(
            Step::StartTempServer,
            format!("failed to create {}: {e}", run_dir.display())
        );
    }
    let temp_spec = mysqld_temp_server_spec(&ctx.runtime.mysqld, &staging, &ctx.paths.init_socket);
    let driver = openvhost_proc::default_driver();
    let mut server = match driver.spawn(&temp_spec) {
        Ok(c) => TempServerGuard {
            driver: Arc::clone(&driver),
            child: c,
            finished: false,
        },
        Err(e) => {
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::StartTempServer,
                format!("failed to start the temporary server: {e}")
            );
        }
    };
    // Drained so a chatty startup can never fill the pipe and block the
    // child — mirrors `service_task::spawn_reader`'s hands-off style.
    if let Some(out) = server.child.take_stdout() {
        tokio::spawn(drain_and_forward(out, log.clone(), "stdout"));
    }
    if let Some(err) = server.child.take_stderr() {
        tokio::spawn(drain_and_forward(err, log.clone(), "stderr"));
    }

    log(
        "stdout",
        "waiting for the temporary server to become reachable".to_string(),
    );
    let ready = poll_until_ready(
        &ctx.runtime.mysqladmin,
        &ctx.paths.init_socket,
        &mut server.child,
        std::time::Duration::from_secs(10),
    )
    .await;
    if !ready {
        let _ = driver.kill(&mut server.child);
        let _ = server.child.wait().await;
        server.finished = true;
        let _ = remove_staging_dir(&staging);
        fail!(
            Step::StartTempServer,
            "the temporary server never became reachable via mysqladmin ping".to_string()
        );
    }

    // ---- SetPassword ----
    log("stdout", "setting the root password".to_string());
    let password = openvhost_core::mysql::generate_root_password();
    let alter_sql = openvhost_core::mysql::alter_user_sql(&password);
    // From here on, EVERY log call — including from readers spawned
    // earlier, e.g. the temp server's stdout/stderr — and every failure
    // reason built below is scrubbed of the password value before it can
    // reach an emitted event or this command's own return value. Poking
    // the cell (not rebinding `log` to a new closure) is what reaches
    // those already-spawned readers too — see `SecretCell`'s doc comment.
    let secret = password.expose().to_string();
    *secret_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(secret.clone());
    match crate::mysql_admin::mysql_alter_password_unauthenticated(
        &ctx.runtime.mysql,
        &ctx.paths.init_socket,
        &alter_sql,
    )
    .await
    {
        Ok(outcome) if outcome.ok => {}
        Ok(outcome) => {
            let _ = driver.kill(&mut server.child);
            let _ = server.child.wait().await;
            server.finished = true;
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::SetPassword,
                redact(
                    &if outcome.stderr.trim().is_empty() {
                        "ALTER USER failed".to_string()
                    } else {
                        outcome.stderr
                    },
                    &secret,
                )
            );
        }
        Err(e) => {
            let _ = driver.kill(&mut server.child);
            let _ = server.child.wait().await;
            server.finished = true;
            let _ = remove_staging_dir(&staging);
            fail!(Step::SetPassword, redact(&e.to_string(), &secret));
        }
    }

    // ---- Shutdown ----
    log("stdout", "shutting down the temporary server".to_string());
    let defaults_file = match EphemeralDefaultsFile::write(&ctx.paths.init_socket, &password) {
        Ok(f) => f,
        Err(e) => {
            let _ = driver.kill(&mut server.child);
            let _ = server.child.wait().await;
            server.finished = true;
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::Shutdown,
                format!("failed to write the ephemeral credential file: {e}")
            );
        }
    };
    let shutdown_result =
        crate::mysql_admin::mysqladmin_shutdown(&ctx.runtime.mysqladmin, &defaults_file.path).await;
    drop(defaults_file); // RAII delete, before acting on the result.
    match shutdown_result {
        Ok(outcome) if outcome.ok => {}
        Ok(outcome) => {
            let _ = driver.kill(&mut server.child);
            let _ = server.child.wait().await;
            server.finished = true;
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::Shutdown,
                redact(
                    &if outcome.stderr.trim().is_empty() {
                        "mysqladmin shutdown failed".to_string()
                    } else {
                        outcome.stderr
                    },
                    &secret,
                )
            );
        }
        Err(e) => {
            let _ = driver.kill(&mut server.child);
            let _ = server.child.wait().await;
            server.finished = true;
            let _ = remove_staging_dir(&staging);
            fail!(Step::Shutdown, redact(&e.to_string(), &secret));
        }
    }
    match tokio::time::timeout(std::time::Duration::from_secs(10), server.child.wait()).await {
        Ok(_) => {
            server.finished = true;
        }
        Err(_) => {
            let _ = driver.kill(&mut server.child);
            let _ = server.child.wait().await;
            server.finished = true;
            let _ = remove_staging_dir(&staging);
            fail!(
                Step::Shutdown,
                "the temporary server did not exit after mysqladmin shutdown succeeded".to_string()
            );
        }
    }

    // ---- Finalize ----
    log("stdout", "finalizing".to_string());
    match openvhost_core::mysql::finalize_staging(&staging, &ctx.paths.datadir) {
        Outcome::Initialized => {
            log(
                "stdout",
                "root password set; datadir initialized".to_string(),
            );
            (Outcome::Initialized, Some(password))
        }
        other @ Outcome::Failed { .. } => {
            let _ = remove_staging_dir(&staging);
            (other, None)
        }
        // `finalize_staging` only ever returns `Initialized` or
        // `Failed { step: Finalize, .. }` (see its own doc comment) — this
        // arm exists only so the match stays exhaustive if that ever widens.
        other => (other, None),
    }
}

/// Read-only environment summary for the Databases page: whether Homebrew
/// was found, where it looked, and one row per MySQL major (spec D7).
/// Deliberately spawns NOTHING — mirrors `php_environment`'s identical
/// contract exactly (reads the managed `RwLock` + `find_brew()`, a
/// filesystem check). `rescan_mysql` is the one that actually probes.
#[tauri::command]
#[specta::specta]
pub async fn mysql_environment(
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<MysqlEnvironmentDto, IpcError> {
    let p = stack_paths(&paths)?;
    let installed = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "mysql runtime list is poisoned".into(),
        })?
        .clone()
        .unwrap_or_default();
    Ok(MysqlEnvironmentDto {
        brew_found: openvhost_core::find_brew().is_some(),
        brew_searched: brew_searched_paths(),
        instances: mysql_rows(&p.home, &installed),
    })
}

/// The explicit, user-initiated re-probe behind the Databases page's rescan
/// affordance — mirrors `rescan_php_runtimes` exactly, including blocking on
/// `InstallLock` for the identical reason (a rescan racing a completed
/// install must never silently revert it).
#[tauri::command]
#[specta::specta]
pub async fn rescan_mysql(
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<MysqlEnvironmentDto, IpcError> {
    let p = stack_paths(&paths)?;
    let _guard = lock.inner().guard.lock().await;
    // No seed — see `rescan_php_runtimes`'s matching call.
    let installed = rescan_mysql_into_state(runtimes.inner(), sup.inner(), &p.home, None).await?;
    Ok(MysqlEnvironmentDto {
        brew_found: openvhost_core::find_brew().is_some(),
        brew_searched: brew_searched_paths(),
        instances: mysql_rows(&p.home, &installed.runtimes),
    })
}

/// `initialize_mysql`'s pre-flight decision: either the command should
/// return immediately with no app/spawn involvement at all (decision 2's
/// catalogue rejection, or an already-answered `AlreadyInitialized`/
/// `Foreign` datadir), or everything needed to drive the real staged-init
/// sequence has been gathered.
///
/// Split out of the command itself (mirroring `write_settings`/
/// `read_settings` taking `&Db` instead of `State` above) so this ordering —
/// classify BEFORE touching anything else — is directly testable without a
/// `tauri::AppHandle`: `tauri::test::mock_builder()` only ever produces one
/// backed by `MockRuntime`, which is a DIFFERENT concrete type than the
/// `AppHandle<Wry>` `initialize_mysql`'s signature needs for `.emit()`, so
/// the full command cannot be invoked directly from a test at all. This
/// function needs no `AppHandle`, so it can be.
#[derive(Debug)]
enum InitializeMysqlGate {
    Early(Result<MysqlInitOutcomeDto, IpcError>),
    // Boxed: `MysqlInitCtx` (a `MysqlRuntime` + a `MysqlPaths`, several
    // `PathBuf`s each) is far larger than `Early`'s payload, and clippy's
    // `large_enum_variant` flags the size gap — box it rather than pay that
    // gap's cost on every `Early` value too.
    Proceed(Box<MysqlInitCtx>),
}

/// Tag `InstallLock`'s shared slot as a MySQL datadir initialization for
/// `major`.
///
/// SECURITY (audit F1). A named function rather than a `set_running` call
/// inlined in [`initialize_mysql`], for the same reason
/// [`initialize_mysql_gate`] exists: the command takes an `AppHandle<Wry>`,
/// which `tauri::test::mock_builder` cannot produce, so its body is
/// unreachable from a test and an inlined tag is a value **no test can see**.
/// That is precisely how the init run came to carry the same
/// `(Mysql, Install)` pair `cancel_mysql_install` fires on, differing only in
/// a `label` that `InstallLock::abort_running_if` neither reads nor should.
/// Retagging an initialization as an install *here* means editing this
/// function, and `a_running_mysql_init_survives_the_databases_page_cancel`
/// fails.
///
/// BYPASSING this helper — tagging inline in `initialize_mysql`, the shape the
/// bug originally had — is invisible to that test, because `initialize_mysql`
/// takes `AppHandle<Wry>` and no test can reach its body. What catches it is
/// the COMPILER: `PackageOperation::Initialize`, `MYSQL_INIT_RUN` and this
/// function would all become dead code, and `-D warnings` turns those three
/// lints into hard errors. That is stronger than a test — it cannot go
/// vacuous — but it holds only while none of the three gains a second
/// non-test user. If one ever does, this seam needs a real test instead.
fn set_running_mysql_init(
    lock: &InstallLock,
    major: &openvhost_core::mysql::MysqlMajor,
    abort: tokio::task::AbortHandle,
) {
    let (kind, operation) = MYSQL_INIT_RUN;
    // The label no longer says "initialization": `operation` carries that, and
    // the quit dialog reads it. Saying it twice is how it came to be said in
    // the one place nothing checks.
    lock.set_running(kind, operation, format!("MySQL {}", major.as_str()), abort);
}

/// Server-side catalogue gate (decision 2): `MysqlMajor::parse` — the
/// catalogue-gated constructor — is the ONLY way this reaches `major`, so an
/// out-of-catalogue discovered major (rendered display-only on the
/// Databases page) is rejected here before anything else runs, even if a
/// client somehow sends one.
///
/// Datadir classification runs BEFORE any spawn, so `AlreadyInitialized`/
/// `Foreign` are reported with zero side effects — no staging directory, no
/// spawn, no log line (mandatory test).
///
/// **The store check is first, and it lives HERE rather than inline in the
/// command, for exactly the reason this function exists at all** (design D2):
/// the command's body is unreachable from a test, so a refusal written there
/// is a decision no test can see. It has to be a refusal and not a degrade —
/// initializing without a store would leave a real datadir on disk whose
/// generated root password was never persisted, and nobody can recover it
/// afterwards. That is the hazard `verify_mysql_connection`'s "no stored root
/// password" branch already documents having been reached the hard way.
async fn initialize_mysql_gate(db: &DbHandle, major: String, home: &Path) -> InitializeMysqlGate {
    use InitializeMysqlGate::{Early, Proceed};

    // Ahead of `MysqlMajor::parse`, and therefore ahead of every path this
    // function derives: nothing below is reached, so nothing is created.
    if let Err(e) = db.require() {
        return Early(Err(e));
    }

    let major = match openvhost_core::mysql::MysqlMajor::parse(&major) {
        Ok(m) => m,
        Err(e) => return Early(Err(e.into())),
    };

    let init_paths = openvhost_core::mysql::mysql_paths(home, &major);
    if let Err(e) = init_paths.check_socket_lengths() {
        return Early(Err(e.into()));
    }

    match openvhost_core::mysql::classify_datadir(&init_paths.datadir) {
        Ok(openvhost_core::mysql::DatadirState::Initialized) => {
            return Early(Ok(MysqlInitOutcomeDto::AlreadyInitialized));
        }
        Ok(openvhost_core::mysql::DatadirState::Foreign { detail }) => {
            return Early(Ok(MysqlInitOutcomeDto::Foreign { detail }));
        }
        Ok(openvhost_core::mysql::DatadirState::NotInitialized) => {}
        Err(e) => {
            return Early(Err(IpcError::Core {
                message: format!("could not inspect {}: {e}", init_paths.datadir.display()),
            }));
        }
    }

    let discovered = match discover_all_mysql(home).await {
        Ok(d) => d,
        Err(e) => return Early(Err(e)),
    };
    let Some(runtime) = discovered.runtimes.into_iter().find(|rt| rt.major == major) else {
        return Early(Err(IpcError::Core {
            message: format!("MySQL {} is not installed", major.as_str()),
        }));
    };

    Proceed(Box::new(MysqlInitCtx {
        major,
        runtime,
        paths: init_paths,
    }))
}

/// Drives Task 4's staged-init sequence for one MySQL major (spec D2),
/// streaming log events live, exactly like `install_php`: `run_task`-style
/// child spawning, a held `AbortHandle` (shared `InstallLock`, decision 5),
/// a `Drop` guard, streamed events. Renders+validates via Task 3 first;
/// registers/refreshes the service ONLY on full success. The pre-flight
/// decision (catalogue gate, datadir classification) is
/// [`initialize_mysql_gate`] — see its doc comment.
#[tauri::command]
#[specta::specta]
pub async fn initialize_mysql(
    app: tauri::AppHandle,
    major: String,
    db: tauri::State<'_, DbHandle>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<MysqlInitOutcomeDto, IpcError> {
    let p = stack_paths(&paths)?;

    let Ok(_guard) = lock.inner().guard.try_lock() else {
        return Err(IpcError::Core {
            message: "an install is already running".into(),
        });
    };

    let ctx = match initialize_mysql_gate(&db, major, &p.home).await {
        InitializeMysqlGate::Early(result) => return result,
        InitializeMysqlGate::Proceed(ctx) => ctx,
    };
    // Resolved HERE, before the first spawn, and never again afterwards: the
    // gate above has already refused an unavailable store, so this cannot fail
    // — and it is written where a failure would still be harmless rather than
    // down beside the upsert, where it would mean a live datadir with an
    // unrecoverable password.
    let store = db.require()?;
    let runtime_for_registration = ctx.runtime.clone();
    let major_for_upsert = ctx.major.clone();
    let major_for_log = ctx.major.as_str().to_string();

    let emitter = app.clone();
    let log: InitLogSink = Arc::new(move |stream: &str, line: String| {
        emit_init_log(&emitter, &major_for_log, stream, line)
    });

    let init_task = tokio::spawn(run_mysql_init(*ctx, log));
    let abort_handle = init_task.abort_handle();
    set_running_mysql_init(lock.inner(), &major_for_upsert, abort_handle.clone());
    let _running_guard = RunningInstallGuard {
        lock: lock.inner(),
        abort: abort_handle,
    };

    let (outcome, password) = match init_task.await {
        Ok(result) => result,
        Err(join_err) if join_err.is_cancelled() => {
            return Err(IpcError::Proc {
                message: "the initialization was aborted because the app is quitting".into(),
            });
        }
        Err(join_err) => {
            return Err(IpcError::Proc {
                message: format!("the initialization task ended unexpectedly: {join_err}"),
            });
        }
    };

    if let (openvhost_core::mysql::MysqlInitOutcome::Initialized, Some(password)) =
        (&outcome, &password)
    {
        openvhost_core::mysql::MysqlInstanceRepo::new(store)
            .upsert(&major_for_upsert, password)
            .await?;
        sup.register(crate::stack::mysql_spec(&p.home, &runtime_for_registration));
    }

    Ok(outcome.into())
}

/// The stored root password for `major` (spec D3's outbound reveal — the
/// ONE place this crosses IPC, deliberately, for the masked field's
/// Reveal/Copy affordance).
///
/// SECURITY (audit H2): this command is the SOLE place in the entire
/// codebase sanctioned to de-redact a `RootPassword` into a plain `String`
/// for a RETURN value. Every other command that touches a stored or
/// freshly generated credential (`initialize_mysql`,
/// `reset_mysql_root_password`, `verify_mysql_connection`) sends it only to
/// a child's stdin or an ephemeral 0600 defaults-file
/// (`EphemeralDefaultsFile`), never back across IPC — see `RootPassword::expose`'s
/// own doc comment for that discipline. This command's `Result` must NEVER
/// be logged: verified today by grep — no Tauri command-result logging
/// exists anywhere in this codebase (nothing wraps
/// `specta_builder.invoke_handler()`/`tauri::Builder::invoke_handler` with a
/// tracing/logging layer of any kind, and neither `log`/`tracing` nor any
/// equivalent crate is even a dependency of this crate). This comment is the
/// tripwire for the day someone adds one: such a layer MUST special-case
/// this command (or, better, generically redact every `String`-typed
/// command result) before it ships.
#[tauri::command]
#[specta::specta]
pub async fn mysql_root_password(
    major: String,
    db: tauri::State<'_, DbHandle>,
) -> Result<String, IpcError> {
    let db = db.require()?;
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;
    let repo = openvhost_core::mysql::MysqlInstanceRepo::new(db);
    let instance = repo.get(&major).await?.ok_or_else(|| IpcError::Core {
        message: format!("no stored root password for MySQL {}", major.as_str()),
    })?;
    Ok(instance.root_password.expose().to_string())
}

/// Regenerate MySQL's root password (spec D3: "reset-by-regenerate ships",
/// user-chosen deferred): authenticates with the STORED (old) password via
/// an ephemeral 0600 defaults-file, runs `ALTER USER` over stdin with a
/// freshly generated password, and — only once that succeeds — persists the
/// new value. A stale stored password (spec Deferred: "desync between
/// state.db and a restored datadir") maps to `AuthFailed`, a distinct,
/// renderable outcome, never a generic error.
#[tauri::command]
#[specta::specta]
pub async fn reset_mysql_root_password(
    major: String,
    db: tauri::State<'_, DbHandle>,
    paths: tauri::State<'_, Option<StackPaths>>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
) -> Result<MysqlResetOutcomeDto, IpcError> {
    // First: a reset that cannot PERSIST the new password must not RUN the
    // `ALTER USER` either, or the server ends up on a password nothing holds.
    let db = db.require()?;
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;
    let p = stack_paths(&paths)?;
    let runtime = find_mysql_runtime(runtimes.inner(), &major)?;
    let mp = openvhost_core::mysql::mysql_paths(&p.home, &major);

    let repo = openvhost_core::mysql::MysqlInstanceRepo::new(db);
    let current = repo.get(&major).await?.ok_or_else(|| IpcError::Core {
        message: format!("MySQL {} has no stored credential to reset", major.as_str()),
    })?;

    let defaults_file =
        EphemeralDefaultsFile::write(&mp.socket, &current.root_password).map_err(|e| {
            IpcError::Core {
                message: format!("failed to write the ephemeral credential file: {e}"),
            }
        })?;

    let new_password = openvhost_core::mysql::generate_root_password();
    let sql = openvhost_core::mysql::alter_user_sql(&new_password);
    let result = crate::mysql_admin::mysql_exec_with_defaults_file(
        &runtime.mysql,
        &defaults_file.path,
        &sql,
    )
    .await;
    drop(defaults_file); // RAII delete, before acting on the result.
    // Both secrets are live here: the CURRENT password authenticated the
    // connection (via the just-dropped `defaults_file`), and the NEW one is
    // what `ALTER USER` tried to set — a failure detail could in principle
    // echo back either, so every redaction below scrubs both (review fix
    // wave finding 1: redacting only the new password left the current,
    // STILL-VALID one reachable through a failure path).
    let secrets = [current.root_password.expose(), new_password.expose()];

    let outcome = result.map_err(|e| IpcError::Core {
        message: redact_all(&e.to_string(), &secrets),
    })?;
    if outcome.ok {
        repo.upsert(&major, &new_password).await?;
        Ok(MysqlResetOutcomeDto::Reset)
    } else if looks_like_auth_failure(&outcome.stderr) {
        Ok(MysqlResetOutcomeDto::AuthFailed {
            detail: redact_all(&outcome.stderr, &secrets),
        })
    } else {
        Err(IpcError::Core {
            message: redact_all(
                &if outcome.stderr.trim().is_empty() {
                    "ALTER USER failed".to_string()
                } else {
                    outcome.stderr
                },
                &secrets,
            ),
        })
    }
}

/// `SELECT VERSION(), @@port` through the running server's socket,
/// authenticating with the stored credential via an ephemeral 0600
/// defaults-file (spec D7's "it works" moment for the Databases page).
#[tauri::command]
#[specta::specta]
pub async fn verify_mysql_connection(
    major: String,
    db: tauri::State<'_, DbHandle>,
    paths: tauri::State<'_, Option<StackPaths>>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
) -> Result<MysqlConnectionProofDto, IpcError> {
    // `Failed { detail }`, NOT `Err` — the shape this command already uses a
    // few lines below for "there is no stored password to connect with", which
    // is the same class of answer: the proof could not be attempted, and the
    // Databases page has a place to render exactly that. Following the existing
    // precedent rather than inventing a second one for the same page.
    //
    // First, because it is the one condition that makes every later step
    // pointless — and it needs no stack, no runtime and no socket to decide.
    let db = match db.require() {
        Ok(db) => db,
        Err(e) => {
            return Ok(MysqlConnectionProofDto::Failed {
                detail: e.to_string(),
            });
        }
    };
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;
    let p = stack_paths(&paths)?;
    let runtime = find_mysql_runtime(runtimes.inner(), &major)?;
    let mp = openvhost_core::mysql::mysql_paths(&p.home, &major);

    let repo = openvhost_core::mysql::MysqlInstanceRepo::new(db);
    let Some(instance) = repo.get(&major).await? else {
        // Review fix wave, minor 4: the old wording ("has never been
        // initialized") was true for only ONE of the two ways this branch is
        // reached. The second: `initialize_mysql` finished
        // `MysqlInitOutcome::Initialized` (the datadir is genuinely on disk)
        // but its OWN `repo.upsert(...).await?` call failed right after —
        // that `?` propagates out of `initialize_mysql` before
        // `sup.register` ever runs, leaving a real, initialized datadir with
        // no stored credential and no service row. Rewording to cover both
        // honestly rather than asserting the (possibly false) stronger claim.
        // `STALE_CREDENTIAL_RECOVERY` (`databases.derive.ts`) is deliberately
        // NOT attached here: that copy ends with "use Reset here once you're
        // back in", but `reset_mysql_root_password` itself requires an
        // EXISTING stored credential to authenticate with (see its own
        // `.ok_or_else` a few lines above) — there is nothing for Reset to
        // authenticate with in EITHER case this branch covers, so pairing it
        // with that copy would just trade one dishonest sentence for another.
        return Ok(MysqlConnectionProofDto::Failed {
            detail: format!(
                "no stored root password for MySQL {} — initialize it, or reset the \
                 password if the folder is already initialized",
                major.as_str()
            ),
        });
    };
    // Scrubbed from every diagnostic string built below, on the same
    // defense-in-depth reasoning `run_mysql_init`'s `redact` applies from
    // SetPassword onward: no child invoked past this point is EXPECTED to
    // ever echo the credential it authenticated with, but nothing here rules
    // out an exotic error path doing so, and scrubbing costs nothing on the
    // (expected, tested) path where there was nothing to redact.
    let secret = instance.root_password.expose().to_string();

    let defaults_file = match EphemeralDefaultsFile::write(&mp.socket, &instance.root_password) {
        Ok(f) => f,
        Err(e) => {
            return Err(IpcError::Core {
                message: format!("failed to write the ephemeral credential file: {e}"),
            });
        }
    };

    let result = crate::mysql_admin::mysql_exec_with_defaults_file(
        &runtime.mysql,
        &defaults_file.path,
        "SELECT VERSION(), @@port;",
    )
    .await;
    drop(defaults_file); // RAII delete, before acting on the result.

    let outcome = match result {
        Ok(o) => o,
        Err(e) => {
            return Ok(MysqlConnectionProofDto::Failed {
                detail: redact(&e.to_string(), &secret),
            });
        }
    };

    if !outcome.ok {
        return Ok(if looks_like_auth_failure(&outcome.stderr) {
            MysqlConnectionProofDto::AuthFailed {
                detail: redact(&outcome.stderr, &secret),
            }
        } else {
            MysqlConnectionProofDto::Failed {
                detail: redact(
                    &if outcome.stderr.trim().is_empty() {
                        "the connection attempt failed".to_string()
                    } else {
                        outcome.stderr
                    },
                    &secret,
                ),
            }
        });
    }

    Ok(match parse_version_and_port(&outcome.stdout) {
        Some((version, port)) => MysqlConnectionProofDto::Ok { version, port },
        None => MysqlConnectionProofDto::Failed {
            detail: redact(
                &format!("could not parse a version/port from: {:?}", outcome.stdout),
                &secret,
            ),
        },
    })
}

/// The default-PHP command surface (default-PHP design D1/D2, spec claims
/// 3/4/5/6).
///
/// Drives `read_default_php`/`write_default_php` over an in-memory database
/// rather than the two `#[tauri::command]` wrappers, the same split
/// `web_server_settings_ipc_tests` uses for `read_settings`/`write_settings`:
/// the commands add nothing but a `State` unwrap and a lock read, and requiring
/// a mock Tauri app to reach the behaviour would put the interesting assertions
/// behind machinery that has nothing to do with them.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod default_php_ipc_tests {
    use super::*;

    fn installed(majors: &[&str]) -> Vec<openvhost_core::PhpRuntime> {
        majors
            .iter()
            .map(|m| openvhost_core::PhpRuntime {
                major: (*m).to_string(),
                fpm_bin: PathBuf::from(format!("/opt/homebrew/opt/php@{m}/sbin/php-fpm")),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            })
            .collect()
    }

    fn expect_validation(e: IpcError) -> (String, String) {
        match e {
            IpcError::Validation { field, message } => (field, message),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // Reading. VACUITY: replacing `read_default_php`'s body with a constant
    // `Ok(DefaultPhpDto::Unset { serving: installed.first()… })` — i.e. never
    // consulting the stored row at all, the shape this slice exists to end —
    // reddens `a_stored_preference_reaches_the_wire_as_preferred` and
    // `an_uninstalled_preference_reaches_the_wire_as_preferred_missing` while
    // leaving `a_fresh_database_reports_unset…` green. Restored afterwards.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_fresh_database_reports_unset() {
        // Every real machine today (design D3). The page must be able to tell
        // this from "you chose 8.3 and 8.3 is what you get", which is the next
        // test. That reading this does not CREATE a row is pinned one layer
        // down, where the SQL lives
        // (`php::settings::tests::a_fresh_database_reads_no_preference_without_writing_a_row`)
        // — this crate has no sqlx dependency to re-count rows with, and
        // adding one to restate someone else's guarantee would be worse than
        // pointing at it.
        let db = Db::open_in_memory().await.unwrap();
        assert_eq!(
            read_default_php(Some(&db), &installed(&["8.1", "8.3"]))
                .await
                .unwrap(),
            DefaultPhpDto::Unset {
                serving: "8.1".to_string()
            }
        );
    }

    #[tokio::test]
    async fn a_fresh_database_with_no_php_reports_nothing_installed() {
        let db = Db::open_in_memory().await.unwrap();
        assert_eq!(
            read_default_php(Some(&db), &[]).await.unwrap(),
            DefaultPhpDto::NothingInstalled
        );
    }

    #[tokio::test]
    async fn a_stored_preference_reaches_the_wire_as_preferred() {
        let db = Db::open_in_memory().await.unwrap();
        let rt = installed(&["8.1", "8.3"]);
        write_default_php(&db, Some("8.3".into()), &rt)
            .await
            .unwrap();
        assert_eq!(
            read_default_php(Some(&db), &rt).await.unwrap(),
            DefaultPhpDto::Preferred {
                major: "8.3".to_string()
            }
        );
    }

    #[tokio::test]
    async fn choosing_the_first_discovered_major_is_still_reported_as_a_choice() {
        // Same served major as `Unset` would give, DIFFERENT wire state. If the
        // two collapsed, the page could not tell "you chose 8.1" from "8.1 is
        // what you happen to get" — the conflation D2 forbids, one layer up
        // from where `DefaultPhp`'s own test pins it.
        let db = Db::open_in_memory().await.unwrap();
        let rt = installed(&["8.1", "8.3"]);
        write_default_php(&db, Some("8.1".into()), &rt)
            .await
            .unwrap();
        let read = read_default_php(Some(&db), &rt).await.unwrap();
        assert_eq!(
            read,
            DefaultPhpDto::Preferred {
                major: "8.1".to_string()
            }
        );
        assert_ne!(
            read,
            DefaultPhpDto::Unset {
                serving: "8.1".to_string()
            }
        );
    }

    #[tokio::test]
    async fn an_uninstalled_preference_reaches_the_wire_as_preferred_missing() {
        // Spec claim 4, at the command boundary: uninstalling the default must
        // leave the state LEGIBLE. Nothing on the uninstall path touches
        // `php_settings`, so the row outlives the runtime — and this is the
        // shape the page reads to say "your default was 8.3, which is no longer
        // installed" instead of quietly serving 8.1.
        let db = Db::open_in_memory().await.unwrap();
        write_default_php(&db, Some("8.3".into()), &installed(&["8.1", "8.3"]))
            .await
            .unwrap();

        // …and now 8.3 is gone.
        assert_eq!(
            read_default_php(Some(&db), &installed(&["8.1"]))
                .await
                .unwrap(),
            DefaultPhpDto::PreferredMissing {
                requested: "8.3".to_string(),
                serving: Some("8.1".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn a_preference_that_comes_back_resolves_again_without_being_reset() {
        // The other half of spec claim 5: a rescan that rediscovers the major
        // must restore `Preferred` on its own. If anything cleared the row when
        // it went missing, this would come back `Unset` and the user's choice
        // would have been silently discarded by an uninstall.
        let db = Db::open_in_memory().await.unwrap();
        write_default_php(&db, Some("8.3".into()), &installed(&["8.1", "8.3"]))
            .await
            .unwrap();
        let _gone = read_default_php(Some(&db), &installed(&["8.1"]))
            .await
            .unwrap();
        assert_eq!(
            read_default_php(Some(&db), &installed(&["8.1", "8.3"]))
                .await
                .unwrap(),
            DefaultPhpDto::Preferred {
                major: "8.3".to_string()
            }
        );
    }

    // ------------------------------------------------------------------
    // Writing. VACUITY: deleting `write_default_php`'s installed-set check
    // reddens `a_major_that_is_not_installed_is_refused_and_names_its_field`
    // and `a_refused_choice_leaves_the_previous_one_exactly_as_it_was`;
    // deleting the `PhpVersion::parse` call reddens
    // `a_malformed_major_is_refused_at_ingress`. Both restored afterwards.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_malformed_major_is_refused_at_ingress_naming_its_column() {
        let db = Db::open_in_memory().await.unwrap();
        let (field, _) = expect_validation(
            write_default_php(&db, Some("../../etc".into()), &installed(&["8.1"]))
                .await
                .expect_err("a traversal-shaped major must be refused"),
        );
        assert_eq!(
            field, "default_major",
            "the error must name the column the form and the repository both use"
        );
    }

    #[tokio::test]
    async fn a_major_that_is_not_installed_is_refused_and_names_its_field() {
        // `PreferredMissing` is a state you ARRIVE at, never one you choose.
        let db = Db::open_in_memory().await.unwrap();
        let (field, message) = expect_validation(
            write_default_php(&db, Some("8.4".into()), &installed(&["8.1", "8.3"]))
                .await
                .expect_err("an uninstalled major must be refused"),
        );
        assert_eq!(field, "default_major");
        assert!(message.contains("8.4"), "got {message}");
    }

    #[tokio::test]
    async fn a_refused_choice_leaves_the_previous_one_exactly_as_it_was() {
        // All-or-nothing, like the settings guard: a rejected write must not
        // take the stored preference down with it.
        let db = Db::open_in_memory().await.unwrap();
        let rt = installed(&["8.1", "8.3"]);
        write_default_php(&db, Some("8.3".into()), &rt)
            .await
            .unwrap();
        let _ = write_default_php(&db, Some("8.4".into()), &rt)
            .await
            .expect_err("an uninstalled major must be refused");
        assert_eq!(
            read_default_php(Some(&db), &rt).await.unwrap(),
            DefaultPhpDto::Preferred {
                major: "8.3".to_string()
            }
        );
    }

    #[tokio::test]
    async fn a_preference_can_be_cleared_back_to_no_preference() {
        // `None` is a value a caller can mean, and clearing has to stay
        // expressible or "give me the old behaviour back" becomes unreachable
        // once a default has ever been set.
        let db = Db::open_in_memory().await.unwrap();
        let rt = installed(&["8.1", "8.3"]);
        write_default_php(&db, Some("8.3".into()), &rt)
            .await
            .unwrap();
        write_default_php(&db, None, &rt).await.unwrap();
        assert_eq!(
            read_default_php(Some(&db), &rt).await.unwrap(),
            DefaultPhpDto::Unset {
                serving: "8.1".to_string()
            }
        );
    }

    #[tokio::test]
    async fn clearing_is_allowed_even_with_nothing_installed() {
        // The installed-set guard must gate a CHOICE, never a clear: a machine
        // whose only PHP has just been removed still has to be able to drop the
        // preference, and `None` names no major for the guard to check.
        let db = Db::open_in_memory().await.unwrap();
        write_default_php(&db, None, &[]).await.unwrap();
        assert_eq!(
            read_default_php(Some(&db), &[]).await.unwrap(),
            DefaultPhpDto::NothingInstalled
        );
    }

    #[test]
    fn every_core_outcome_has_its_own_wire_shape() {
        // EXHAUSTIVENESS, from the other end: `From<&DefaultPhp>` is a full
        // match, so a fifth core variant fails to compile there — but a match
        // that compiles could still map two outcomes onto one wire value. This
        // pins that it does not.
        use openvhost_core::DefaultPhp as D;
        let dtos = [
            DefaultPhpDto::from(&D::NothingInstalled),
            DefaultPhpDto::from(&D::Unset {
                serving: "8.1".into(),
            }),
            DefaultPhpDto::from(&D::Preferred {
                major: "8.1".into(),
            }),
            DefaultPhpDto::from(&D::PreferredMissing {
                requested: "8.4".into(),
                serving: Some("8.1".into()),
            }),
        ];
        for (i, a) in dtos.iter().enumerate() {
            for (j, b) in dtos.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "wire states {i} and {j} collapsed into one");
                }
            }
        }
    }

    #[test]
    fn the_wire_tags_are_the_ones_the_frontend_switches_on() {
        // The frontend's `switch (defaultPhp.kind)` ends in
        // `const unreachable: never`, so a renamed tag would fail TS typecheck
        // — but only after the bindings are regenerated. This fails here first,
        // in the crate that owns the name.
        use openvhost_core::DefaultPhp as D;
        let cases = [
            (
                DefaultPhpDto::from(&D::NothingInstalled),
                "nothingInstalled",
            ),
            (
                DefaultPhpDto::from(&D::Unset {
                    serving: "8.1".into(),
                }),
                "unset",
            ),
            (
                DefaultPhpDto::from(&D::Preferred {
                    major: "8.1".into(),
                }),
                "preferred",
            ),
            (
                DefaultPhpDto::from(&D::PreferredMissing {
                    requested: "8.4".into(),
                    serving: None,
                }),
                "preferredMissing",
            ),
        ];
        for (dto, tag) in cases {
            let json = serde_json::to_value(&dto).unwrap();
            assert_eq!(json.get("kind").and_then(|k| k.as_str()), Some(tag));
        }
    }

    /// `set_default_php` REFUSES with no store (design D2). Storing the
    /// preference IS the command — it applies nothing — so there is no half of
    /// it left to degrade to, and an `Ok` that stored nothing would leave the
    /// Languages page showing a default the next Apply will not honour.
    ///
    /// Vacuity: with all 13 `let db = db.require()?;` guards replaced by
    /// `let db = &Db::open_in_memory().await.unwrap();`, this test failed on
    /// `unwrap_err`, which reported the `Ok(())` the command then returns.
    /// Reverted and re-run green.
    #[tokio::test]
    async fn set_default_php_refuses_and_names_why_when_the_store_is_down() {
        use tauri::Manager;

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: None,
            php: installed(&["8.4"]),
        })));

        assert_store_refusal(
            &set_default_php(app.state(), app.state(), Some("8.4".to_string()))
                .await
                .unwrap_err(),
            "set_default_php",
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod mysql_ipc_tests {
    use tauri::Manager;

    use super::*;

    /// Warmed at creation, outside the `PROBE_TIMEOUT`-bounded calls these
    /// tests then time — see [`crate::tests_support`] for what that costs and
    /// why every fixture helper in this workspace does it.
    fn fake_cli(dir: &Path, name: &str, body: &str) -> PathBuf {
        // `<dir>/bin/<name>`, alongside the `lib/plugin` and `share/mysql/…`
        // directories a real install has — NOT a bare `<dir>/<name>`.
        //
        // These fakes stand in for a DISCOVERED runtime, and since spec D3
        // (2026-08-04) the Render step derives the four pinned runtime
        // directories from exactly that path (`openvhost_core::mysql::
        // mysql_runtime_dirs`). A fake laid out in a shape no real install
        // has would make every test here pass against a runtime the
        // production code correctly refuses — which is the failure mode
        // "the fixture was incomplete, so the test proved nothing" that this
        // project has already paid for once.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(dir.join("lib/plugin")).unwrap();
        std::fs::create_dir_all(dir.join("share/mysql/charsets")).unwrap();
        std::fs::create_dir_all(dir.join("share/mysql/english")).unwrap();
        let p = bin.join(name);
        crate::tests_support::write_exec_fixture(&p, body);
        p
    }

    // ---- argv shape: no --defaults-file pre-finalize ----
    //
    // `--no-defaults` is deliberate containment on its own merits — NOT a
    // fix for a datadir-mismatch bug, which does not exist. An earlier fix
    // wave claimed combining `--defaults-file=<my.cnf>` with argv
    // `--datadir=<staging>` corrupted InnoDB's undo-tablespace bookkeeping;
    // that diagnosis was WRONG (a misdiagnosis of the leading-dot
    // staging-basename bug, decisively isolated afterward — see spec D2's
    // dated correction note) and is retracted. A SEPARATE earlier claim in
    // this same comment — that `--no-defaults` gains exclusion of
    // machine-wide option files (`/etc/my.cnf`, `~/.my.cnf`) — was ALSO
    // wrong: `--defaults-file=<path>` already excludes those on its own; that
    // was never something `--no-defaults` added. The genuine gain: our
    // rendered my.cnf ends with `!includedir <custom_confd>`, so under
    // `--defaults-file` both pre-finalize steps read whatever the USER has
    // dropped into that directory — arbitrary user-controlled configuration
    // reaching the init sequence while root@localhost still has an EMPTY
    // password. `--no-defaults` removes ALL of it, user-controlled included.
    //
    // Audit finding (security-auditor BLOCK, HIGH): the previous version of
    // these two tests pinned the ABSENCE of `--defaults-file` without
    // pinning what explicitly replaced the settings it used to carry — so
    // dropping `mysqlx=OFF` (which my.cnf carried, silently lost when
    // `--defaults-file` was removed) slipped through unnoticed. The X
    // Plugin's default is ON, binding `*:33060` — a listener with no
    // narrower bind-address of its own, live for the entire window between
    // temp-server start and `ALTER USER`, i.e. while root@localhost has an
    // EMPTY password from `--initialize-insecure`. Fixed by adding
    // `--mysqlx=OFF` explicitly to both specs' argv (redundant but harmless
    // for `mysqld_init_spec`, which starts no server at all — kept for
    // symmetry). These four tests now pin BOTH the absence of
    // `--defaults-file` AND the exact positive argv shape, so a future edit
    // that silently drops any one flag fails loudly instead of compiling
    // clean.

    #[test]
    fn mysqld_init_spec_carries_no_defaults_file_and_no_defaults_first() {
        let spec = mysqld_init_spec(
            Path::new("/opt/homebrew/opt/mysql@8.4/bin/mysqld"),
            Path::new("/tmp/ovh/data/mysql/init-8-4-deadbeef"),
        );
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a.starts_with("--defaults-file")),
            "got {args:?}"
        );
        assert_eq!(
            args.first().map(String::as_str),
            Some("--no-defaults"),
            "--no-defaults must be first (mysqld requirement), got {args:?}"
        );
        assert!(
            args.contains(&"--initialize-insecure".to_string()),
            "got {args:?}"
        );
        assert!(
            args.contains(&"--datadir=/tmp/ovh/data/mysql/init-8-4-deadbeef".to_string()),
            "got {args:?}"
        );
        assert!(args.contains(&"--mysqlx=OFF".to_string()), "got {args:?}");
    }

    /// Exhaustive sibling to the test above (audit-mandated): an EXACT match
    /// on the whole argv list, not just `.contains()` checks, so a future
    /// edit that drops (or silently reorders ahead of `--no-defaults`) any
    /// one of these settings fails loudly rather than compiling clean.
    #[test]
    fn mysqld_init_spec_argv_is_exactly_the_required_set() {
        let spec = mysqld_init_spec(
            Path::new("/opt/homebrew/opt/mysql@8.4/bin/mysqld"),
            Path::new("/tmp/ovh/data/mysql/init-8-4-deadbeef"),
        );
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "--no-defaults".to_string(),
                "--initialize-insecure".to_string(),
                "--datadir=/tmp/ovh/data/mysql/init-8-4-deadbeef".to_string(),
                "--mysqlx=OFF".to_string(),
            ],
            "got {args:?}"
        );
    }

    #[test]
    fn mysqld_temp_server_spec_carries_no_defaults_file_and_no_defaults_first() {
        let spec = mysqld_temp_server_spec(
            Path::new("/opt/homebrew/opt/mysql@8.4/bin/mysqld"),
            Path::new("/tmp/ovh/data/mysql/init-8-4-deadbeef"),
            Path::new("/tmp/ovh/run/mysql-8.4-init.sock"),
        );
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a.starts_with("--defaults-file")),
            "got {args:?}"
        );
        assert_eq!(
            args.first().map(String::as_str),
            Some("--no-defaults"),
            "--no-defaults must be first (mysqld requirement), got {args:?}"
        );
        assert!(
            args.contains(&"--datadir=/tmp/ovh/data/mysql/init-8-4-deadbeef".to_string()),
            "got {args:?}"
        );
        assert!(
            args.contains(&"--skip-networking".to_string()),
            "got {args:?}"
        );
        assert!(
            args.contains(&"--mysqlx=OFF".to_string()),
            "got {args:?} — measured on real mysql@8.4.11: at its default, the X \
             Plugin binds /tmp/mysqlx.sock at mode srwxrwxrwx (world read/write, \
             outside the 0700 home) AND *:33060, for the entire window \
             root@localhost has an empty password; --skip-networking is TCP-only \
             and does not touch either"
        );
        assert!(
            args.contains(&"--socket=/tmp/ovh/run/mysql-8.4-init.sock".to_string()),
            "got {args:?}"
        );
    }

    /// Exhaustive sibling to the test above (audit-mandated): an EXACT match
    /// on the whole argv list, not just `.contains()` checks, so a future
    /// edit that drops (or silently reorders ahead of `--no-defaults`) any
    /// one of these settings fails loudly rather than compiling clean.
    #[test]
    fn mysqld_temp_server_spec_argv_is_exactly_the_required_set() {
        let spec = mysqld_temp_server_spec(
            Path::new("/opt/homebrew/opt/mysql@8.4/bin/mysqld"),
            Path::new("/tmp/ovh/data/mysql/init-8-4-deadbeef"),
            Path::new("/tmp/ovh/run/mysql-8.4-init.sock"),
        );
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "--no-defaults".to_string(),
                "--datadir=/tmp/ovh/data/mysql/init-8-4-deadbeef".to_string(),
                "--skip-networking".to_string(),
                "--socket=/tmp/ovh/run/mysql-8.4-init.sock".to_string(),
                "--mysqlx=OFF".to_string(),
            ],
            "got {args:?}"
        );
    }

    // ---- write_or_cleanup (review fix wave finding 3) ----

    #[test]
    fn write_or_cleanup_succeeds_and_leaves_the_file_when_the_write_works() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("candidate");
        let f = std::fs::File::create(&path).unwrap();
        write_or_cleanup(&path, f, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    /// `f` here is a pipe's write end whose READ end was closed the moment
    /// it was created: writing to a pipe with no reader raises `EPIPE`
    /// (Rust's runtime ignores `SIGPIPE` at startup specifically so this
    /// surfaces as an `Err` rather than terminating the process). `path` is
    /// a SEPARATE, real file — `write_or_cleanup`'s own contract is just
    /// "write via the given handle; on failure, remove the given path", so
    /// the two do not need to be the same underlying file for this test to
    /// isolate that behavior. Deterministic, and touches no process-wide
    /// state (no rlimit, no real disk pressure), so this is safe to run
    /// alongside every other test in this binary's default parallel
    /// execution — `/dev/full` (the more obvious choice) does not exist on
    /// macOS, and a raw `close()` of the fd `f` itself already owns trips
    /// libstd's own double-close IO-safety abort.
    #[test]
    fn write_or_cleanup_removes_the_file_when_the_write_itself_fails() {
        use std::os::unix::io::FromRawFd;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("candidate");
        std::fs::write(&path, b"").unwrap();
        assert!(path.exists());

        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (read_fd, write_fd) = (fds[0], fds[1]);
        // SAFETY: `read_fd` is a freshly created, valid, owned fd from the
        // `pipe()` call immediately above, closed here (and only here) so
        // the pipe has no reader for the rest of this test.
        unsafe {
            libc::close(read_fd);
        }
        // SAFETY: `write_fd` is a freshly created, valid, owned fd from the
        // same `pipe()` call; this is the only place that takes ownership
        // of it (as this `File`), and it is never touched as a raw fd again.
        let f = unsafe { std::fs::File::from_raw_fd(write_fd) };

        let _err = write_or_cleanup(&path, f, b"secret contents").unwrap_err();
        assert!(
            !path.exists(),
            "the partially-written file must be removed when the write fails"
        );
    }

    // ---- EphemeralDefaultsFile (audit finding M2) ----

    /// Audit finding M2: without an explicit `protocol=SOCKET`, a missing or
    /// wrong `socket=` line lets the `mysql`/`mysqladmin` CLI silently fall
    /// back to TCP `127.0.0.1:3306` — which may be a DIFFERENT mysqld (e.g.
    /// Homebrew's own `brew services` instance, spec Owner Caveat 1) — and
    /// hand it this app's stored root credential. Pinning the protocol
    /// closes that fallback: the client refuses to try TCP at all.
    #[test]
    fn ephemeral_defaults_file_pins_the_client_to_the_unix_socket_protocol() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("run").join("mysql-8.4.sock");
        let password = openvhost_core::mysql::generate_root_password();

        let file = EphemeralDefaultsFile::write(&socket, &password).unwrap();

        let contents = std::fs::read_to_string(&file.path).unwrap();
        assert!(
            contents.lines().any(|l| l == "protocol=SOCKET"),
            "got {contents:?}"
        );
    }

    /// A working MySQL runtime built entirely from fakes: `mysqld` handles
    /// `--validate-config`/`--initialize-insecure`/`--skip-networking`,
    /// `mysqladmin` handles `ping`/`shutdown` (shared body, [`FAKE_MYSQLADMIN_BODY`]),
    /// and the caller supplies `mysql_body` for the ALTER USER step — used
    /// by the temp-server-containment regression test below, which needs
    /// that ONE step to hang so there is a window to abort the whole init
    /// task while the temp server is confirmed alive.
    fn fake_runtime_with_mysql(
        dir: &Path,
        major: &openvhost_core::mysql::MysqlMajor,
        mysql_body: &str,
    ) -> openvhost_core::mysql::MysqlRuntime {
        openvhost_core::mysql::MysqlRuntime {
            major: major.clone(),
            mysqld: fake_cli(
                dir,
                "mysqld",
                r#"
datadir=""
socket=""
for arg in "$@"; do
  case "$arg" in
    --datadir=*) datadir="${arg#--datadir=}" ;;
    --socket=*) socket="${arg#--socket=}" ;;
  esac
done
case "$*" in
  *--validate-config*)
    exit 0
    ;;
  *--initialize-insecure*)
    mkdir -p "$datadir/mysql"
    echo "[auto]" > "$datadir/auto.cnf"
    exit 0
    ;;
  *--skip-networking*)
    echo $$ > "$socket.pid"
    trap 'exit 0' TERM
    while true; do sleep 1; done
    ;;
  *)
    exit 1
    ;;
esac
"#,
            ),
            mysql: fake_cli(dir, "mysql", mysql_body),
            mysqladmin: fake_cli(dir, "mysqladmin", FAKE_MYSQLADMIN_BODY),
            source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
        }
    }

    /// Shared by every fake runtime in this module: `ping` is gated on the
    /// server's own pidfile (a real `mysqladmin ping` only succeeds once
    /// the server is genuinely accepting connections — without this gate,
    /// `ping` always says yes and `poll_until_ready` proceeds long before
    /// the fake server, a SEPARATE process, has reached its own
    /// `echo $$ > "$socket.pid"` line, so a later `shutdown` call races
    /// ahead of that write and silently kills nothing); `shutdown` reads
    /// the target socket from either `--socket=` or a `--defaults-file`'s
    /// `socket=` line and signals whatever pid it finds there.
    const FAKE_MYSQLADMIN_BODY: &str = r#"
socket=""
for arg in "$@"; do
  case "$arg" in
    --socket=*) socket="${arg#--socket=}" ;;
    --defaults-file=*)
      f="${arg#--defaults-file=}"
      socket=$(sed -n 's/^socket=//p' "$f")
      ;;
  esac
done
case "$*" in
  *ping*)
    if [ -f "$socket.pid" ]; then
      exit 0
    else
      exit 1
    fi
    ;;
  *shutdown*)
    if [ -f "$socket.pid" ]; then
      kill "$(cat "$socket.pid")" 2>/dev/null
    fi
    exit 0
    ;;
  *)
    exit 1
    ;;
esac
"#;

    /// Like [`fake_runtime`], but the temp server's OWN stdout leaks the
    /// SQL text `mysql` (the ALTER client) received on stdin — standing in
    /// for a server that happens to log something containing the
    /// credential it was just given. Real `mysqld`/`mysql` are two
    /// SEPARATE processes with no shared memory, so the fake `mysql`
    /// writes what it read to a marker file next to the socket
    /// (`<init_socket>.alter-sql-seen`), and the fake `mysqld` server loop
    /// polls for that file and echoes its contents to ITS OWN stdout —
    /// which is exactly the stream `run_mysql_init`'s `drain_and_forward`
    /// readers pick up. Exists for the review fix wave's finding 2: those
    /// readers are spawned (and clone the log sink) BEFORE the password
    /// exists, so they are the one path a "rebind `log` to a new
    /// redacting wrapper once the secret exists" fix does not reach.
    fn fake_runtime_that_leaks_the_alter_sql_via_server_stdout(
        dir: &Path,
        major: &openvhost_core::mysql::MysqlMajor,
    ) -> openvhost_core::mysql::MysqlRuntime {
        openvhost_core::mysql::MysqlRuntime {
            major: major.clone(),
            mysqld: fake_cli(
                dir,
                "mysqld",
                r#"
datadir=""
socket=""
for arg in "$@"; do
  case "$arg" in
    --datadir=*) datadir="${arg#--datadir=}" ;;
    --socket=*) socket="${arg#--socket=}" ;;
  esac
done
case "$*" in
  *--validate-config*)
    exit 0
    ;;
  *--initialize-insecure*)
    mkdir -p "$datadir/mysql"
    echo "[auto]" > "$datadir/auto.cnf"
    exit 0
    ;;
  *--skip-networking*)
    echo $$ > "$socket.pid"
    trap 'exit 0' TERM
    while true; do
      if [ -f "$socket.alter-sql-seen" ]; then
        echo "observed on the wire: $(cat "$socket.alter-sql-seen")"
        rm -f "$socket.alter-sql-seen"
        touch "$socket.alter-sql-echoed"
      fi
      sleep 0.01
    done
    ;;
  *)
    exit 1
    ;;
esac
"#,
            ),
            mysql: fake_cli(
                dir,
                "mysql",
                r#"
socket=""
for arg in "$@"; do
  case "$arg" in
    --socket=*) socket="${arg#--socket=}" ;;
  esac
done
cat > "$socket.alter-sql-seen"
exit 0
"#,
            ),
            // A DEDICATED mysqladmin (not the shared `FAKE_MYSQLADMIN_BODY`):
            // `shutdown` here waits (bounded, 5s) for the server's
            // `alter-sql-echoed` marker before killing it. A poll-based
            // "leak" is otherwise an unwinnable race — a shell blocked in
            // `sleep` does not act on a pending TERM until that sleep call
            // itself returns (verified against this exact shell empirically
            // while designing this fixture), so an ordinary shutdown call
            // almost always kills the server before its loop gets another
            // chance to notice the marker file, making the leak this
            // fixture exists to simulate silently never happen. Explicit
            // synchronization, not a shorter poll interval, is what makes
            // this deterministic.
            mysqladmin: fake_cli(
                dir,
                "mysqladmin",
                r#"
socket=""
for arg in "$@"; do
  case "$arg" in
    --socket=*) socket="${arg#--socket=}" ;;
    --defaults-file=*)
      f="${arg#--defaults-file=}"
      socket=$(sed -n 's/^socket=//p' "$f")
      ;;
  esac
done
case "$*" in
  *ping*)
    if [ -f "$socket.pid" ]; then
      exit 0
    else
      exit 1
    fi
    ;;
  *shutdown*)
    if [ -f "$socket.pid" ]; then
      i=0
      while [ ! -f "$socket.alter-sql-echoed" ] && [ "$i" -lt 500 ]; do
        sleep 0.01
        i=$((i + 1))
      done
      rm -f "$socket.alter-sql-echoed"
      kill "$(cat "$socket.pid")" 2>/dev/null
    fi
    exit 0
    ;;
  *)
    exit 1
    ;;
esac
"#,
            ),
            source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
        }
    }

    // ---- mysql_rows (environment shape) ----

    #[test]
    fn mysql_rows_lists_the_catalogue_and_reflects_installed_and_initialized_state() {
        let home = tempfile::tempdir().unwrap();
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        let installed = vec![openvhost_core::mysql::MysqlRuntime {
            major: major.clone(),
            mysqld: PathBuf::from("/opt/homebrew/opt/mysql@8.4/bin/mysqld"),
            mysql: PathBuf::from("/opt/homebrew/opt/mysql@8.4/bin/mysql"),
            mysqladmin: PathBuf::from("/opt/homebrew/opt/mysql@8.4/bin/mysqladmin"),
            source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
        }];

        let rows = mysql_rows(home.path(), &installed);
        assert_eq!(rows.len(), openvhost_core::mysql::MYSQL_CATALOGUE.len());
        let row = rows.iter().find(|r| r.major == "8.4").unwrap();
        assert!(row.installed);
        assert!(row.cataloged);
        assert_eq!(row.datadir_state, MysqlDatadirStateDto::NotInitialized);
        assert!(
            row.service_id.is_none(),
            "installed but not initialized — no service row yet"
        );
        assert!(row.socket_path.is_none());

        let datadir = home.path().join("data/mysql/8.4");
        std::fs::create_dir_all(datadir.join("mysql")).unwrap();
        std::fs::write(datadir.join("auto.cnf"), b"[auto]\n").unwrap();

        let rows2 = mysql_rows(home.path(), &installed);
        let row2 = rows2.iter().find(|r| r.major == "8.4").unwrap();
        assert_eq!(row2.datadir_state, MysqlDatadirStateDto::Initialized);
        assert_eq!(row2.service_id.as_deref(), Some("mysql-8.4"));
        assert!(row2.socket_path.is_some());
    }

    #[test]
    fn mysql_rows_still_lists_an_out_of_catalogue_installed_major() {
        // `MysqlMajor::from_probe` is `pub(crate)` to openvhost-core, so an
        // out-of-catalogue `MysqlRuntime` is built the same way that crate's
        // OWN tests build one: through the real, publicly-reachable
        // `discover_mysql` against a fake prefix, never a private
        // constructor.
        let prefix = tempfile::tempdir().unwrap();
        let bin_dir = prefix.path().join("opt").join("mysql@9.7").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        for name in ["mysqld", "mysql", "mysqladmin"] {
            std::fs::write(bin_dir.join(name), b"#!/bin/sh\n").unwrap();
        }
        let no_packages = tempfile::tempdir().unwrap();
        let installed = openvhost_core::mysql::discover_mysql(
            &openvhost_core::PackagesRoot::from_home(no_packages.path()),
            &[prefix.path()],
            &|_| Some("9.7".to_string()),
        )
        .runtimes;
        assert_eq!(installed.len(), 1);

        let home = tempfile::tempdir().unwrap();
        let rows = mysql_rows(home.path(), &installed);
        let row = rows
            .iter()
            .find(|r| r.major == "9.7")
            .expect("an out-of-catalogue installed major must still be listed");
        assert!(row.installed);
        assert!(
            !row.cataloged,
            "out-of-catalogue major must render with no Install affordance"
        );
    }

    // ---- out-of-catalogue action rejection (decision 2) ----

    /// Mirrors PHP's `a_rejected_version_names_the_field_so_the_ui_can_mark_it`
    /// exactly: `install_mysql`'s FIRST line is `MysqlMajor::parse(&major)?`,
    /// so proving that parse rejects an out-of-catalogue major with a
    /// `mysql_version`-named field IS proving the command rejects it before
    /// anything else runs — the same reasoning the PHP test already
    /// establishes for its own command.
    #[test]
    fn install_mysql_rejects_an_out_of_catalogue_major_server_side() {
        let e: IpcError = openvhost_core::mysql::MysqlMajor::parse("9.7")
            .unwrap_err()
            .into();
        match e {
            IpcError::Validation { field, .. } => assert_eq!(field, "mysql_version"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    /// The `initialize_mysql` equivalent, driven through the REAL gate
    /// function the command calls (`initialize_mysql_gate`) rather than the
    /// full command — see that enum's doc comment for why the full command
    /// cannot be invoked directly from a test at all.
    #[tokio::test]
    async fn initialize_mysql_rejects_an_out_of_catalogue_major_server_side() {
        let unreached = PathBuf::from("/tmp/openvhost-test-unreached");
        let ready = DbHandle::Ready(Db::open_in_memory().await.unwrap());
        let gate = initialize_mysql_gate(&ready, "9.7".to_string(), &unreached).await;
        match gate {
            InitializeMysqlGate::Early(Err(IpcError::Validation { field, .. })) => {
                assert_eq!(field, "mysql_version")
            }
            InitializeMysqlGate::Early(other) => {
                panic!("expected a Validation error, got {other:?}")
            }
            InitializeMysqlGate::Proceed(_) => {
                panic!("an out-of-catalogue major must never reach Proceed")
            }
        }
    }

    // ---- Foreign datadir: reported without touching it ----

    #[tokio::test]
    async fn initialize_on_a_foreign_datadir_reports_foreign_without_touching_it() {
        let home = tempfile::tempdir().unwrap();
        let major_dir = home.path().join("data/mysql/8.4");
        std::fs::create_dir_all(&major_dir).unwrap();
        std::fs::write(major_dir.join("some-note.txt"), b"do not touch").unwrap();

        let ready = DbHandle::Ready(Db::open_in_memory().await.unwrap());
        let gate = initialize_mysql_gate(&ready, "8.4".to_string(), home.path()).await;

        match gate {
            InitializeMysqlGate::Early(Ok(MysqlInitOutcomeDto::Foreign { detail })) => {
                assert!(detail.contains("some-note.txt"), "got {detail:?}")
            }
            other => panic!("expected Early(Ok(Foreign)), got {other:?}"),
        }
        assert!(
            major_dir.join("some-note.txt").exists(),
            "the foreign file must survive untouched"
        );
        let entries: Vec<_> = std::fs::read_dir(home.path().join("data/mysql"))
            .unwrap()
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "no staging directory should ever have been created"
        );
    }

    // ---- REFUSE with no store (optional-state.db design D2) --------------

    /// `initialize_mysql` must refuse PRE-FLIGHT, with the datadir untouched.
    ///
    /// This is the one REFUSE where degrading would destroy something that
    /// cannot be rebuilt: an initialized datadir whose generated root password
    /// was never persisted is a server nobody can log into, and
    /// `reset_mysql_root_password` cannot help — it authenticates with the
    /// stored credential it does not have. `verify_mysql_connection`'s "no
    /// stored root password" branch documents that state being reached the
    /// hard way already.
    ///
    /// Driven through the real gate, like the two tests above and for the same
    /// reason: `initialize_mysql` takes an `AppHandle<Wry>` and cannot be
    /// called from a test at all.
    ///
    /// VACUITY, measured, and one result is worth writing down because it is
    /// NOT what it looks like:
    ///
    /// - Deleting the `db.require()` block from `initialize_mysql_gate`: this
    ///   test failed on "an unavailable store must never reach Proceed — that
    ///   is a datadir". So the refusal itself is pinned.
    /// - MOVING that block to the very end of the gate, immediately before
    ///   `Proceed`: this test still **passed**. The gate creates nothing
    ///   anywhere in its body — `classify_datadir` reads and `mysql_paths`
    ///   derives — so the four filesystem assertions below are not sensitive
    ///   to ordering *within* the gate. What they pin is that the gate stays
    ///   side-effect-free, which is the property the command depends on: the
    ///   moment anything here starts creating a staging directory before
    ///   deciding, they go red. They are not, and should not be read as,
    ///   evidence about where in the gate the check sits — the deletion
    ///   mutation above is what covers that.
    ///
    /// Both reverted and re-run green.
    #[tokio::test]
    async fn initialize_mysql_refuses_before_touching_the_datadir_when_the_store_is_down() {
        let home = tempfile::tempdir().unwrap();

        let gate = initialize_mysql_gate(&store_down(), "8.4".to_string(), home.path()).await;

        match gate {
            InitializeMysqlGate::Early(Err(e)) => assert_store_refusal(&e, "initialize_mysql"),
            InitializeMysqlGate::Early(Ok(other)) => {
                panic!("expected a refusal, got Early(Ok({other:?}))")
            }
            InitializeMysqlGate::Proceed(_) => {
                panic!("an unavailable store must never reach Proceed — that is a datadir")
            }
        }

        // Not "an error came back": nothing was created. `mysql_paths` derives
        // every one of these from `home`, and the refusal happens before that
        // derivation runs at all.
        let paths = openvhost_core::mysql::mysql_paths(
            home.path(),
            &openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap(),
        );
        assert!(!paths.datadir.exists(), "the datadir must not exist");
        assert!(
            !paths.staging_parent.exists(),
            "no staging parent may have been created"
        );
        assert!(
            !paths.my_cnf.exists(),
            "no config may have been rendered for a refused initialization"
        );
        assert!(
            !home.path().join("run").exists(),
            "no run directory may have been created"
        );
    }

    /// The three remaining MySQL credential commands, grouped: they share one
    /// refusal, and each is named so a member that stops refusing is reported
    /// by name. `verify_mysql_connection` is the odd one and is checked for its
    /// own shape — see the test below it.
    ///
    /// Vacuity: with all 13 `let db = db.require()?;` guards replaced by
    /// `let db = &Db::open_in_memory().await.unwrap();`, this test failed —
    /// the commands then answer with their own "no stored root password"
    /// errors, which `assert_store_refusal` rejects for not carrying the
    /// sentinel reason. Reverted and re-run green.
    #[tokio::test]
    async fn the_mysql_credential_commands_refuse_and_name_why_when_the_store_is_down() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("nginx.conf"),
        }));
        // A runtime IS present, so nothing here is refused for lack of one —
        // the refusal under test is the store's.
        app.manage(RwLock::new(Some(vec![
            openvhost_core::mysql::MysqlRuntime {
                major: openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap(),
                mysqld: home.path().join("mysqld"),
                mysql: home.path().join("mysql"),
                mysqladmin: home.path().join("mysqladmin"),
                source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
            },
        ])));

        assert_store_refusal(
            &mysql_root_password("8.4".to_string(), app.state())
                .await
                .unwrap_err(),
            "mysql_root_password",
        );
        assert_store_refusal(
            &reset_mysql_root_password(
                "8.4".to_string(),
                app.state(),
                app.state(),
                app.state::<RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>(),
            )
            .await
            .unwrap_err(),
            "reset_mysql_root_password",
        );
        // Nothing may have been spawned or written on the way to that refusal:
        // the ephemeral 0600 defaults-file is the only thing this command
        // creates, and it must never have existed.
        assert!(
            !home.path().join("run").exists(),
            "reset must not have written a credential file before refusing"
        );
    }

    /// `verify_mysql_connection` refuses as `Failed { detail }`, **not** `Err`.
    ///
    /// It already answers exactly this way for "there is no stored password",
    /// which is the same class of answer — the proof could not be attempted,
    /// and the Databases page has a place to render that. Following the
    /// existing precedent rather than inventing a second shape for one page.
    ///
    /// Vacuity: with the refusal rewritten as `let db = db.require()?;` —
    /// i.e. an `Err` instead of a `Failed` — this test failed on `unwrap()`,
    /// reporting the store refusal as an `Err(Core { .. })`. Reverted and
    /// re-run green.
    #[tokio::test]
    async fn verify_mysql_connection_refuses_as_failed_not_err_when_the_store_is_down() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(RwLock::new(Some(vec![
            openvhost_core::mysql::MysqlRuntime {
                major: openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap(),
                mysqld: home.path().join("mysqld"),
                mysql: home.path().join("mysql"),
                mysqladmin: home.path().join("mysqladmin"),
                source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
            },
        ])));

        let proof = verify_mysql_connection(
            "8.4".to_string(),
            app.state(),
            app.state(),
            app.state::<RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>(),
        )
        .await
        .unwrap();

        match proof {
            MysqlConnectionProofDto::Failed { detail } => {
                assert!(
                    detail.contains(STORE_DOWN_REASON),
                    "the rendered detail must carry the reason: {detail:?}"
                );
                assert!(
                    !detail.contains(".manage()"),
                    "a user must never be told to call a Rust API: {detail:?}"
                );
            }
            other => panic!("expected Failed (the no-stored-password precedent), got {other:?}"),
        }
    }

    // ---- custom conf.d directory (review fix wave, Important 2) ----

    /// The rendered my.cnf's `!includedir` points at `custom_confd`, but
    /// nothing used to create it — the Render step must create it BEFORE
    /// Validate ever runs, so neither validate-config nor a real supervised
    /// start ever meets a missing `!includedir` target.
    #[tokio::test]
    async fn render_step_creates_the_custom_confd_directory_before_validation() {
        let home = tempfile::tempdir().unwrap();
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        let runtime = fake_runtime_with_mysql(home.path(), &major, "exit 0");
        let paths = openvhost_core::mysql::mysql_paths(home.path(), &major);
        let custom_confd = paths.custom_confd.clone();
        assert!(
            !custom_confd.exists(),
            "must not exist before init for this test to prove anything"
        );

        let ctx = MysqlInitCtx {
            major: major.clone(),
            runtime,
            paths,
        };
        let log: InitLogSink = Arc::new(|_stream, _line| {});
        let (outcome, _password) = run_mysql_init(ctx, log).await;

        assert_eq!(
            outcome,
            openvhost_core::mysql::MysqlInitOutcome::Initialized,
            "the fake-binary sequence must reach full success for this test to prove anything"
        );
        assert!(
            custom_confd.is_dir(),
            "the Render step must create the custom conf.d directory the rendered \
             my.cnf's !includedir points at"
        );
    }

    // ---- the no-secret-in-events test (SECRETS block, mandatory) ----

    /// Drives `run_mysql_init` end to end against fake `mysqld`/`mysql`/
    /// `mysqladmin` scripts (a test-injected runtime, per the brief), then
    /// asserts NONE of the collected log lines — the exact content
    /// `initialize_mysql` would otherwise hand to `.emit()` as
    /// `MysqlInitLogEvent`s — contain the password `run_mysql_init` itself
    /// generated. The sink IS the only path to an emitted event (see
    /// `InitLogSink`'s doc comment), so this is not a weaker proxy for "no
    /// secret in events": it is the same content the real command would
    /// stream, captured before Tauri's own (orthogonal) serialization layer.
    #[tokio::test]
    async fn no_emitted_log_line_contains_the_generated_password() {
        let home = tempfile::tempdir().unwrap();
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        // The leak-simulating runtime (not plain `fake_runtime`): its temp
        // server echoes the ALTER SQL back on its OWN stdout during the
        // SetPassword window, exercising the `drain_and_forward` reader
        // path — review fix wave finding 2's blind spot — not just the
        // direct `log(...)` calls a naive fix could leave uncovered.
        let runtime = fake_runtime_that_leaks_the_alter_sql_via_server_stdout(home.path(), &major);
        let paths = openvhost_core::mysql::mysql_paths(home.path(), &major);

        let lines: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let for_sink = Arc::clone(&lines);
        let log: InitLogSink = Arc::new(move |_stream, line| {
            for_sink.lock().unwrap().push(line);
        });

        let ctx = MysqlInitCtx {
            major: major.clone(),
            runtime,
            paths,
        };

        let (outcome, password) = run_mysql_init(ctx, log).await;

        assert_eq!(
            outcome,
            openvhost_core::mysql::MysqlInitOutcome::Initialized,
            "the fake-binary sequence must reach full success for this test to prove anything"
        );
        let password = password.expect("Initialized must carry the generated password");

        let captured = lines.lock().unwrap();
        assert!(
            !captured.is_empty(),
            "the sequence should have logged something"
        );
        for line in captured.iter() {
            assert!(
                !line.contains(password.expose()),
                "a log line leaked the generated password: {line:?}"
            );
        }
    }

    // ---- TempServerGuard: containment on abort ----

    /// THE TEMP-SERVER CONTAINMENT REGRESSION TEST. The temp server is
    /// spawned directly via `ProcessDriver::spawn`, outside `run_task`/
    /// `Supervisor` — nothing else in the app would ever kill it if
    /// `run_mysql_init`'s future were dropped (aborted, e.g. by
    /// `perform_quit`) while it was running. Reproduces exactly that: aborts
    /// the init task while the fake temp server is CONFIRMED alive (its own
    /// pidfile exists), then polls for the real OS process to actually die.
    ///
    /// The fake `mysql` (the ALTER step) hangs for a few seconds so there is
    /// a real window to observe the temp server's pidfile and issue the
    /// abort before `SetPassword` would otherwise complete on its own.
    ///
    /// VACUITY CHECK (mirrors `quit.rs`'s identical regression test): with
    /// `TempServerGuard`'s `Drop` impl neutered (commented out its `kill`
    /// call), this test fails — `still_alive` stays `true` for the whole
    /// deadline. Restoring it makes the test pass again.
    #[cfg(unix)]
    #[tokio::test]
    async fn aborting_the_init_task_kills_the_still_running_temp_server() {
        let home = tempfile::tempdir().unwrap();
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        let runtime =
            fake_runtime_with_mysql(home.path(), &major, "cat > /dev/null\nsleep 100\nexit 0");
        let paths = openvhost_core::mysql::mysql_paths(home.path(), &major);
        let pidfile = format!("{}.pid", paths.init_socket.display());

        let ctx = MysqlInitCtx {
            major: major.clone(),
            runtime,
            paths,
        };
        let log: InitLogSink = Arc::new(|_stream, _line| {});
        let init_task = tokio::spawn(run_mysql_init(ctx, log));

        // Wait for proof the temp server is actually running before
        // aborting on it.
        let pid: i32 = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pidfile)
                    && let Ok(pid) = text.trim().parse::<i32>()
                {
                    return pid;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the temp server never wrote its pidfile within the deadline");

        // SAFETY: signal 0 performs no action; it only checks existence/permission.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "temp server {pid} was not alive before aborting on it"
        );

        init_task.abort();
        let _ = init_task.await;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut still_alive = true;
        while std::time::Instant::now() < deadline {
            // SAFETY: signal 0 performs no action; it only checks existence/permission.
            if unsafe { libc::kill(pid, 0) } != 0 {
                still_alive = false;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Defensive cleanup on both the pass and fail path, same as
        // `quit.rs`'s identical regression test.
        if still_alive {
            // SAFETY: plain kill syscall, cleaning up a leaked descendant.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }

        assert!(
            !still_alive,
            "temp server {pid} was still alive after aborting the init task — \
             TempServerGuard's Drop never ran or never killed it"
        );
    }

    // ---- reset: auth-failure maps to its distinct state ----

    #[tokio::test]
    async fn reset_maps_an_access_denied_failure_to_the_distinct_auth_failed_state() {
        let home = tempfile::tempdir().unwrap();
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        let old_password = openvhost_core::mysql::generate_root_password();
        openvhost_core::mysql::MysqlInstanceRepo::new(&db)
            .upsert(&major, &old_password)
            .await
            .unwrap();
        manage_db(&app, db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        let fake_mysql = fake_cli(
            home.path(),
            "mysql",
            r#"echo "ERROR 1045 (28000): Access denied for user 'root'@'localhost' (using password: YES)" 1>&2; exit 1"#,
        );
        app.manage(RwLock::new(Some(vec![
            openvhost_core::mysql::MysqlRuntime {
                major: major.clone(),
                mysqld: home.path().join("mysqld"),
                mysql: fake_mysql,
                mysqladmin: home.path().join("mysqladmin"),
                source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
            },
        ])));

        let outcome = reset_mysql_root_password(
            "8.4".to_string(),
            app.state::<DbHandle>(),
            app.state::<Option<StackPaths>>(),
            app.state::<RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>(),
        )
        .await
        .unwrap();

        match outcome {
            MysqlResetOutcomeDto::AuthFailed { detail } => {
                assert!(detail.contains("Access denied"), "got {detail:?}")
            }
            other => panic!("expected AuthFailed, got {other:?}"),
        }
    }

    /// REVIEW FIX WAVE, finding 1 (CRITICAL): the ephemeral defaults-file
    /// `reset_mysql_root_password` writes to authenticate carries the
    /// CURRENT (still-valid) stored password, not the freshly generated
    /// one — so a failure detail that happens to echo back what it
    /// authenticated with leaks the credential that is STILL ACTIVE on the
    /// running server, which is strictly worse than leaking the new one
    /// (that one is about to be overwritten if the ALTER ever succeeds; the
    /// current one keeps working regardless). The fake `mysql` below reads
    /// its own `--defaults-file` (exactly as a real client would to
    /// authenticate) and echoes the password it found there back on
    /// stderr — standing in for any diagnostic that quotes its own
    /// connection parameters — then fails with an "Access denied" shape so
    /// this exercises the `AuthFailed` branch specifically.
    #[tokio::test]
    async fn reset_redacts_the_current_password_from_a_failure_detail() {
        let home = tempfile::tempdir().unwrap();
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        let current_password = openvhost_core::mysql::generate_root_password();
        openvhost_core::mysql::MysqlInstanceRepo::new(&db)
            .upsert(&major, &current_password)
            .await
            .unwrap();
        manage_db(&app, db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        let fake_mysql = fake_cli(
            home.path(),
            "mysql",
            r#"
pw=""
for arg in "$@"; do
  case "$arg" in
    --defaults-file=*)
      f="${arg#--defaults-file=}"
      pw=$(sed -n 's/^password=//p' "$f")
      ;;
  esac
done
echo "ERROR 1045 (28000): Access denied for user 'root'@'localhost' (using password: $pw)" 1>&2
exit 1
"#,
        );
        app.manage(RwLock::new(Some(vec![
            openvhost_core::mysql::MysqlRuntime {
                major: major.clone(),
                mysqld: home.path().join("mysqld"),
                mysql: fake_mysql,
                mysqladmin: home.path().join("mysqladmin"),
                source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
            },
        ])));

        let outcome = reset_mysql_root_password(
            "8.4".to_string(),
            app.state::<DbHandle>(),
            app.state::<Option<StackPaths>>(),
            app.state::<RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>(),
        )
        .await
        .unwrap();

        let detail = match outcome {
            MysqlResetOutcomeDto::AuthFailed { detail } => detail,
            other => panic!("expected AuthFailed, got {other:?}"),
        };
        assert!(
            !detail.contains(current_password.expose()),
            "the CURRENT (still-valid) password leaked into a reset failure detail: {detail:?}"
        );
    }

    // ---- redact ----

    #[test]
    fn redact_replaces_every_occurrence() {
        let s = redact("pw=abc123 again abc123", "abc123");
        assert_eq!(s, "pw=<redacted> again <redacted>");
        assert!(!s.contains("abc123"));
    }

    #[test]
    fn redact_all_scrubs_every_secret_independent_of_which_one_appears() {
        let secrets = ["current-pw", "new-pw"];
        assert_eq!(
            redact_all("auth used current-pw", &secrets),
            "auth used <redacted>"
        );
        assert_eq!(
            redact_all("ALTER failed setting new-pw", &secrets),
            "ALTER failed setting <redacted>"
        );
        assert_eq!(
            redact_all("current-pw and new-pw both present", &secrets),
            "<redacted> and <redacted> both present"
        );
    }

    /// Defense-in-depth pin for `verify_mysql_connection`: even an EXOTIC
    /// failure that echoes the stored credential back on stderr (something
    /// no real `mysql` invocation is expected to do, but not something this
    /// command can rule out for every past and future MySQL version) must
    /// never reach the returned `detail`.
    #[tokio::test]
    async fn verify_connection_redacts_the_stored_password_from_a_failure_detail() {
        let home = tempfile::tempdir().unwrap();
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        let password = openvhost_core::mysql::generate_root_password();
        openvhost_core::mysql::MysqlInstanceRepo::new(&db)
            .upsert(&major, &password)
            .await
            .unwrap();
        manage_db(&app, db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        // An exotic fake that echoes its own defaults-file's contents back on
        // stderr before failing — standing in for "some future mysql client
        // quotes its connection parameters in an error message".
        let fake_mysql = fake_cli(
            home.path(),
            "mysql",
            r#"
for arg in "$@"; do
  case "$arg" in
    --defaults-file=*) cat "${arg#--defaults-file=}" 1>&2 ;;
  esac
done
exit 1
"#,
        );
        app.manage(RwLock::new(Some(vec![
            openvhost_core::mysql::MysqlRuntime {
                major: major.clone(),
                mysqld: home.path().join("mysqld"),
                mysql: fake_mysql,
                mysqladmin: home.path().join("mysqladmin"),
                source: openvhost_core::mysql::MysqlRuntimeSource::Homebrew,
            },
        ])));

        let outcome = verify_mysql_connection(
            "8.4".to_string(),
            app.state::<DbHandle>(),
            app.state::<Option<StackPaths>>(),
            app.state::<RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>(),
        )
        .await
        .unwrap();

        let detail = match outcome {
            MysqlConnectionProofDto::Failed { detail } => detail,
            other => panic!("expected Failed, got {other:?}"),
        };
        assert!(
            !detail.contains(password.expose()),
            "the stored password leaked into a failure detail: {detail:?}"
        );
    }

    // ------------------------------------------------------------------
    // Audit F1 (BLOCKING): `cancel_mysql_install` aborted datadir
    // initializations.
    //
    // `initialize_mysql` itself is unreachable from a test (it needs an
    // `AppHandle<Wry>`), so what is driven here is the exact function it
    // calls to tag the shared slot — see `set_running_mysql_init`'s doc
    // comment for why the tag lives in a function at all.
    // ------------------------------------------------------------------

    /// A mock-runtime app managing `lock`, so the real
    /// `cancel_mysql_install` command can be invoked. It takes only a
    /// `tauri::State` (no `AppHandle`), so unlike `initialize_mysql` it IS
    /// reachable from a test — which is why the tag, not the cancel, is the
    /// half that needed extracting.
    fn app_with(lock: InstallLock) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(lock);
        app
    }

    /// The blocker, end to end through both real functions: tag the slot the
    /// way `initialize_mysql` does, then invoke the actual
    /// `cancel_mysql_install` command. The initialization must be untouched.
    ///
    /// Before the fix `set_running_mysql_init`'s body was
    /// `set_running(Mysql, Install, "MySQL 8.4 initialization", …)`, which this
    /// cancel matched on both discriminators — the label was the only
    /// difference and nothing compares labels.
    #[tokio::test]
    async fn a_running_mysql_init_survives_the_databases_page_cancel() {
        let lock = InstallLock::default();
        let init = tokio::spawn(std::future::pending::<()>());
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        set_running_mysql_init(&lock, &major, init.abort_handle());
        let app = app_with(lock);

        let stopped = crate::mysql_pkg::cancel_mysql_install(app.state::<InstallLock>())
            .await
            .unwrap();

        assert!(
            !stopped,
            "the Databases page's cancel claimed it stopped a datadir initialization"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), init)
                .await
                .is_err(),
            "the Databases page's cancel ABORTED a datadir initialization"
        );
    }

    /// Non-vacuity twin: the same command, against the run it IS for, must
    /// still stop it. Without this, a `cancel_mysql_install` that aborted
    /// nothing at all would pass the test above.
    #[tokio::test]
    async fn the_same_cancel_still_stops_a_real_mysql_install() {
        let lock = InstallLock::default();
        let install = tokio::spawn(std::future::pending::<()>());
        let (kind, operation) = MYSQL_INSTALL_RUN;
        lock.set_running(
            kind,
            operation,
            "MySQL 8.4".to_string(),
            install.abort_handle(),
        );
        let app = app_with(lock);

        assert!(
            crate::mysql_pkg::cancel_mysql_install(app.state::<InstallLock>())
                .await
                .unwrap()
        );

        match tokio::time::timeout(std::time::Duration::from_secs(5), install)
            .await
            .expect("the install did not settle after its own cancel fired")
        {
            Err(join_err) => assert!(join_err.is_cancelled(), "got {join_err:?}"),
            Ok(()) => panic!("the cancel returned true but the install ran to completion"),
        }
    }

    /// And the label an initialization carries no longer smuggles the
    /// operation into prose — `PackageOperation` carries it, which is the only
    /// place `abort_running_if` can see it.
    #[tokio::test]
    async fn an_initialization_is_tagged_as_one_and_labelled_plainly() {
        let lock = InstallLock::default();
        let init = tokio::spawn(std::future::pending::<()>());
        let major = openvhost_core::mysql::MysqlMajor::parse("8.4").unwrap();
        set_running_mysql_init(&lock, &major, init.abort_handle());

        assert_eq!(
            lock.running_install(),
            Some((
                InstallKind::Mysql,
                PackageOperation::Initialize,
                "MySQL 8.4".to_string()
            ))
        );
        init.abort();
    }
}

// ---------------------------------------------------------------------------
// MariaDB (Databases page)
// spec docs/superpowers/specs/2026-08-05-p1-mariadb-ui-design.md
// ---------------------------------------------------------------------------

/// Mirrors `openvhost_core::mariadb::MariadbDatadirState` 1:1 as a wire-safe
/// copy — the MariaDB counterpart of `MysqlDatadirStateDto`. No sibling of
/// `Foreign`'s message for a missing/unreadable datadir here either: same
/// "never silently downgrade to the safe-looking state" discipline as
/// `classify_datadir_dto` below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbDatadirStateDto {
    NotInitialized,
    Initialized { version: String },
    Foreign { detail: String },
}

impl From<openvhost_core::MariadbDatadirState> for MariadbDatadirStateDto {
    fn from(s: openvhost_core::MariadbDatadirState) -> Self {
        match s {
            openvhost_core::MariadbDatadirState::NotInitialized => Self::NotInitialized,
            openvhost_core::MariadbDatadirState::Initialized { version } => {
                Self::Initialized { version }
            }
            openvhost_core::MariadbDatadirState::Foreign { detail } => Self::Foreign { detail },
        }
    }
}

/// Classify MariaDB's datadir for the wire — see `classify_datadir_dto`'s
/// matching doc comment for why an `io::Error` folds into `Foreign` rather
/// than `NotInitialized`.
fn classify_mariadb_datadir_dto(dir: &Path) -> MariadbDatadirStateDto {
    match openvhost_core::classify_mariadb_datadir(dir) {
        Ok(state) => state.into(),
        Err(e) => MariadbDatadirStateDto::Foreign {
            detail: format!("could not inspect {}: {e}", dir.display()),
        },
    }
}

/// The MariaDB row on the Databases page.
///
/// A single struct, never a `Vec`: this build ships exactly one series
/// (`openvhost_core::MARIADB_SERIES`), so a list whose length is always 0 or
/// 1 would invent a key nothing can vary — the same reasoning
/// `MariadbInstanceRepo`'s own doc comment gives for leaving a `major` field
/// off `MariadbInstance` (design D6: "the store holds scalars, not
/// dictionaries").
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MariadbEnvironmentDto {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    /// `Some` ONLY once BOTH installed and the datadir is genuinely
    /// Initialized — mirrors `MysqlInstanceDto::socket_path`'s identical gate
    /// (spec D6).
    pub socket_path: Option<String>,
    pub service_id: Option<String>,
    pub datadir_state: MariadbDatadirStateDto,
    /// Whether THIS BUILD offers to install MariaDB on THIS host, and what it
    /// would install — see [`crate::mariadb_pkg::MariadbPackageOfferDto`] for
    /// the third state (`AwaitingRelease`) MySQL's own offer type does not
    /// need (design D2).
    pub offer: crate::mariadb_pkg::MariadbPackageOfferDto,
}

/// One line of a MariaDB package operation's output, forwarded live while it
/// runs. Same shape and reasoning as [`MysqlInstallLogEvent`] — carries
/// **no `major`/`series` field**, unlike its PHP/MySQL siblings: this build
/// ships exactly one series, so a field nothing can vary would be pure
/// overhead (the same reasoning `MariadbInstance` gives for leaving `major`
/// off its own struct).
///
/// In practice this channel is filled only by an uninstall's
/// `Removal::PackageTree` step failing to report through it — see
/// `uninstall::run::emit_uninstall_log`'s own doc comment for why it exists
/// as a real, registered channel even though MariaDB's ordinary uninstall
/// never streams through it either (no child process to stream from).
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MariadbInstallLogEvent {
    pub ts_ms: u64,
    pub stream: String,
    pub line: String,
}

/// One line of `initialize_mariadb`'s init sequence, relayed after the fact
/// on failure — see [`initialize_mariadb`]'s own doc comment for why this is
/// a post-hoc relay rather than a live stream, and [`MariadbInstallLogEvent`]
/// for why there is no `major`/`series` field.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MariadbInitLogEvent {
    pub ts_ms: u64,
    pub stream: String,
    pub line: String,
}

/// Mirrors `openvhost_core::mariadb::MariadbInitStep` 1:1 as a wire-safe
/// copy — the MariaDB counterpart of [`MysqlInitStepDto`]. No `Validate`
/// variant: MariaDB has no `--validate-config`, so there is no pre-flight
/// step to fail at (mirrors the core type's own doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum MariadbInitStepDto {
    Render,
    Initialize,
    StartTempServer,
    SetPassword,
    Shutdown,
    Finalize,
}

impl From<openvhost_core::mariadb::MariadbInitStep> for MariadbInitStepDto {
    fn from(s: openvhost_core::mariadb::MariadbInitStep) -> Self {
        use openvhost_core::mariadb::MariadbInitStep as S;
        match s {
            S::Render => Self::Render,
            S::Initialize => Self::Initialize,
            S::StartTempServer => Self::StartTempServer,
            S::SetPassword => Self::SetPassword,
            S::Shutdown => Self::Shutdown,
            S::Finalize => Self::Finalize,
        }
    }
}

/// Mirrors `openvhost_core::mariadb::MariadbInitOutcome` 1:1 as a wire-safe
/// copy (spec D7's `initialize_mariadb`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbInitOutcomeDto {
    Initialized,
    AlreadyInitialized,
    Foreign {
        detail: String,
    },
    Failed {
        step: MariadbInitStepDto,
        reason: String,
    },
}

impl From<openvhost_core::mariadb::MariadbInitOutcome> for MariadbInitOutcomeDto {
    fn from(o: openvhost_core::mariadb::MariadbInitOutcome) -> Self {
        use openvhost_core::mariadb::MariadbInitOutcome as O;
        match o {
            O::Initialized => Self::Initialized,
            O::AlreadyInitialized => Self::AlreadyInitialized,
            O::Foreign { detail } => Self::Foreign { detail },
            O::Failed { step, reason } => Self::Failed {
                step: step.into(),
                reason,
            },
        }
    }
}

/// `reset_mariadb_root_password`'s outcome — the MariaDB mirror of
/// [`MysqlResetOutcomeDto`], and the identical reasoning: auth failure is an
/// EXPECTED, renderable outcome (the stored password may be stale), never
/// thrown as an `IpcError`.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbResetOutcomeDto {
    Reset,
    AuthFailed { detail: String },
}

/// `verify_mariadb_connection`'s outcome — the MariaDB mirror of
/// [`MysqlConnectionProofDto`]: outcome-shaped, never an `IpcError`, so the
/// "Verify connection" button always has something to render.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MariadbConnectionProofDto {
    Ok { version: String, port: u32 },
    AuthFailed { detail: String },
    Failed { detail: String },
}

/// Build the MariaDB row — the single-instance mirror of `mysql_rows`.
fn mariadb_row(
    home: &Path,
    installed: Option<&openvhost_core::MariadbRuntime>,
) -> MariadbEnvironmentDto {
    let paths = openvhost_core::mariadb_paths(home);
    let datadir_state = classify_mariadb_datadir_dto(&paths.datadir);
    let registered =
        installed.is_some() && matches!(datadir_state, MariadbDatadirStateDto::Initialized { .. });
    MariadbEnvironmentDto {
        installed: installed.is_some(),
        version: installed.map(|rt| rt.version.clone()),
        path: installed.map(|rt| rt.mariadbd.display().to_string()),
        socket_path: registered.then(|| paths.socket.display().to_string()),
        service_id: registered
            .then(|| crate::stack::mariadb_service_id(openvhost_core::MARIADB_SERIES)),
        datadir_state,
        offer: crate::mariadb_pkg::package_offer(),
    }
}

/// Probe OpenVHost's own package tree for a MariaDB install, write the
/// result into the managed `RwLock`, and register a supervisor row when one
/// is found with an Initialized datadir — the single-series mirror of
/// `rescan_mysql_into_state`, called both from [`rescan_mariadb`] below and
/// from `uninstall::run::uninstall_package`'s post-uninstall reconciliation.
///
/// No seed parameter, unlike `rescan_mysql_into_state`: a seed exists only to
/// paper over an expensive post-install probe (a freshly `brew install`ed
/// `mysqld`'s first execution under Gatekeeper), and MariaDB's version is a
/// directory name chosen at install time, never probed at all — a rescan
/// immediately after `install_mariadb` already sees the fresh tree with
/// nothing to seed.
pub(crate) async fn rescan_mariadb_into_state(
    runtimes: &RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>,
    sup: &Supervisor,
    home: &Path,
) -> Result<Vec<openvhost_core::MariadbRuntime>, IpcError> {
    let root = openvhost_core::PackagesRoot::from_home(home);
    let found = openvhost_core::discover_mariadb(&root).runtimes;
    *runtimes.write().map_err(|_| IpcError::Core {
        message: "mariadb runtime list is poisoned".into(),
    })? = Some(found.clone());

    let id = crate::stack::mariadb_service_id(openvhost_core::MARIADB_SERIES);
    let already_registered = sup.snapshot().into_iter().any(|s| s.id == id);
    match found.first() {
        Some(rt) if !already_registered && crate::stack::mariadb_datadir_is_initialized(home) => {
            sup.register(crate::stack::mariadb_spec(home, rt));
        }
        _ => {}
    }
    // Mirrors `unregister_vanished`'s reasoning for a single row (design D5):
    // an in-app uninstall or a manually removed package tree must converge
    // the Services page on the next rescan exactly as PHP/MySQL's already do.
    if found.is_empty() && already_registered {
        let _ = sup.unregister(&id);
    }
    Ok(found)
}

/// Look up the cached, already-discovered MariaDB runtime — the single-series
/// mirror of `find_mysql_runtime`: there is no major to match against, only
/// "is anything installed at all".
fn find_mariadb_runtime(
    runtimes: &RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>,
) -> Result<openvhost_core::MariadbRuntime, IpcError> {
    runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "mariadb runtime list is poisoned".into(),
        })?
        .as_ref()
        .and_then(|rts| rts.first().cloned())
        .ok_or_else(|| IpcError::Core {
            message: format!(
                "MariaDB {} is not installed",
                openvhost_core::MARIADB_SERIES
            ),
        })
}

/// Pre-flight for [`initialize_mariadb`]: the store, then the runtime.
///
/// Split out for the same reason [`initialize_mysql_gate`] is, and it is the
/// MariaDB half of the same design-D2 requirement — the command takes an
/// `AppHandle<Wry>`, which `tauri::test::mock_builder` cannot produce, so its
/// body is unreachable from a test and a refusal written inline there is a
/// decision **no test can see**.
///
/// The store check is FIRST, and this whole function runs before the command
/// spawns anything or derives a single path (it takes no `home` at all), so an
/// unavailable store is refused with the datadir untouched. Degrading instead
/// would leave a real, initialized datadir whose generated root password was
/// never persisted — unrecoverable, and the exact hazard
/// `verify_mysql_connection` already documents.
fn initialize_mariadb_gate<'a>(
    db: &'a DbHandle,
    runtimes: &RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>,
) -> Result<(&'a Db, openvhost_core::MariadbRuntime), IpcError> {
    let store = db.require()?;
    let runtime = find_mariadb_runtime(runtimes)?;
    Ok((store, runtime))
}

/// Tag `InstallLock`'s shared slot as a MariaDB datadir initialization.
///
/// A named function rather than an inlined `set_running` call inside
/// [`initialize_mariadb`], for the identical audit F1 reason
/// `set_running_mysql_init` is one: [`initialize_mariadb`] takes an
/// `AppHandle<Wry>`, which `tauri::test::mock_builder` cannot produce, so its
/// body is unreachable from a test and an inlined tag would be a value no
/// test can see.
fn set_running_mariadb_init(lock: &InstallLock, abort: tokio::task::AbortHandle) {
    let (kind, operation) = MARIADB_INIT_RUN;
    lock.set_running(
        kind,
        operation,
        format!("MariaDB {}", openvhost_core::MARIADB_SERIES),
        abort,
    );
}

fn emit_mariadb_init_log(app: &tauri::AppHandle, stream: &str, line: String) {
    let _ = MariadbInitLogEvent {
        ts_ms: now_ms(),
        stream: stream.to_string(),
        line,
    }
    .emit(app);
}

#[tauri::command]
#[specta::specta]
pub async fn mariadb_environment(
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<MariadbEnvironmentDto, IpcError> {
    let p = stack_paths(&paths)?;
    let installed = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "mariadb runtime list is poisoned".into(),
        })?
        .clone()
        .unwrap_or_default();
    Ok(mariadb_row(&p.home, installed.first()))
}

/// The explicit, user-initiated re-probe behind the Databases page's rescan
/// affordance — mirrors `rescan_mysql`, including blocking on `InstallLock`
/// for the identical reason (a rescan racing a completed install must never
/// silently revert it).
#[tauri::command]
#[specta::specta]
pub async fn rescan_mariadb(
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<MariadbEnvironmentDto, IpcError> {
    let p = stack_paths(&paths)?;
    let _guard = lock.inner().guard.lock().await;
    let found = rescan_mariadb_into_state(runtimes.inner(), sup.inner(), &p.home).await?;
    Ok(mariadb_row(&p.home, found.first()))
}

/// Scrub a freshly generated password out of a `Failed` init outcome's
/// `reason` before it can reach [`MariadbInitLogEvent`] or the returned
/// [`MariadbInitOutcomeDto`] — defence in depth, the MariaDB counterpart of
/// `redact`'s own reasoning for `run_mysql_init`'s SetPassword/Shutdown
/// steps (see `redact`'s doc comment).
///
/// SECURITY (audit Low 3). `openvhost_core::mariadb::initialize_mariadb`
/// pairs EVERY `Failed` outcome with `password: None` — its own doc comment
/// says the password is returned only alongside `Initialized` — so
/// `password` is never `Some` here against a real run today: the temp
/// server's log tail is captured before the password is generated
/// (`mariadb::init.rs:839` vs `:850`), and the one step where a live
/// password exists (`SetPassword`) quotes only the client's own stderr,
/// never the SQL sent over stdin. The gap that reasoning does not close: a
/// MariaDB client echoing a statement fragment back in a `near '...'`
/// syntax error, which today's pure-hex generator cannot trigger but
/// `root_password_sql`'s own doc comment already names as the assumption a
/// future user-chosen password would break. Redacting here costs nothing on
/// the expected path (nothing to replace) and closes that gap the moment it
/// stops being hypothetical.
///
/// Split out of [`initialize_mariadb`] for the identical testability reason
/// `initialize_mysql_gate` is: the command takes an `AppHandle<Wry>`, which
/// `tauri::test::mock_builder` cannot produce, so the command's own body is
/// unreachable from a test. This function needs no `AppHandle`.
fn redact_mariadb_init_outcome(
    outcome: openvhost_core::mariadb::MariadbInitOutcome,
    password: &Option<openvhost_core::mysql::RootPassword>,
) -> openvhost_core::mariadb::MariadbInitOutcome {
    use openvhost_core::mariadb::MariadbInitOutcome as O;
    match outcome {
        O::Initialized => O::Initialized,
        O::AlreadyInitialized => O::AlreadyInitialized,
        O::Foreign { detail } => O::Foreign { detail },
        O::Failed { step, reason } => O::Failed {
            step,
            reason: match password {
                Some(password) => redact(&reason, password.expose()),
                None => reason,
            },
        },
    }
}

/// Drives MariaDB's staged init (spec D7) via
/// `openvhost_core::mariadb::initialize_mariadb`, which — unlike MySQL's
/// `run_mysql_init` — already owns its whole staged sequence (slice A; see
/// that module's own doc comment for why the driver lives in openvhost-core
/// for this engine and not in this file). This command is therefore thin
/// wiring: look the discovered runtime up, spawn the core function so its
/// `AbortHandle` can be registered on the shared `InstallLock`
/// (`MARIADB_INIT_RUN`, D4/F1), persist the generated password on success,
/// then register the service row.
///
/// **No live per-line log.** Unlike `run_mysql_init`, the core function
/// reports no intermediate output — it returns a single terminal outcome —
/// so there is nothing to stream WHILE a run is in progress. A `Failed`
/// outcome's `reason` DOES carry real diagnostic content (it already
/// includes the temp server's own log tail; see `mariadb::init`'s
/// `server_log`), so that is relayed through [`MariadbInitLogEvent`] once
/// the run ends, rather than left unsent — a post-hoc relay of something
/// real, never a fabricated live one. [`redact_mariadb_init_outcome`] runs
/// first, so a redacted `reason` is what BOTH that relay and the returned
/// DTO ever see (audit Low 3).
#[tauri::command]
#[specta::specta]
pub async fn initialize_mariadb(
    app: tauri::AppHandle,
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<MariadbInitOutcomeDto, IpcError> {
    let p = stack_paths(&paths)?;

    let Ok(_guard) = lock.inner().guard.try_lock() else {
        return Err(IpcError::Core {
            message: "an install is already running".into(),
        });
    };

    // Both refusals in one pre-flight, ahead of the context and the spawn —
    // see [`initialize_mariadb_gate`] for why the store check lives there and
    // not inline here.
    let (store, runtime) = initialize_mariadb_gate(&db, runtimes.inner())?;
    let ctx = openvhost_core::mariadb::MariadbInitCtx::new(&p.home, runtime.clone());

    let init_task = tokio::spawn(async move {
        openvhost_core::mariadb::initialize_mariadb(&ctx, openvhost_proc::default_driver()).await
    });
    let abort_handle = init_task.abort_handle();
    set_running_mariadb_init(lock.inner(), abort_handle.clone());
    let _running_guard = RunningInstallGuard {
        lock: lock.inner(),
        abort: abort_handle,
    };

    let (outcome, password) = match init_task.await {
        Ok(result) => result,
        Err(join_err) if join_err.is_cancelled() => {
            return Err(IpcError::Proc {
                message: "the initialization was aborted because the app is quitting".into(),
            });
        }
        Err(join_err) => {
            return Err(IpcError::Proc {
                message: format!("the initialization task ended unexpectedly: {join_err}"),
            });
        }
    };
    let outcome = redact_mariadb_init_outcome(outcome, &password);

    if let openvhost_core::mariadb::MariadbInitOutcome::Failed { reason, .. } = &outcome {
        for line in reason.lines() {
            emit_mariadb_init_log(&app, "stderr", line.to_string());
        }
    }

    if let (openvhost_core::mariadb::MariadbInitOutcome::Initialized, Some(password)) =
        (&outcome, &password)
    {
        openvhost_core::mariadb::MariadbInstanceRepo::new(store)
            .upsert(password)
            .await?;
        sup.register(crate::stack::mariadb_spec(&p.home, &runtime));
    }

    Ok(outcome.into())
}

/// The stored root password for MariaDB (spec D7's outbound reveal) — the
/// MariaDB mirror of [`mysql_root_password`], and the identical SECURITY
/// discipline: audit H2's rule that this is the SOLE place sanctioned to
/// de-redact a `RootPassword` into a plain `String` for a RETURN value
/// applies here too, unchanged — see that command's own doc comment.
#[tauri::command]
#[specta::specta]
pub async fn mariadb_root_password(db: tauri::State<'_, DbHandle>) -> Result<String, IpcError> {
    let db = db.require()?;
    let repo = openvhost_core::mariadb::MariadbInstanceRepo::new(db);
    let instance = repo.get().await?.ok_or_else(|| IpcError::Core {
        message: format!(
            "no stored root password for MariaDB {}",
            openvhost_core::MARIADB_SERIES
        ),
    })?;
    Ok(instance.root_password.expose().to_string())
}

/// Regenerate MariaDB's root password: authenticates with the STORED (old)
/// password via an ephemeral 0600 defaults-file, runs
/// `openvhost_core::mariadb::root_password_sql` over stdin with a freshly
/// generated password, and — only once that succeeds — persists the new
/// value. The MariaDB mirror of [`reset_mysql_root_password`], with one
/// deliberate difference: it runs MariaDB's own multi-statement
/// `root_password_sql`, not MySQL's single-statement `alter_user_sql`.
/// `mariadb::init`'s own doc comment measured root existing at FOUR hosts
/// after this build's init (`localhost`, `127.0.0.1`, `::1`, plus a removed
/// hostname row), so a single `ALTER USER 'root'@'localhost'` would leave the
/// other two loopback accounts on their OLD password after a reset — the
/// exact hole that comment documents finding.
#[tauri::command]
#[specta::specta]
pub async fn reset_mariadb_root_password(
    db: tauri::State<'_, DbHandle>,
    paths: tauri::State<'_, Option<StackPaths>>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>,
) -> Result<MariadbResetOutcomeDto, IpcError> {
    // First, exactly as in `reset_mysql_root_password`: a reset that cannot
    // persist the new password must not run the SQL that sets it either.
    let db = db.require()?;
    let p = stack_paths(&paths)?;
    let runtime = find_mariadb_runtime(runtimes.inner())?;
    let mp = openvhost_core::mariadb_paths(&p.home);

    let repo = openvhost_core::mariadb::MariadbInstanceRepo::new(db);
    let current = repo.get().await?.ok_or_else(|| IpcError::Core {
        message: format!(
            "MariaDB {} has no stored credential to reset",
            openvhost_core::MARIADB_SERIES
        ),
    })?;

    let defaults_file =
        EphemeralDefaultsFile::write(&mp.socket, &current.root_password).map_err(|e| {
            IpcError::Core {
                message: format!("failed to write the ephemeral credential file: {e}"),
            }
        })?;

    let new_password = openvhost_core::mysql::generate_root_password();
    let sql = openvhost_core::mariadb::root_password_sql(&new_password);
    // `mysql_exec_with_defaults_file` is reused in place: it is generic in
    // what it runs (a `--defaults-file`-authenticated batch script over
    // stdin against whatever binary `mysql_bin` names), and MariaDB's own
    // `mariadb` client accepts the same `--defaults-file`/`--batch`/
    // `--skip-column-names` flags this function builds — see
    // `mariadb_admin_ping_argv`'s doc comment for the ONE flag that
    // genuinely differs between the two clients (`--no-login-paths`, which
    // this function never uses either).
    let result = crate::mysql_admin::mysql_exec_with_defaults_file(
        &runtime.mariadb,
        &defaults_file.path,
        &sql,
    )
    .await;
    drop(defaults_file); // RAII delete, before acting on the result.
    let secrets = [current.root_password.expose(), new_password.expose()];

    let outcome = result.map_err(|e| IpcError::Core {
        message: redact_all(&e.to_string(), &secrets),
    })?;
    if outcome.ok {
        repo.upsert(&new_password).await?;
        Ok(MariadbResetOutcomeDto::Reset)
    } else if looks_like_auth_failure(&outcome.stderr) {
        Ok(MariadbResetOutcomeDto::AuthFailed {
            detail: redact_all(&outcome.stderr, &secrets),
        })
    } else {
        Err(IpcError::Core {
            message: redact_all(
                &if outcome.stderr.trim().is_empty() {
                    "ALTER USER failed".to_string()
                } else {
                    outcome.stderr
                },
                &secrets,
            ),
        })
    }
}

/// `SELECT VERSION(), @@port` through the running server's socket,
/// authenticating with the stored credential via an ephemeral 0600
/// defaults-file — the MariaDB mirror of [`verify_mysql_connection`].
#[tauri::command]
#[specta::specta]
pub async fn verify_mariadb_connection(
    db: tauri::State<'_, DbHandle>,
    paths: tauri::State<'_, Option<StackPaths>>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>,
) -> Result<MariadbConnectionProofDto, IpcError> {
    // `Failed { detail }` rather than `Err`, and first — the mirror of
    // `verify_mysql_connection`'s own refusal, following the precedent its
    // "no stored root password" branch below already sets for this page.
    let db = match db.require() {
        Ok(db) => db,
        Err(e) => {
            return Ok(MariadbConnectionProofDto::Failed {
                detail: e.to_string(),
            });
        }
    };
    let p = stack_paths(&paths)?;
    let runtime = find_mariadb_runtime(runtimes.inner())?;
    let mp = openvhost_core::mariadb_paths(&p.home);

    let repo = openvhost_core::mariadb::MariadbInstanceRepo::new(db);
    let Some(instance) = repo.get().await? else {
        return Ok(MariadbConnectionProofDto::Failed {
            detail: format!(
                "no stored root password for MariaDB {} — initialize it, or reset the \
                 password if the folder is already initialized",
                openvhost_core::MARIADB_SERIES
            ),
        });
    };
    let secret = instance.root_password.expose().to_string();

    let defaults_file = match EphemeralDefaultsFile::write(&mp.socket, &instance.root_password) {
        Ok(f) => f,
        Err(e) => {
            return Err(IpcError::Core {
                message: format!("failed to write the ephemeral credential file: {e}"),
            });
        }
    };

    let result = crate::mysql_admin::mysql_exec_with_defaults_file(
        &runtime.mariadb,
        &defaults_file.path,
        "SELECT VERSION(), @@port;",
    )
    .await;
    drop(defaults_file); // RAII delete, before acting on the result.

    let outcome = match result {
        Ok(o) => o,
        Err(e) => {
            return Ok(MariadbConnectionProofDto::Failed {
                detail: redact(&e.to_string(), &secret),
            });
        }
    };

    if !outcome.ok {
        return Ok(if looks_like_auth_failure(&outcome.stderr) {
            MariadbConnectionProofDto::AuthFailed {
                detail: redact(&outcome.stderr, &secret),
            }
        } else {
            MariadbConnectionProofDto::Failed {
                detail: redact(
                    &if outcome.stderr.trim().is_empty() {
                        "the connection attempt failed".to_string()
                    } else {
                        outcome.stderr
                    },
                    &secret,
                ),
            }
        });
    }

    Ok(match parse_version_and_port(&outcome.stdout) {
        Some((version, port)) => MariadbConnectionProofDto::Ok { version, port },
        None => MariadbConnectionProofDto::Failed {
            detail: redact(
                &format!("could not parse a version/port from: {:?}", outcome.stdout),
                &secret,
            ),
        },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod mariadb_ipc_tests {
    use tauri::Manager;

    use super::*;
    use openvhost_proc::{Supervisor, default_driver};

    /// Warmed at creation, outside the `PROBE_TIMEOUT`-bounded calls these
    /// tests then time — see [`crate::tests_support`] for what that costs and
    /// why every fixture helper in this workspace does it.
    fn fake_cli(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        crate::tests_support::write_exec_fixture(&p, body);
        p
    }

    fn fake_runtime(
        home: &Path,
        version: &str,
        mariadb: PathBuf,
    ) -> openvhost_core::MariadbRuntime {
        openvhost_core::MariadbRuntime {
            series: openvhost_core::MARIADB_SERIES,
            version: version.to_string(),
            mariadbd: home.join("mariadbd"),
            mariadb,
            mariadb_admin: home.join("mariadb-admin"),
        }
    }

    /// Lay down `<home>/packages/mariadb/11.4/<version>/bin/{mariadbd,
    /// mariadb,mariadb-admin}` and swing `current` onto it — the minimum
    /// shape `discover_mariadb`'s "all three or nothing" rule requires.
    fn install_fake_mariadb_package(home: &Path, version: &str) {
        let dir = home.join("packages/mariadb/11.4").join(version).join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["mariadbd", "mariadb", "mariadb-admin"] {
            std::fs::write(dir.join(name), "#!/bin/sh\nexit 0\n").unwrap();
        }
        let current = home.join("packages/mariadb/11.4/current");
        #[cfg(unix)]
        std::os::unix::fs::symlink(version, &current).unwrap();
    }

    fn app_with(lock: InstallLock) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(lock);
        app
    }

    /// Whether a spawned run is still going — see `mysql_ipc_tests`'s
    /// identical helper for why this is not a bare `is_finished()` check.
    async fn still_running(task: &mut tokio::task::JoinHandle<()>) -> bool {
        tokio::time::timeout(std::time::Duration::from_millis(200), task)
            .await
            .is_err()
    }

    // ------------------------------------------------------------------
    // Group 1 — the discriminators (D4/F1). THE required property: Cancel
    // on one engine's install must never abort the other's.
    // ------------------------------------------------------------------

    #[test]
    fn mariadbs_install_pair_is_distinct_from_mysqls_on_the_kind_discriminator() {
        assert_ne!(MARIADB_INSTALL_RUN, MYSQL_INSTALL_RUN);
        assert_ne!(MARIADB_INSTALL_RUN.0, MYSQL_INSTALL_RUN.0);
        // Same operation on both — it really is the KIND that must differ,
        // not merely the pair as an opaque whole.
        assert_eq!(MARIADB_INSTALL_RUN.1, MYSQL_INSTALL_RUN.1);
    }

    #[test]
    fn a_mariadb_init_is_not_tagged_as_an_install() {
        assert_ne!(
            MARIADB_INIT_RUN, MARIADB_INSTALL_RUN,
            "an initialization tagged as an install is cancellable by \
             cancel_mariadb_install, whatever its label says"
        );
        assert_eq!(MARIADB_INIT_RUN.1, PackageOperation::Initialize);
    }

    /// THE property this task exists to prove, both directions in one test
    /// so neither can be fixed at the expense of the other.
    ///
    /// VACUITY (neuter-and-watch-it-fail): retagging `MARIADB_INSTALL_RUN` to
    /// `(InstallKind::Mysql, PackageOperation::Install)` — i.e. sharing
    /// MySQL's pair, the audit F1 shape — turned BOTH halves of this test
    /// red: direction 1's `abort_running_if(mariadb_kind, ...)` then matched
    /// the running MySQL install and aborted it, and direction 2's own
    /// MariaDB install could then be stopped by `MYSQL_INSTALL_RUN` too
    /// (the pairs were now identical). Restoring the distinct
    /// `InstallKind::Mariadb` discriminator made it pass again.
    #[tokio::test]
    async fn a_mariadb_install_and_a_mysql_install_cannot_abort_each_other() {
        // Direction 1: a MySQL install is running; MariaDB's cancel must not
        // touch it.
        {
            let lock = InstallLock::default();
            let mut mysql_install = tokio::spawn(std::future::pending::<()>());
            let (kind, operation) = MYSQL_INSTALL_RUN;
            lock.set_running(
                kind,
                operation,
                "MySQL 8.4".to_string(),
                mysql_install.abort_handle(),
            );

            let (mariadb_kind, mariadb_operation) = MARIADB_INSTALL_RUN;
            assert!(
                !lock.abort_running_if(mariadb_kind, mariadb_operation),
                "a MariaDB-install cancel claimed it stopped a running MySQL install"
            );
            assert!(
                still_running(&mut mysql_install).await,
                "a MariaDB-install cancel ABORTED a running MySQL install"
            );
            mysql_install.abort();
        }

        // Direction 2: a MariaDB install is running; MySQL's cancel must not
        // touch it.
        {
            let lock = InstallLock::default();
            let mut mariadb_install = tokio::spawn(std::future::pending::<()>());
            let (kind, operation) = MARIADB_INSTALL_RUN;
            lock.set_running(
                kind,
                operation,
                "MariaDB 11.4".to_string(),
                mariadb_install.abort_handle(),
            );

            let (mysql_kind, mysql_operation) = MYSQL_INSTALL_RUN;
            assert!(
                !lock.abort_running_if(mysql_kind, mysql_operation),
                "a MySQL-install cancel claimed it stopped a running MariaDB install"
            );
            assert!(
                still_running(&mut mariadb_install).await,
                "a MySQL-install cancel ABORTED a running MariaDB install"
            );
            mariadb_install.abort();
        }
    }

    /// Non-vacuity twin: MariaDB's own cancel must still stop MariaDB's own
    /// install. Without this, a `cancel_mariadb_install` that aborted
    /// nothing whatsoever would pass the isolation test above.
    #[tokio::test]
    async fn the_same_cancel_still_stops_a_real_mariadb_install() {
        let lock = InstallLock::default();
        let install = tokio::spawn(std::future::pending::<()>());
        let (kind, operation) = MARIADB_INSTALL_RUN;
        lock.set_running(
            kind,
            operation,
            "MariaDB 11.4".to_string(),
            install.abort_handle(),
        );
        let app = app_with(lock);

        assert!(
            crate::mariadb_pkg::cancel_mariadb_install(app.state::<InstallLock>())
                .await
                .unwrap()
        );

        match tokio::time::timeout(std::time::Duration::from_secs(5), install)
            .await
            .expect("the install did not settle after its own cancel fired")
        {
            Err(join_err) => assert!(join_err.is_cancelled(), "got {join_err:?}"),
            Ok(()) => panic!("the cancel returned true but the install ran to completion"),
        }
    }

    /// The audit F1 shape reproduced for MariaDB's own init: a Cancel that
    /// targets an INSTALL must not touch an INITIALIZATION, even for the
    /// same engine.
    #[tokio::test]
    async fn a_running_mariadb_init_survives_the_databases_page_install_cancel() {
        let lock = InstallLock::default();
        let mut init = tokio::spawn(std::future::pending::<()>());
        set_running_mariadb_init(&lock, init.abort_handle());

        let (kind, operation) = MARIADB_INSTALL_RUN;
        assert!(
            !lock.abort_running_if(kind, operation),
            "the Databases page's install-cancel claimed it stopped a datadir initialization"
        );
        assert!(
            still_running(&mut init).await,
            "the Databases page's install-cancel ABORTED a datadir initialization"
        );
        init.abort();
    }

    #[tokio::test]
    async fn an_initialization_is_tagged_as_one_and_labelled_plainly() {
        let lock = InstallLock::default();
        let init = tokio::spawn(std::future::pending::<()>());
        set_running_mariadb_init(&lock, init.abort_handle());

        assert_eq!(
            lock.running_install(),
            Some((
                InstallKind::Mariadb,
                PackageOperation::Initialize,
                "MariaDB 11.4".to_string()
            ))
        );
        init.abort();
    }

    // ------------------------------------------------------------------
    // Group 2 — the offer and outcome DTOs.
    // ------------------------------------------------------------------

    #[test]
    fn every_mariadb_init_outcome_state_serializes_distinctly() {
        let tag = |v: &MariadbInitOutcomeDto| serde_json::to_value(v).unwrap()["kind"].clone();
        let all = [
            MariadbInitOutcomeDto::Initialized,
            MariadbInitOutcomeDto::AlreadyInitialized,
            MariadbInitOutcomeDto::Foreign { detail: "x".into() },
            MariadbInitOutcomeDto::Failed {
                step: MariadbInitStepDto::SetPassword,
                reason: "boom".into(),
            },
        ];
        let tags: Vec<_> = all.iter().map(tag).collect();
        for (i, a) in tags.iter().enumerate() {
            for (j, b) in tags.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{:?} and {:?} share a tag", all[i], all[j]);
                }
            }
        }
    }

    /// SECURITY (audit Low 3), defence in depth: [`redact_mariadb_init_outcome`]
    /// must scrub a generated password out of a `Failed` reason before it can
    /// reach either consumer — `reason.lines()`'s per-line
    /// `MariadbInitLogEvent`s, and the `MariadbInitOutcomeDto` `.into()`
    /// produces — even though that function's own doc comment explains why
    /// today's core `initialize_mariadb` never actually pairs `Failed` with
    /// `Some(password)`. Exercised directly rather than through the full
    /// `initialize_mariadb` command for the same `AppHandle<Wry>` reason
    /// [`redact_mariadb_init_outcome`] is split out at all.
    ///
    /// VACUITY: temporarily changed the `Some(password)` arm to return
    /// `reason` unredacted — this test failed with the password still
    /// present, both assertions below firing.
    #[test]
    fn redact_mariadb_init_outcome_scrubs_a_generated_password_from_a_failure_reason() {
        let password = openvhost_core::mysql::generate_root_password();
        let outcome = openvhost_core::mariadb::MariadbInitOutcome::Failed {
            step: openvhost_core::mariadb::MariadbInitStep::SetPassword,
            reason: format!(
                "failed to set the root password: ERROR 1064 (42000): You have an error in \
                 your SQL syntax; check the manual … near '{}' at line 2",
                password.expose()
            ),
        };

        let redacted = redact_mariadb_init_outcome(outcome, &Some(password.clone()));

        // Reaches `MariadbInitLogEvent` as `reason.lines()` — asserted
        // directly on the (redacted) reason string `initialize_mariadb`
        // emits line-by-line.
        match &redacted {
            openvhost_core::mariadb::MariadbInitOutcome::Failed { reason, .. } => {
                assert!(
                    !reason.contains(password.expose()),
                    "the generated password leaked into a Failed init reason: {reason:?}"
                );
                assert!(reason.contains("<redacted>"), "got {reason:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }

        // Reaches the returned DTO as a straight field copy of `reason` (see
        // `From<MariadbInitOutcome> for MariadbInitOutcomeDto>`), so
        // asserting on `redacted` itself — what `initialize_mariadb` passes
        // to `.into()` — is enough to cover it too.
        match redacted.into() {
            MariadbInitOutcomeDto::Failed { reason, .. } => {
                assert!(!reason.contains(password.expose()), "got {reason:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // Group 3 — environment/rescan, against a real fake package tree.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn mariadb_environment_reports_not_installed_when_nothing_is_discovered() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(RwLock::<Option<Vec<openvhost_core::MariadbRuntime>>>::new(
            None,
        ));

        let env = mariadb_environment(
            app.state::<RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>(),
            app.state::<Option<StackPaths>>(),
        )
        .await
        .unwrap();
        assert!(!env.installed);
        assert!(env.version.is_none());
        assert!(env.service_id.is_none());
        assert!(env.socket_path.is_none());
        assert!(matches!(
            env.datadir_state,
            MariadbDatadirStateDto::NotInitialized
        ));
    }

    /// VACUITY: run against a home with NO fake package laid down first —
    /// `found.len()` was 0 and this failed, confirming the fixture (not an
    /// always-true `discover_mariadb` stub) is what makes it pass.
    #[tokio::test]
    async fn rescan_discovers_a_packaged_install_and_reports_it_uninitialized() {
        let home = tempfile::tempdir().unwrap();
        install_fake_mariadb_package(home.path(), "11.4.9");

        let sup = Supervisor::new(default_driver());
        let runtimes: RwLock<Option<Vec<openvhost_core::MariadbRuntime>>> = RwLock::new(None);

        let found = rescan_mariadb_into_state(&runtimes, &sup, home.path())
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version, "11.4.9");

        let env = mariadb_row(home.path(), found.first());
        assert!(env.installed);
        assert_eq!(env.version.as_deref(), Some("11.4.9"));
        // The datadir was never initialized, so no supervisor row registered
        // and no socket path reported — "installed" is not "running".
        assert!(env.service_id.is_none());
        assert!(env.socket_path.is_none());
        assert!(sup.snapshot().is_empty());
    }

    // ------------------------------------------------------------------
    // Group 4 — the credential never reaches argv (plan Global Constraints
    // SECRETS block).
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn mariadb_root_password_returns_the_stored_value() {
        let db = Db::open_in_memory().await.unwrap();
        let password = openvhost_core::mysql::generate_root_password();
        openvhost_core::mariadb::MariadbInstanceRepo::new(&db)
            .upsert(&password)
            .await
            .unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, db);

        let returned = mariadb_root_password(app.state::<DbHandle>())
            .await
            .unwrap();
        assert_eq!(returned, password.expose());
    }

    // ---- REFUSE with no store (optional-state.db design D2) --------------

    /// The MariaDB mirror of the MySQL initialization refusal, and the same
    /// stakes: an initialized datadir whose generated root password was never
    /// persisted is unrecoverable. Driven through the real pre-flight, because
    /// `initialize_mariadb` takes an `AppHandle<Wry>` and cannot be called
    /// from a test at all — see [`initialize_mariadb_gate`].
    ///
    /// A runtime IS present in the lock below, so the gate would otherwise
    /// return `Ok`: what this pins is that the STORE check comes first and
    /// wins.
    ///
    /// VACUITY, and the honest form of it: `let store = db.require()?;` cannot
    /// be *deleted* from this gate, because it is the only way to obtain the
    /// `&'a Db` the signature returns — there is no degraded version of this
    /// function to write. That is design D6 rather than a gap in the test. The
    /// neuter used instead was `store_down()` handing back a real
    /// `DbHandle::Ready`, under which this test failed on `expect`. Reverted
    /// and re-run green. See `assert_store_refusal`'s doc comment.
    #[tokio::test]
    async fn initialize_mariadb_refuses_before_touching_the_datadir_when_the_store_is_down() {
        let home = tempfile::tempdir().unwrap();
        let runtimes = RwLock::new(Some(vec![openvhost_core::MariadbRuntime {
            series: openvhost_core::MARIADB_SERIES,
            version: "11.4.9".to_string(),
            mariadbd: home.path().join("mariadbd"),
            mariadb: home.path().join("mariadb"),
            mariadb_admin: home.path().join("mariadb-admin"),
        }]));

        let err = initialize_mariadb_gate(&store_down(), &runtimes)
            .err()
            .expect("an unavailable store must refuse");
        assert_store_refusal(&err, "initialize_mariadb");

        // Not "an error came back": nothing was created. The gate takes no
        // `home` at all, so it cannot have derived a path — these assertions
        // pin that the refusal happens while that is still true.
        let paths = openvhost_core::mariadb_paths(home.path());
        assert!(!paths.datadir.exists(), "the datadir must not exist");
        assert!(
            !paths.staging_parent.exists(),
            "no staging parent may have been created"
        );
        assert!(
            !paths.my_cnf.exists(),
            "no config may have been rendered for a refused initialization"
        );
        assert!(
            !home.path().join("run").exists(),
            "no run directory may have been created"
        );
    }

    /// The two MariaDB credential commands that answer with `Err`, grouped and
    /// named — the mirror of the MySQL group.
    ///
    /// Vacuity: with all 13 `let db = db.require()?;` guards replaced by
    /// `let db = &Db::open_in_memory().await.unwrap();`, this test failed —
    /// both commands then answer with their own "no stored credential" errors,
    /// which `assert_store_refusal` rejects for not carrying the sentinel
    /// reason. Reverted and re-run green.
    #[tokio::test]
    async fn the_mariadb_credential_commands_refuse_and_name_why_when_the_store_is_down() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(RwLock::new(Some(vec![openvhost_core::MariadbRuntime {
            series: openvhost_core::MARIADB_SERIES,
            version: "11.4.9".to_string(),
            mariadbd: home.path().join("mariadbd"),
            mariadb: home.path().join("mariadb"),
            mariadb_admin: home.path().join("mariadb-admin"),
        }])));

        assert_store_refusal(
            &mariadb_root_password(app.state()).await.unwrap_err(),
            "mariadb_root_password",
        );
        assert_store_refusal(
            &reset_mariadb_root_password(
                app.state(),
                app.state(),
                app.state::<RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>(),
            )
            .await
            .unwrap_err(),
            "reset_mariadb_root_password",
        );
        assert!(
            !home.path().join("run").exists(),
            "reset must not have written a credential file before refusing"
        );
    }

    /// `verify_mariadb_connection` refuses as `Failed { detail }`, not `Err` —
    /// the mirror of `verify_mysql_connection`, following the same
    /// no-stored-password precedent this page already sets.
    ///
    /// Vacuity: with the refusal rewritten as `let db = db.require()?;` —
    /// i.e. an `Err` instead of a `Failed` — this test failed on `unwrap()`,
    /// reporting the store refusal as an `Err(Core { .. })`. Reverted and
    /// re-run green.
    #[tokio::test]
    async fn verify_mariadb_connection_refuses_as_failed_not_err_when_the_store_is_down() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(RwLock::new(Some(vec![openvhost_core::MariadbRuntime {
            series: openvhost_core::MARIADB_SERIES,
            version: "11.4.9".to_string(),
            mariadbd: home.path().join("mariadbd"),
            mariadb: home.path().join("mariadb"),
            mariadb_admin: home.path().join("mariadb-admin"),
        }])));

        let proof = verify_mariadb_connection(
            app.state(),
            app.state(),
            app.state::<RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>(),
        )
        .await
        .unwrap();

        match proof {
            MariadbConnectionProofDto::Failed { detail } => {
                assert!(
                    detail.contains(STORE_DOWN_REASON),
                    "the rendered detail must carry the reason: {detail:?}"
                );
                assert!(
                    !detail.contains(".manage()"),
                    "a user must never be told to call a Rust API: {detail:?}"
                );
            }
            other => panic!("expected Failed (the no-stored-password precedent), got {other:?}"),
        }
    }

    /// The mandated test: drive the REAL command, against a fake `mariadb`
    /// that records its own argv (never its stdin) to a side file, and
    /// assert the stored (OLD) password never appears in it.
    #[tokio::test]
    async fn reset_mariadb_never_puts_the_password_on_argv() {
        let home = tempfile::tempdir().unwrap();
        let evidence = home.path().join("argv.txt");

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        let old_password = openvhost_core::mysql::generate_root_password();
        openvhost_core::mariadb::MariadbInstanceRepo::new(&db)
            .upsert(&old_password)
            .await
            .unwrap();
        manage_db(&app, db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        let fake_mariadb = fake_cli(
            home.path(),
            "mariadb",
            &format!(r#"echo "$@" >> "{}"; exit 0"#, evidence.display()),
        );
        app.manage(RwLock::new(Some(vec![fake_runtime(
            home.path(),
            "11.4.9",
            fake_mariadb,
        )])));

        let outcome = reset_mariadb_root_password(
            app.state::<DbHandle>(),
            app.state::<Option<StackPaths>>(),
            app.state::<RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>(),
        )
        .await
        .unwrap();
        assert!(matches!(outcome, MariadbResetOutcomeDto::Reset));

        let argv_seen = std::fs::read_to_string(&evidence).unwrap();
        assert!(
            !argv_seen.contains(old_password.expose()),
            "the stored password reached argv: {argv_seen:?}"
        );
        assert!(argv_seen.contains("--defaults-file="), "got {argv_seen:?}");
        assert!(argv_seen.contains("--batch"), "got {argv_seen:?}");
        assert!(!argv_seen.contains("--password"), "got {argv_seen:?}");
    }

    #[tokio::test]
    async fn verify_mariadb_connection_never_puts_the_password_on_argv() {
        let home = tempfile::tempdir().unwrap();
        let evidence = home.path().join("argv.txt");

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        let password = openvhost_core::mysql::generate_root_password();
        openvhost_core::mariadb::MariadbInstanceRepo::new(&db)
            .upsert(&password)
            .await
            .unwrap();
        manage_db(&app, db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        let fake_mariadb = fake_cli(
            home.path(),
            "mariadb",
            &format!(
                "echo \"$@\" >> \"{}\"; printf '11.4.9\\t3307\\n'",
                evidence.display()
            ),
        );
        app.manage(RwLock::new(Some(vec![fake_runtime(
            home.path(),
            "11.4.9",
            fake_mariadb,
        )])));

        let outcome = verify_mariadb_connection(
            app.state::<DbHandle>(),
            app.state::<Option<StackPaths>>(),
            app.state::<RwLock<Option<Vec<openvhost_core::MariadbRuntime>>>>(),
        )
        .await
        .unwrap();
        match outcome {
            MariadbConnectionProofDto::Ok { version, port } => {
                assert_eq!(version, "11.4.9");
                assert_eq!(port, 3307);
            }
            other => panic!("expected Ok, got {other:?}"),
        }

        let argv_seen = std::fs::read_to_string(&evidence).unwrap();
        assert!(
            !argv_seen.contains(password.expose()),
            "the stored password reached argv: {argv_seen:?}"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod site_ipc_tests {
    use tauri::Manager;

    use super::*;

    fn valid_input() -> SiteInput {
        SiteInput {
            name: "myshop".into(),
            domain: "myshop.localhost".into(),
            docroot: "/srv/www/myshop".into(),
            web_server: "nginx".into(),
            php_version: "8.3".into(),
            enabled: true,
        }
    }

    #[test]
    fn valid_input_converts_to_newsite() {
        let new: NewSite = valid_input().try_into().unwrap();
        assert_eq!(new.name.as_str(), "myshop");
        assert_eq!(new.domain.as_str(), "myshop.localhost");
        assert_eq!(new.docroot.as_str(), "/srv/www/myshop");
        assert_eq!(new.web_server.as_str(), "nginx");
        assert_eq!(new.php_version.as_str(), "8.3");
        assert!(new.enabled);
    }

    /// Every hostile field must be rejected AND name the offending field, so
    /// the form can mark the right input. This is the IPC ingress guard.
    #[test]
    fn hostile_input_is_rejected_with_the_right_field() {
        let cases: &[(&str, SiteInput)] = &[
            (
                "name",
                SiteInput {
                    name: "bad name".into(),
                    ..valid_input()
                },
            ),
            (
                "name",
                SiteInput {
                    name: "quote\"".into(),
                    ..valid_input()
                },
            ),
            (
                "domain",
                SiteInput {
                    domain: "evil\";inject".into(),
                    ..valid_input()
                },
            ),
            (
                "domain",
                SiteInput {
                    domain: "has space.localhost".into(),
                    ..valid_input()
                },
            ),
            (
                "docroot",
                SiteInput {
                    docroot: "relative/path".into(),
                    ..valid_input()
                },
            ),
            (
                "docroot",
                SiteInput {
                    docroot: "/has\"quote".into(),
                    ..valid_input()
                },
            ),
            (
                // B1: a `$`-bearing docroot must be rejected at the IPC
                // ingress — nginx's `root` expands variables even inside
                // quotes, so this would otherwise become a request-header-
                // controlled document root that still passes `nginx -t`.
                "docroot",
                SiteInput {
                    docroot: "/tmp/x$http_evil".into(),
                    ..valid_input()
                },
            ),
            (
                "php_version",
                SiteInput {
                    php_version: "8.x".into(),
                    ..valid_input()
                },
            ),
            (
                "web_server",
                SiteInput {
                    web_server: "caddy".into(),
                    ..valid_input()
                },
            ),
        ];
        for (field, input) in cases {
            let err = NewSite::try_from(input.clone()).unwrap_err();
            match err {
                IpcError::Validation { field: f, .. } => {
                    assert_eq!(&f, field, "wrong field for {input:?}");
                }
                other => panic!("expected Validation for {input:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn dto_round_trips_a_site() {
        let new: NewSite = valid_input().try_into().unwrap();
        let site = Site {
            id: SiteId::new(),
            name: new.name,
            domain: new.domain,
            docroot: new.docroot,
            web_server: new.web_server,
            php_version: new.php_version,
            enabled: new.enabled,
            created_at: 111,
            updated_at: 222,
        };
        let dto = SiteDto::from(site.clone());
        assert_eq!(dto.id, site.id.as_str());
        assert_eq!(dto.name, "myshop");
        assert_eq!(dto.web_server, "nginx");
        assert_eq!(dto.created_at, 111);
        assert_eq!(dto.updated_at, 222);
        assert!(dto.enabled);
    }

    /// Every `ScaffoldOutcome` variant must survive the DTO mirror with its
    /// payload intact — same reasoning as `dto_round_trips_a_site` above, for
    /// the mirror `create_site` added alongside `SiteDto`.
    #[test]
    fn scaffold_outcome_dto_round_trips_every_variant() {
        assert!(matches!(
            ScaffoldOutcomeDto::from(ScaffoldOutcome::Created),
            ScaffoldOutcomeDto::Created
        ));

        let kept = ScaffoldOutcomeDto::from(ScaffoldOutcome::KeptExisting {
            existing: "index.php".to_string(),
        });
        match kept {
            ScaffoldOutcomeDto::KeptExisting { existing } => assert_eq!(existing, "index.php"),
            other => panic!("expected KeptExisting, got {other:?}"),
        }

        let failed = ScaffoldOutcomeDto::from(ScaffoldOutcome::Failed {
            step: ScaffoldStep::WritePlaceholder,
            reason: "disk full".to_string(),
        });
        match failed {
            ScaffoldOutcomeDto::Failed { step, reason } => {
                assert!(matches!(step, ScaffoldStepDto::WritePlaceholder));
                assert_eq!(reason, "disk full");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn scaffold_step_dto_round_trips_every_variant() {
        assert!(matches!(
            ScaffoldStepDto::from(ScaffoldStep::CreateDir),
            ScaffoldStepDto::CreateDir
        ));
        assert!(matches!(
            ScaffoldStepDto::from(ScaffoldStep::Inspect),
            ScaffoldStepDto::Inspect
        ));
        assert!(matches!(
            ScaffoldStepDto::from(ScaffoldStep::WritePlaceholder),
            ScaffoldStepDto::WritePlaceholder
        ));
    }

    // -----------------------------------------------------------------------
    // `create_site` end to end, against a real (in-memory) `Db` — the same
    // `tauri::test::mock_builder`/`app.state()` harness `list_web_servers_*`
    // and `rescan_blocks_while_an_install_holds_the_lock` already use below in
    // this file (see the Cargo.toml comment on the `tauri "test"` feature:
    // `tauri::test::mock_builder` is the ONLY way to obtain a `tauri::State`
    // outside the framework itself). This is what makes it possible to pin
    // `create_site`'s two properties that a `SiteInput`/`NewSite`-only test
    // cannot reach: the actual insert-before-scaffold ORDER, and the actual
    // bytes written to `state.db` for the docroot column.
    // -----------------------------------------------------------------------

    /// Joined-docroot storage: `input.docroot` is the PARENT the user picked,
    /// and the column actually written — echoed back on `result.site.docroot`
    /// — must be that parent JOINED with the site's name, because that is the
    /// literal directory `scaffold` went on to create. A test that only
    /// checked `scaffold_path` in isolation (already covered in
    /// `site/scaffold.rs`) would miss a `create_site` that forgot to route the
    /// join's output back into `new.docroot` before the insert.
    #[tokio::test]
    async fn create_site_stores_the_joined_docroot_and_scaffolds_it_when_requested() {
        let parent_dir = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());

        let input = SiteInput {
            docroot: parent_dir.path().to_str().unwrap().to_string(),
            ..valid_input()
        };
        let result = create_site(app.state(), input, true).await.unwrap();

        let joined = parent_dir.path().join("myshop");
        assert_eq!(
            result.site.docroot,
            joined.to_str().unwrap(),
            "the stored docroot must be the JOINED path, not the raw parent the \
             caller picked"
        );
        assert!(
            joined.is_dir(),
            "scaffold must have created the docroot itself"
        );
        assert!(
            joined.join("index.html").exists(),
            "scaffold must have written the placeholder page"
        );
        assert!(
            matches!(result.scaffold, Some(ScaffoldOutcomeDto::Created)),
            "expected Some(Created), got {:?}",
            result.scaffold
        );
    }

    /// Insert-before-scaffold ordering (spec D2): a UNIQUE violation on `name`
    /// must leave NO folder behind for the rejected site. This is provable
    /// from the source without running anything — `repo.create(new).await?`
    /// diverges out of `create_site` on `Err` before the `scaffold` line is
    /// ever reached — but it is exactly the kind of invariant worth pinning
    /// with a real failing insert rather than trusting the reading alone.
    #[tokio::test]
    async fn a_unique_violation_leaves_no_folder_behind_for_the_rejected_site() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());

        let first_root = tempfile::tempdir().unwrap();
        create_site(
            app.state(),
            SiteInput {
                docroot: first_root.path().to_str().unwrap().to_string(),
                ..valid_input()
            },
            true,
        )
        .await
        .unwrap();

        // Same NAME as the first (`valid_input()`'s "myshop") but a different
        // domain, so `name` is the ONLY constraint this violates — `sites`
        // also has a UNIQUE index on `domain`, and leaving that unchanged too
        // would let sqlite report either column depending on index-check
        // order, making the `field` assertion below flaky about which one.
        let second_root = tempfile::tempdir().unwrap();
        let err = create_site(
            app.state(),
            SiteInput {
                docroot: second_root.path().to_str().unwrap().to_string(),
                domain: "myshop-again.localhost".into(),
                ..valid_input()
            },
            true,
        )
        .await
        .unwrap_err();
        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "name"),
            other => panic!("expected Validation on name, got {other:?}"),
        }

        let rejected_docroot = second_root.path().join("myshop");
        assert!(
            !rejected_docroot.exists(),
            "a rejected create must leave NO folder behind at {} — scaffold must \
             never run for a site that was never actually inserted",
            rejected_docroot.display()
        );
    }

    /// The other half of "`scaffold: None` means not requested": with
    /// `create_folder: false`, the RAW docroot is stored untouched (no join)
    /// and nothing is ever written to disk — even one that does not exist at
    /// all, which would otherwise be exactly the case a stray join or an
    /// unconditional scaffold call would surface on.
    #[tokio::test]
    async fn create_site_stores_the_raw_docroot_and_skips_scaffolding_when_not_requested() {
        let parent_dir = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());

        let docroot = parent_dir.path().join("never-created");
        let input = SiteInput {
            docroot: docroot.to_str().unwrap().to_string(),
            ..valid_input()
        };
        let result = create_site(app.state(), input, false).await.unwrap();

        assert_eq!(result.site.docroot, docroot.to_str().unwrap());
        assert!(
            result.scaffold.is_none(),
            "scaffold must not run when not requested"
        );
        assert!(
            !docroot.exists(),
            "no folder may appear when create_folder is false"
        );
    }

    #[test]
    fn core_validation_error_maps_to_ipc_validation() {
        let core = openvhost_core::CoreError::Validation {
            field: "domain",
            reason: "bad".into(),
        };
        match IpcError::from(core) {
            IpcError::Validation { field, message } => {
                assert_eq!(field, "domain");
                assert_eq!(message, "bad");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // REFUSE with no store (optional-state.db design D2). Every site command
    // reads or writes state.db, so a degraded answer would be a wrong one: an
    // empty list reads as "you have no sites", and a create that stored
    // nothing would still have made a folder.
    //
    // Vacuity for the refusal itself is measured on `assert_store_refusal` —
    // see its doc comment. What is measured HERE is the extra assertion this
    // test makes on top of that, the one about the filesystem: with all 13
    // `let db = db.require()?;` guards replaced by
    // `let db = &Db::open_in_memory().await.unwrap();` (the shape a "degrade"
    // regression takes) and the two `unwrap_err`s above it relaxed so the run
    // reaches it, the folder assertion FAILED — "create_site refused, so it
    // must not have scaffolded a folder", with `<parent>/myshop` on disk.
    // Reverted and re-run green.
    // -----------------------------------------------------------------------

    /// All five site commands, in one test on purpose: they share one refusal
    /// and one reason to have it, and a per-command test would be five copies
    /// of the same three assertions. Each is named through
    /// `assert_store_refusal`'s `what`, so a member that stops refusing is
    /// reported by name rather than by line number — and no member can be
    /// silently dropped, because every one of these calls must produce an
    /// `Err` for the test to reach its end.
    #[tokio::test]
    async fn every_site_command_refuses_and_names_why_when_the_store_is_down() {
        let parent_dir = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);

        assert_store_refusal(&list_sites(app.state()).await.unwrap_err(), "list_sites");

        // `create_folder: true`, so this also pins the ORDER: the refusal beats
        // the scaffold, and a rejected create leaves nothing on disk.
        let input = SiteInput {
            docroot: parent_dir.path().to_str().unwrap().to_string(),
            ..valid_input()
        };
        assert_store_refusal(
            &create_site(app.state(), input, true).await.unwrap_err(),
            "create_site",
        );
        assert!(
            !parent_dir.path().join("myshop").exists(),
            "create_site refused, so it must not have scaffolded a folder"
        );

        assert_store_refusal(
            &update_site(
                app.state(),
                SiteId::new().as_str().to_string(),
                valid_input(),
            )
            .await
            .unwrap_err(),
            "update_site",
        );
        assert_store_refusal(
            &delete_site(app.state(), SiteId::new().as_str().to_string())
                .await
                .unwrap_err(),
            "delete_site",
        );
        // `open_site` itself takes an `AppHandle<Wry>` and cannot be called
        // from a test at all; this is the function that holds every decision it
        // makes before opening anything. See `open_site_url`'s doc comment.
        assert_store_refusal(
            &open_site_url(&store_down(), SiteId::new().as_str())
                .await
                .unwrap_err(),
            "open_site (via open_site_url)",
        );
    }

    /// The scheme is PREPENDED and fixed. A stored value must never be able to
    /// choose it — `Domain`'s guard is a charset check, not a policy check, so it
    /// is not the thing standing between a stored row and, say, a `file://` or
    /// `javascript:` URL reaching the OS opener. This test is what pins that.
    #[test]
    fn site_url_always_prepends_a_fixed_http_scheme() {
        assert_eq!(site_url("hello.localhost"), "http://hello.localhost:8080");
        // Even if a scheme-looking value somehow reached the column, the result is
        // still an http URL naming it as a host — never a `file:`/`javascript:` URL.
        assert!(site_url("file:///etc/passwd").starts_with("http://"));
        assert!(site_url("javascript:alert(1)").starts_with("http://"));
    }

    /// Every applied site listens on `LISTEN_PORT` (8080), not 80 — a URL
    /// missing the port sends the browser to a port nothing is bound to, and
    /// it connection-errors instead of loading the site. This test fails if
    /// the port is ever dropped from `site_url`.
    #[test]
    fn site_url_includes_the_port_every_site_actually_listens_on() {
        let url = site_url("hello.localhost");
        assert!(
            url.ends_with(&format!(":{}", openvhost_core::site::apply::LISTEN_PORT)),
            "expected {url:?} to end with the LISTEN_PORT the applied site actually listens on"
        );
    }

    /// The count must report pids that actually produced a figure, not pids that
    /// were listed. A service that exits between the snapshot and the read drops
    /// out of BOTH the sum and the count, so the number and its label can never
    /// disagree (spec §6).
    #[test]
    fn memory_sum_counts_only_the_pids_that_answered() {
        let readings = vec![Some(1000u64), None, Some(2500u64), None];
        let (bytes, count) = sum_readings(readings.into_iter());
        assert_eq!(bytes, 3500);
        assert_eq!(count, 2);
    }

    #[test]
    fn memory_sum_of_nothing_is_zero_with_a_zero_count() {
        let (bytes, count) = sum_readings(std::iter::empty());
        assert_eq!(bytes, 0);
        assert_eq!(count, 0);
    }

    /// Saturating, not wrapping: an absurd reading must not wrap the total to a
    /// small number that looks plausible.
    #[test]
    fn memory_sum_saturates_instead_of_wrapping() {
        let readings = vec![Some(u64::MAX), Some(1000u64)];
        let (bytes, _) = sum_readings(readings.into_iter());
        assert_eq!(bytes, u64::MAX);
    }

    /// The abort rule (spec §4.1): once a reading comes back `Err`, the whole
    /// collection must stop — not just return an error eventually, but stop
    /// ASKING. A hand-written loop that reads every pid and only THEN returns
    /// the first error would also satisfy a bare `is_err()` check here, and it
    /// is the wrong, worse behaviour: it spends however many reads the
    /// remaining pids would have cost on a result that is going to be thrown
    /// away. The call counter is the load-bearing half of this test — without
    /// it, this test cannot tell a real abort from "abort, eventually".
    #[test]
    fn collect_readings_aborts_without_reading_pids_after_the_failure() {
        use std::cell::Cell;
        let calls: Cell<u32> = Cell::new(0);
        let pids = vec![1u32, 2, 3, 4];
        let result = collect_readings(pids.into_iter(), |pid| {
            calls.set(calls.get() + 1);
            match pid {
                1 => Ok(Some(1_000)),
                2 => Err(std::io::Error::other("boom")),
                // Would answer for pids 3 and 4 if `collect_readings` ever
                // asked — asserted below that it never does.
                _ => Ok(Some(9_999)),
            }
        });
        assert!(
            result.is_err(),
            "an Err reading must fail the whole collection"
        );
        assert_eq!(
            calls.get(),
            2,
            "the reader must be called for pid 1 and the failing pid 2, and NO \
             FURTHER — a count of 4 here would mean every pid was read before the \
             error was returned, which is exactly the regression this test exists \
             to catch"
        );
    }

    /// A mix of `Ok(Some(_))` and `Ok(None)` must come back in the same order
    /// the pids went in, untouched: `sum_readings` is the only place allowed to
    /// drop a `None` from the count, so nothing upstream of it may reorder or
    /// filter readings first.
    #[test]
    fn collect_readings_passes_through_hits_and_misses_in_order() {
        let pids = vec![10u32, 20, 30, 40];
        let result = collect_readings(pids.into_iter(), |pid| {
            Ok(match pid {
                10 => Some(111),
                20 => None,
                30 => Some(333),
                _ => None,
            })
        });
        assert_eq!(
            result.unwrap(),
            vec![Some(111), None, Some(333), None],
            "readings must arrive in pid order with Ok(None) preserved, not \
             dropped or reordered before sum_readings sees them"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod web_server_ipc_tests {
    use std::path::PathBuf;

    use super::*;

    /// Not a real installation: `web_server_rows` and `live_config_path` only ever
    /// move these paths around (nothing here is opened), so distinctive sentinels
    /// prove a value was passed through rather than re-derived.
    fn sample_paths() -> StackPaths {
        StackPaths {
            home: PathBuf::from("/nonexistent/openvhost-test-home"),
            nginx_bin: Some(PathBuf::from("/nonexistent/openvhost-test-home/bin/nginx")),
            nginx_conf: PathBuf::from("/nonexistent/openvhost-test-home/conf/nginx.conf"),
        }
    }

    /// Positive mappings only — the CLOSED-list property is pinned by
    /// `unknown_brand_is_a_validation_error_naming_the_field` below, so this name
    /// says just what it checks.
    #[test]
    fn brand_parses_the_known_ids() {
        assert_eq!(
            WebServerBrand::parse("nginx").unwrap(),
            WebServerBrand::Nginx
        );
        assert_eq!(
            WebServerBrand::parse("apache").unwrap(),
            WebServerBrand::Apache
        );
    }

    /// The client never sends a path — only this id. An unknown id must be a
    /// validation error, NOT a silent fallback to nginx, or a typo in the UI
    /// would quietly operate on the wrong server.
    #[test]
    fn unknown_brand_is_a_validation_error_naming_the_field() {
        let e = WebServerBrand::parse("../../etc/passwd").unwrap_err();
        match e {
            IpcError::Validation { field, .. } => assert_eq!(field, "id"),
            other => panic!("expected Validation, got {other:?}"),
        }
        assert!(
            WebServerBrand::parse("NGINX").is_err(),
            "parsing must be exact-match"
        );
        assert!(WebServerBrand::parse("").is_err());
        // Plausible ALIASES, not just hostile input: the closed list is the whole
        // property, so a convenience arm like `"caddy" => Ok(Self::Nginx)` must
        // fail here rather than silently operating on a server nobody named.
        for alias in ["caddy", "apache2", "httpd", "nginx-full", "nginx "] {
            assert!(
                WebServerBrand::parse(alias).is_err(),
                "{alias:?} is not on the closed list and must not parse"
            );
        }
    }

    /// Apache has no adapter and no template, so it must be LISTED but not
    /// operable — dropping the row would leave the site editor's Apache option
    /// unexplained, and returning empty output instead of an error would let a UI
    /// bug render "Apache's config is empty" for "Apache has no config".
    ///
    /// Also pins the page's whole purpose: nginx's row reports the paths the
    /// supervisor registered, verbatim, never anything re-probed.
    #[test]
    fn rows_list_apache_and_report_nginx_from_the_given_paths() {
        let p = sample_paths();
        let listed = web_server_rows(&p, Some("1.27.3".into()), Some(true), None);

        let nginx = listed
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must be listed, got {listed:?}"));
        assert!(nginx.supported);
        assert_eq!(nginx.service_id.as_deref(), Some("nginx"));
        assert_eq!(nginx.version.as_deref(), Some("1.27.3"));
        let bin = p.nginx_bin.as_deref().unwrap().display().to_string();
        let conf = p.nginx_conf.display().to_string();
        assert_eq!(nginx.binary_path.as_deref(), Some(bin.as_str()));
        assert_eq!(nginx.config_path.as_deref(), Some(conf.as_str()));

        let apache = listed
            .iter()
            .find(|r| r.id == "apache")
            .unwrap_or_else(|| panic!("apache must be listed, got {listed:?}"));
        assert!(!apache.supported);
        assert!(apache.binary_path.is_none());
        assert!(apache.config_path.is_none());
        assert!(apache.service_id.is_none());
    }

    /// True BY CONSTRUCTION now: the support gate lives inside the only function
    /// that hands out a path, so no reordering of statements in
    /// `read_web_server_config`/`validate_web_server_config` can read or validate a
    /// file before the brand is checked.
    #[test]
    fn unsupported_brand_is_rejected_before_any_path_is_touched() {
        let p = sample_paths();
        match WebServerBrand::Apache.live_config_path(&p) {
            Err(IpcError::Validation { field, message }) => {
                assert_eq!(field, "id");
                assert!(message.to_lowercase().contains("apache"));
            }
            Err(other) => panic!("expected Validation, got {other:?}"),
            Ok(path) => panic!("apache must not yield a path, got {}", path.display()),
        }
        // The supported brand gets ITS OWN config path out of managed state.
        assert_eq!(
            WebServerBrand::Nginx.live_config_path(&p).unwrap(),
            p.nginx_conf.as_path()
        );
        assert!(WebServerBrand::Nginx.require_supported().is_ok());
        assert!(WebServerBrand::Apache.require_supported().is_err());
    }

    /// The whole validator invocation is brand-keyed, not just its config argument.
    ///
    /// The compiler carries most of this: `ValidationTarget` has one variant, so a
    /// new brand cannot reach `validate_web_server_config` without an arm being
    /// written for it. What a test can still add is that the ONE variant is wired to
    /// the right four values out of managed state — including `err_log`, which is
    /// derived rather than borrowed and so is the piece a refactor can silently
    /// change.
    ///
    /// `home` is asserted against [`openvhost_core::nginx_prefix_dir`], AND
    /// against `p.home` directly with `assert_ne!` (4B fix-wave, item 1):
    /// `-p` must be the dedicated prefix, never the real home that carries
    /// `state.db`.
    ///
    /// VACUITY (neuter-and-watch-it-fail): reverted `validation_target`'s
    /// `home` field to `paths.home.clone()` — failed on the `assert_eq!`
    /// above (`left: paths.home, right: nginx_prefix_dir(&paths.home)`)
    /// before the `assert_ne!` even ran; restoring `nginx_prefix_dir` made it
    /// pass again.
    #[test]
    fn the_validator_invocation_is_resolved_as_one_unit_from_the_brand() {
        let p = sample_paths();
        match WebServerBrand::Nginx.validation_target(&p) {
            Ok(ValidationTarget::NginxT {
                bin,
                conf,
                err_log,
                home,
            }) => {
                assert_eq!(bin, p.nginx_bin.as_deref().unwrap());
                assert_eq!(conf, p.nginx_conf.as_path());
                assert_eq!(err_log, p.home.join("logs/nginx.error.log"));
                assert_eq!(home, openvhost_core::nginx_prefix_dir(&p.home));
                assert_ne!(
                    home, p.home,
                    "-p must never be the real home — it carries state.db"
                );
            }
            Err(e) => panic!("nginx must yield a validator invocation, got {e:?}"),
        }
        // Mirrors `unsupported_brand_is_rejected_before_any_path_is_touched`: the
        // gate runs INSIDE the accessor, so an unsupported brand cannot obtain a
        // binary either, no matter how the caller orders its statements.
        match WebServerBrand::Apache.validation_target(&p) {
            Err(IpcError::Validation { field, .. }) => assert_eq!(field, "id"),
            Err(other) => panic!("expected Validation, got {other:?}"),
            Ok(_) => panic!("apache must not yield a validator invocation"),
        }
    }

    /// Design D3, one of the six decision sites the 4B fix-wave audit found
    /// untested (item 2): with no nginx binary at all, `validation_target`
    /// must refuse honestly — the SAME message `apply_config` gives for the
    /// identical condition — rather than handing back a validator invocation
    /// for a binary that does not exist.
    #[test]
    fn validation_target_refuses_honestly_when_no_nginx_binary_was_found() {
        let p = StackPaths {
            nginx_bin: None,
            ..sample_paths()
        };
        match WebServerBrand::Nginx.validation_target(&p) {
            Err(IpcError::Core { message }) => {
                assert_eq!(message, "no nginx binary was found on this machine");
            }
            Err(other) => panic!("expected Core, got {other:?}"),
            Ok(_) => panic!("nginx must not yield a validator invocation with no binary"),
        }
    }

    /// The unsupported-brand message must name the brand that was ASKED FOR.
    ///
    /// With only two variants — one supported — no single mutation can distinguish a
    /// hardcoded "Apache" from `self.display_name()`, because Apache is the only
    /// brand that can reach this message today. So the expected substring is
    /// COMPUTED from the same source the message uses: a message that stops tracking
    /// `display_name` diverges the moment `display_name` changes, which is the
    /// earliest point at which the bug becomes observable at all.
    #[test]
    fn the_unsupported_message_names_the_brand_that_was_asked_for() {
        assert_eq!(WebServerBrand::Nginx.display_name(), "nginx");
        assert_eq!(WebServerBrand::Apache.display_name(), "Apache");
        let err = WebServerBrand::Apache.require_supported().unwrap_err();
        match err {
            IpcError::Validation { field, message } => {
                assert_eq!(field, "id");
                assert!(
                    message.contains(WebServerBrand::Apache.display_name()),
                    "the message must name the brand asked for, got {message:?}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // `NginxRuntimeSourceDto` — the wire tag is `NginxRuntimeSource::as_str()`,
    // not a second spelling of it (nginx source design D1). Mirrors
    // `mysql_pkg.rs`'s identical group for `MysqlRuntimeSourceDto`.
    // ------------------------------------------------------------------

    fn tag_of(value: &impl serde::Serialize) -> String {
        serde_json::to_value(value).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The one that stops this DTO drifting: for EVERY source, the wire tag is
    /// literally what `as_str()` says. The match is exhaustive, so a third
    /// source has to be added here too.
    #[test]
    fn the_wire_tag_is_nginx_runtime_source_as_str() {
        use openvhost_core::nginx::NginxRuntimeSource;
        let sources = [
            NginxRuntimeSource::Packaged {
                version: "1.30.4".to_string(),
            },
            NginxRuntimeSource::Homebrew,
        ];
        for source in &sources {
            let dto = NginxRuntimeSourceDto::from(source);
            assert_eq!(tag_of(&dto), source.as_str(), "tag drifted for {source:?}");
        }
    }

    /// A Homebrew runtime carries NO version over the wire — deliberately, so
    /// the UI cannot render an invented patch number (design D2).
    #[test]
    fn a_homebrew_nginx_carries_no_version_over_the_wire() {
        let wire = serde_json::to_value(NginxRuntimeSourceDto::from(
            &openvhost_core::nginx::NginxRuntimeSource::Homebrew,
        ))
        .unwrap();
        assert_eq!(wire.get("version"), None);
        assert_eq!(wire["kind"], "homebrew");
    }

    #[test]
    fn a_packaged_nginx_carries_its_exact_version() {
        let wire = serde_json::to_value(NginxRuntimeSourceDto::from(
            &openvhost_core::nginx::NginxRuntimeSource::Packaged {
                version: "1.30.4".to_string(),
            },
        ))
        .unwrap();
        assert_eq!(wire["kind"], "packaged");
        assert_eq!(wire["version"], "1.30.4");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod apply_ipc_tests {
    use tauri::Manager;

    use super::*;

    /// The dialog switches on these; a rename here silently breaks its badges.
    #[test]
    fn change_kind_maps_to_a_stable_wire_string() {
        assert_eq!(change_kind_str(ChangeKind::Added), "added");
        assert_eq!(change_kind_str(ChangeKind::Modified), "modified");
        assert_eq!(change_kind_str(ChangeKind::Removed), "removed");
    }

    #[test]
    fn a_missing_runtime_reaches_the_ui_naming_the_site_and_versions() {
        let e: IpcError = ApplyError::MissingRuntime {
            site: "legacy".into(),
            requested: "7.4".into(),
            available: vec!["8.4".into()],
        }
        .into();
        match e {
            IpcError::Core { message } => {
                assert!(message.contains("legacy"));
                assert!(message.contains("7.4"));
                assert!(message.contains("8.4"));
            }
            other => panic!("expected Core, got {other:?}"),
        }
    }

    #[test]
    fn a_service_that_did_not_stop_in_time_is_not_started_and_is_reported() {
        let running = vec!["nginx".to_string(), "php-fpm-8.4".to_string()];
        let stragglers = vec!["nginx".to_string()];
        let started = std::cell::RefCell::new(Vec::new());
        let (restarted, problems) = restart_outcome(&running, &stragglers, |id| {
            started.borrow_mut().push(id.to_string());
            Ok(())
        });
        // Starting a service that is still shutting down is a no-op that leaves it
        // stopped moments later — the exact way a green Apply takes a site down.
        assert_eq!(started.into_inner(), vec!["php-fpm-8.4".to_string()]);
        assert_eq!(restarted, vec!["php-fpm-8.4".to_string()]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "nginx");
        assert!(problems[0].reason.contains("did not stop"));
    }

    #[test]
    fn a_failed_start_is_reported_without_hiding_the_other_services() {
        let running = vec!["php-fpm-8.4".to_string(), "nginx".to_string()];
        let (restarted, problems) = restart_outcome(&running, &[], |id| {
            if id == "php-fpm-8.4" {
                Err("no such service".into())
            } else {
                Ok(())
            }
        });
        // The failure must not abort the loop: nginx still gets its start.
        assert_eq!(restarted, vec!["nginx".to_string()]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "php-fpm-8.4");
        assert!(problems[0].reason.contains("no such service"));
    }

    #[test]
    fn a_clean_restart_reports_no_problems() {
        let running = vec!["php-fpm-8.4".to_string(), "nginx".to_string()];
        let (restarted, problems) = restart_outcome(&running, &[], |_| Ok(()));
        assert_eq!(restarted, running);
        assert!(problems.is_empty());
    }

    #[test]
    fn the_runtime_set_can_be_replaced_after_startup() {
        // The Languages page installs a version at runtime; if this state could not
        // be replaced, apply would never learn about it and Install would appear
        // to succeed while changing nothing.
        let state = RwLock::new(None::<InstalledRuntimes>);
        assert!(state.read().unwrap().is_none());
        *state.write().unwrap() = Some(InstalledRuntimes {
            nginx_bin: Some(PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx")),
            php: vec![openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            }],
        });
        let seen = state.read().unwrap().clone().unwrap();
        assert_eq!(seen.php.len(), 1);
        assert_eq!(seen.php[0].major, "8.3");
    }

    /// One of the six decision sites the 4B fix-wave audit found untested
    /// (item 2): with no nginx binary at all, `apply_config` must refuse
    /// honestly and by name, rather than handing `NginxValidator` a path to a
    /// binary that does not exist.
    ///
    /// Zero sites and zero installed PHP majors, deliberately: `render_set`
    /// unconditionally renders the main nginx config and the catch-all site
    /// regardless of either, so on a fresh, empty home the plan is still
    /// non-empty (A3's "nothing to write means nothing to restart" early
    /// return is NOT taken) and the nginx_bin check downstream is actually
    /// reached.
    #[tokio::test]
    async fn apply_config_refuses_up_front_when_no_nginx_binary_was_found() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: None,
            php: vec![],
        })));
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("config/generated/nginx/nginx.conf"),
        }));
        app.manage(Arc::new(Supervisor::new(openvhost_proc::default_driver())));
        app.manage(ApplyLock::default());

        let err = apply_config(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            app.state::<Arc<Supervisor>>(),
            app.state::<ApplyLock>(),
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Core { message } => {
                assert_eq!(message, "no nginx binary was found on this machine");
            }
            other => panic!("expected Core, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // FAIL CLOSED with no store (optional-state.db design D2).
    //
    // These two tests are a pair and only mean something together: the first
    // says Apply refuses and deletes nothing, the second says an empty site
    // list really would have deleted something. Without the second, the first
    // is a test that cannot distinguish a guard from an accident.
    // -----------------------------------------------------------------------

    /// A home with one already-generated vhost, and no store.
    ///
    /// `<home>/config/generated/nginx/sites/<domain>.conf` is the tree Apply
    /// owns: a `.conf` in there that the desired render does not contain is
    /// classified `Removed` and deleted. So an empty site list does not
    /// produce a smaller config — it produces a config with no vhosts, and
    /// `nginx -t` ACCEPTS that, which is why nothing downstream would roll it
    /// back.
    ///
    /// The stand-in nginx below exits 0 on purpose: with a validator that
    /// cannot spawn, an unguarded Apply would fail and restore the file
    /// anyway, and this assertion would pass for the wrong reason.
    ///
    /// VACUITY, measured. With `let db = db.require()?;` replaced by
    /// `let db = &Db::open_in_memory().await.unwrap();` — the exact shape a
    /// "degrade to an empty site list" regression takes — this test failed
    /// first on `unwrap_err`, which reported
    /// `Ok(ApplyOutcomeDto { applied: 3, not_started: ["nginx"], .. })`. With
    /// that `unwrap_err` relaxed so the run continues, it then failed on
    /// "apply_config refused, so the existing vhost must still be on disk":
    /// **the vhost was deleted**. Reverted and re-run green.
    #[cfg(unix)]
    #[tokio::test]
    async fn apply_fails_closed_and_deletes_no_vhost_when_the_store_is_down() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let vhost = home
            .path()
            .join("config/generated/nginx/sites/shop.localhost.conf");
        std::fs::create_dir_all(vhost.parent().unwrap()).unwrap();
        std::fs::write(
            &vhost,
            "server { listen 8080; server_name shop.localhost; }\n",
        )
        .unwrap();

        let nginx = home.path().join("nginx");
        std::fs::write(&nginx, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&nginx, std::fs::Permissions::from_mode(0o755)).unwrap();

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: Some(nginx.clone()),
            php: vec![],
        })));
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(nginx),
            nginx_conf: home.path().join("config/generated/nginx/nginx.conf"),
        }));
        app.manage(Arc::new(Supervisor::new(openvhost_proc::default_driver())));
        app.manage(ApplyLock::default());

        assert_store_refusal(
            &apply_config(
                app.state::<DbHandle>(),
                app.state::<RwLock<Option<InstalledRuntimes>>>(),
                app.state::<Option<StackPaths>>(),
                app.state::<Arc<Supervisor>>(),
                app.state::<ApplyLock>(),
            )
            .await
            .unwrap_err(),
            "apply_config",
        );
        assert!(
            vhost.exists(),
            "apply_config refused, so the existing vhost must still be on disk"
        );

        // The read-only half of the same pipeline, over the same config set:
        // it feeds the pending-changes banner, so degrading it would light that
        // banner up with "remove every vhost" as a normal state.
        assert_store_refusal(
            &plan_config_apply(
                app.state::<DbHandle>(),
                app.state::<RwLock<Option<InstalledRuntimes>>>(),
                app.state::<Option<StackPaths>>(),
            )
            .await
            .unwrap_err(),
            "plan_config_apply",
        );
    }

    /// The other half: with a store that is PRESENT and simply has no sites,
    /// the very same home plans the REMOVAL of that vhost.
    ///
    /// This is what "fails closed" is protecting against, stated as a fact
    /// about this code rather than an assertion in a design document — and it
    /// is why the test above is a real guard and not a formality.
    /// `plan_config_apply` rather than `apply_config`, because the claim is
    /// about what the plan SAYS; nothing needs to be deleted twice to make it.
    ///
    /// Vacuity: with the fixture vhost not written, the plan came back as two
    /// `added` entries (the main config and the catch-all) and no `removed` at
    /// all, so this assertion failed — it is driven by the file on disk, not
    /// by something every plan contains. Reverted and re-run green.
    #[tokio::test]
    async fn an_empty_site_list_plans_the_removal_of_an_existing_vhost() {
        let home = tempfile::tempdir().unwrap();
        let vhost = home
            .path()
            .join("config/generated/nginx/sites/shop.localhost.conf");
        std::fs::create_dir_all(vhost.parent().unwrap()).unwrap();
        std::fs::write(
            &vhost,
            "server { listen 8080; server_name shop.localhost; }\n",
        )
        .unwrap();

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: Some(home.path().join("nginx")),
            php: vec![],
        })));
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(home.path().join("nginx")),
            nginx_conf: home.path().join("config/generated/nginx/nginx.conf"),
        }));

        let plan = plan_config_apply(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
        )
        .await
        .unwrap();

        assert!(
            plan.changes
                .iter()
                .any(|c| c.kind == "removed" && c.path.contains("shop.localhost")),
            "an empty site list must plan the vhost's REMOVAL — that is the \
             destructive answer `apply_config`'s refusal exists to prevent; got {:?}",
            plan.changes
                .iter()
                .map(|c| (&c.kind, &c.path))
                .collect::<Vec<_>>()
        );
    }
}

// Unix-only: the stand-in nginx is a `#!/bin/sh` script made executable via
// `PermissionsExt`, exactly as `openvhost-conf`'s inspect tests do it. Windows
// has no supported web-server stack yet (`stack::macos_stack` is the only
// builder), so there is nothing here a Windows run would be covering.
#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod list_web_servers_tests {
    use std::path::{Path, PathBuf};

    use openvhost_conf::WebServerAdapter;
    use tauri::Manager;

    use super::*;

    /// A stand-in nginx that records its argv and prints a banner on STDERR, where
    /// real nginx writes it. NOT a mock of the probe: `probe_nginx_version` really
    /// spawns this, so the argv file is evidence about the actual invocation.
    /// Warmed at creation, outside the `PROBE_TIMEOUT`-bounded probe these
    /// tests then time — see [`crate::tests_support`].
    ///
    /// This fixture is exactly why the warm-up must not run the BODY:
    /// `a_packaged_source_reports_the_tree_version_and_never_spawns_a_probe`
    /// reads `!argv_out.exists()` as "nothing spawned me", so a warm-up that
    /// reached the redirection below would create that file and turn a real
    /// tripwire into a permanent false alarm. The guard line
    /// `write_exec_fixture` prepends exits first, so it does not.
    fn fake_nginx(dir: &Path, argv_out: &Path, version: &str) -> PathBuf {
        let p = dir.join("nginx");
        crate::tests_support::write_exec_fixture(
            &p,
            &format!(
                "echo \"$@\" > \"{}\"\necho 'nginx version: nginx/{version}' 1>&2",
                argv_out.display()
            ),
        );
        p
    }

    /// Drives `list_web_servers` ITSELF, which nothing did before: `web_server_rows`
    /// takes `version` as a parameter, and that is exactly why the code PRODUCING it
    /// had no coverage. Replacing the probe with `let version: Option<String> =
    /// None;` left `cargo test -p openvhost-desktop` and `clippy --all-targets -D
    /// warnings` both at exit 0 — and it also removed the only read of `p.home`,
    /// taking the `err_log` derivation with it. The page would then read
    /// `Version: Unknown` forever, and the spec's central security disclosure —
    /// that merely navigating to this page spawns `nginx -v` — would silently become
    /// false in the direction the auditor was told to worry about.
    ///
    /// A `tauri::test` mock app is the only way to obtain a `tauri::State`: `State`'s
    /// single field is private and `StateManager::new` is `pub(crate)`, so there is
    /// no other constructor. Hence the dev-only `tauri/test` feature.
    #[tokio::test]
    async fn list_web_servers_probes_the_version_and_derives_the_error_log_from_home() {
        let home = tempfile::tempdir().unwrap();
        let argv = home.path().join("argv.txt");
        let bin = fake_nginx(home.path(), &argv, "1.27.3");

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(bin.clone()),
            nginx_conf: home.path().join("conf/nginx.conf"),
        }));
        // `Homebrew`, deliberately: this test's whole point is that the probe
        // RUNS, and design D2 only ever probes a Homebrew source — a
        // `Packaged` source here would make `list_web_servers` read the
        // version off `nginx_source` instead and never touch `bin` at all.
        app.manage(Some(openvhost_core::nginx::NginxRuntimeSource::Homebrew));

        let rows = list_web_servers(app.state(), app.state()).await.unwrap();
        let nginx = rows
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must be listed, got {rows:?}"));

        // 1. The probe RAN, and the version it reported is the one on the row —
        //    a distinctive value, so a hardcoded string cannot satisfy it either.
        assert_eq!(nginx.version.as_deref(), Some("1.27.3"));

        // 2. `-e` was derived from `p.home`. Asserted against the argv the fake
        //    recorded, so it is the real invocation being checked and not a
        //    re-derivation of the same expression.
        let recorded = std::fs::read_to_string(&argv).unwrap();
        let expected_err_log = home.path().join("logs/nginx.error.log");
        assert!(
            recorded.contains(&format!("-e {}", expected_err_log.display())),
            "the version probe must pass -e derived from p.home; argv was: {recorded}"
        );
        assert!(recorded.contains("-v"), "argv was: {recorded}");

        // 3. The binary probed is the one managed state named, not a fresh probe.
        assert_eq!(nginx.binary_path.as_deref(), Some(&*bin.to_string_lossy()));

        // 4. Hot reload is READ OFF the adapter. Compared against the adapter rather
        //    than against `true`: `NginxAdapter::supports_hot_reload()` returns
        //    `true` today, so a literal would be satisfied by hardcoding the field.
        //    This form ties the row to the source of truth, so the two cannot
        //    disagree even after the adapter's answer changes.
        assert_eq!(
            nginx.supports_hot_reload,
            openvhost_conf::NginxAdapter.supports_hot_reload()
        );
    }

    /// Design D3, another of the six decision sites the 4B fix-wave audit
    /// found untested (item 2): with no nginx binary at all, the row must
    /// report BOTH fields as `None` — `binary_path` because there is nothing
    /// to name, and `version` because there is nothing to spawn a probe
    /// against.
    ///
    /// VACUITY (neuter-and-watch-it-fail), two mutations: (1) changed the
    /// `None` arm to `Some("FAKE".to_string())` — failed, `left: Some("FAKE")
    /// right: None`; (2) the REALISTIC regression this guards against —
    /// changed the `None` arm to probe a hardcoded
    /// `/opt/homebrew/opt/nginx/bin/nginx` (the exact shape of the retired
    /// `fallback_brew()` bug) — ALSO failed here, `left: Some("1.31.3")`,
    /// because this machine happens to have that binary installed. Both
    /// reverts restored a pass.
    ///
    /// LIMITATION, stated rather than left implicit: mutation (2) only fails
    /// BECAUSE this machine has a real Homebrew nginx at that exact path — on
    /// a machine without one, `probe_nginx_version` would return `None` for
    /// the missing-binary case exactly as it does for the correct "no probe
    /// at all" case, and this assertion could not tell the two apart. This
    /// test is therefore hermetic in the sense of not touching any binary
    /// ITSELF, but its discriminating power against a REINTRODUCED hardcoded
    /// fallback is machine-dependent. `list_web_servers` has no injectable
    /// prober seam (unlike `discover_php`'s closure parameter) that would
    /// let a fake binary stand in regardless of machine state.
    #[tokio::test]
    async fn list_web_servers_reports_no_binary_and_no_version_when_none_was_found() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("conf/nginx.conf"),
        }));
        // No binary, so no source either — the honest pairing `macos_stack`
        // itself always produces (nginx source design D2).
        app.manage(None::<openvhost_core::nginx::NginxRuntimeSource>);

        let rows = list_web_servers(app.state(), app.state()).await.unwrap();
        let nginx = rows
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must still be listed, got {rows:?}"));

        assert_eq!(
            nginx.binary_path, None,
            "design D3: no binary was found, so none may be invented"
        );
        assert_eq!(
            nginx.version, None,
            "design D3: with no binary, list_web_servers must not spawn a probe at all"
        );
    }

    /// New coverage alongside the test above rather than an edit to it: with
    /// no nginx found, the row's new `source` field must read the same
    /// "nothing to report" way `binary_path`/`version` already do.
    #[tokio::test]
    async fn list_web_servers_reports_no_source_when_no_nginx_was_found() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("conf/nginx.conf"),
        }));
        app.manage(None::<openvhost_core::nginx::NginxRuntimeSource>);

        let rows = list_web_servers(app.state(), app.state()).await.unwrap();
        let nginx = rows
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must still be listed, got {rows:?}"));
        assert_eq!(
            nginx.source, None,
            "design D1: no nginx was found, so no source may be invented"
        );
    }

    /// Nginx source design D2's central behaviour, and the one this whole
    /// slice exists to add: a PACKAGED source's version comes from the tree
    /// discovery already resolved, and `list_web_servers` must not spawn a
    /// probe to get it.
    ///
    /// `fake_nginx`'s own banner ("1.19.0") is DELIBERATELY different from
    /// the tree version handed to `nginx_source` ("9.9.9") — so a regression
    /// that still probed would report the WRONG version here, not merely an
    /// extra one. Non-vacuity (neuter-and-watch-it-fail): temporarily
    /// changing the `Packaged` arm to call `probe_nginx_version` as well
    /// (the exact regression this test exists to catch) fails BOTH
    /// assertions below — `nginx.version` becomes `Some("1.19.0")` and
    /// `argv.txt` appears — confirming this test would catch the fallback
    /// design D2 forbids; reverted after confirming.
    #[tokio::test]
    async fn a_packaged_source_reports_the_tree_version_and_never_spawns_a_probe() {
        let home = tempfile::tempdir().unwrap();
        let argv = home.path().join("argv.txt");
        // Would leave evidence in `argv` if spawned — the same fixture the
        // Homebrew-path test below uses, so the two tests differ only in
        // which source is managed.
        let bin = fake_nginx(home.path(), &argv, "1.19.0");

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(bin),
            nginx_conf: home.path().join("conf/nginx.conf"),
        }));
        app.manage(Some(openvhost_core::nginx::NginxRuntimeSource::Packaged {
            version: "9.9.9".to_string(),
        }));

        let rows = list_web_servers(app.state(), app.state()).await.unwrap();
        let nginx = rows
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must be listed, got {rows:?}"));

        assert_eq!(
            nginx.version.as_deref(),
            Some("9.9.9"),
            "the TREE version must reach the row, not whatever the binary itself claims"
        );
        assert_eq!(
            nginx.source,
            Some(NginxRuntimeSourceDto::Packaged {
                version: "9.9.9".to_string()
            })
        );
        assert!(
            !argv.exists(),
            "a packaged source must never spawn a version probe, but the fake nginx ran"
        );
    }

    /// The contrasting half of the test above: a Homebrew source still has no
    /// other way to learn its exact patch release, so it IS probed, and the
    /// probe's own answer is what reaches the row.
    #[tokio::test]
    async fn a_homebrew_source_is_probed_and_reports_the_binarys_own_answer() {
        let home = tempfile::tempdir().unwrap();
        let argv = home.path().join("argv.txt");
        let bin = fake_nginx(home.path(), &argv, "1.31.3");

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(bin),
            nginx_conf: home.path().join("conf/nginx.conf"),
        }));
        app.manage(Some(openvhost_core::nginx::NginxRuntimeSource::Homebrew));

        let rows = list_web_servers(app.state(), app.state()).await.unwrap();
        let nginx = rows
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must be listed, got {rows:?}"));

        assert_eq!(nginx.version.as_deref(), Some("1.31.3"));
        assert_eq!(nginx.source, Some(NginxRuntimeSourceDto::Homebrew));
        assert!(
            argv.exists(),
            "a Homebrew source has no other way to learn its version, so the probe must run"
        );
    }

    #[test]
    fn the_nginx_row_reports_whether_its_config_is_actually_there() {
        // The page disables Start on `Some(false)`, so a row that claims
        // `Some(true)` when the file is absent sends the user at a service that
        // will exit immediately — and one that claims `Some(false)` when the
        // file IS there hides a working button. `web_server_rows` only ever
        // moves the tri-state through; it never invents a third value.
        let p = StackPaths {
            home: PathBuf::from("/x/.openvhost"),
            nginx_bin: Some(PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx")),
            nginx_conf: PathBuf::from("/x/.openvhost/config/generated/nginx/nginx.conf"),
        };

        let present = web_server_rows(&p, None, Some(true), None);
        let nginx = present
            .iter()
            .find(|r| r.id == "nginx")
            .expect("an nginx row");
        assert_eq!(
            nginx.config_exists,
            Some(true),
            "Some(true) must reach the nginx row"
        );

        let absent = web_server_rows(&p, None, Some(false), None);
        let nginx = absent
            .iter()
            .find(|r| r.id == "nginx")
            .expect("an nginx row");
        assert_eq!(
            nginx.config_exists,
            Some(false),
            "Some(false) must reach the nginx row"
        );

        // The unknown case: a stat that could not be performed must reach the
        // row AS `None`, not collapse into `Some(false)` on the way through.
        let unknown = web_server_rows(&p, None, None, None);
        let nginx = unknown
            .iter()
            .find(|r| r.id == "nginx")
            .expect("an nginx row");
        assert_eq!(
            nginx.config_exists, None,
            "None must reach the nginx row unchanged, not become Some(false)"
        );
    }

    #[test]
    fn apache_never_claims_a_confirmed_config() {
        // Apache is unsupported and has no config path at all, so `Some(true)`
        // (a confirmed presence) would be the row asserting something about a
        // file it cannot even name. `Some(false)` is what this crate picks —
        // see `WebServerDto::apache`'s doc comment for why that is a confirmed
        // absence rather than an unresolved `None`.
        let apache = WebServerDto::apache();
        assert_eq!(apache.config_path, None);
        assert_eq!(apache.config_exists, Some(false));
    }

    /// Closes the gap `the_nginx_row_reports_whether_its_config_is_actually_there`
    /// leaves open: that test drives `web_server_rows`, which only ever MOVES a
    /// bool around — nothing there touches the filesystem. This one drives
    /// `list_web_servers` itself, so the real `tokio::fs::try_exists` stat against
    /// `p.nginx_conf` is what is under test. Proven non-vacuous by hand: hardcoding
    /// `let config_exists = true;` in `list_web_servers` leaves
    /// `the_nginx_row_reports_whether_its_config_is_actually_there` green (it never
    /// calls the command) while this test catches it, because the "removed" half
    /// of the assertion below would then fail.
    #[tokio::test]
    async fn list_web_servers_reports_the_real_state_of_the_config_file_on_disk() {
        let home = tempfile::tempdir().unwrap();
        let argv = home.path().join("argv.txt");
        let bin = fake_nginx(home.path(), &argv, "1.27.3");
        let conf = home.path().join("conf/nginx.conf");

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: Some(bin),
            nginx_conf: conf.clone(),
        }));
        // `nginx_bin` is a real (fake-nginx) path, so the honest pairing is a
        // known source rather than `None` — nothing under test here asserts
        // on `version`/`source`, only `config_exists`, so any source would
        // do; this is the one `macos_stack` would actually have produced
        // alongside a `Some(nginx_bin)`.
        app.manage(Some(openvhost_core::nginx::NginxRuntimeSource::Homebrew));

        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "# placeholder\n").unwrap();
        let rows = list_web_servers(app.state(), app.state()).await.unwrap();
        let nginx = rows
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must be listed, got {rows:?}"));
        assert_eq!(
            nginx.config_exists,
            Some(true),
            "the file is there; the row must say so"
        );

        std::fs::remove_file(&conf).unwrap();
        let rows = list_web_servers(app.state(), app.state()).await.unwrap();
        let nginx = rows
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must be listed, got {rows:?}"));
        assert_eq!(
            nginx.config_exists,
            Some(false),
            "the file is gone; the row must not claim otherwise"
        );
    }

    /// The stat seam directly: `tokio::fs::try_exists` really does return `Err`
    /// when the path cannot be traversed (a locked-down parent directory here,
    /// standing in for the permission-denied / dangling-symlink cases named in
    /// `WebServerDto::config_exists`'s doc comment), and `.ok()` — the exact
    /// mapping `list_web_servers` applies — turns that `Err` into `None`, never
    /// into `Some(false)`.
    ///
    /// This does not drive `list_web_servers` end to end the way the test above
    /// does, because there is no portable, non-root way to make THAT command's
    /// own stat fail without also breaking `tauri::test`'s mock app setup (which
    /// itself touches the temp directory tree). What it does cover, faithfully,
    /// is the one line the whole fix is about: `try_exists(..).await.ok()`.
    #[tokio::test]
    async fn a_stat_that_cannot_be_performed_yields_none_not_some_false() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let locked_dir = home.path().join("locked");
        std::fs::create_dir(&locked_dir).unwrap();
        let conf = locked_dir.join("nginx.conf");

        // Strip ALL permissions, including execute, so the directory cannot be
        // traversed even by its owner — this is what turns the stat below into
        // a permission error rather than a confirmed absence.
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = tokio::fs::try_exists(&conf).await;

        // Restore permissions before the tempdir's Drop tries to remove it.
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            result.is_err(),
            "a stat through an untraversable parent must error, not report \
             Ok(false); got {result:?}"
        );
        assert_eq!(
            result.ok(),
            None,
            "the mapping list_web_servers applies (.ok()) must turn that error \
             into None — collapsing it into Some(false) is the bug this test \
             guards against"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod web_server_settings_ipc_tests {
    use tauri::Manager;

    use super::*;

    /// A DTO whose every field differs from the default, so a test that
    /// mutates one field is genuinely checking that field and a dropped
    /// mapping cannot be masked by a coincidental default.
    fn valid_dto() -> WebServerSettingsDto {
        WebServerSettingsDto {
            worker_connections: 2048,
            client_max_body_size: "512m".into(),
            keepalive_timeout: 30,
            tcp_nodelay: false,
            fastcgi_connect_timeout: 10,
            fastcgi_send_timeout: 120,
            fastcgi_read_timeout: 900,
            gzip: true,
            gzip_comp_level: 6,
            gzip_types: "text/css application/json".into(),
        }
    }

    fn expect_validation(e: IpcError) -> (String, String) {
        match e {
            IpcError::Validation { field, message } => (field, message),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn a_bad_setting_reaches_the_ui_marked_on_its_own_field() {
        // The form marks one input; a flattened Core error would mark none.
        let e: IpcError = openvhost_conf::GzipLevel::parse(99).unwrap_err().into();
        let (field, _) = expect_validation(e);
        assert_eq!(field, "gzip_comp_level");
    }

    #[test]
    fn a_malformed_gzip_type_names_the_offending_token() {
        let e: IpcError = openvhost_conf::GzipTypes::parse("text/html; } server {")
            .unwrap_err()
            .into();
        let (field, message) = expect_validation(e);
        assert_eq!(field, "gzip_types");
        assert!(message.contains("text/html;"), "got {message}");
    }

    #[test]
    fn a_non_field_conf_error_becomes_a_banner_not_a_field_mark() {
        // Only `InvalidField` knows which input to highlight. Anything else
        // pinned to some arbitrary field name would mark an input the user
        // never touched.
        let e: IpcError = openvhost_conf::ConfError::EmptyUpstream.into();
        match e {
            IpcError::Core { message } => assert!(!message.is_empty()),
            other => panic!("expected Core, got {other:?}"),
        }
    }

    #[test]
    fn the_dto_round_trips_through_the_domain_type() {
        let dto = valid_dto();
        let domain: openvhost_conf::WebServerSettings = dto.clone().try_into().unwrap();
        let back = WebServerSettingsDto::from(domain);
        assert_eq!(back, dto);
    }

    /// Every one of the four `Seconds` fields, not just one.
    ///
    /// `Seconds::parse` names its field `"seconds"` — a field that exists on
    /// no form — for all four, so without the per-call-site relabel a bad
    /// `fastcgi_read_timeout` highlights nothing at all. Testing one field
    /// would pass while the other three stayed broken, which is exactly how
    /// this class of bug survives.
    #[test]
    fn each_timeout_field_surfaces_its_own_name_not_seconds() {
        // `0` is outside `Seconds`' `1..=86400` bound.
        type BreakOne = fn(&mut WebServerSettingsDto);
        let cases: [(&str, BreakOne); 4] = [
            ("keepalive_timeout", |d| d.keepalive_timeout = 0),
            ("fastcgi_connect_timeout", |d| d.fastcgi_connect_timeout = 0),
            ("fastcgi_send_timeout", |d| d.fastcgi_send_timeout = 0),
            ("fastcgi_read_timeout", |d| d.fastcgi_read_timeout = 0),
        ];
        for (expected, break_it) in cases {
            let mut dto = valid_dto();
            break_it(&mut dto);
            let e = openvhost_conf::WebServerSettings::try_from(dto)
                .expect_err("an out-of-range timeout must be rejected");
            let (field, _) = expect_validation(e);
            assert_eq!(
                field, expected,
                "a bad {expected} must mark {expected}, not \"seconds\" — no form has a \
                 field called \"seconds\", so the user would see nothing highlighted"
            );
        }
    }

    #[tokio::test]
    async fn an_absent_row_reads_as_the_documented_defaults() {
        let db = Db::open_in_memory().await.unwrap();
        assert_eq!(
            read_settings(&db).await.unwrap(),
            WebServerSettingsDto::default()
        );
    }

    #[test]
    fn the_dto_default_is_the_domain_default_and_not_a_second_copy_of_it() {
        // Hardcoding the numbers here would create a second source of truth
        // that drifts the first time spec §5's table changes on one side only.
        assert_eq!(
            WebServerSettingsDto::default(),
            WebServerSettingsDto::from(openvhost_conf::WebServerSettings::default())
        );
    }

    #[tokio::test]
    async fn a_saved_setting_reads_back_through_the_dto() {
        let db = Db::open_in_memory().await.unwrap();
        write_settings(&db, valid_dto(), None).await.unwrap();
        assert_eq!(read_settings(&db).await.unwrap(), valid_dto());
    }

    /// A rejected field must not take the others down with it.
    ///
    /// The save is all-or-nothing: the guard runs before the repository is
    /// touched, so a form submitted with one bad value leaves every stored
    /// value exactly as it was. Without this, a user fixing one field could
    /// silently lose the nine they had already saved.
    #[tokio::test]
    async fn a_rejected_field_leaves_every_other_stored_value_untouched() {
        let db = Db::open_in_memory().await.unwrap();
        write_settings(&db, valid_dto(), None).await.unwrap();

        // Every field changed, and one of them invalid.
        let bad = WebServerSettingsDto {
            worker_connections: 4096,
            client_max_body_size: "1g".into(),
            keepalive_timeout: 45,
            tcp_nodelay: true,
            fastcgi_connect_timeout: 20,
            fastcgi_send_timeout: 30,
            fastcgi_read_timeout: 40,
            gzip: false,
            gzip_comp_level: 99, // rejected: outside 1..=9
            gzip_types: "text/plain".into(),
        };
        let e = write_settings(&db, bad, None)
            .await
            .expect_err("99 is not 1..=9");
        let (field, _) = expect_validation(e);
        assert_eq!(field, "gzip_comp_level");

        assert_eq!(
            read_settings(&db).await.unwrap(),
            valid_dto(),
            "a rejected field must not clobber the values already stored"
        );
    }

    // -----------------------------------------------------------------------
    // The nginx pre-check on the save path
    // -----------------------------------------------------------------------

    /// A checker with a canned verdict that records whether it was consulted.
    struct FakeChecker {
        verdict: openvhost_conf::SettingsCheck,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeChecker {
        fn new(verdict: openvhost_conf::SettingsCheck) -> Self {
            Self {
                verdict,
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn accepting() -> Self {
            Self::new(openvhost_conf::SettingsCheck::Accepted {
                stderr: String::new(),
            })
        }
        fn rejecting(field: Option<&'static str>, stderr: &str) -> Self {
            Self::new(openvhost_conf::SettingsCheck::Rejected {
                field,
                stderr: stderr.to_string(),
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl SettingsChecker for FakeChecker {
        async fn check(
            &self,
            _: &openvhost_conf::WebServerSettings,
        ) -> Result<openvhost_conf::SettingsCheck, openvhost_conf::ConfError> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self.verdict.clone())
        }
    }

    /// The point of the whole change: a value nginx refuses is refused HERE,
    /// on the field the user just edited, and never reaches the row.
    ///
    /// Without this, the value stores fine and instead breaks the NEXT
    /// `apply_config` — including one started from the Sites page, where the
    /// error names an nginx internal and marks no field at all.
    #[tokio::test]
    async fn an_nginx_rejection_marks_the_field_and_stores_nothing() {
        let db = Db::open_in_memory().await.unwrap();
        write_settings(&db, valid_dto(), None).await.unwrap();

        let checker = FakeChecker::rejecting(
            Some("gzip_comp_level"),
            "nginx: [emerg] value must be between 1 and 9 in /h/run/x/nginx.conf:17\n",
        );
        let mut submitted = valid_dto();
        submitted.gzip_comp_level = 9;
        let e = write_settings(&db, submitted, Some(&checker))
            .await
            .expect_err("nginx rejected this render");

        let (field, message) = expect_validation(e);
        assert_eq!(field, "gzip_comp_level");
        assert!(
            message.contains("value must be between 1 and 9"),
            "nginx's reason must survive to the user, got {message:?}"
        );
        assert!(
            !message.contains("/h/run/x/nginx.conf"),
            "the throwaway path is a file the user cannot open; it must not be shown: {message:?}"
        );
        assert_eq!(
            read_settings(&db).await.unwrap(),
            valid_dto(),
            "a value nginx refused must not be stored"
        );
    }

    /// A rejection that maps to no field is a banner — and still stores
    /// nothing. Falling through to the save "because we could not tell which
    /// field it was" would store exactly the values this check exists to keep
    /// out.
    #[tokio::test]
    async fn an_untraceable_rejection_is_a_banner_and_still_stores_nothing() {
        let db = Db::open_in_memory().await.unwrap();
        let checker = FakeChecker::rejecting(None, "nginx: [emerg] something structural\n");
        let e = write_settings(&db, valid_dto(), Some(&checker))
            .await
            .expect_err("a rejection with no field is still a rejection");
        match e {
            IpcError::Core { message } => assert!(message.contains("something structural")),
            other => panic!("expected a banner, got {other:?}"),
        }
        assert_eq!(
            read_settings(&db).await.unwrap(),
            WebServerSettingsDto::default(),
            "nothing was ever stored"
        );
    }

    #[tokio::test]
    async fn an_accepted_check_stores_the_settings() {
        let db = Db::open_in_memory().await.unwrap();
        let checker = FakeChecker::accepting();
        write_settings(&db, valid_dto(), Some(&checker))
            .await
            .unwrap();
        assert_eq!(checker.calls(), 1, "nginx must actually have been asked");
        assert_eq!(read_settings(&db).await.unwrap(), valid_dto());
    }

    /// With nginx not installed there is no checker — and the page must still
    /// save. Blocking the save would make the Web server settings uneditable
    /// on a fresh machine, for a check that cannot matter there: with no
    /// nginx there is no apply to break.
    #[tokio::test]
    async fn settings_still_save_when_nginx_is_not_installed() {
        let db = Db::open_in_memory().await.unwrap();
        write_settings(&db, valid_dto(), None).await.unwrap();
        assert_eq!(read_settings(&db).await.unwrap(), valid_dto());
    }

    /// The command-level half of `settings_still_save_when_nginx_is_not_installed`
    /// above — one of the six decision sites the 4B fix-wave audit found
    /// untested (item 2). That test drives `write_settings` directly with a
    /// hand-supplied `None`; nothing before this test proved
    /// `save_web_server_settings` itself actually threads `input` through and
    /// reaches the repository at all, rather than e.g. panicking on the
    /// `.map` or silently dropping the submitted values.
    ///
    /// VACUITY (neuter-and-watch-it-fail): changed the command to save
    /// `WebServerSettingsDto::default()` instead of `input` — failed with
    /// `left` (the stored defaults) `!= right` (`valid_dto()`); restoring
    /// `input` made it pass again.
    ///
    /// LIMITATION, stated rather than left implicit: this test does NOT
    /// hermetically prove `checker` came out `None` rather than
    /// `Some(NginxSettingsChecker { bin: <some other path>, .. })` — a
    /// checker built with the WRONG (e.g. hardcoded-fallback) `bin` and
    /// `write_settings`'s own `Err(ConfError::ValidatorSpawn { .. }) => save
    /// anyway` degradation (see `a_validator_that_cannot_be_spawned_does_not_block_the_save`
    /// below, which pins that behaviour deliberately) produce the IDENTICAL
    /// observable outcome this test checks. Distinguishing the two would need
    /// a way to inject a spy checker into `save_web_server_settings` itself,
    /// which the command does not offer today.
    #[tokio::test]
    async fn save_web_server_settings_saves_unchecked_when_no_nginx_binary_was_found() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("config/generated/nginx/nginx.conf"),
        }));

        save_web_server_settings(
            app.state::<DbHandle>(),
            app.state::<Option<StackPaths>>(),
            valid_dto(),
        )
        .await
        .unwrap();

        // Bound rather than inlined: `require()` borrows the handle, so the
        // `State` guard has to outlive the read.
        let handle = app.state::<DbHandle>();
        assert_eq!(
            read_settings(handle.require().unwrap()).await.unwrap(),
            valid_dto(),
            "design D3: with no nginx binary, the save must go through unchecked, not be blocked"
        );
    }

    /// A checker whose binary cannot be spawned, and one that hangs. The two
    /// must NOT behave the same way.
    struct FailingChecker(openvhost_conf::ConfError);

    #[async_trait::async_trait]
    impl SettingsChecker for FailingChecker {
        async fn check(
            &self,
            _: &openvhost_conf::WebServerSettings,
        ) -> Result<openvhost_conf::SettingsCheck, openvhost_conf::ConfError> {
            Err(match &self.0 {
                openvhost_conf::ConfError::ValidatorTimeout { bin, secs } => {
                    openvhost_conf::ConfError::ValidatorTimeout {
                        bin: bin.clone(),
                        secs: *secs,
                    }
                }
                _ => openvhost_conf::ConfError::ValidatorSpawn {
                    bin: "nginx".into(),
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                },
            })
        }
    }

    /// nginx removed since launch: the recorded binary will not spawn. That is
    /// the no-nginx case arriving late, so the save must still go through —
    /// otherwise uninstalling nginx silently makes this page unsavable.
    #[tokio::test]
    async fn a_validator_that_cannot_be_spawned_does_not_block_the_save() {
        let db = Db::open_in_memory().await.unwrap();
        let checker = FailingChecker(openvhost_conf::ConfError::ValidatorSpawn {
            bin: "nginx".into(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        });
        write_settings(&db, valid_dto(), Some(&checker))
            .await
            .unwrap();
        assert_eq!(read_settings(&db).await.unwrap(), valid_dto());
    }

    /// A validator that HUNG is not a validator that is absent. Treating a
    /// timeout as "unchecked, carry on" would store the very values the check
    /// exists to keep out, on the machines where nginx is present.
    #[tokio::test]
    async fn a_validator_that_times_out_fails_the_save() {
        let db = Db::open_in_memory().await.unwrap();
        let checker = FailingChecker(openvhost_conf::ConfError::ValidatorTimeout {
            bin: "nginx".into(),
            secs: 5,
        });
        write_settings(&db, valid_dto(), Some(&checker))
            .await
            .expect_err("a hung validator must not pass as checked");
        assert_eq!(
            read_settings(&db).await.unwrap(),
            WebServerSettingsDto::default(),
            "nothing may be stored when the check never completed"
        );
    }

    /// The newtype guard runs FIRST: an out-of-range value costs no process
    /// spawn, and reports the precise reason the newtype knows rather than
    /// whatever nginx would have said about the rendered line.
    #[tokio::test]
    async fn a_value_the_newtypes_reject_never_reaches_nginx() {
        let db = Db::open_in_memory().await.unwrap();
        let checker = FakeChecker::accepting();
        let mut bad = valid_dto();
        bad.gzip_comp_level = 99; // outside 1..=9
        let e = write_settings(&db, bad, Some(&checker))
            .await
            .expect_err("99 is not 1..=9");
        assert_eq!(expect_validation(e).0, "gzip_comp_level");
        assert_eq!(
            checker.calls(),
            0,
            "the cheap guard must reject before spawning a validator"
        );
    }

    #[test]
    fn a_rejection_message_keeps_the_reason_and_drops_the_throwaway_path() {
        assert_eq!(
            rejection_message("nginx: [emerg] value must be between 1 and 9 in /h/nginx.conf:17\n"),
            "nginx rejected this value: value must be between 1 and 9"
        );
        assert_eq!(
            rejection_message(
                "nginx: [emerg] \"client_max_body_size\" directive invalid value in /h/x.conf:7"
            ),
            "nginx rejected this value: \"client_max_body_size\" directive invalid value"
        );
    }

    /// An unparseable stderr must be passed through, not swallowed: a message
    /// we failed to parse is still the only diagnostic the user has.
    #[test]
    fn a_rejection_message_we_cannot_parse_is_shown_whole() {
        assert_eq!(rejection_message("total nonsense\n"), "total nonsense");
        assert_eq!(
            rejection_message("nginx: [emerg] no line reference here"),
            "nginx rejected this value: no line reference here"
        );
    }

    /// Both settings commands REFUSE with no store — design D3's decision,
    /// pinned so a later "just serve the defaults" edit has to argue with a
    /// test rather than slip through. The read is the one that matters: a
    /// populated, editable form whose Save can only ever fail is the quiet
    /// wrong answer this project keeps getting burned by.
    ///
    /// The save is checked against a stack with no nginx binary, so the
    /// `nginx -t` pre-check is skipped and this is genuinely reaching the
    /// store's refusal rather than a validator's.
    ///
    /// Vacuity: with all 13 `let db = db.require()?;` guards and
    /// `web_server_settings`' own `read_settings(db.require()?)` replaced by
    /// `Db::open_in_memory()`, this test failed on `unwrap_err`. Reverted and
    /// re-run green.
    #[tokio::test]
    async fn both_settings_commands_refuse_and_name_why_when_the_store_is_down() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: None,
            nginx_conf: home.path().join("config/generated/nginx/nginx.conf"),
        }));

        assert_store_refusal(
            &web_server_settings(app.state()).await.unwrap_err(),
            "web_server_settings",
        );
        assert_store_refusal(
            &save_web_server_settings(app.state(), app.state(), valid_dto())
                .await
                .unwrap_err(),
            "save_web_server_settings",
        );
    }
}

// ---------------------------------------------------------------------------
// Live log viewer (Logs page)
// spec docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md
// ---------------------------------------------------------------------------

/// The ONLY way a log's identity crosses IPC (spec D5): a CLOSED, tagged
/// enum — never a path, a filename, or any string the renderer gets to pick
/// unconstrained. `domain`/`major` arrive as plain strings and are parsed
/// into the existing `Domain`/`PhpVersion` newtypes at ingress
/// (`TryFrom<LogSourceDto> for LogSource`, below) before anything else
/// touches them — the same "parse, don't validate" discipline as
/// `SiteInput`'s own `TryFrom`. `ServiceRing`'s `id` stays a bare `String`:
/// it never becomes a filesystem path (see `derive_path`), only a
/// `Supervisor` registry lookup, exactly like `service_log_tail`'s existing
/// `id` parameter.
///
/// Mirrors `WebServerBrand::parse` → `live_config_path`'s shape (a closed
/// set, parsed before anything is derived from it), which this project's
/// auditor has already accepted for the identical class of problem.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LogSourceDto {
    NginxError,
    NginxAccess,
    PhpFpm { major: String },
    SiteAccess { domain: String },
    SiteError { domain: String },
    ServiceRing { id: String },
}

/// `LogSourceDto` after ingress parsing: every `domain`/`major` is now a
/// validated newtype, so `LogPaths` (via `derive_path`, below) can turn one
/// into a path without re-checking a charset. Deliberately does NOT cross
/// IPC (no `specta::Type`, no serde derive) — this is the internal shape
/// the catalogue check and the path derivation both consume.
#[derive(Debug, Clone)]
enum LogSource {
    NginxError,
    NginxAccess,
    PhpFpm(PhpVersion),
    SiteAccess(Domain),
    SiteError(Domain),
    ServiceRing(String),
}

impl TryFrom<LogSourceDto> for LogSource {
    type Error = IpcError;

    fn try_from(dto: LogSourceDto) -> Result<Self, IpcError> {
        Ok(match dto {
            LogSourceDto::NginxError => LogSource::NginxError,
            LogSourceDto::NginxAccess => LogSource::NginxAccess,
            LogSourceDto::PhpFpm { major } => LogSource::PhpFpm(PhpVersion::parse(&major)?),
            LogSourceDto::SiteAccess { domain } => LogSource::SiteAccess(Domain::parse(&domain)?),
            LogSourceDto::SiteError { domain } => LogSource::SiteError(Domain::parse(&domain)?),
            LogSourceDto::ServiceRing { id } => LogSource::ServiceRing(id),
        })
    }
}

/// Spec D5's live-catalogue check: verify `source` names something that
/// genuinely exists RIGHT NOW — BEFORE any path is derived or any
/// filesystem call is made. A deleted site or an uninstalled PHP major is
/// rejected HERE, so the rejection is provably filesystem-free rather than
/// merely a missing-file read three lines later (which `read_window` would
/// also handle safely, via `exists: false` — but that is not the same
/// property: it would still touch the filesystem for something that should
/// never have been looked up at all).
///
/// `NginxError`/`NginxAccess` have no catalogue to check against — nginx's
/// globals are not tied to any one site or runtime. `ServiceRing` passes
/// through unconditionally too: it is rejected later, structurally, by
/// `derive_path` (it has no on-disk path at all), not by a catalogue rule
/// here.
///
/// **`db` is [`DbHandle::Unavailable`] when `state.db` did not open, and only
/// the site arm fails closed** (optional-state.db design D2's SPLIT). That
/// asymmetry is the whole decision, so it is stated rather than left to be read
/// out of the arms:
///
/// - nginx's globals, a php-fpm pool and a ring are all named by a catalogue
///   that does not live in `state.db`, so a missing store tells us nothing
///   about them and they **proceed**. Failing the whole function closed would
///   look safer and produce the opposite of the design — the nginx error log
///   would become unreadable exactly when the app is broken, which is the one
///   thing a user needs then.
/// - the site arm **refuses**, because for a site this check IS the
///   path-confinement gate: the domain arrives over IPC, and `state.db` is the
///   only thing that says which domains exist. With no store there is nothing
///   to check it against, and degrading would derive
///   `<home>/logs/sites/<domain>/…` for a domain nothing vetted.
///
/// The whole [`DbHandle`] rather than an `Option<&Db>`, so this refusal can
/// carry the REASON the store is missing — *unable to open database file* —
/// which is the entire point of [`DbHandle::Unavailable`]'s `reason` field. An
/// `Option` throws that away at the call site, and the inline refusal is what a
/// user reads at the moment they are blocked; deferring the detail to the
/// app-level banner is the same "a reader could infer it" reasoning this
/// codebase keeps rejecting.
async fn check_catalogue(
    source: &LogSource,
    db: &DbHandle,
    runtimes: &RwLock<Option<InstalledRuntimes>>,
) -> Result<(), IpcError> {
    match source {
        LogSource::NginxError | LogSource::NginxAccess | LogSource::ServiceRing(_) => Ok(()),
        LogSource::PhpFpm(major) => {
            let installed = runtimes
                .read()
                .map_err(|_| IpcError::Core {
                    message: "runtime list is poisoned".into(),
                })?
                .as_ref()
                .is_some_and(|r| r.php.iter().any(|rt| rt.major == major.as_str()));
            if installed {
                Ok(())
            } else {
                Err(IpcError::Validation {
                    field: "php_version".into(),
                    message: format!("PHP {} is not installed", major.as_str()),
                })
            }
        }
        LogSource::SiteAccess(domain) | LogSource::SiteError(domain) => {
            // Fail closed. `IpcError::Core`, not `Validation` on `domain`: the
            // domain is not what is wrong, and a page that renders this must
            // not read it as "no such site".
            //
            // Matched exhaustively rather than routed through
            // `DbHandle::require()`, for the one thing `require` cannot do: keep
            // the clause that says why THIS refusal follows from the store being
            // down. The shared sentence and the reason still come from
            // `unavailable_message`, so only the trailing clause is local — and a
            // third `DbHandle` variant would fail to compile here rather than
            // fall into a wildcard.
            let db = match db {
                DbHandle::Ready(db) => db,
                DbHandle::Unavailable { reason } => {
                    return Err(IpcError::Core {
                        message: format!(
                            "{}, so no site's log can be confirmed as one of yours",
                            crate::db_state::unavailable_message(reason)
                        ),
                    });
                }
            };
            let repo = SqliteSiteRepository::new(db);
            let known = repo
                .list()
                .await?
                .iter()
                .any(|s| s.domain.as_str() == domain.as_str());
            if known {
                Ok(())
            } else {
                Err(IpcError::Validation {
                    field: "domain".into(),
                    message: format!("no site named {} exists", domain.as_str()),
                })
            }
        }
    }
}

/// Spec D5's `starts_with(<home>/logs)` post-condition: the one-line
/// assertion a reviewer can verify at a glance, right at the IPC boundary —
/// not merely documented in `LogPaths`'s own module doc, three files away.
/// `LogPaths`'s construction already makes this provably true for every
/// path it derives (its charset-validated newtypes cannot contain `..` or
/// `/`), so this can never actually fail in production. It degrades to an
/// honest error instead of a panic if it ever did — the same "must never
/// crash on an invariant a type already guarantees" discipline as
/// `WebServerBrand::live_config_path`'s unreachable `Apache` arm.
fn confined(path: PathBuf, root: &Path) -> Result<PathBuf, IpcError> {
    if path.starts_with(root) {
        Ok(path)
    } else {
        Err(IpcError::Core {
            message: format!(
                "internal error: derived log path {} escaped {}",
                path.display(),
                root.display()
            ),
        })
    }
}

/// The ONLY place a `LogSource` becomes a filesystem path (spec D5),
/// confined by `confined` above. `ServiceRing` has no on-disk path: ring
/// output is read through the EXISTING `service_log_tail` command +
/// `service-log` event push, not through `read_log_window` — spec D7's
/// two-mechanism seam, deliberately NOT unified here (unifying it behind
/// the poll would add 500ms to output that is instant today). Rejected as a
/// validation error rather than silently routed through, so a caller that
/// sends a ring source to `read_log_window`/`reveal_log_folder` gets an
/// honest "wrong command" message instead of a confusing "not found".
fn derive_path(source: &LogSource, paths: &openvhost_core::LogPaths) -> Result<PathBuf, IpcError> {
    let path = match source {
        LogSource::NginxError => paths.nginx_error(),
        LogSource::NginxAccess => paths.nginx_access(),
        LogSource::PhpFpm(major) => paths.php_fpm_error(major),
        LogSource::SiteAccess(domain) => paths.site_access(domain),
        LogSource::SiteError(domain) => paths.site_error(domain),
        LogSource::ServiceRing(id) => {
            return Err(IpcError::Validation {
                field: "source".into(),
                message: format!(
                    "ring source {id:?} is read through service_log_tail, not this command"
                ),
            });
        }
    };
    confined(path, &paths.root())
}

/// Ingress parse → live-catalogue check → `LogPaths` derivation →
/// confinement post-condition, in that order (spec D5) — the ONE place
/// `read_log_window` and `reveal_log_folder` both turn a wire
/// `LogSourceDto` into a path they may actually touch. The catalogue check
/// runs BEFORE `derive_path`, so an unknown/deleted site or an uninstalled
/// PHP major is rejected without a single filesystem call.
async fn resolve_log_path(
    dto: LogSourceDto,
    db: &DbHandle,
    runtimes: &RwLock<Option<InstalledRuntimes>>,
    paths: &openvhost_core::LogPaths,
) -> Result<PathBuf, IpcError> {
    let source: LogSource = dto.try_into()?;
    check_catalogue(&source, db, runtimes).await?;
    derive_path(&source, paths)
}

/// Spec D3's 256-byte cap on a log filter query, enforced HERE at IPC
/// ingress: `openvhost_core::LogQuery::needle` accepts whatever `String` it
/// is given and applies no length bound of its own (see that module's own
/// doc comment, which explicitly defers this cap to this command layer).
/// Parse-don't-validate, mirroring `Domain`/`Docroot`: constructible only
/// via `parse`, so an oversized needle can never reach the reader from this
/// command surface.
const LOG_NEEDLE_MAX_BYTES: usize = 256;

struct LogNeedle(String);

impl LogNeedle {
    fn parse(s: &str) -> Result<Self, IpcError> {
        if s.len() > LOG_NEEDLE_MAX_BYTES {
            return Err(IpcError::Validation {
                field: "needle".into(),
                message: format!("must be at most {LOG_NEEDLE_MAX_BYTES} bytes"),
            });
        }
        Ok(Self(s.to_string()))
    }

    fn into_inner(self) -> String {
        self.0
    }
}

/// `read_log_window`'s opaque cursor, decoded from the wire string a
/// previous call handed back. See `openvhost_core::LogCursor`'s own doc
/// comment for why accepting whatever JSON the caller sends is safe rather
/// than a confinement bypass: a forged identity or offset can only ever
/// move the resume point WITHIN whatever file `source` (independently
/// parsed, catalogue-checked and derived above) already resolves to. A
/// shape that does not even decode is a genuine ingress error, not a
/// silently-tolerated forgery.
fn decode_cursor(raw: Option<String>) -> Result<Option<openvhost_core::LogCursor>, IpcError> {
    raw.map(|s| {
        serde_json::from_str::<openvhost_core::LogCursor>(&s).map_err(|e| IpcError::Validation {
            field: "cursor".into(),
            message: format!("invalid cursor: {e}"),
        })
    })
    .transpose()
}

/// The other half of `decode_cursor`: `LogCursor` derives `Serialize`, so
/// this is a plain, lossless round-trip — see that type's doc comment for
/// why handing it back to the caller unmodified is deliberate
/// ("transport-transparent, not opaque").
fn encode_cursor(cursor: Option<openvhost_core::LogCursor>) -> Result<Option<String>, IpcError> {
    cursor
        .map(|c| {
            serde_json::to_string(&c).map_err(|e| IpcError::Core {
                message: format!("failed to encode log cursor: {e}"),
            })
        })
        .transpose()
}

/// One line of `read_log_window`'s returned window. `level` reuses
/// `openvhost_proc::LogLevel` directly (already specta-typed and already
/// crossing IPC via `ServiceLogEvent`/`ServiceStatus`) rather than a
/// duplicate wire enum — see `openvhost_core::logs::read`'s own doc comment
/// on why this is the ONE classifier for file lines, deliberately distinct
/// from the ring's own classifier.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LogRowDto {
    pub level: LogLevel,
    pub text: String,
}

/// Mirrors `openvhost_core::LogReset` 1:1 as a wire-safe copy (that type
/// carries no `serde`/`specta`, since `openvhost-core`'s logs module is
/// kept serde/specta-free by design).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum LogResetDto {
    Rotated,
    Truncated,
}

impl From<openvhost_core::LogReset> for LogResetDto {
    fn from(r: openvhost_core::LogReset) -> Self {
        match r {
            openvhost_core::LogReset::Rotated => LogResetDto::Rotated,
            openvhost_core::LogReset::Truncated => LogResetDto::Truncated,
        }
    }
}

/// `read_log_window`'s output (spec D7).
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LogWindowDto {
    /// Matching, classified, possibly-truncated lines, oldest first.
    pub rows: Vec<LogRowDto>,
    /// Opaque JSON, round-tripped verbatim through `decode_cursor`/
    /// `encode_cursor`. `None` only when `exists` is `false`.
    pub cursor: Option<String>,
    /// `false` when the source's file does not exist right now — a normal,
    /// pollable state (a service that has not started yet, a site whose
    /// Apply has not run), never an error.
    pub exists: bool,
    pub reset: Option<LogResetDto>,
    pub has_more: bool,
    /// The file's total size, in bytes. `u64` crossing the
    /// `.dangerously_cast_bigints_to_number()` boundary — see `lib.rs`'s
    /// standing warning. A log file is nowhere near 2^53 bytes.
    pub size_bytes: u64,
    /// How many bytes of the file THIS call actually read (always bounded —
    /// see `openvhost_core::logs::read`'s own doc comment). Same bigint
    /// note as `size_bytes`.
    pub scanned_bytes: u64,
    pub truncated_lines: u32,
    pub scan_bound_reached: bool,
}

/// `read_log_window`'s input.
#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LogWindowQuery {
    pub source: LogSourceDto,
    pub cursor: Option<String>,
    /// Capped at `LOG_NEEDLE_MAX_BYTES` bytes by `LogNeedle::parse` (spec
    /// D3) — the reader itself applies no bound.
    pub needle: Option<String>,
    pub case_sensitive: bool,
    pub min_level: Option<LogLevel>,
}

fn log_window_dto(w: openvhost_core::LogWindow) -> Result<LogWindowDto, IpcError> {
    Ok(LogWindowDto {
        rows: w
            .rows
            .into_iter()
            .map(|r| LogRowDto {
                level: r.level,
                text: r.text,
            })
            .collect(),
        cursor: encode_cursor(w.cursor)?,
        exists: w.exists,
        reset: w.reset.map(LogResetDto::from),
        has_more: w.has_more,
        size_bytes: w.size_bytes,
        scanned_bytes: w.scanned_bytes,
        truncated_lines: w.truncated_lines,
        scan_bound_reached: w.scan_bound_reached,
    })
}

/// `"file"` | `"ring"` — spec D7's two-mechanism seam: a `"file"` row is
/// read via `read_log_window`; a `"ring"` row is read via the EXISTING
/// `service_log_tail` command + `service-log` event push, never through
/// `read_log_window` (see `derive_path`'s doc comment for why this is
/// deliberately not unified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum LogSourceKindDto {
    File,
    Ring,
}

/// One row of `list_log_sources`'s catalogue (spec D7).
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LogSourceRowDto {
    pub source: LogSourceDto,
    pub label: String,
    pub kind: LogSourceKindDto,
    pub exists: bool,
    /// `None` for a `"ring"` row (a ring buffer has no on-disk byte size)
    /// and for a `"file"` row whose file does not exist yet. Same
    /// `.dangerously_cast_bigints_to_number()` note as
    /// `LogWindowDto::size_bytes` when present.
    pub size_bytes: Option<u64>,
    /// The supervisor id to drive `serviceLogTail`/`onServiceLog` with, for
    /// a `"ring"` row. `None` for a `"file"` row.
    pub service_id: Option<String>,
}

/// Build one `"file"`-kind row: stat the path via `symlink_metadata` (never
/// following — mirroring `read_window`'s own refusal) and report
/// `exists`/`sizeBytes` from a REGULAR FILE only. A symlink planted at a log
/// path would make `read_log_window` refuse it anyway (spec D5); listing it
/// as existing here would invite a click that goes nowhere. `tokio::fs`,
/// not `std::fs`: a stalled `OPENVHOST_HOME` mount must not pin a tokio
/// worker while this enumerates every source (mirrors `list_web_servers`'s
/// own reasoning for its config-file stat).
async fn file_row(
    source: LogSourceDto,
    label: impl Into<String>,
    path: PathBuf,
) -> LogSourceRowDto {
    let meta = tokio::fs::symlink_metadata(&path).await.ok();
    let size_bytes = meta.as_ref().filter(|m| m.is_file()).map(|m| m.len());
    LogSourceRowDto {
        source,
        label: label.into(),
        kind: LogSourceKindDto::File,
        exists: size_bytes.is_some(),
        size_bytes,
        service_id: None,
    }
}

/// The full log-source catalogue: nginx's two globals (always listed), one
/// row per INSTALLED php-fpm major, two rows (access + error) per site in
/// `state.db`, and one ring row per service the supervisor knows about
/// (spec D7).
#[tauri::command]
#[specta::specta]
// DEGRADE (optional-state.db design D2): with no store the catalogue keeps
// every row `state.db` was not the source of — nginx's globals, one per
// installed php-fpm major, one per supervised service — and loses only the
// per-site pairs. A shorter list is indistinguishable from "you have no sites",
// which is why D5's banner is what makes this honest rather than quiet.
pub async fn list_log_sources(
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    stack: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
) -> Result<Vec<LogSourceRowDto>, IpcError> {
    let p = stack_paths(&stack)?;
    let log_paths = openvhost_core::LogPaths::new(&p.home);

    let mut rows = vec![
        file_row(
            LogSourceDto::NginxError,
            "nginx error log",
            log_paths.nginx_error(),
        )
        .await,
        file_row(
            LogSourceDto::NginxAccess,
            "nginx access log",
            log_paths.nginx_access(),
        )
        .await,
    ];

    // Owned clone, guard dropped immediately — never held across an
    // `.await` (mirrors `php_environment`'s identical pattern).
    let installed_php: Vec<openvhost_core::PhpRuntime> = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .as_ref()
        .map(|r| r.php.clone())
        .unwrap_or_default();
    for rt in &installed_php {
        // Skip silently rather than fail the whole listing: a malformed
        // major here would mean the PHP discovery probe produced something
        // `PhpVersion::parse` rejects, which should not happen in practice
        // and has no user-facing error channel on a listing row.
        if let Ok(major) = PhpVersion::parse(&rt.major) {
            let label = format!("PHP {} pool log", major.as_str());
            let path = log_paths.php_fpm_error(&major);
            rows.push(
                file_row(
                    LogSourceDto::PhpFpm {
                        major: rt.major.clone(),
                    },
                    label,
                    path,
                )
                .await,
            );
        }
    }

    // The only rows `state.db` is the source of, and so the only ones a
    // degraded store costs. Every other row above and below is a filesystem or
    // supervisor fact and is unaffected.
    if let Some(db) = db.optional() {
        let repo = SqliteSiteRepository::new(db);
        for site in repo.list().await? {
            let domain = site.domain.as_str().to_string();
            rows.push(
                file_row(
                    LogSourceDto::SiteAccess {
                        domain: domain.clone(),
                    },
                    format!("{domain} access log"),
                    log_paths.site_access(&site.domain),
                )
                .await,
            );
            rows.push(
                file_row(
                    LogSourceDto::SiteError {
                        domain: domain.clone(),
                    },
                    format!("{domain} error log"),
                    log_paths.site_error(&site.domain),
                )
                .await,
            );
        }
    }

    for status in sup.snapshot() {
        rows.push(LogSourceRowDto {
            source: LogSourceDto::ServiceRing {
                id: status.id.clone(),
            },
            label: format!("{} output", status.display_name),
            kind: LogSourceKindDto::Ring,
            exists: true,
            size_bytes: None,
            service_id: Some(status.id),
        });
    }

    Ok(rows)
}

/// A bounded, filtered window of one log source (spec D3/D4/D7). The
/// catalogue check (spec D5) happens inside `resolve_log_path`, before any
/// path is derived or any filesystem call is made.
#[tauri::command]
#[specta::specta]
// SPLIT (optional-state.db design D2): the store reaches only `check_catalogue`,
// which refuses a site source and lets the other three through — see its doc.
pub async fn read_log_window(
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    stack: tauri::State<'_, Option<StackPaths>>,
    input: LogWindowQuery,
) -> Result<LogWindowDto, IpcError> {
    let p = stack_paths(&stack)?;
    let log_paths = openvhost_core::LogPaths::new(&p.home);
    let path = resolve_log_path(input.source, db.inner(), runtimes.inner(), &log_paths).await?;

    let cursor = decode_cursor(input.cursor)?;
    let needle = input
        .needle
        .map(|n| LogNeedle::parse(&n))
        .transpose()?
        .map(LogNeedle::into_inner);
    let query = openvhost_core::LogQuery {
        needle,
        case_sensitive: input.case_sensitive,
        min_level: input.min_level,
    };
    let limits = openvhost_core::LogLimits::default();

    // `read_window` is synchronous std::fs I/O, up to `limits.scan` (16 MiB)
    // per call — run on the blocking pool so a stalled `OPENVHOST_HOME`
    // mount cannot pin a tokio worker (same reasoning as
    // `read_web_server_config`'s doc comment and `home_disk_usage`'s
    // identical `spawn_blocking` use).
    let window = tauri::async_runtime::spawn_blocking(move || {
        openvhost_core::read_window(&path, cursor, &query, &limits)
    })
    .await
    .map_err(|e| IpcError::Core {
        message: format!("the log-read task failed to run: {e}"),
    })?
    .map_err(IpcError::from)?;

    log_window_dto(window)
}

/// The folder `reveal_log_folder` should open for `source`: everything the
/// command needs EXCEPT the actual OS call.
///
/// Split out for the same reason `initialize_mysql_gate` is (see its doc
/// comment): `tauri::test::mock_builder()` only ever produces an
/// `AppHandle<MockRuntime>`, a DIFFERENT concrete type than the
/// `AppHandle<Wry>` `reveal_log_folder`'s signature needs to call
/// `OpenerExt::opener()`, so the full command cannot be invoked directly
/// from a test at all. This function needs no `AppHandle`, so the
/// catalogue-check-then-derive logic it shares with `read_log_window` (via
/// `resolve_log_path`) — plus the one bit unique to this command, taking
/// the derived path's parent — stays directly testable.
async fn reveal_log_folder_target(
    source: LogSourceDto,
    db: &DbHandle,
    runtimes: &RwLock<Option<InstalledRuntimes>>,
    paths: &openvhost_core::LogPaths,
) -> Result<PathBuf, IpcError> {
    let path = resolve_log_path(source, db, runtimes, paths).await?;
    path.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| IpcError::Core {
            message: format!("{} has no parent directory", path.display()),
        })
}

/// Open the folder containing `source`'s log file(s) in the OS file manager
/// — "Open log folder" (spec D8), the user's one recourse against unbounded
/// on-disk growth this slice ships without rotation. The path is derived
/// entirely in Rust (`reveal_log_folder_target` → `resolve_log_path`, spec
/// D5); the caller only ever names a `LogSourceDto`. No `capabilities/`
/// grant is needed — this calls the opener plugin's Rust API directly
/// rather than its JS-invoked command, exactly like
/// `open_site`/`open_homebrew_site` above.
#[tauri::command]
#[specta::specta]
// SPLIT, through the same `check_catalogue` seam `read_log_window` goes
// through — see its doc for which arms proceed with no store and which refuses.
pub async fn reveal_log_folder(
    app: tauri::AppHandle,
    db: tauri::State<'_, DbHandle>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    stack: tauri::State<'_, Option<StackPaths>>,
    source: LogSourceDto,
) -> Result<(), IpcError> {
    use tauri_plugin_opener::OpenerExt;
    let p = stack_paths(&stack)?;
    let log_paths = openvhost_core::LogPaths::new(&p.home);
    let folder = reveal_log_folder_target(source, db.inner(), runtimes.inner(), &log_paths).await?;
    app.opener()
        .open_path(folder.display().to_string(), None::<&str>)
        .map_err(|e| IpcError::Core {
            message: e.to_string(),
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod log_ipc_tests {
    use tauri::Manager;

    use super::*;

    fn stack(home: &Path) -> Option<StackPaths> {
        Some(StackPaths {
            home: home.to_path_buf(),
            nginx_bin: Some(home.join("nginx")),
            nginx_conf: home.join("nginx.conf"),
        })
    }

    async fn site_in(db: &Db, domain: &str) {
        let repo = SqliteSiteRepository::new(db);
        let new: NewSite = SiteInput {
            name: domain.split('.').next().unwrap().to_string(),
            domain: domain.to_string(),
            docroot: "/tmp/does-not-matter".into(),
            web_server: "nginx".into(),
            php_version: "8.3".into(),
            enabled: true,
        }
        .try_into()
        .unwrap();
        repo.create(new).await.unwrap();
    }

    fn query(source: LogSourceDto, cursor: Option<String>) -> LogWindowQuery {
        LogWindowQuery {
            source,
            cursor,
            needle: None,
            case_sensitive: false,
            min_level: None,
        }
    }

    // ---- catalogue: unknown/deleted site, out-of-catalogue major --------
    //
    // Vacuity method: WITHOUT `check_catalogue`, both calls below would
    // resolve straight to a `LogPaths`-derived path, and `read_window` would
    // report `exists: false` (a missing file is not an error — spec D3), so
    // the call would return `Ok`, not `Err`. Asserting `Err(Validation)` is
    // therefore a genuine red/green distinction, not a tautology — confirmed
    // directly: temporarily replacing `check_catalogue`'s call in
    // `resolve_log_path` with a no-op made exactly these tests fail (and no
    // others), then reverting made them pass again.
    //
    // These tests deliberately do NOT assert "no filesystem call happened":
    // nothing in this code path ever creates `<home>/logs` either on
    // rejection OR on a successful-but-missing-file read (only `Apply`/
    // `ensure_log_dir` create that directory), so a directory-existence
    // check would be true either way and prove nothing. The actual
    // guarantee — that a rejected catalogue check can reach no filesystem
    // call at all — is structural, not something a black-box test here can
    // observe: `resolve_log_path`'s body is a straight `?`-chain,
    // `check_catalogue(...).await?` returns early on `Err`, so `derive_path`
    // (and therefore `read_window`) is never reached once the catalogue
    // check fails. What these tests DO prove, and the only claim their names
    // make, is that the rejection is a `Validation` error on the right
    // field — not a silently-passed-through `Ok`.

    #[tokio::test]
    async fn unknown_site_is_rejected_with_a_validation_error() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(
                LogSourceDto::SiteAccess {
                    domain: "ghost.localhost".into(),
                },
                None,
            ),
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "domain"),
            other => panic!("expected Validation on domain, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_deleted_sites_domain_is_rejected_the_same_way() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        site_in(&db, "shop.localhost").await;
        let repo = SqliteSiteRepository::new(&db);
        let id = repo.list().await.unwrap()[0].id.clone();
        assert!(repo.delete(&id).await.unwrap());
        manage_db(&app, db);
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(
                LogSourceDto::SiteError {
                    domain: "shop.localhost".into(),
                },
                None,
            ),
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "domain"),
            other => panic!("expected Validation on domain, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uninstalled_php_major_is_rejected_with_a_validation_error() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: Some(home.path().join("nginx")),
            php: vec![],
        })));

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(
                LogSourceDto::PhpFpm {
                    major: "9.9".into(),
                },
                None,
            ),
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "php_version"),
            other => panic!("expected Validation on php_version, got {other:?}"),
        }
    }

    // ---- derive_path: a ServiceRing source has no on-disk path -----------
    //
    // Spec D7's two-mechanism seam: a `ServiceRing` source must be REJECTED
    // by both file-reading commands, never silently routed to a path arm
    // (which would mean reading/revealing whatever `paths.root()` or some
    // other arm's path happens to resolve to under a ring id). Vacuity
    // method: temporarily made `derive_path`'s `ServiceRing` arm fall
    // through to `paths.nginx_error()` instead of returning early — both
    // tests below failed (rejecting nothing, `read_log_window` returned
    // `Ok` and `reveal_log_folder_target` returned nginx's log directory);
    // reverting made them pass again.

    #[tokio::test]
    async fn a_ring_source_is_rejected_by_read_log_window() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(LogSourceDto::ServiceRing { id: "nginx".into() }, None),
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "source"),
            other => panic!("expected Validation on source, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_ring_source_is_rejected_by_reveal_log_folder_target() {
        let home = tempfile::tempdir().unwrap();
        let db = Db::open_in_memory().await.unwrap();
        let runtimes = RwLock::new(None::<InstalledRuntimes>);
        let log_paths = openvhost_core::LogPaths::new(home.path());

        let err = reveal_log_folder_target(
            LogSourceDto::ServiceRing { id: "nginx".into() },
            &DbHandle::Ready(db),
            &runtimes,
            &log_paths,
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "source"),
            other => panic!("expected Validation on source, got {other:?}"),
        }
    }

    // ---- confinement: a symlink at a derived log path is refused --------
    //
    // Exercises the REAL reader end to end: the catalogue check PASSES (the
    // site genuinely exists), a path is genuinely derived, and only THEN
    // does `read_window`'s own `symlink_metadata` refusal fire.

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_planted_at_a_derived_log_path_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        site_in(&db, "shop.localhost").await;
        manage_db(&app, db);
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let site_dir = home.path().join("logs/sites/shop.localhost");
        std::fs::create_dir_all(&site_dir).unwrap();
        let victim = home.path().join("victim.txt");
        std::fs::write(&victim, b"secret").unwrap();
        std::os::unix::fs::symlink(&victim, site_dir.join("access.log")).unwrap();

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(
                LogSourceDto::SiteAccess {
                    domain: "shop.localhost".into(),
                },
                None,
            ),
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Core { message } => {
                assert!(message.contains("not a plain file"), "got {message:?}")
            }
            other => panic!("expected Core (NotAPlainFile), got {other:?}"),
        }
    }

    // ---- ingress: the 256-byte query cap ---------------------------------
    //
    // Vacuity method: `NginxError` needs no catalogue check and its file need
    // not exist (a missing file is `exists: false`, not an error), so
    // WITHOUT `LogNeedle`'s cap this exact call would succeed — the same
    // red/green distinction as the catalogue tests above.

    #[tokio::test]
    async fn an_over_long_query_is_rejected_at_ingress() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            LogWindowQuery {
                source: LogSourceDto::NginxError,
                cursor: None,
                needle: Some("a".repeat(LOG_NEEDLE_MAX_BYTES + 1)),
                case_sensitive: false,
                min_level: None,
            },
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "needle"),
            other => panic!("expected Validation on needle, got {other:?}"),
        }
    }

    #[test]
    fn a_query_at_exactly_the_cap_is_accepted() {
        assert!(LogNeedle::parse(&"a".repeat(LOG_NEEDLE_MAX_BYTES)).is_ok());
    }

    #[test]
    fn a_query_one_byte_over_the_cap_is_rejected() {
        assert!(LogNeedle::parse(&"a".repeat(LOG_NEEDLE_MAX_BYTES + 1)).is_err());
    }

    // ---- the starts_with post-condition itself ---------------------------
    //
    // Neuter-proof: `LogPaths` can never actually PRODUCE a violation (its
    // own test suite pins that), so this exercises `confined` directly with
    // a synthetic one — replacing its real body with `Ok(path)`
    // unconditionally would make the first case here fail.

    #[test]
    fn confined_rejects_a_path_outside_the_given_root() {
        let root = Path::new("/home/x/logs");
        let outside = PathBuf::from("/etc/passwd");
        assert!(confined(outside, root).is_err());
    }

    #[test]
    fn confined_accepts_a_path_under_the_given_root() {
        let root = Path::new("/home/x/logs");
        let inside = root.join("nginx.error.log");
        assert_eq!(confined(inside.clone(), root).unwrap(), inside);
    }

    // ---- list_log_sources: shape for a fixture home ----------------------

    #[tokio::test]
    async fn list_log_sources_enumerates_every_kind_for_a_fixture_home() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let db = Db::open_in_memory().await.unwrap();
        site_in(&db, "shop.localhost").await;
        manage_db(&app, db);
        app.manage(stack(home.path()));
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: Some(home.path().join("nginx")),
            php: vec![openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: home.path().join("php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            }],
        })));
        let sup = Arc::new(Supervisor::new(openvhost_proc::default_driver()));
        sup.register(openvhost_proc::ServiceSpec {
            id: "nginx".into(),
            display_name: "nginx".into(),
            endpoint: None,
            spawn: openvhost_proc::SpawnSpec {
                program: PathBuf::from("/usr/bin/true"),
                args: vec![],
                cwd: None,
                env: vec![],
            },
            readiness: openvhost_proc::ReadinessProbe::default(),
            grace: openvhost_proc::DEFAULT_GRACE,
        });
        app.manage(sup);

        let rows = list_log_sources(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            app.state::<Arc<Supervisor>>(),
        )
        .await
        .unwrap();

        assert!(
            rows.iter()
                .any(|r| r.source == LogSourceDto::NginxError && r.kind == LogSourceKindDto::File)
        );
        assert!(rows.iter().any(|r| r.source == LogSourceDto::NginxAccess));
        assert!(rows.iter().any(
            |r| matches!(&r.source, LogSourceDto::PhpFpm { major } if major.as_str() == "8.3")
        ));
        assert!(rows.iter().any(|r| matches!(
            &r.source,
            LogSourceDto::SiteAccess { domain } if domain.as_str() == "shop.localhost"
        )));
        assert!(rows.iter().any(|r| matches!(
            &r.source,
            LogSourceDto::SiteError { domain } if domain.as_str() == "shop.localhost"
        )));
        assert!(rows.iter().any(|r| matches!(
            &r.source,
            LogSourceDto::ServiceRing { id } if id.as_str() == "nginx"
        ) && r.kind == LogSourceKindDto::Ring
            && r.service_id.as_deref() == Some("nginx")));

        for r in rows.iter().filter(|r| r.kind == LogSourceKindDto::File) {
            assert!(
                !r.exists,
                "expected {:?} to not exist in a fresh fixture home",
                r.source
            );
            assert_eq!(r.size_bytes, None);
        }
    }

    /// `file_row`'s POSITIVE branch: the previous test only ever sees a
    /// fresh, empty fixture home, so `exists`/`size_bytes` are always the
    /// `false`/`None` arm there — this is the only test that puts a REAL
    /// file at a listed path and checks `list_log_sources` reports it, byte
    /// count included, rather than exercising that shape only indirectly
    /// through `read_log_window`.
    #[tokio::test]
    async fn list_log_sources_reports_a_real_files_existence_and_size() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));
        app.manage(Arc::new(Supervisor::new(openvhost_proc::default_driver())));

        let log_dir = home.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        let contents = b"line one\nline two\n";
        std::fs::write(log_dir.join("nginx.error.log"), contents).unwrap();

        let rows = list_log_sources(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            app.state::<Arc<Supervisor>>(),
        )
        .await
        .unwrap();

        let row = rows
            .iter()
            .find(|r| r.source == LogSourceDto::NginxError)
            .expect("an nginx error row");
        assert!(
            row.exists,
            "a real file at the path must report exists: true"
        );
        assert_eq!(row.size_bytes, Some(contents.len() as u64));
    }

    // ---- read_log_window: cursor round-trip ------------------------------

    #[tokio::test]
    async fn read_log_window_round_trips_a_cursor() {
        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_db(&app, Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let log_dir = home.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("nginx.error.log"), b"line one\n").unwrap();

        let first = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(LogSourceDto::NginxError, None),
        )
        .await
        .unwrap();
        assert_eq!(first.rows.len(), 1);
        assert_eq!(first.rows[0].text, "line one");
        let cursor = first.cursor.expect("a cursor after a successful read");

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(log_dir.join("nginx.error.log"))
            .unwrap();
        std::io::Write::write_all(&mut f, b"line two\n").unwrap();
        drop(f);

        let second = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(LogSourceDto::NginxError, Some(cursor)),
        )
        .await
        .unwrap();
        assert_eq!(
            second.rows.len(),
            1,
            "resuming from the round-tripped cursor must return ONLY the new \
             line, not re-scan from the tail"
        );
        assert_eq!(second.rows[0].text, "line two");
        assert!(second.reset.is_none());
    }

    // ---- reveal_log_folder's AppHandle-free half --------------------------
    //
    // `reveal_log_folder` itself cannot be called directly from this harness
    // (see `reveal_log_folder_target`'s own doc comment) — this exercises
    // the shared catalogue check plus the one bit unique to this command,
    // taking the derived path's parent.

    #[tokio::test]
    async fn reveal_log_folder_target_rejects_an_unknown_site_with_a_validation_error() {
        let home = tempfile::tempdir().unwrap();
        let db = Db::open_in_memory().await.unwrap();
        let runtimes = RwLock::new(None::<InstalledRuntimes>);
        let log_paths = openvhost_core::LogPaths::new(home.path());

        let err = reveal_log_folder_target(
            LogSourceDto::SiteError {
                domain: "ghost.localhost".into(),
            },
            &DbHandle::Ready(db),
            &runtimes,
            &log_paths,
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(field, "domain"),
            other => panic!("expected Validation on domain, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reveal_log_folder_target_is_the_sites_log_directory_not_the_file() {
        let home = tempfile::tempdir().unwrap();
        let db = Db::open_in_memory().await.unwrap();
        site_in(&db, "shop.localhost").await;
        let runtimes = RwLock::new(None::<InstalledRuntimes>);
        let log_paths = openvhost_core::LogPaths::new(home.path());

        let folder = reveal_log_folder_target(
            LogSourceDto::SiteAccess {
                domain: "shop.localhost".into(),
            },
            &DbHandle::Ready(db),
            &runtimes,
            &log_paths,
        )
        .await
        .unwrap();

        assert_eq!(folder, home.path().join("logs/sites/shop.localhost"));
    }

    // ---- SPLIT: which arms survive a store that never opened --------------
    //
    // Optional-state.db design D2. `check_catalogue` is NOT uniform, and that
    // is the whole decision: making all of it fail closed looks safer and
    // produces the opposite of the design — the nginx error log would become
    // unreadable exactly when the app is broken, which is the one thing a user
    // needs then. Every test in this group would still pass under that
    // mistake, so the two directions are proven separately and explicitly.
    //
    // Vacuity, measured by mutation on the ONE arm that decides it:
    //
    //  * `check_catalogue`'s site arm returning `Ok(())` on an `Unavailable`
    //    handle instead of refusing reddens `a_site_log_is_refused_…` and
    //    `reveal_log_folder_target_refuses_a_site_…` (the read then SUCCEEDS
    //    and hands back the planted file's contents, which is precisely the
    //    leak the gate exists to stop) and leaves this group's other three
    //    tests green.
    //  * Making the whole function refuse when the store is down — the
    //    plausible over-correction — reddens
    //    `the_nginx_error_log_is_still_readable_…`,
    //    `a_php_pool_log_is_still_readable_…` and
    //    `a_ring_source_is_still_rejected_…` and leaves the two site tests
    //    green. Neither mutation is detectable by the group's other half,
    //    which is why both halves are here.
    //  * Building the refusal from `STORE_UNAVAILABLE` instead of
    //    `unavailable_message(reason)` — i.e. going back to what an
    //    `Option<&Db>` could carry — reddens exactly the two `STORE_DOWN_REASON`
    //    assertions in `a_site_log_is_refused_…` and
    //    `reveal_log_folder_target_refuses_a_site_…`, and nothing else. Measured.

    /// The store's own fixture: down, with the log files a user would want to
    /// read at that exact moment already on disk.
    fn store_down_app(home: &Path) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        manage_store_down(&app);
        app.manage(stack(home));
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: Some(home.join("nginx")),
            php: vec![openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: home.join("php-fpm"),
                source: openvhost_core::PhpRuntimeSource::Homebrew,
            }],
        })));
        app
    }

    #[tokio::test]
    async fn the_nginx_error_log_is_still_readable_with_no_store() {
        let home = tempfile::tempdir().unwrap();
        let app = store_down_app(home.path());

        let log_dir = home.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(
            log_dir.join("nginx.error.log"),
            b"2026/08/09 10:00:00 [emerg] the app is broken\n",
        )
        .unwrap();

        let window = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(LogSourceDto::NginxError, None),
        )
        .await
        .expect("nginx's error log must stay readable when the store is down");

        assert!(window.exists);
        assert_eq!(window.rows.len(), 1, "got {:?}", window.rows);
        assert!(window.rows[0].text.contains("the app is broken"));

        // …and the same source through the OTHER command, which reaches the
        // same gate. `reveal_log_folder` itself takes an `AppHandle<Wry>` and
        // cannot be invoked from this harness at all (see
        // `reveal_log_folder_target`'s doc); this is its whole decision minus
        // the OS call.
        let log_paths = openvhost_core::LogPaths::new(home.path());
        let folder = reveal_log_folder_target(
            LogSourceDto::NginxError,
            app.state::<DbHandle>().inner(),
            app.state::<RwLock<Option<InstalledRuntimes>>>().inner(),
            &log_paths,
        )
        .await
        .expect("revealing nginx's log folder must not need the store either");
        assert_eq!(folder, log_dir);
    }

    /// The second arm that needs no store: an installed php-fpm major is named
    /// by the managed runtime list, not by `state.db`.
    #[tokio::test]
    async fn a_php_pool_log_is_still_readable_with_no_store() {
        let home = tempfile::tempdir().unwrap();
        let app = store_down_app(home.path());

        let pool_dir = home.path().join("logs/services/php-fpm-8.3");
        std::fs::create_dir_all(&pool_dir).unwrap();
        std::fs::write(pool_dir.join("error.log"), b"[09-Aug-2026] NOTICE: ready\n").unwrap();

        let window = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(
                LogSourceDto::PhpFpm {
                    major: "8.3".into(),
                },
                None,
            ),
        )
        .await
        .expect("an installed pool's log must stay readable when the store is down");

        assert!(window.exists);
        assert_eq!(window.rows.len(), 1, "got {:?}", window.rows);
        assert!(window.rows[0].text.contains("NOTICE: ready"));
    }

    /// The third arm: a ring source passes the catalogue check with or without
    /// a store and is rejected one step later by `derive_path`. The assertion
    /// is that the store being down changes NOTHING here — same refusal, same
    /// field — rather than that the source is somehow readable.
    #[tokio::test]
    async fn a_ring_source_is_still_rejected_by_derive_path_with_no_store() {
        let home = tempfile::tempdir().unwrap();
        let app = store_down_app(home.path());

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(LogSourceDto::ServiceRing { id: "nginx".into() }, None),
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Validation { field, .. } => assert_eq!(
                field, "source",
                "a ring source must still be refused by derive_path, not by the store gate"
            ),
            other => panic!("expected Validation on source, got {other:?}"),
        }
    }

    /// The direction that must NOT degrade. `state.db` is the only thing that
    /// says which domains are the user's, so with no store there is nothing to
    /// check an IPC-supplied domain against — and this check is the
    /// path-confinement gate that stands between that domain and
    /// `<home>/logs/sites/<domain>/…`.
    ///
    /// The planted file is what makes the assertion discriminating rather than
    /// decorative: if the gate degraded open, the derived path would exist and
    /// this call would return its contents instead of an error.
    #[tokio::test]
    async fn a_site_log_is_refused_with_no_store_by_the_confinement_gate() {
        let home = tempfile::tempdir().unwrap();
        let app = store_down_app(home.path());

        let site_dir = home.path().join("logs/sites/ghost.localhost");
        std::fs::create_dir_all(&site_dir).unwrap();
        std::fs::write(site_dir.join("access.log"), b"a line nothing vetted\n").unwrap();

        let err = read_log_window(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            query(
                LogSourceDto::SiteAccess {
                    domain: "ghost.localhost".into(),
                },
                None,
            ),
        )
        .await
        .unwrap_err();

        // `Core`, and specifically NOT `Validation { field: "domain" }`: the
        // domain is not what is wrong, and a page that read this as "no such
        // site" would be told a second untruth on top of the first.
        match err {
            IpcError::Core { message } => {
                assert!(
                    message.contains(crate::db_state::STORE_UNAVAILABLE),
                    "the refusal must name the store, got {message:?}"
                );
                // The reason, in the message the user is shown at the moment
                // they are blocked — not left to be inferred from the banner.
                // Reddens the instant `check_catalogue` goes back to an
                // `Option<&Db>`, which cannot carry it.
                assert!(
                    message.contains(STORE_DOWN_REASON),
                    "the refusal must carry the reason the store is missing: {message:?}"
                );
                assert!(
                    !message.contains(".manage()"),
                    "the user must never be told to call a Rust API: {message:?}"
                );
            }
            other => panic!("expected the store refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reveal_log_folder_target_refuses_a_site_with_no_store() {
        let home = tempfile::tempdir().unwrap();
        let runtimes = RwLock::new(None::<InstalledRuntimes>);
        let log_paths = openvhost_core::LogPaths::new(home.path());

        let err = reveal_log_folder_target(
            LogSourceDto::SiteError {
                domain: "ghost.localhost".into(),
            },
            &store_down(),
            &runtimes,
            &log_paths,
        )
        .await
        .unwrap_err();

        match err {
            IpcError::Core { message } => {
                assert!(
                    message.contains(crate::db_state::STORE_UNAVAILABLE),
                    "the refusal must name the store, got {message:?}"
                );
                assert!(
                    message.contains(STORE_DOWN_REASON),
                    "and it must carry the reason, not only the shared sentence: {message:?}"
                );
            }
            other => panic!("expected the store refusal, got {other:?}"),
        }
    }

    // ---- DEGRADE: list_log_sources keeps every non-site row ---------------
    //
    // Vacuity: swapping `db.optional()` for `db.require()?` makes this test's
    // call return `Err` and reddens it, while
    // `list_log_sources_enumerates_every_kind_for_a_fixture_home` — which
    // manages a healthy store — stays green. That pair is what separates
    // "degrades" from "refuses", and the positive assertions below are what
    // separate "degrades" from "returns an empty list".

    #[tokio::test]
    async fn list_log_sources_degrades_to_the_rows_the_store_was_not_the_source_of() {
        let home = tempfile::tempdir().unwrap();
        let app = store_down_app(home.path());
        let sup = Arc::new(Supervisor::new(openvhost_proc::default_driver()));
        sup.register(openvhost_proc::ServiceSpec {
            id: "nginx".into(),
            display_name: "nginx".into(),
            endpoint: None,
            spawn: openvhost_proc::SpawnSpec {
                program: PathBuf::from("/usr/bin/true"),
                args: vec![],
                cwd: None,
                env: vec![],
            },
            readiness: openvhost_proc::ReadinessProbe::default(),
            grace: openvhost_proc::DEFAULT_GRACE,
        });
        app.manage(sup);

        let rows = list_log_sources(
            app.state::<DbHandle>(),
            app.state::<RwLock<Option<InstalledRuntimes>>>(),
            app.state::<Option<StackPaths>>(),
            app.state::<Arc<Supervisor>>(),
        )
        .await
        .expect("a degraded store must shorten the catalogue, not error it");

        assert!(
            rows.iter().any(|r| r.source == LogSourceDto::NginxError),
            "got {:?}",
            rows.iter().map(|r| &r.source).collect::<Vec<_>>()
        );
        assert!(rows.iter().any(|r| r.source == LogSourceDto::NginxAccess));
        assert!(
            rows.iter().any(
                |r| matches!(&r.source, LogSourceDto::PhpFpm { major } if major.as_str() == "8.3")
            ),
            "an installed pool row comes from the runtime list, not the store"
        );
        assert!(
            rows.iter().any(|r| matches!(
                &r.source,
                LogSourceDto::ServiceRing { id } if id.as_str() == "nginx"
            )),
            "a ring row comes from the supervisor, not the store"
        );
        assert!(
            !rows.iter().any(|r| matches!(
                r.source,
                LogSourceDto::SiteAccess { .. } | LogSourceDto::SiteError { .. }
            )),
            "a site row can only come from state.db, so none can be listed"
        );
    }
}
