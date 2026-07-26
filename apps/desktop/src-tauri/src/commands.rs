// SPDX-License-Identifier: GPL-3.0-or-later
//! Tauri command surface — thin validation + delegation to openvhost-core
//! (business logic never lives here; master plan §5).

use std::path::{Path, PathBuf};

// `WebServerAdapter` is imported for its `supports_hot_reload` method: it is a
// trait method, so the trait must be in scope even though the call site names
// the concrete `NginxAdapter`.
use openvhost_conf::WebServerAdapter;

use openvhost_core::{
    ApplyError, ApplyInput, ChangeKind, CoreInfo, Db, Docroot, Domain, InstalledRuntimes, NewSite,
    PhpVersion, Site, SiteId, SiteName, SiteRepository, SqliteSiteRepository, WebServer,
};

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

#[tauri::command]
#[specta::specta]
pub async fn create_site(db: tauri::State<'_, Db>, input: SiteInput) -> Result<SiteDto, IpcError> {
    let new: NewSite = input.try_into()?;
    let repo = SqliteSiteRepository::new(db.inner());
    Ok(SiteDto::from(repo.create(new).await?))
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

/// `http://<domain>`. Extracted so the one thing worth pinning — that the scheme
/// is fixed and prepended, never taken from the stored value — is testable
/// without a live `AppHandle` and a real database.
fn site_url(domain: &str) -> String {
    format!("http://{domain}")
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

/// Build the apply input from state.db plus the runtimes probed at startup.
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
    Ok(ApplyInput {
        home: paths.home.clone(),
        sites: repo.list().await?,
        runtimes: runtimes.clone(),
    })
}

/// What Apply would change. Read-only and process-free — the pending-changes
/// banner calls this after every site mutation.
#[tauri::command]
#[specta::specta]
pub async fn plan_site_apply(
    db: tauri::State<'_, Db>,
    runtimes: tauri::State<'_, Option<InstalledRuntimes>>,
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<ApplyPlanDto, IpcError> {
    let input = apply_input(db.inner(), runtimes.inner(), paths.inner()).await?;
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

/// Apply the sites, then restart whichever affected services are running.
///
/// The restart is the app's job, not core's: `openvhost-core` has no supervisor
/// and must stay usable from the CLI.
#[tauri::command]
#[specta::specta]
pub async fn apply_sites(
    db: tauri::State<'_, Db>,
    runtimes: tauri::State<'_, Option<InstalledRuntimes>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
) -> Result<ApplyOutcomeDto, IpcError> {
    let input = apply_input(db.inner(), runtimes.inner(), paths.inner()).await?;
    let Some(stack) = paths.inner().as_ref() else {
        return Err(IpcError::Core {
            message: "no web server stack is configured for this platform".into(),
        });
    };
    let p = openvhost_core::plan(&input)?;
    let validator = openvhost_core::NginxValidator {
        bin: stack.nginx_bin.clone(),
        err_log: stack.home.join("logs/nginx.error.log"),
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
/// Split out from `apply_sites` so the straggler logic is testable without
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
                // Third occurrence of this literal in the crate; folding the three
                // behind a `StackPaths::nginx_error_log()` accessor is a recorded
                // follow-up rather than part of this wave.
                err_log: paths.home.join("logs/nginx.error.log"),
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
fn web_server_rows(p: &StackPaths, version: Option<String>) -> Vec<WebServerDto> {
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
    let err_log = p.home.join("logs/nginx.error.log");
    let version = openvhost_conf::probe_nginx_version(&p.nginx_bin, &err_log).await;
    Ok(web_server_rows(p, version))
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod site_ipc_tests {
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
        assert_eq!(site_url("hello.localhost"), "http://hello.localhost");
        // Even if a scheme-looking value somehow reached the column, the result is
        // still an http URL naming it as a host — never a `file:`/`javascript:` URL.
        assert!(site_url("file:///etc/passwd").starts_with("http://"));
        assert!(site_url("javascript:alert(1)").starts_with("http://"));
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
        let listed = web_server_rows(&p, Some("1.27.3".into()));

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
}

// Unix-only: the stand-in nginx is a `#!/bin/sh` script made executable via
// `PermissionsExt`, exactly as `openvhost-conf`'s inspect tests do it. Windows
// has no supported web-server stack yet (`stack::macos_stack` is the only
// builder), so there is nothing here a Windows run would be covering.
#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
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
}
