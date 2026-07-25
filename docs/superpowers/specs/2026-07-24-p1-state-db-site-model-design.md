# Phase 1 · state.db + Site domain model — design

**Status:** approved design, 2026-07-24. First Phase 1 *backend* slice. Owner: rust-core-engineer.

## 1. Goal & context

Give `openvhost-core` its persistent state store (`state.db`, SQLite via `sqlx`, one file) and the first domain entity — **`Site`** — behind a repository seam. This is the foundation that unblocks the Phase 1 headline (per-site PHP version selection), the Sites CRUD UI, the package-manager UI, and the config apply/diff pipeline (master plan: *"generation is a pure function of (state.db snapshot + templates)"*).

The master plan already fixed the technology (§ tech table: *"App state store — SQLite via `sqlx` (bundled, no server) — single file `state.db`"*) and names the eventual entities (`Site`, `ServicePackage`, `ServiceInstance`, `Certificate`, `HostsEntry`). Today `openvhost-core` is nearly empty (`home`/`error`/`info`/`platform`) and has NO database. `openvhost-core` must not depend on tauri (plan §5) — keeps it testable and reusable by the `openvhost` CLI.

This slice ships **Site only** (YAGNI — the other four entities have no consumer yet). The P0-7 `RenderCtx { server_name, docroot, php_major, listen_addr, php_upstream, upstream_name }` is the concrete downstream consumer that tells us the minimum a `Site` must carry.

## 2. Approved decisions

- **Site only** + the `state.db` foundation. The other four entities and their tables are deferred to the slices that consume them.
- **sqlx compile-time macros** (`query!`/`query_as!`) — SQL is checked against the schema at build time. **Commit the `.sqlx/` offline metadata** so `cargo build` needs no live DB; document the `cargo sqlx prepare` step (run after changing any query) in CLAUDE.md.
- **Validated newtypes at the boundary** (parse-don't-validate) for every field that flows into generated config or file paths — the P0-7 lesson (config-injection must be stopped at ingress, not only at render).

## 3. Location & structure (`crates/openvhost-core/src/`)

- `db/mod.rs` — `Db` handle wrapping a `sqlx::SqlitePool`. `Db::open(home: &Path) -> Result<Db, CoreError>` opens `<home>/state.db` via `SqliteConnectOptions` with `create_if_missing(true)`, `journal_mode(WAL)`, `foreign_keys(true)`, and a `busy_timeout`; then runs migrations (`sqlx::migrate!`). A `Db::open_in_memory()` for tests (`sqlite::memory:` — same migrations, real SQL).
- `db/migrations/0001_sites.sql` — the `sites` table (embedded via `sqlx::migrate!("src/db/migrations")`). Migrations are append-only and idempotent (sqlx tracks applied versions in `_sqlx_migrations`).
- `site/mod.rs` — the `Site` entity + validated newtypes (`SiteId`, `SiteName`, `Domain`, `PhpVersion`, enum `WebServer`).
- `site/repo.rs` — `SiteRepository` trait + `SqliteSiteRepository` over a `Db`.
- `error.rs` — add `CoreError::Db(#[from] sqlx::Error)` and `CoreError::Validation { field: &'static str, reason: String }`.
- `lib.rs` — `pub mod db; pub mod site;` + re-exports (`Db`, `Site`, `SiteRepository`, `SqliteSiteRepository`, the newtypes).

## 4. The `Site` model & validation

Fields (table `sites`): `id TEXT PK`, `name TEXT UNIQUE NOT NULL`, `domain TEXT UNIQUE NOT NULL`, `docroot TEXT NOT NULL`, `web_server TEXT NOT NULL`, `php_version TEXT NOT NULL`, `enabled INTEGER NOT NULL` (0/1), `created_at INTEGER NOT NULL`, `updated_at INTEGER NOT NULL` (both unix-epoch millis, `i64` — no date-lib dep; computed from `SystemTime`).

Validated newtypes (each `parse`/`TryFrom<&str>` returns `CoreError::Validation`; the ONLY way to construct one — invalid states are unrepresentable):
- `SiteId(String)` — a v4 UUID (via the `uuid` crate), stored as TEXT. Stable and safe to send across IPC later.
- `SiteName(String)` — slug charset `^[a-z0-9][a-z0-9-]{0,62}$`. Names an identifier surface.
- `Domain(String)` — hostname charset only (`[a-z0-9.-]`, no spaces/quotes/control, ≤253 bytes, labels 1–63) — flows into nginx `server_name`.
- `PhpVersion(String)` — `^\d+\.\d+$` (e.g. `8.3`) — maps to `RenderCtx.php_major`.
- `WebServer` — enum `Nginx | Apache` (TEXT `"nginx"`/`"apache"` in the db; reject anything else on read).
- `docroot` — must be an **absolute** path, valid UTF-8, no NUL/control chars, no `"` (the exact class the P0-7 `to_config_path` rejects) — defense-in-depth at ingress, not only at render.

**Security rationale (binding):** `name`, `domain`, `docroot`, `php_version` all become generated nginx directives and/or filesystem paths downstream. Validating them as they enter `state.db` means a hostile/typo value can never reach the config renderer. This mirrors P0-7's `to_config_path` quote/control-char reject and `upstream_name` `[a-z0-9_]` rule — pushed to the data boundary.

## 5. Repository seam

```
trait SiteRepository: Send + Sync {
    async fn create(&self, new: NewSite) -> Result<Site, CoreError>;   // validates, assigns id + timestamps
    async fn get(&self, id: &SiteId) -> Result<Option<Site>, CoreError>;
    async fn list(&self) -> Result<Vec<Site>, CoreError>;              // ordered by name
    async fn update(&self, site: &Site) -> Result<Site, CoreError>;    // bumps updated_at
    async fn delete(&self, id: &SiteId) -> Result<bool, CoreError>;    // false if absent
}
```

`NewSite` is the un-persisted input (validated newtypes, no id/timestamps). `SqliteSiteRepository { pool }` implements it with `query_as!`/`query!`. A unique-constraint violation on `name`/`domain` maps to a clear `CoreError::Validation { field, .. }` (not a raw sqlx error) so callers can surface "name already taken". Business logic and UI depend on the **trait**, never the concrete type.

The methods are `async` (sqlx is async) — this composes cleanly with the app's already-`async` Tauri command surface (no `spawn_blocking`, unlike a `rusqlite` path would need).

**Async-in-trait note (implementation):** native `async fn` in a trait produces futures that are not guaranteed `Send`, which bites only when the repo is later held as `Arc<dyn SiteRepository>` behind a multi-thread Tauri `State` and awaited across threads. This slice wires the **concrete** `SqliteSiteRepository` (whose sqlx-backed futures ARE `Send`), so plain `async fn` is fine here. If a later slice needs `dyn SiteRepository`, make the futures `Send` then (e.g. `#[trait_variant::make(Send)]` or `-> impl Future<Output = …> + Send`) — do NOT reach for a heavyweight `async-trait`-crate boxing just to satisfy this slice.

## 6. App wiring & proof

The desktop app opens the store at startup and manages it, WITHOUT adding any IPC command yet (Sites IPC is its own UI slice): in `apps/desktop/src-tauri/src/lib.rs`, after the single-instance lock is acquired (P0-8) and `resolve_home()` succeeds, `Db::open(&home)` then `app.manage(db)`. On `Db::open` error, log and continue (the Services UI still works; Sites features simply aren't available) — do not panic. This proves migrations run against a real `state.db` on the real home. `openvhost-proc` is unaffected; `openvhost-core` gains the db module but stays tauri-free (the app does the wiring).

## 7. Testing

- **Repository round-trip** (in-memory db, real SQL): create → get → list → update → delete; assert field fidelity through the newtypes.
- **Unique constraints:** a second `create` with a duplicate `name` (and separately `domain`) returns `CoreError::Validation { field: "name" | "domain" }`, not a raw db error.
- **Validation floor:** `NewSite`/newtype construction rejects a `domain`/`name`/`docroot` containing a quote, space, control char, or (docroot) a relative path — each returns `CoreError::Validation`.
- **Migrations idempotent:** `Db::open` twice on the same file applies cleanly and re-open is a no-op (sqlx version tracking).
- **Offline build:** `cargo build` succeeds with `.sqlx/` committed and no `DATABASE_URL` set (CI/other-dev friendly).
- **App wiring:** the desktop app builds and `Db::open` runs at startup on the real home (manual/inspection; no new IPC to test).

## 8. Non-goals (own future slices)

Sites IPC commands + the Sites panel UI (slice C); hosts-file management; the config apply/diff/reload pipeline; the other four entities (`ServicePackage`/`ServiceInstance`/`Certificate`/`HostsEntry`); actual per-site PHP *switching* logic (this slice only stores the chosen version); port/socket allocation; secrets-at-rest handling (no secret columns in `sites`).

## 9. Delivery constraints

- Branch `feat/p1-state-db-site-model` off `main`. SPDX `// SPDX-License-Identifier: GPL-3.0-or-later` line 1 of every new `.rs`; migration `.sql` files carry an SPDX comment. No `unwrap()`/`expect()` outside `#[cfg(test)]`. `openvhost-core` stays tauri-free. New deps: `sqlx` (features `runtime-tokio`, `sqlite`, `macros`), `uuid` (v4) — both MIT/Apache; the `cargo deny` license gate must pass. Commit `.sqlx/`. DCO `git commit -s`, no `Co-Authored-By`, Conventional Commits. No security-auditor gate (no helper/cert/download/hosts/IPC-ACL surface — but the validated-newtype ingress guard IS the security-relevant core of this slice and the reviewer must check it). CI disabled → local gates are the merge gate.
