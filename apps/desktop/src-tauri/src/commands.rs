// SPDX-License-Identifier: GPL-3.0-or-later
//! Tauri command surface — thin validation + delegation to openvhost-core
//! (business logic never lives here; master plan §5).

use std::path::Path;

// `WebServerAdapter` is imported for its `supports_hot_reload` method: it is a
// trait method, so the trait must be in scope even though the call site names
// the concrete `NginxAdapter`.
use openvhost_conf::WebServerAdapter;

use openvhost_core::{
    CoreInfo, Db, Docroot, Domain, NewSite, PhpVersion, Site, SiteId, SiteName, SiteRepository,
    SqliteSiteRepository, WebServer,
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

#[tauri::command]
#[specta::specta]
pub async fn service_log_tail(
    sup: tauri::State<'_, Arc<Supervisor>>,
    id: String,
    n: u32,
) -> Result<Vec<LogLine>, IpcError> {
    sup.log_tail(&id, n as usize).map_err(IpcError::from)
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

#[tauri::command]
#[specta::specta]
pub async fn delete_site(db: tauri::State<'_, Db>, id: String) -> Result<bool, IpcError> {
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db.inner());
    Ok(repo.delete(&site_id).await?)
}

/// The web servers OpenVHost knows about. A CLOSED list: the client sends only
/// this id — never a path, filename or argument — and every path used by the
/// commands below is derived server-side from managed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebServerBrand {
    Nginx,
    Apache,
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

    /// Whether OpenVHost can actually operate this brand end to end: adapter,
    /// template, real paths in `StackPaths` and a validator that speaks its
    /// config. Exhaustive rather than `matches!`, so a third variant has to be
    /// classified here instead of silently defaulting to unsupported.
    fn supported(self) -> bool {
        match self {
            Self::Nginx => true,
            // Flipping this to `true` is NOT on its own enough to support Apache.
            // It also needs its own paths on `StackPaths`, its own arm in
            // `live_config_path` below, and its own validator — the one
            // `validate_web_server_config` uses runs `nginx -t`. Until all three
            // exist, `live_config_path` is what stops a supported-but-pathless
            // brand from being handed nginx's config under an Apache heading.
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
            message: "OpenVHost cannot serve Apache sites yet — it only generates nginx config"
                .into(),
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
/// `None` means **no web server stack was managed for this platform**, which
/// today is every target except macOS: the OpenVHost home resolved perfectly
/// well and there is simply no stack builder yet. Hence the platform-agnostic
/// message — it must not blame home resolution.
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
    // mount would block a tokio worker thread, and on a desktop-sized worker pool
    // that stalls the supervisor event pump and other in-flight commands too.
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
    // Same brand-keyed lookup as `read_web_server_config`, and it is what makes
    // the nginx binary below correct: `validate_live` runs `nginx -t`, so it may
    // only ever be handed a path this accessor yielded for `Nginx`. A brand it
    // rejects stops here rather than getting a green "valid" badge for a config
    // nginx never examined.
    let conf = brand.live_config_path(p)?;
    // Read-only: `validate_live` runs `-t` against the config in place and never
    // calls `materialize`. `-e` keeps nginx's OWN error log inside our home
    // instead of its compiled-in prefix; no config file is written.
    let err_log = p.home.join("logs/nginx.error.log");
    let report = openvhost_conf::validate_live(&p.nginx_bin, conf, &err_log)
        .await
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
}
