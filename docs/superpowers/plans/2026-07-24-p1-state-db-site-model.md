# Phase 1 · state.db + Site domain model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `openvhost-core` a persistent SQLite state store (`state.db`) and the first domain entity — `Site` — behind a repository seam, with validated newtypes that stop config-injection at the data boundary.

**Architecture:** `Db` wraps a `sqlx::SqlitePool` on `<home>/state.db` (WAL, FK on, embedded migrations run at open). `Site` is built from parse-don't-validate newtypes; `SiteRepository` (trait) + `SqliteSiteRepository` do CRUD with sqlx compile-time-checked queries; `.sqlx/` offline metadata is committed so builds need no live DB. The desktop app opens + manages the store at startup (no IPC yet). `openvhost-core` stays tauri-free.

**Tech Stack:** Rust 2024, `sqlx` (sqlite + runtime-tokio + macros), `uuid` v4, tokio (dev/tests). No `regex`/`chrono` deps (manual validation, `i64` unix-millis timestamps).

**Spec:** `docs/superpowers/specs/2026-07-24-p1-state-db-site-model-design.md`

## Global Constraints

- Branch `feat/p1-state-db-site-model` off `main`.
- SPDX line 1 of every new `.rs` (`// SPDX-License-Identifier: GPL-3.0-or-later`); every new `.sql` migration starts with `-- SPDX-License-Identifier: GPL-3.0-or-later`.
- No `unwrap()`/`expect()` outside `#[cfg(test)]`. `openvhost-core` must NOT depend on `tauri`.
- New deps `sqlx` + `uuid` are MIT/Apache — `cargo deny check licenses` must pass (fix `deny.toml` only for a genuine new transitive license, never to hide a real finding).
- **Commit `.sqlx/`** (offline query metadata) so `cargo build` succeeds with no `DATABASE_URL`. Document the `cargo sqlx prepare` workflow in `CLAUDE.md`.
- Validated newtypes are the ONLY constructor for `SiteName`/`Domain`/`PhpVersion`/`docroot`/`SiteId` — invalid states unrepresentable; every field that reaches generated config or a filesystem path is charset-checked at ingress (P0-7 lesson) AND re-validated on read from the db.
- DCO `git commit -s`, no `Co-Authored-By`, Conventional Commits.
- Gate each task: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh` (offline — `.sqlx/` present). Run `cargo fmt` first.

---

### Task 1: `Db` foundation — deps, connection, migrations, error variants

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`: add `sqlx`, `uuid`)
- Modify: `crates/openvhost-core/Cargo.toml` (deps: `sqlx`, `uuid`)
- Create: `crates/openvhost-core/src/db/mod.rs`
- Create: `crates/openvhost-core/src/db/migrations/0001_sites.sql`
- Modify: `crates/openvhost-core/src/error.rs` (add `Db`, `Validation`)
- Modify: `crates/openvhost-core/src/lib.rs` (`pub mod db;` + re-export `Db`)

**Interfaces produced:**
- `Db` with `Db::open(home: &Path) -> Result<Db, CoreError>`, `Db::open_in_memory() -> Result<Db, CoreError>`, `Db::pool(&self) -> &sqlx::SqlitePool`.
- `CoreError::Db(sqlx::Error)` (via `#[from]`), `CoreError::Validation { field: &'static str, reason: String }`.
- `pub(crate) fn now_ms() -> i64`.

- [ ] **Step 1: Branch + workspace deps**

```bash
git checkout main && git pull --ff-only && git checkout -b feat/p1-state-db-site-model
```

In the ROOT `Cargo.toml` `[workspace.dependencies]`, add (keep alphabetical if the section is sorted):

```toml
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite", "macros", "migrate"] }
uuid = { version = "1", features = ["v4"] }
```

In `crates/openvhost-core/Cargo.toml` `[dependencies]`:

```toml
sqlx = { workspace = true }
uuid = { workspace = true }
```

(`tokio` is already a dev-dependency for the async tests. The version above reflects mid-2026 knowledge — before writing code, confirm the current `sqlx` 0.x API/features per the master plan's version caveat; if `0.8` has moved, use the current release and keep the same feature set.)

- [ ] **Step 2: Add the error variants**

In `crates/openvhost-core/src/error.rs`, add to `enum CoreError`:

```rust
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    /// A domain value failed validation at the boundary (parse-don't-validate).
    #[error("invalid {field}: {reason}")]
    Validation { field: &'static str, reason: String },
```

- [ ] **Step 3: Write the migration**

`crates/openvhost-core/src/db/migrations/0001_sites.sql`:

```sql
-- SPDX-License-Identifier: GPL-3.0-or-later
CREATE TABLE sites (
    id          TEXT    PRIMARY KEY NOT NULL,
    name        TEXT    NOT NULL UNIQUE,
    domain      TEXT    NOT NULL UNIQUE,
    docroot     TEXT    NOT NULL,
    web_server  TEXT    NOT NULL,
    php_version TEXT    NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
```

- [ ] **Step 4: Write the failing test for `Db::open_in_memory` + idempotent migrations**

`crates/openvhost-core/src/db/mod.rs` (bottom):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_db_runs_migrations() {
        let db = Db::open_in_memory().await.unwrap();
        // The migration created `sites`; querying it must succeed (empty).
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sites")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn open_is_idempotent_on_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let a = Db::open(home).await.unwrap();
        drop(a);
        // Re-open the same file: migrations already applied, no error.
        let b = Db::open(home).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sites")
            .fetch_one(b.pool())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
```

- [ ] **Step 5: Run to verify failure**

Run: `cargo test -p openvhost-core db:: 2>&1 | tail -5`
Expected: compile error — `Db` undefined.

- [ ] **Step 6: Implement `Db`**

Top of `crates/openvhost-core/src/db/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! SQLite state store (`state.db`) — one file under the OpenVHost home.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::error::CoreError;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/db/migrations");

/// Milliseconds since the Unix epoch (no date-lib dependency).
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Handle to the SQLite state store.
#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Open (creating if absent) `<home>/state.db` and run migrations.
    pub async fn open(home: &Path) -> Result<Db, CoreError> {
        let path = home.join("state.db");
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        Self::from_options(opts).await
    }

    /// In-memory store for tests — same migrations, real SQL.
    pub async fn open_in_memory() -> Result<Db, CoreError> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        Self::from_options(opts).await
    }

    async fn from_options(opts: SqliteConnectOptions) -> Result<Db, CoreError> {
        // A single connection keeps `:memory:` coherent and is plenty for a
        // desktop app; WAL handles concurrency on the file path.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        MIGRATOR.run(&pool).await.map_err(sqlx::Error::from)?;
        Ok(Db { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
```

Add to `crates/openvhost-core/src/lib.rs`: `pub mod db;` and `pub use db::Db;`.

- [ ] **Step 7: Green + gate + commit**

```bash
cargo test -p openvhost-core db:: 2>&1 | tail -6
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh
git add Cargo.toml Cargo.lock crates/openvhost-core deny.toml 2>/dev/null; git commit -s -m "feat(core): state.db handle — sqlx SqlitePool, WAL, embedded migrations"
```

Expected: `sites` table migration runs in-memory + on a temp file, idempotent; workspace green; `cargo deny` exit 0 (sqlx/uuid MIT/Apache). No `query!` macros yet, so this compiles with no `DATABASE_URL`.

---

### Task 2: `Site` entity + validated newtypes

**Files:**
- Create: `crates/openvhost-core/src/site/mod.rs`
- Modify: `crates/openvhost-core/src/lib.rs` (`pub mod site;` + re-exports)

**Interfaces:**
- Consumes: `CoreError::Validation` (Task 1).
- Produces: `SiteId`, `SiteName`, `Domain`, `PhpVersion`, `WebServer` (enum `Nginx|Apache`), `Site`, `NewSite`. Each newtype: `parse(&str) -> Result<Self, CoreError>` and `as_str(&self) -> &str`. `WebServer::parse`/`as_str` (`"nginx"`/`"apache"`). `Site { id, name, domain, docroot, web_server, php_version, enabled, created_at, updated_at }` (docroot: `PathBuf`, enabled: `bool`, timestamps: `i64`). `NewSite { name, domain, docroot, web_server, php_version, enabled }` (validated newtypes, no id/timestamps).

- [ ] **Step 1: Write the failing validation tests**

`crates/openvhost-core/src/site/mod.rs` (bottom):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sitename_accepts_slug_rejects_hostile() {
        assert!(SiteName::parse("my-shop").is_ok());
        assert!(SiteName::parse("blog1").is_ok());
        for bad in ["", "-lead", "UPPER", "has space", "quote\"", "semi;colon", "a/b"] {
            assert!(SiteName::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn domain_accepts_hostname_rejects_hostile() {
        assert!(Domain::parse("myshop.localhost").is_ok());
        for bad in ["", "bad domain", "a..b", ".lead", "trail.", "quote\".x", "x\n.y", "under_score.x"] {
            assert!(Domain::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn phpversion_major_minor_only() {
        assert!(PhpVersion::parse("8.3").is_ok());
        for bad in ["8", "8.3.1", "8.x", "v8.3", ""] {
            assert!(PhpVersion::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn docroot_absolute_utf8_no_control_or_quote() {
        assert!(NewSite::docroot_from("/srv/www/shop").is_ok());
        for bad in ["relative/path", "/has\"quote", "/has\0nul", "/has\ncontrol"] {
            assert!(NewSite::docroot_from(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn webserver_roundtrip() {
        assert_eq!(WebServer::parse("nginx").unwrap().as_str(), "nginx");
        assert_eq!(WebServer::parse("apache").unwrap().as_str(), "apache");
        assert!(WebServer::parse("caddy").is_err());
    }

    #[test]
    fn siteid_new_is_a_uuid_and_parses_back() {
        let id = SiteId::new();
        assert!(SiteId::parse(id.as_str()).is_ok());
        assert!(SiteId::parse("not-a-uuid").is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core site:: 2>&1 | tail -5`
Expected: compile error — types undefined.

- [ ] **Step 3: Implement the newtypes + `Site`/`NewSite`**

Top of `crates/openvhost-core/src/site/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! The `Site` domain entity + parse-don't-validate newtypes. Every field that
//! reaches generated config or a filesystem path is charset-checked here, at
//! the boundary (the P0-7 config-injection lesson pushed to ingress).

use std::path::{Path, PathBuf};

use crate::error::CoreError;

fn invalid(field: &'static str, reason: impl Into<String>) -> CoreError {
    CoreError::Validation { field, reason: reason.into() }
}

macro_rules! newtype_str {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

newtype_str!(SiteId);
newtype_str!(SiteName);
newtype_str!(Domain);
newtype_str!(PhpVersion);

impl SiteId {
    /// A fresh v4 UUID.
    pub fn new() -> Self {
        SiteId(uuid::Uuid::new_v4().to_string())
    }
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        uuid::Uuid::parse_str(s).map_err(|_| invalid("id", "not a UUID"))?;
        Ok(SiteId(s.to_string()))
    }
}
impl Default for SiteId {
    fn default() -> Self {
        Self::new()
    }
}

impl SiteName {
    /// Slug: `[a-z0-9]` first char, then `[a-z0-9-]`, length 1..=63.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let ok = (1..=63).contains(&s.len())
            && s.bytes().enumerate().all(|(i, b)| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || (i > 0 && b == b'-')
            });
        if !ok {
            return Err(invalid("name", "must be a 1-63 char [a-z0-9-] slug starting alphanumeric"));
        }
        Ok(SiteName(s.to_string()))
    }
}

impl Domain {
    /// Hostname: labels of `[a-z0-9-]` (no leading/trailing `-`), dot-joined,
    /// each label 1..=63, total ≤253, lowercase only.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let total_ok = (1..=253).contains(&s.len());
        let labels_ok = !s.is_empty()
            && s.split('.').all(|label| {
                (1..=63).contains(&label.len())
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            });
        if !(total_ok && labels_ok) {
            return Err(invalid("domain", "must be a lowercase dotted hostname (labels [a-z0-9-])"));
        }
        Ok(Domain(s.to_string()))
    }
}

impl PhpVersion {
    /// `major.minor`, digits only (e.g. `8.3`).
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let ok = match s.split_once('.') {
            Some((maj, min)) => {
                !maj.is_empty()
                    && !min.is_empty()
                    && maj.bytes().all(|b| b.is_ascii_digit())
                    && min.bytes().all(|b| b.is_ascii_digit())
            }
            None => false,
        };
        if !ok {
            return Err(invalid("php_version", "must be major.minor digits, e.g. 8.3"));
        }
        Ok(PhpVersion(s.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServer {
    Nginx,
    Apache,
}
impl WebServer {
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        match s {
            "nginx" => Ok(WebServer::Nginx),
            "apache" => Ok(WebServer::Apache),
            other => Err(invalid("web_server", format!("unknown web server {other:?}"))),
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            WebServer::Nginx => "nginx",
            WebServer::Apache => "apache",
        }
    }
}

/// A persisted site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub id: SiteId,
    pub name: SiteName,
    pub domain: Domain,
    pub docroot: PathBuf,
    pub web_server: WebServer,
    pub php_version: PhpVersion,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Un-persisted input (no id/timestamps) — all fields already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSite {
    pub name: SiteName,
    pub domain: Domain,
    pub docroot: PathBuf,
    pub web_server: WebServer,
    pub php_version: PhpVersion,
    pub enabled: bool,
}

impl NewSite {
    /// Validate a docroot string: absolute, valid UTF-8 (it's `&str`), no NUL /
    /// control chars / `"` (the exact class P0-7's `to_config_path` rejects).
    pub fn docroot_from(s: &str) -> Result<PathBuf, CoreError> {
        if !Path::new(s).is_absolute() {
            return Err(invalid("docroot", "must be an absolute path"));
        }
        if s.bytes().any(|b| b == 0 || b == b'"' || b.is_ascii_control()) {
            return Err(invalid("docroot", "contains a NUL, quote, or control character"));
        }
        Ok(PathBuf::from(s))
    }
}
```

Add to `crates/openvhost-core/src/lib.rs`: `pub mod site;` and `pub use site::{Domain, NewSite, PhpVersion, Site, SiteId, SiteName, WebServer};`.

- [ ] **Step 4: Green + gate + commit**

```bash
cargo test -p openvhost-core site:: 2>&1 | tail -8
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-core && git commit -s -m "feat(core): Site entity + validated newtypes (parse-don't-validate)"
```

Expected: all newtype tests pass — hostile `name`/`domain`/`docroot` rejected, valid ones accepted.

---

### Task 3: `SiteRepository` + `SqliteSiteRepository` (compile-checked queries + `.sqlx/`)

**Files:**
- Create: `crates/openvhost-core/src/site/repo.rs`
- Modify: `crates/openvhost-core/src/site/mod.rs` (`pub mod repo;`)
- Modify: `crates/openvhost-core/src/lib.rs` (re-export `SiteRepository`, `SqliteSiteRepository`)
- Create: `.sqlx/` (generated, committed)

**Interfaces:**
- Consumes: `Db::pool` (Task 1), `Site`/`NewSite`/newtypes (Task 2), `now_ms`, `CoreError`.
- Produces: `trait SiteRepository { create/get/list/update/delete }`, `SqliteSiteRepository::new(db: &Db) -> Self`.

- [ ] **Step 1: Write the failing repo tests**

`crates/openvhost-core/src/site/repo.rs` (bottom):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::site::{Domain, NewSite, PhpVersion, SiteName, WebServer};

    fn sample(name: &str, domain: &str) -> NewSite {
        NewSite {
            name: SiteName::parse(name).unwrap(),
            domain: Domain::parse(domain).unwrap(),
            docroot: NewSite::docroot_from("/srv/www/shop").unwrap(),
            web_server: WebServer::Nginx,
            php_version: PhpVersion::parse("8.3").unwrap(),
            enabled: true,
        }
    }

    async fn repo() -> (Db, SqliteSiteRepository) {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteSiteRepository::new(&db);
        (db, repo)
    }

    #[tokio::test]
    async fn create_get_list_update_delete_roundtrip() {
        let (_db, repo) = repo().await;
        let created = repo.create(sample("shop", "shop.localhost")).await.unwrap();
        assert_eq!(created.name.as_str(), "shop");
        assert!(created.created_at > 0 && created.updated_at == created.created_at);

        let got = repo.get(&created.id).await.unwrap().unwrap();
        assert_eq!(got, created);

        repo.create(sample("blog", "blog.localhost")).await.unwrap();
        let all = repo.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name.as_str(), "blog"); // ordered by name

        let mut edit = created.clone();
        edit.php_version = PhpVersion::parse("8.2").unwrap();
        let updated = repo.update(&edit).await.unwrap();
        assert_eq!(updated.php_version.as_str(), "8.2");
        assert!(updated.updated_at >= created.updated_at);

        assert!(repo.delete(&created.id).await.unwrap());
        assert!(repo.get(&created.id).await.unwrap().is_none());
        assert!(!repo.delete(&created.id).await.unwrap()); // already gone
    }

    #[tokio::test]
    async fn duplicate_name_and_domain_map_to_validation() {
        let (_db, repo) = repo().await;
        repo.create(sample("shop", "shop.localhost")).await.unwrap();
        let dup_name = repo.create(sample("shop", "other.localhost")).await;
        assert!(matches!(dup_name, Err(CoreError::Validation { field: "name", .. })));
        let dup_domain = repo.create(sample("other", "shop.localhost")).await;
        assert!(matches!(dup_domain, Err(CoreError::Validation { field: "domain", .. })));
    }

    #[tokio::test]
    async fn tampered_row_is_rejected_on_read() {
        let (db, repo) = repo().await;
        // Hand-insert a row with a hostile domain (simulating a tampered db).
        sqlx::query("INSERT INTO sites VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(crate::site::SiteId::new().as_str())
            .bind("x").bind("evil\";inject").bind("/srv/www")
            .bind("nginx").bind("8.3").bind(1).bind(1).bind(1)
            .execute(db.pool())
            .await
            .unwrap();
        // list() re-validates via TryFrom<SiteRow> and must reject it.
        assert!(matches!(repo.list().await, Err(CoreError::Validation { field: "domain", .. })));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core site::repo 2>&1 | tail -5`
Expected: compile error — `SqliteSiteRepository` undefined.

- [ ] **Step 3: Implement the repository (compile-checked queries)**

`crates/openvhost-core/src/site/repo.rs` (top):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Site persistence. Queries are compile-time-checked (`query_as!`/`query!`);
//! rows re-validate through the domain newtypes on read (`TryFrom<SiteRow>`),
//! so a hand-edited `state.db` can never feed an unvalidated value downstream.

use crate::db::{now_ms, Db};
use crate::error::CoreError;
use crate::site::{Domain, NewSite, PhpVersion, Site, SiteId, SiteName, WebServer};
use sqlx::SqlitePool;

/// The persistence seam. Consumers depend on this, not the concrete type.
pub trait SiteRepository: Send + Sync {
    fn create(&self, new: NewSite) -> impl std::future::Future<Output = Result<Site, CoreError>> + Send;
    fn get(&self, id: &SiteId) -> impl std::future::Future<Output = Result<Option<Site>, CoreError>> + Send;
    fn list(&self) -> impl std::future::Future<Output = Result<Vec<Site>, CoreError>> + Send;
    fn update(&self, site: &Site) -> impl std::future::Future<Output = Result<Site, CoreError>> + Send;
    fn delete(&self, id: &SiteId) -> impl std::future::Future<Output = Result<bool, CoreError>> + Send;
}

/// Raw DB row (primitive columns) — decoded by sqlx, then re-validated.
struct SiteRow {
    id: String,
    name: String,
    domain: String,
    docroot: String,
    web_server: String,
    php_version: String,
    enabled: i64,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<SiteRow> for Site {
    type Error = CoreError;
    fn try_from(r: SiteRow) -> Result<Site, CoreError> {
        Ok(Site {
            id: SiteId::parse(&r.id)?,
            name: SiteName::parse(&r.name)?,
            domain: Domain::parse(&r.domain)?,
            docroot: NewSite::docroot_from(&r.docroot)?,
            web_server: WebServer::parse(&r.web_server)?,
            php_version: PhpVersion::parse(&r.php_version)?,
            enabled: r.enabled != 0,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

/// Maps a sqlx UNIQUE-constraint error to a `Validation` on the right field.
fn map_insert_err(e: sqlx::Error, name: &str, domain: &str) -> CoreError {
    if let sqlx::Error::Database(dbe) = &e {
        let msg = dbe.message();
        if msg.contains("sites.name") || msg.contains("sites.domain") {
            let field = if msg.contains("sites.name") { "name" } else { "domain" };
            let val = if field == "name" { name } else { domain };
            return CoreError::Validation { field, reason: format!("{val:?} is already taken") };
        }
    }
    CoreError::Db(e)
}

pub struct SqliteSiteRepository {
    pool: SqlitePool,
}

impl SqliteSiteRepository {
    pub fn new(db: &Db) -> Self {
        Self { pool: db.pool().clone() }
    }
}

impl SiteRepository for SqliteSiteRepository {
    async fn create(&self, new: NewSite) -> Result<Site, CoreError> {
        let id = SiteId::new();
        let ts = now_ms();
        let docroot = new.docroot.to_str().ok_or_else(|| CoreError::Validation {
            field: "docroot",
            reason: "path is not valid UTF-8".into(),
        })?;
        let ws = new.web_server.as_str();
        let enabled = i64::from(new.enabled);
        sqlx::query!(
            "INSERT INTO sites (id, name, domain, docroot, web_server, php_version, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id.as_str(),
            new.name.as_str(),
            new.domain.as_str(),
            docroot,
            ws,
            new.php_version.as_str(),
            enabled,
            ts,
            ts,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| map_insert_err(e, new.name.as_str(), new.domain.as_str()))?;

        Ok(Site {
            id,
            name: new.name,
            domain: new.domain,
            docroot: new.docroot,
            web_server: new.web_server,
            php_version: new.php_version,
            enabled: new.enabled,
            created_at: ts,
            updated_at: ts,
        })
    }

    async fn get(&self, id: &SiteId) -> Result<Option<Site>, CoreError> {
        let row = sqlx::query_as!(
            SiteRow,
            "SELECT id, name, domain, docroot, web_server, php_version, enabled, created_at, updated_at \
             FROM sites WHERE id = ?",
            id.as_str()
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Site::try_from).transpose()
    }

    async fn list(&self) -> Result<Vec<Site>, CoreError> {
        let rows = sqlx::query_as!(
            SiteRow,
            "SELECT id, name, domain, docroot, web_server, php_version, enabled, created_at, updated_at \
             FROM sites ORDER BY name"
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Site::try_from).collect()
    }

    async fn update(&self, site: &Site) -> Result<Site, CoreError> {
        let ts = now_ms();
        let docroot = site.docroot.to_str().ok_or_else(|| CoreError::Validation {
            field: "docroot",
            reason: "path is not valid UTF-8".into(),
        })?;
        let enabled = i64::from(site.enabled);
        sqlx::query!(
            "UPDATE sites SET name = ?, domain = ?, docroot = ?, web_server = ?, php_version = ?, enabled = ?, updated_at = ? \
             WHERE id = ?",
            site.name.as_str(),
            site.domain.as_str(),
            docroot,
            site.web_server.as_str(),
            site.php_version.as_str(),
            enabled,
            ts,
            site.id.as_str(),
        )
        .execute(&self.pool)
        .await
        .map_err(|e| map_insert_err(e, site.name.as_str(), site.domain.as_str()))?;

        let mut updated = site.clone();
        updated.updated_at = ts;
        Ok(updated)
    }

    async fn delete(&self, id: &SiteId) -> Result<bool, CoreError> {
        let res = sqlx::query!("DELETE FROM sites WHERE id = ?", id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
```

Add `pub mod repo;` to `site/mod.rs` and re-export `pub use site::repo::{SiteRepository, SqliteSiteRepository};` from `lib.rs`.

- [ ] **Step 4: Generate the offline query cache (`.sqlx/`)**

The `query!`/`query_as!` macros need the schema at compile time. Generate the offline cache once against a throwaway migrated DB, then commit it so all later builds are offline:

```bash
cargo install sqlx-cli --no-default-features --features sqlite   # if not already installed
export DATABASE_URL="sqlite://$PWD/target/_prepare.db"
sqlx database create
sqlx migrate run --source crates/openvhost-core/src/db/migrations
cargo sqlx prepare --workspace          # writes .sqlx/ at the workspace root
unset DATABASE_URL
rm -f target/_prepare.db
```

Verify offline build works with NO `DATABASE_URL`:

Run: `cargo build -p openvhost-core 2>&1 | tail -3`
Expected: compiles using `.sqlx/` (no DB needed).

- [ ] **Step 5: Green + gate + commit (include `.sqlx/`)**

```bash
cargo test -p openvhost-core site::repo -- --nocapture 2>&1 | tail -12
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-core .sqlx && git commit -s -m "feat(core): SiteRepository + SQLite impl (compile-checked queries, re-validate on read)"
```

Expected: round-trip, ordered list, dup-name/domain→`Validation`, and the tampered-row-rejected test all pass. `.sqlx/` committed.

---

### Task 4: App wiring + CLAUDE.md workflow + full gate + PR

**Files:**
- Modify: `apps/desktop/src-tauri/src/lib.rs` (open + manage `Db` at startup)
- Modify: `CLAUDE.md` (the `cargo sqlx prepare` workflow)

- [ ] **Step 1: Wire `Db` into app startup**

In `apps/desktop/src-tauri/src/lib.rs`, inside the `Ok(Some(lock))` arm (after `app.manage(lock);` ~line 121, alongside the supervisor build), open and manage the store — best-effort, never panic:

```rust
            match tauri::async_runtime::block_on(openvhost_core::Db::open(&home)) {
                Ok(db) => {
                    app.manage(db);
                }
                Err(e) => {
                    eprintln!("openvhost: state.db unavailable ({e}); Sites features disabled this run");
                }
            }
```

(`home` is the `PathBuf` from the enclosing `resolve_home()` match. `Db::open` is async; `setup` is sync, so drive it with Tauri's `async_runtime::block_on` — the same runtime the app already uses. Keep the existing supervisor wiring unchanged.)

- [ ] **Step 2: Document the sqlx workflow in CLAUDE.md**

Under `## Commands` in `CLAUDE.md`, add:

```markdown
- state.db uses sqlx compile-time-checked queries with committed offline
  metadata (`.sqlx/`). After changing any `query!`/`query_as!` or a migration:
  `DATABASE_URL="sqlite://$PWD/target/_prepare.db" sqlx database create && \
   sqlx migrate run --source crates/openvhost-core/src/db/migrations && \
   cargo sqlx prepare --workspace` — then commit the updated `.sqlx/`. Builds
  and CI run offline against the committed cache (no DB required).
```

- [ ] **Step 3: Build the app + full gate**

```bash
cargo build -p openvhost-desktop 2>&1 | tail -3
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh
cargo test -p openvhost-desktop export_bindings 2>&1 | tail -3   # bindings unchanged: no new IPC
pnpm -C apps/desktop build 2>&1 | tail -2
```

Expected: app builds and manages `Db` at startup; no IPC command added (bindings unchanged); everything offline-green; `cargo deny` exit 0.

- [ ] **Step 4: Windows cross-check**

```bash
cargo check --target x86_64-pc-windows-msvc -p openvhost-core 2>&1 | tail -5
```

Expected: clean (sqlx sqlite bundles its own C SQLite; `Db`/newtypes are portable). If the msvc target is missing: `rustup target add x86_64-pc-windows-msvc`.

- [ ] **Step 5: Push + PR**

```bash
git add apps/desktop/src-tauri/src/lib.rs CLAUDE.md && git commit -s -m "feat(desktop): open + manage state.db at startup; doc sqlx offline workflow"
git push -u origin feat/p1-state-db-site-model
gh pr create --title "feat: Phase 1 — state.db + Site domain model (openvhost-core)" --body "Adds openvhost-core's persistent state store (state.db, sqlx SqlitePool, WAL + FK + embedded migrations) and the Site entity behind a SiteRepository seam, with parse-don't-validate newtypes (SiteName/Domain/PhpVersion/docroot) that stop config-injection at the data boundary and re-validate on read. sqlx compile-time-checked queries with committed .sqlx/ offline metadata (builds need no DB). The desktop app opens + manages the store at startup — no IPC command yet (Sites UI is its own slice). openvhost-core stays tauri-free.

Unblocks: Sites CRUD UI, per-site PHP version switching (the Phase 1 headline), package-manager UI, and the config apply/diff pipeline (generation = pure fn of state.db snapshot + templates). Deferred: the other four entities, Sites IPC/UI, hosts-file, apply/diff. Full local gates + cargo deny green; msvc cross-check clean; no security-auditor gate (no helper/cert/download/hosts/IPC-ACL surface — the validated-newtype ingress guard is the security-relevant core and is tested). CI disabled (billing).

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

- [ ] **Step 6: Hand back to controller** — final whole-branch review, then owner merge decision.

---

## Self-review (controller: verify before dispatching Task 1)

- **Spec coverage:** §3 structure → T1 (db) + T2 (site) + T3 (repo); §4 model+validation → T2 (newtypes) + T3 (re-validate on read); §5 repository seam → T3 (trait + impl, unique→Validation); §6 app wiring → T4; §7 testing → T1/T2/T3 tests + T4 build/msvc; §8 non-goals honored (no IPC/UI, Site only); §9 delivery → Global Constraints + T3 (.sqlx) + T4 (CLAUDE.md, deny, msvc, PR). Every spec section maps to a task.
- **Type consistency:** `Db::open`/`open_in_memory`/`pool`, `now_ms`, `CoreError::{Db,Validation{field,reason}}`, `SiteId/SiteName/Domain/PhpVersion` (`parse`/`as_str`), `WebServer` (`parse`/`as_str`), `Site`/`NewSite` (fields), `NewSite::docroot_from`, `SiteRepository`/`SqliteSiteRepository::new`, `SiteRow`+`TryFrom` — consistent across tasks.
- **Placeholder scan:** every code step is complete; the sqlx-prepare workflow has exact commands.
- **Hazards flagged for implementers:** the `query!` macros compile ONLY with `.sqlx/` present or a live `DATABASE_URL` — Task 3 Step 4 generates the cache BEFORE the gate; the trait uses `-> impl Future + Send` (not `async fn`, so the concrete impl's Send futures satisfy a future `dyn`/`State` need without an `async-trait` box — per the spec's async-in-trait note); timestamps are `i64` millis (no chrono); `STRICT` table + `enabled` stored as `i64` (0/1) then mapped to `bool`; `Db::open` is async but `setup` is sync → `async_runtime::block_on`. The sqlx version in the plan is mid-2026 — confirm the current release/features before writing (master plan version caveat).
