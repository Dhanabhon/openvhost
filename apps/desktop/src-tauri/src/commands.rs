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
    PhpVersion, Site, SiteId, SiteName, SiteRepository, SqliteSiteRepository,
    SqliteWebServerSettings, WebServer, WebServerSettingsRepository,
};
// Not re-exported at the crate root like the flat types above: `scaffold`'s
// home is the `site` submodule (Tasks 2-3), and it stays that way rather than
// growing `lib.rs`'s re-export list for a type only this one command needs.
use openvhost_core::site::scaffold::{ScaffoldOutcome, ScaffoldStep, scaffold, scaffold_path};

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

// These commands build a repository per call from the managed `Db` (cheap —
// cloning a pool handle) rather than managing a second type. If `state.db`
// failed to open at startup, `Db` is not managed and Tauri's State extraction
// fails; the frontend's normalizeError surfaces that in the error banner.
#[tauri::command]
#[specta::specta]
pub async fn list_sites(db: tauri::State<'_, Db>) -> Result<Vec<SiteDto>, IpcError> {
    let repo = SqliteSiteRepository::new(db.inner());
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
    db: tauri::State<'_, Db>,
    input: SiteInput,
    create_folder: bool,
) -> Result<CreateSiteResult, IpcError> {
    let mut new: NewSite = input.try_into()?;
    if create_folder {
        // Re-parse of the JOINED path: over-length or bad-charset joins fail
        // here as a docroot field error, before any row or folder exists.
        new.docroot = scaffold_path(&new.docroot, &new.name)?;
    }
    let repo = SqliteSiteRepository::new(db.inner());
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
    db: tauri::State<'_, Db>,
    id: String,
    input: SiteInput,
) -> Result<SiteDto, IpcError> {
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db.inner());
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
    db: tauri::State<'_, Db>,
    app: tauri::AppHandle,
    id: String,
) -> Result<(), IpcError> {
    use tauri_plugin_opener::OpenerExt;
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db.inner());
    let site = repo.get(&site_id).await?.ok_or_else(|| IpcError::Core {
        message: format!("site {id} not found"),
    })?;
    let url = site_url(site.domain.as_str());
    // `None` for `with`: let the OS pick the default handler rather than naming a
    // browser we would then have to keep a list of.
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| IpcError::Core {
            message: e.to_string(),
        })
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
pub async fn delete_site(db: tauri::State<'_, Db>, id: String) -> Result<bool, IpcError> {
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db.inner());
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
/// The nginx settings are read here, alongside the sites, so BOTH entry points
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
    Ok(ApplyInput {
        home: paths.home.clone(),
        sites: repo.list().await?,
        runtimes: runtimes.clone(),
        // Absent row => documented defaults, and nothing is written. See
        // `WebServerSettingsRepository::get`.
        settings: settings.get().await?,
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
    db: tauri::State<'_, Db>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<ApplyPlanDto, IpcError> {
    // Clone out of the guard and drop it before the `.await` below: holding a
    // `std::sync::RwLockReadGuard` across an await point makes this command's
    // future non-`Send`, which fails to compile.
    let runtimes = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .clone();
    let input = apply_input(db.inner(), &runtimes, paths.inner()).await?;
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
    db: tauri::State<'_, Db>,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, ApplyLock>,
) -> Result<ApplyOutcomeDto, IpcError> {
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

    let input = apply_input(db.inner(), &runtimes, paths.inner()).await?;
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

    let validator = openvhost_core::NginxValidator {
        bin: stack.nginx_bin.clone(),
        err_log: openvhost_core::LogPaths::new(&stack.home).nginx_error(),
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
    /// `<bin> -e <err_log> -t -c <conf>`. Only ever constructed for `Nginx`.
    NginxT {
        bin: &'a Path,
        conf: &'a Path,
        err_log: PathBuf,
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
            Self::Nginx => Ok(ValidationTarget::NginxT {
                bin: &paths.nginx_bin,
                conf: &paths.nginx_conf,
                err_log: openvhost_core::LogPaths::new(&paths.home).nginx_error(),
            }),
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
fn stack_paths<'a>(
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
) -> Vec<WebServerDto> {
    vec![
        WebServerDto {
            id: "nginx".into(),
            display_name: "nginx".into(),
            supported: true,
            service_id: Some("nginx".into()),
            binary_path: Some(p.nginx_bin.display().to_string()),
            version,
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
) -> Result<Vec<WebServerDto>, IpcError> {
    let p = stack_paths(&paths)?;
    // Probing the version SPAWNS `nginx -v`, so merely opening this page starts
    // a process. Bounded: one short-lived probe, fixed argv, PROBE_TIMEOUT.
    // `-e` is mandatory on EVERY nginx invocation, `-v` included, so that nothing
    // this app runs can write into nginx's compiled-in prefix instead of our home
    // — see `openvhost_conf::probe_nginx_version`'s own doc comment.
    let err_log = openvhost_core::LogPaths::new(&p.home).nginx_error();
    let version = openvhost_conf::probe_nginx_version(&p.nginx_bin, &err_log).await;
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
    Ok(web_server_rows(p, version, config_exists))
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
        ValidationTarget::NginxT { bin, conf, err_log } => {
            openvhost_conf::validate_live(bin, conf, &err_log).await
        }
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
#[tauri::command]
#[specta::specta]
pub async fn web_server_settings(
    db: tauri::State<'_, Db>,
) -> Result<WebServerSettingsDto, IpcError> {
    read_settings(db.inner()).await
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
    db: tauri::State<'_, Db>,
    paths: tauri::State<'_, Option<StackPaths>>,
    input: WebServerSettingsDto,
) -> Result<(), IpcError> {
    // No stack (nginx not installed) => no checker; see `write_settings`.
    let checker = paths.inner().as_ref().map(|p| NginxSettingsChecker {
        bin: p.nginx_bin.clone(),
        scratch_root: p.home.join("run"),
    });
    write_settings(
        db.inner(),
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
    pub recommended: bool,
    /// A more precise version string than `major` (e.g. a patch level), when
    /// one is known. `None` does NOT mean anything is wrong with this row —
    /// it means we do not know the patch level. The only prober we have,
    /// `openvhost_conf::probe_php_fpm_version`, returns `major.minor` and
    /// never a patch level, so today this is `None` for every row. Echoing
    /// `major` back into this field instead would render "8.3" twice next to
    /// each other and imply a patch level was fetched when it was not.
    pub full_version: Option<String>,
    pub path: Option<String>,
    /// Where this version's pool listens. `None` until installed.
    pub socket_path: Option<String>,
    /// The supervisor id for this version's pool, so the UI can drive
    /// start/stop from the row without inventing the id itself.
    pub service_id: Option<String>,
}

/// What the Languages page needs to decide which of the three states to show
/// (spec §6.1). `brew_found` false means the page must guide, not list.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PhpEnvironmentDto {
    pub brew_found: bool,
    pub brew_searched: Vec<String>,
    pub runtimes: Vec<PhpRuntimeDto>,
}

/// The outcome of an `install_php` call. `detected: false` alongside
/// `exit_code: Some(0)` is the case that matters most: brew reporting success
/// while no `php-fpm` appears afterwards is the silent-failure class this
/// project keeps catching, so the DTO carries it explicitly rather than
/// leaving the UI to infer it from an empty rescan.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InstallOutcomeDto {
    pub major: String,
    pub exit_code: Option<i32>,
    pub detected: bool,
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
fn now_ms() -> u64 {
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
/// `full_versions` maps a major to a more precise string (e.g. a patch
/// level) the page can show next to it, when one is actually known. In
/// production this is empty: the only prober we have,
/// `openvhost_conf::probe_php_fpm_version`, returns `major.minor` and never
/// a patch level, so there is nothing more precise to hand in today — and
/// echoing `major` back in as if it were that string would render "8.3"
/// twice and imply a patch level had been fetched when it had not. Wiring a
/// true patch-level prober is future work; keeping this a separate parameter
/// means that upgrade will not have to touch this function's callers beyond
/// what they pass in.
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
            recommended: Some(major) == newest,
            full_version: found.and_then(|_| {
                full_versions
                    .iter()
                    .find(|(m, _)| *m == major)
                    .map(|(_, v)| (*v).to_string())
            }),
            path: found.map(|rt| rt.fpm_bin.display().to_string()),
            socket_path: spec.as_ref().and_then(|s| s.endpoint.clone()),
            service_id: spec.map(|s| s.id),
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

/// Probe every known Homebrew prefix for installed PHP runtimes.
///
/// `openvhost_core::discover_php_in` takes a SYNCHRONOUS probe closure, but
/// `openvhost_conf::probe_php_fpm_version` is async. Resolved by running the
/// whole directory walk on `spawn_blocking` and calling the async prober via
/// `Handle::block_on` from INSIDE that blocking closure: `spawn_blocking`
/// hands the closure its own blocking-pool thread, not one of the async
/// worker threads, so blocking there to wait on a future cannot deadlock the
/// runtime the way calling `block_on` directly inside an async command would.
///
/// The other option the task allowed — pre-building a `path -> version` map
/// by probing candidates asynchronously first, then handing `discover_php_in`
/// a closure that only reads that map — was passed over because the set of
/// candidate paths is exactly what `discover_php_in`'s own (private)
/// directory walk already computes. Re-deriving that candidate list here
/// first would duplicate discovery logic that already exists and is already
/// tested, which is the kind of copy-paste drift the project's own
/// coding-style rules warn against; this approach reuses `discover_php_in`
/// untouched instead.
async fn discover_all_php() -> Result<Vec<openvhost_core::PhpRuntime>, IpcError> {
    tauri::async_runtime::spawn_blocking(|| {
        let handle = tokio::runtime::Handle::current();
        let prefixes: Vec<&Path> = openvhost_core::BREW_PREFIXES
            .iter()
            .map(Path::new)
            .collect();
        openvhost_core::discover_php_in(&prefixes, &|bin| {
            handle.block_on(openvhost_conf::probe_php_fpm_version(bin))
        })
    })
    .await
    .map_err(|e| IpcError::Core {
        message: format!("the PHP discovery task failed to run: {e}"),
    })
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
/// the app) is likewise not returned here — there is no `Supervisor`
/// unregister, and a row pointing at a now-missing binary simply fails
/// honestly the next time it is started.
fn newly_installed_majors(before: &[String], found: &[String]) -> Vec<String> {
    found
        .iter()
        .filter(|major| !before.contains(major))
        .cloned()
        .collect()
}

/// Probe for installed PHP runtimes, write the result into the managed
/// `RwLock`, and register a supervisor row for every NEWLY discovered major.
///
/// Only new majors are registered — see `newly_installed_majors` for why
/// re-registering an already-known major (even an unchanged one) is not
/// idempotent in the way it looks: it can silently erase a `Failed` row's
/// stderr and exit code.
///
/// Shared by `rescan_php_runtimes` and `install_php` so the two commands
/// cannot register two different service shapes for the same version.
async fn rescan_into_state(
    runtimes: &RwLock<Option<InstalledRuntimes>>,
    sup: &Supervisor,
    paths: &StackPaths,
) -> Result<Vec<openvhost_core::PhpRuntime>, IpcError> {
    let before: Vec<String> = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "runtime list is poisoned".into(),
        })?
        .as_ref()
        .map(|r| r.php.iter().map(|rt| rt.major.clone()).collect())
        .unwrap_or_default();

    let php = discover_all_php().await?;
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

    Ok(php)
}

/// `openvhost_core::BREW_PREFIXES` joined with `bin/brew`, so the UI can say
/// exactly where Homebrew was looked for.
fn brew_searched_paths() -> Vec<String> {
    openvhost_core::BREW_PREFIXES
        .iter()
        .map(|prefix| Path::new(prefix).join("bin/brew").display().to_string())
        .collect()
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
pub async fn php_environment(
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
    // See `php_rows`'s doc comment: there is no patch-level prober yet, so
    // there is nothing more precise than `major` to hand in here. An empty
    // map, not a `(major, major)` echo — `full_version` must read as
    // "unknown", not as a copy of `major`.
    Ok(PhpEnvironmentDto {
        brew_found: openvhost_core::find_brew().is_some(),
        brew_searched: brew_searched_paths(),
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
pub async fn rescan_php_runtimes(
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
    let installed = rescan_into_state(runtimes.inner(), sup.inner(), p).await?;
    // See `php_rows`'s doc comment: there is no patch-level prober yet, so
    // there is nothing more precise than `major` to hand in here. An empty
    // map, not a `(major, major)` echo — `full_version` must read as
    // "unknown", not as a copy of `major`.
    Ok(PhpEnvironmentDto {
        brew_found: openvhost_core::find_brew().is_some(),
        brew_searched: brew_searched_paths(),
        runtimes: php_rows(&p.home, &installed, &[]),
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
}

/// Wire-safe copy of [`InstallKind`] for [`PendingInstallDto`] — `InstallKind`
/// itself carries no `specta::Type`/`Serialize`; it is purely an internal
/// discriminator for `InstallLock`'s slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum InstallKindDto {
    Php,
    Mysql,
}

impl From<InstallKind> for InstallKindDto {
    fn from(kind: InstallKind) -> Self {
        match kind {
            InstallKind::Php => Self::Php,
            InstallKind::Mysql => Self::Mysql,
        }
    }
}

/// What [`pending_install`] reports: which kind of install/init occupies
/// `InstallLock`'s shared slot, and its label — e.g. `"8.4"` for a PHP
/// install, `"MySQL 8.4"` for a MySQL install, `"MySQL 8.4 initialization"`
/// for an init run (see `install_php`/`install_mysql`/`initialize_mysql`'s
/// own `set_running` calls for the exact shapes).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PendingInstallDto {
    pub kind: InstallKindDto,
    pub label: String,
}

/// The kind, label, and abort handle of the one install/init run
/// `InstallLock` may have in flight, so `perform_quit` can abort it and
/// `pending_install` can tell the user what they are about to lose,
/// regardless of kind.
struct RunningInstall {
    kind: InstallKind,
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
        label: String,
        abort: tokio::task::AbortHandle,
    ) {
        let mut slot = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(RunningInstall { kind, label, abort });
    }

    fn clear_running(&self) {
        let mut slot = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = None;
    }

    /// The kind and label of whatever install/init run currently occupies
    /// the slot, if any — `None` only when nothing is running. The
    /// generalization (review fix wave Important 1) of the old PHP-only
    /// `running_php_major`, which used to `.filter(|r| r.kind == InstallKind::Php)`
    /// here and silently returned `None` for a MySQL occupant. The quit
    /// dialog's copy reads this through the [`pending_install`] command.
    pub(crate) fn running_install(&self) -> Option<(InstallKind, String)> {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|r| (r.kind, r.label.clone()))
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
struct RunningInstallGuard<'a> {
    lock: &'a InstallLock,
    abort: tokio::task::AbortHandle,
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
        .map(|(kind, label)| PendingInstallDto {
            kind: kind.into(),
            label,
        }))
}

/// Install a PHP major via Homebrew, streaming its output live, then rescan
/// so the freshly installed version (if it appears) gets a supervisor row.
///
/// Every argument that reaches `brew`'s argv is validated or derived from
/// managed state before this function does anything observable: `major` is
/// parsed and checked against the catalogue allowlist, `brew` is located by
/// absolute path (never `PATH`), and `brew_install_spec` itself refuses a
/// non-absolute `brew` path.
#[tauri::command]
#[specta::specta]
pub async fn install_php(
    app: tauri::AppHandle,
    major: String,
    runtimes: tauri::State<'_, RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<InstallOutcomeDto, IpcError> {
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
    let spec = openvhost_core::brew_install_spec(&brew, &major)?;
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
    lock.inner().set_running(
        InstallKind::Php,
        major.as_str().to_string(),
        abort_handle.clone(),
    );
    // Cleared AND aborted on every return path below via `Drop`, including
    // the two `?`s still to come — see `RunningInstallGuard`'s doc comment
    // for why that is a `Drop` impl and not a matching call at each return
    // point, and why it aborts rather than merely clearing the slot.
    let _running_guard = RunningInstallGuard {
        lock: lock.inner(),
        abort: abort_handle,
    };

    let exit_code = match install_task.await {
        Ok(result) => result?,
        // Aborted by `perform_quit`: the task's future was genuinely dropped
        // (so `KillOnDrop` ran and brew's process group is gone), and this
        // command has nothing left to report but that it did not finish.
        Err(join_err) if join_err.is_cancelled() => {
            return Err(IpcError::Proc {
                message: "the install was aborted because the app is quitting".into(),
            });
        }
        // Any other join failure (a panic inside `run_task`) is not this
        // command's fault to hide.
        Err(join_err) => {
            return Err(IpcError::Proc {
                message: format!("the install task ended unexpectedly: {join_err}"),
            });
        }
    };
    let _ = pump.await;

    // Rescan even on a non-zero exit: brew can fail late having already
    // linked the formula, and the truth is on disk either way.
    let found = rescan_into_state(runtimes.inner(), sup.inner(), p).await?;
    let detected = found.iter().any(|r| r.major == major.as_str());

    Ok(InstallOutcomeDto {
        major: major.as_str().to_string(),
        exit_code,
        detected,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod php_ipc_tests {
    use tauri::Manager;

    use super::*;

    #[test]
    fn every_catalogue_entry_is_listed_with_its_installed_state() {
        let installed = vec![openvhost_core::PhpRuntime {
            major: "8.3".into(),
            fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
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
        // Our only prober returns major.minor. Echoing it into `full_version`
        // would render "8.3" twice and imply a patch level we never fetched.
        let installed = vec![openvhost_core::PhpRuntime {
            major: "8.3".into(),
            fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
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
        // register, and nothing to unregister either — the supervisor has no
        // unregister, and a row pointing at a missing binary fails honestly.
        let before = ["8.3".to_string(), "8.5".to_string()];
        let found = ["8.5".to_string()];
        assert!(newly_installed_majors(&before, &found).is_empty());
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
        }];
        let rows = php_rows(Path::new("/tmp/ovh"), &installed, &[("7.4", "7.4.33")]);
        assert!(rows.iter().any(|r| r.major == "7.4" && r.installed));
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
            nginx_bin: home.path().join("nginx"),
            nginx_conf: home.path().join("nginx.conf"),
        }));
        app.manage(Arc::new(Supervisor::new(openvhost_proc::default_driver())));
        app.manage(InstallLock::default());

        // Hold the guard the way `install_php`'s `try_lock` would while a
        // build is running.
        let lock = app.state::<InstallLock>();
        let held = lock.inner().guard.lock().await;

        let handle = app.handle().clone();
        let task = tokio::spawn(async move {
            rescan_php_runtimes(
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
        lock.set_running(InstallKind::Php, "8.4".to_string(), abort.clone());

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
            Some((InstallKind::Mysql, "MySQL 8.4".to_string()))
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
            "8.4".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );

        assert!(
            lock.guard.try_lock().is_err(),
            "install_mysql's try_lock must fail while a PHP install holds the guard"
        );
        assert_eq!(
            lock.running_install(),
            Some((InstallKind::Php, "8.4".to_string()))
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
            "MySQL 8.4 initialization".to_string(),
            tokio::spawn(std::future::pending::<()>()).abort_handle(),
        );
        app.manage(lock);

        let pending = pending_install(app.state::<InstallLock>()).await.unwrap();

        assert_eq!(
            pending,
            Some(PendingInstallDto {
                kind: InstallKindDto::Mysql,
                label: "MySQL 8.4 initialization".to_string(),
            }),
            "a MySQL occupant must be visible, correctly tagged — the whole \
             point of the generalization"
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

/// Mirrors `InstallOutcomeDto` for MySQL (spec D7's `install_mysql`).
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MysqlInstallOutcomeDto {
    pub major: String,
    pub exit_code: Option<i32>,
    pub detected: bool,
}

/// One line of `brew install mysql@<major>`'s output, forwarded live while an
/// install runs. Same shape and reasoning as [`PhpInstallLogEvent`].
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

/// Probe every known Homebrew prefix for installed MySQL runtimes. Mirrors
/// `discover_all_php`'s `spawn_blocking` + `Handle::block_on` bridge exactly
/// — see its doc comment for why.
async fn discover_all_mysql() -> Result<Vec<openvhost_core::mysql::MysqlRuntime>, IpcError> {
    tauri::async_runtime::spawn_blocking(|| {
        let handle = tokio::runtime::Handle::current();
        let prefixes: Vec<&Path> = openvhost_core::BREW_PREFIXES
            .iter()
            .map(Path::new)
            .collect();
        openvhost_core::mysql::discover_mysql(&prefixes, &|bin| {
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
async fn rescan_mysql_into_state(
    runtimes: &RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>,
    sup: &Supervisor,
    home: &Path,
) -> Result<Vec<openvhost_core::mysql::MysqlRuntime>, IpcError> {
    if let Err(e) =
        openvhost_core::mysql::sweep_stale_staging(&openvhost_core::mysql::mysql_data_root(home))
    {
        eprintln!("mysql: failed to sweep abandoned staging directories: {e}");
    }

    let found = discover_all_mysql().await?;

    *runtimes.write().map_err(|_| IpcError::Core {
        message: "mysql runtime list is poisoned".into(),
    })? = Some(found.clone());

    let already_registered: std::collections::HashSet<String> =
        sup.snapshot().into_iter().map(|s| s.id).collect();
    for rt in &found {
        let id = format!("mysql-{}", rt.major.as_str());
        if already_registered.contains(&id) {
            continue;
        }
        if crate::stack::mysql_datadir_is_initialized(home, rt) {
            sup.register(crate::stack::mysql_spec(home, rt));
        }
    }

    Ok(found)
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
    let mysql_ctx = openvhost_conf::MysqlCtx {
        my_cnf: ctx.paths.my_cnf.clone(),
        datadir: ctx.paths.datadir.clone(),
        socket: ctx.paths.socket.clone(),
        pid_file: ctx.paths.pid_file.clone(),
        custom_confd: ctx.paths.custom_confd.clone(),
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
    let installed = rescan_mysql_into_state(runtimes.inner(), sup.inner(), &p.home).await?;
    Ok(MysqlEnvironmentDto {
        brew_found: openvhost_core::find_brew().is_some(),
        brew_searched: brew_searched_paths(),
        instances: mysql_rows(&p.home, &installed),
    })
}

/// Install a MySQL major via Homebrew, streaming its output live, then
/// rescan so a freshly installed version (if it appears) is picked up —
/// mirrors `install_php` exactly, sharing `InstallLock` (decision 5) — see
/// `InstallKind`'s doc comment for why the quit dialog's PHP-specific copy
/// is unaffected by this.
#[tauri::command]
#[specta::specta]
pub async fn install_mysql(
    app: tauri::AppHandle,
    major: String,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<MysqlInstallOutcomeDto, IpcError> {
    // The catalogue gate, before anything else happens (decision 2).
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;

    let Ok(_guard) = lock.inner().guard.try_lock() else {
        return Err(IpcError::Core {
            message: "an install is already running".into(),
        });
    };

    let p = stack_paths(&paths)?;

    let before: Vec<String> = runtimes
        .read()
        .map_err(|_| IpcError::Core {
            message: "mysql runtime list is poisoned".into(),
        })?
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|rt| rt.major.as_str().to_string())
        .collect();

    if before.iter().any(|m| m == major.as_str()) {
        return Err(IpcError::Core {
            message: format!("MySQL {} is already installed", major.as_str()),
        });
    }

    let brew = openvhost_core::find_brew().ok_or_else(|| IpcError::Core {
        message: format!(
            "Homebrew was not found. Looked for bin/brew under: {}",
            openvhost_core::BREW_PREFIXES.join(", ")
        ),
    })?;

    let spec = openvhost_core::mysql::mysql_brew_install_spec(&brew, &major)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);

    let emitter = app.clone();
    let for_event = major.as_str().to_string();
    let pump = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let openvhost_proc::TaskEvent::Line { stream, text } = ev {
                let _ = MysqlInstallLogEvent {
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

    let install_task = tokio::spawn(openvhost_proc::run_task(
        openvhost_proc::default_driver(),
        spec,
        tx,
    ));
    let abort_handle = install_task.abort_handle();
    lock.inner().set_running(
        InstallKind::Mysql,
        format!("MySQL {}", major.as_str()),
        abort_handle.clone(),
    );
    let _running_guard = RunningInstallGuard {
        lock: lock.inner(),
        abort: abort_handle,
    };

    let exit_code = match install_task.await {
        Ok(result) => result?,
        Err(join_err) if join_err.is_cancelled() => {
            return Err(IpcError::Proc {
                message: "the install was aborted because the app is quitting".into(),
            });
        }
        Err(join_err) => {
            return Err(IpcError::Proc {
                message: format!("the install task ended unexpectedly: {join_err}"),
            });
        }
    };
    let _ = pump.await;

    let found = rescan_mysql_into_state(runtimes.inner(), sup.inner(), &p.home).await?;
    let detected = found.iter().any(|r| r.major.as_str() == major.as_str());

    Ok(MysqlInstallOutcomeDto {
        major: major.as_str().to_string(),
        exit_code,
        detected,
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

/// Server-side catalogue gate (decision 2): `MysqlMajor::parse` — the
/// catalogue-gated constructor — is the ONLY way this reaches `major`, so an
/// out-of-catalogue discovered major (rendered display-only on the
/// Databases page) is rejected here before anything else runs, even if a
/// client somehow sends one.
///
/// Datadir classification runs BEFORE any spawn, so `AlreadyInitialized`/
/// `Foreign` are reported with zero side effects — no staging directory, no
/// spawn, no log line (mandatory test).
async fn initialize_mysql_gate(major: String, home: &Path) -> InitializeMysqlGate {
    use InitializeMysqlGate::{Early, Proceed};

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

    let discovered = match discover_all_mysql().await {
        Ok(d) => d,
        Err(e) => return Early(Err(e)),
    };
    let Some(runtime) = discovered.into_iter().find(|rt| rt.major == major) else {
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
    db: tauri::State<'_, Db>,
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

    let ctx = match initialize_mysql_gate(major, &p.home).await {
        InitializeMysqlGate::Early(result) => return result,
        InitializeMysqlGate::Proceed(ctx) => ctx,
    };
    let runtime_for_registration = ctx.runtime.clone();
    let major_for_upsert = ctx.major.clone();
    let major_for_log = ctx.major.as_str().to_string();

    let emitter = app.clone();
    let log: InitLogSink = Arc::new(move |stream: &str, line: String| {
        emit_init_log(&emitter, &major_for_log, stream, line)
    });

    let init_task = tokio::spawn(run_mysql_init(*ctx, log));
    let abort_handle = init_task.abort_handle();
    lock.inner().set_running(
        InstallKind::Mysql,
        format!("MySQL {} initialization", major_for_upsert.as_str()),
        abort_handle.clone(),
    );
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
        openvhost_core::mysql::MysqlInstanceRepo::new(db.inner())
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
    db: tauri::State<'_, Db>,
) -> Result<String, IpcError> {
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;
    let repo = openvhost_core::mysql::MysqlInstanceRepo::new(db.inner());
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
    db: tauri::State<'_, Db>,
    paths: tauri::State<'_, Option<StackPaths>>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
) -> Result<MysqlResetOutcomeDto, IpcError> {
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;
    let p = stack_paths(&paths)?;
    let runtime = find_mysql_runtime(runtimes.inner(), &major)?;
    let mp = openvhost_core::mysql::mysql_paths(&p.home, &major);

    let repo = openvhost_core::mysql::MysqlInstanceRepo::new(db.inner());
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
    db: tauri::State<'_, Db>,
    paths: tauri::State<'_, Option<StackPaths>>,
    runtimes: tauri::State<'_, RwLock<Option<Vec<openvhost_core::mysql::MysqlRuntime>>>>,
) -> Result<MysqlConnectionProofDto, IpcError> {
    let major = openvhost_core::mysql::MysqlMajor::parse(&major)?;
    let p = stack_paths(&paths)?;
    let runtime = find_mysql_runtime(runtimes.inner(), &major)?;
    let mp = openvhost_core::mysql::mysql_paths(&p.home, &major);

    let repo = openvhost_core::mysql::MysqlInstanceRepo::new(db.inner());
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod mysql_ipc_tests {
    use tauri::Manager;

    use super::*;

    fn fake_cli(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        let installed =
            openvhost_core::mysql::discover_mysql(&[prefix.path()], &|_| Some("9.7".to_string()));
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
        let gate = initialize_mysql_gate("9.7".to_string(), &unreached).await;
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

        let gate = initialize_mysql_gate("8.4".to_string(), home.path()).await;

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
        app.manage(db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: home.path().join("nginx"),
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
            },
        ])));

        let outcome = reset_mysql_root_password(
            "8.4".to_string(),
            app.state::<Db>(),
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
        app.manage(db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: home.path().join("nginx"),
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
            },
        ])));

        let outcome = reset_mysql_root_password(
            "8.4".to_string(),
            app.state::<Db>(),
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
        app.manage(db);
        app.manage(Some(StackPaths {
            home: home.path().to_path_buf(),
            nginx_bin: home.path().join("nginx"),
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
            },
        ])));

        let outcome = verify_mysql_connection(
            "8.4".to_string(),
            app.state::<Db>(),
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
        app.manage(Db::open_in_memory().await.unwrap());

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
        app.manage(Db::open_in_memory().await.unwrap());

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
        app.manage(Db::open_in_memory().await.unwrap());

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
            nginx_bin: PathBuf::from("/nonexistent/openvhost-test-home/bin/nginx"),
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
        let listed = web_server_rows(&p, Some("1.27.3".into()), Some(true));

        let nginx = listed
            .iter()
            .find(|r| r.id == "nginx")
            .unwrap_or_else(|| panic!("nginx must be listed, got {listed:?}"));
        assert!(nginx.supported);
        assert_eq!(nginx.service_id.as_deref(), Some("nginx"));
        assert_eq!(nginx.version.as_deref(), Some("1.27.3"));
        let bin = p.nginx_bin.display().to_string();
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
    /// the right three values out of managed state — including `err_log`, which is
    /// derived rather than borrowed and so is the piece a refactor can silently
    /// change.
    #[test]
    fn the_validator_invocation_is_resolved_as_one_unit_from_the_brand() {
        let p = sample_paths();
        match WebServerBrand::Nginx.validation_target(&p) {
            Ok(ValidationTarget::NginxT { bin, conf, err_log }) => {
                assert_eq!(bin, p.nginx_bin.as_path());
                assert_eq!(conf, p.nginx_conf.as_path());
                assert_eq!(err_log, p.home.join("logs/nginx.error.log"));
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod apply_ipc_tests {
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
            nginx_bin: PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx"),
            php: vec![openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
            }],
        });
        let seen = state.read().unwrap().clone().unwrap();
        assert_eq!(seen.php.len(), 1);
        assert_eq!(seen.php[0].major, "8.3");
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
    fn fake_nginx(dir: &Path, argv_out: &Path, version: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("nginx");
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\necho \"$@\" > \"{}\"\necho 'nginx version: nginx/{version}' 1>&2\n",
                argv_out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
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
            nginx_bin: bin.clone(),
            nginx_conf: home.path().join("conf/nginx.conf"),
        }));

        let rows = list_web_servers(app.state()).await.unwrap();
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

    #[test]
    fn the_nginx_row_reports_whether_its_config_is_actually_there() {
        // The page disables Start on `Some(false)`, so a row that claims
        // `Some(true)` when the file is absent sends the user at a service that
        // will exit immediately — and one that claims `Some(false)` when the
        // file IS there hides a working button. `web_server_rows` only ever
        // moves the tri-state through; it never invents a third value.
        let p = StackPaths {
            home: PathBuf::from("/x/.openvhost"),
            nginx_bin: PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx"),
            nginx_conf: PathBuf::from("/x/.openvhost/config/generated/nginx/nginx.conf"),
        };

        let present = web_server_rows(&p, None, Some(true));
        let nginx = present
            .iter()
            .find(|r| r.id == "nginx")
            .expect("an nginx row");
        assert_eq!(
            nginx.config_exists,
            Some(true),
            "Some(true) must reach the nginx row"
        );

        let absent = web_server_rows(&p, None, Some(false));
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
        let unknown = web_server_rows(&p, None, None);
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
            nginx_bin: bin,
            nginx_conf: conf.clone(),
        }));

        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "# placeholder\n").unwrap();
        let rows = list_web_servers(app.state()).await.unwrap();
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
        let rows = list_web_servers(app.state()).await.unwrap();
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
async fn check_catalogue(
    source: &LogSource,
    db: &Db,
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
    db: &Db,
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
pub async fn list_log_sources(
    db: tauri::State<'_, Db>,
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

    let repo = SqliteSiteRepository::new(db.inner());
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
pub async fn read_log_window(
    db: tauri::State<'_, Db>,
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
    db: &Db,
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
pub async fn reveal_log_folder(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
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
            nginx_bin: home.join("nginx"),
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
        app.manage(Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<Db>(),
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
        app.manage(db);
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<Db>(),
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
        app.manage(Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: home.path().join("nginx"),
            php: vec![],
        })));

        let err = read_log_window(
            app.state::<Db>(),
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
        app.manage(Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<Db>(),
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
            &db,
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
        app.manage(db);
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let site_dir = home.path().join("logs/sites/shop.localhost");
        std::fs::create_dir_all(&site_dir).unwrap();
        let victim = home.path().join("victim.txt");
        std::fs::write(&victim, b"secret").unwrap();
        std::os::unix::fs::symlink(&victim, site_dir.join("access.log")).unwrap();

        let err = read_log_window(
            app.state::<Db>(),
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
        app.manage(Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let err = read_log_window(
            app.state::<Db>(),
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
        app.manage(db);
        app.manage(stack(home.path()));
        app.manage(RwLock::new(Some(InstalledRuntimes {
            nginx_bin: home.path().join("nginx"),
            php: vec![openvhost_core::PhpRuntime {
                major: "8.3".into(),
                fpm_bin: home.path().join("php-fpm"),
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
            app.state::<Db>(),
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
        app.manage(Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));
        app.manage(Arc::new(Supervisor::new(openvhost_proc::default_driver())));

        let log_dir = home.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        let contents = b"line one\nline two\n";
        std::fs::write(log_dir.join("nginx.error.log"), contents).unwrap();

        let rows = list_log_sources(
            app.state::<Db>(),
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
        app.manage(Db::open_in_memory().await.unwrap());
        app.manage(stack(home.path()));
        app.manage(RwLock::new(None::<InstalledRuntimes>));

        let log_dir = home.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("nginx.error.log"), b"line one\n").unwrap();

        let first = read_log_window(
            app.state::<Db>(),
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
            app.state::<Db>(),
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
            &db,
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
            &db,
            &runtimes,
            &log_paths,
        )
        .await
        .unwrap();

        assert_eq!(folder, home.path().join("logs/sites/shop.localhost"));
    }
}
