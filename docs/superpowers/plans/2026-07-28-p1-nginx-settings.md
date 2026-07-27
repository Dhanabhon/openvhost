<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Editable nginx Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a developer change nginx's connection limits, timeouts, upload size and compression from the Web server page, with the same diff-then-apply safety the Sites page already has.

**Architecture:** The settings become **inputs to the config generator**, never edits to the generated file. `render_set` reads them and hands them to the main-config renderer, so `plan()` sees `nginx.conf` as Modified and the existing diff → `nginx -t` → rollback → restart pipeline covers them with no second path.

**Tech Stack:** Rust 2021 (sqlx with compile-time-checked queries, thiserror, Tera), Tauri 2 + tauri-specta, SvelteKit + Svelte 5 runes, vitest.

**Source spec:** `docs/superpowers/specs/2026-07-28-p1-nginx-settings-design.md`

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later` (`<!-- ... -->` for `.svelte`, `-- ...` for `.sql`).
- Commits are DCO-signed (`git commit -s`), Conventional Commits.
- No `unwrap()`/`expect()` outside `#[cfg(test)]`. The workspace denies `clippy::unwrap_used`/`expect_used` under `-D warnings`, and **clippy compiles the lib without `cfg(test)`** — an import used only by tests must be `#[cfg(test)]`-gated.
- `openvhost-core` must never depend on `tauri`.
- **Nothing unparsed reaches a template.** Every setting travels from the webview into a config file — the same boundary a `$` slipped through in the site-apply slice.
- `gzip_types` accepts at most **64 tokens, each at most 128 bytes**, each matching `^[a-z0-9][a-z0-9.+-]*/[a-z0-9][a-z0-9.+-]*$`.
- Defaults: `worker_connections` 1024 · `client_max_body_size` 256m · `keepalive_timeout` 65 · `tcp_nodelay` on · `fastcgi_connect_timeout` 60 · `fastcgi_send_timeout` 300 · `fastcgi_read_timeout` 300 · `gzip` off · `gzip_comp_level` 1 · `gzip_types` the ServBay set.
- Every value is written to the config **even at its default** (spec §5.1).
- Tauri DTOs must not expose `usize`/`isize` — specta rejects them.
- After changing any `query!`/`query_as!` or a migration, regenerate the offline cache per CLAUDE.md and commit `.sqlx/`.
- Gate before every commit: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`, plus `pnpm -C apps/desktop test`, `pnpm -C apps/desktop lint`, `pnpm -C apps/desktop exec svelte-check` for frontend tasks.

## Correction to the spec, found while planning

Spec §4 puts `WebServerSettings` in `openvhost-core::settings`. **It cannot live there.**
`generate_main_config` is in `openvhost-conf`, and **`openvhost-core` depends on
`openvhost-conf`, not the reverse** — conf cannot name a core type.

The settings type and its newtypes therefore live in **`openvhost-conf::settings`**, which is
also the more honest home: they are the values a config template needs. Two consequences:

- `parse` returns `ConfError::InvalidField { field, value, reason }`, which already exists and
  already carries exactly the field-level shape the UI needs.
- `openvhost-core`'s repository persists and returns `openvhost_conf::WebServerSettings`. Core
  depending on a conf type is the direction the graph already allows.

This is the same shape as the `find_brew_binaries` correction in the PHP slice: the spec asked
for a home that would have inverted the dependency graph.

## File Structure

**`crates/openvhost-conf`**
- `src/settings/mod.rs` — create: `WebServerSettings`, `Default`, re-exports.
- `src/settings/value.rs` — create: the newtypes and their `parse`.
- `src/webserver.rs` — modify: `generate_main_config(home, &settings)`.
- `templates/nginx/main.conf.tera` — modify: render the directives.
- `src/lib.rs` — modify: exports.

**`crates/openvhost-core`**
- `src/db/migrations/0002_web_server_settings.sql` — create.
- `src/settings_repo.rs` — create: the persistence seam.
- `src/site/apply/mod.rs` — modify: `ApplyInput` gains `settings`; `render_set` passes it.
- `src/lib.rs` — modify: exports.
- `.sqlx/` — regenerated, committed.

**`apps/desktop`**
- `src-tauri/src/commands.rs` — modify: two new commands, two renames, the `ConfError` → `IpcError::Validation` mapping.
- `src-tauri/src/lib.rs` — modify: command registration.
- `src/lib/ipc/bindings.ts` — regenerated, committed.
- `src/lib/ipc/index.ts` — modify: wrappers, renames.
- `src/lib/websettings.svelte.ts` (+ `.test.ts`) — create: the form store.
- `src/lib/components/WebServerSettingsForm.svelte` (+ `.test.ts`) — create.
- `src/routes/web-server/+page.svelte` — modify: mount the form, drop "Read-only".
- `src/lib/components/WebServerPanel.svelte` — modify: subtitle.

---

## Task 1: The settings type and its newtypes

Pure values, no IO, no database. This is where the injection boundary is, so it is its own task.

**Files:**
- Create: `crates/openvhost-conf/src/settings/mod.rs`
- Create: `crates/openvhost-conf/src/settings/value.rs`
- Modify: `crates/openvhost-conf/src/lib.rs`
- Test: `crates/openvhost-conf/src/settings/value.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ConfError::InvalidField { field: &'static str, value: String, reason: &'static str }` — read `crates/openvhost-conf/src/error.rs` and match its real field types before writing.
- Produces:
```rust
  pub struct WorkerConnections(u32);   // 1..=65535
  pub struct Seconds(u32);             // 1..=86400
  pub struct GzipLevel(u32);           // 1..=9
  pub struct BodySize(String);         // ^\d+[kKmMgG]?$
  pub struct GzipTypes(Vec<String>);   // <=64 tokens, each <=128 bytes, MIME-shaped
  pub struct OnOff(bool);

  impl WorkerConnections { pub fn parse(v: u32) -> Result<Self, ConfError>; pub fn get(&self) -> u32; }
  impl Seconds          { pub fn parse(v: u32) -> Result<Self, ConfError>; pub fn get(&self) -> u32; }
  impl GzipLevel        { pub fn parse(v: u32) -> Result<Self, ConfError>; pub fn get(&self) -> u32; }
  impl BodySize   { pub fn parse(s: &str) -> Result<Self, ConfError>; pub fn as_str(&self) -> &str; }
  impl GzipTypes  { pub fn parse(s: &str) -> Result<Self, ConfError>; pub fn as_directive(&self) -> String; }
  impl OnOff      { pub fn new(on: bool) -> Self; pub fn as_str(&self) -> &'static str; /* "on"/"off" */ }

  pub struct WebServerSettings {
      pub worker_connections: WorkerConnections,
      pub client_max_body_size: BodySize,
      pub keepalive_timeout: Seconds,
      pub tcp_nodelay: OnOff,
      pub fastcgi_connect_timeout: Seconds,
      pub fastcgi_send_timeout: Seconds,
      pub fastcgi_read_timeout: Seconds,
      pub gzip: OnOff,
      pub gzip_comp_level: GzipLevel,
      pub gzip_types: GzipTypes,
  }
  impl Default for WebServerSettings {
    /// Development-appropriate rather than nginx's own (spec §5). Safe to
    /// choose because the diff preview shows the user exactly what changes
    /// before it lands — without that, nginx's values would be the only
    /// defensible defaults.
    ///
    /// Built from `new_unchecked` rather than `parse`, because `Default`
    /// cannot fail and threading a fallible constructor through it would hide
    /// which value is real behind error handling that can never fire. Every
    /// constant here is inside the bounds its own `parse` enforces, and
    /// `every_default_would_survive_its_own_parser` is what keeps that true.
    fn default() -> Self {
        Self {
            worker_connections: WorkerConnections::new_unchecked(1024),
            client_max_body_size: BodySize::new_unchecked("256m"),
            keepalive_timeout: Seconds::new_unchecked(65),
            tcp_nodelay: OnOff::new(true),
            fastcgi_connect_timeout: Seconds::new_unchecked(60),
            fastcgi_send_timeout: Seconds::new_unchecked(300),
            fastcgi_read_timeout: Seconds::new_unchecked(300),
            gzip: OnOff::new(false),
            gzip_comp_level: GzipLevel::new_unchecked(1),
            gzip_types: GzipTypes::new_unchecked(DEFAULT_GZIP_TYPES),
        }
    }
}
```

Add a `pub(crate) fn new_unchecked` to each newtype in `value.rs` — `pub(crate)`, not `pub`, so
the escape hatch exists for this module and nowhere else. Give each one a comment saying it is
for `Default` only and that `parse` is the boundary. For `GzipTypes::new_unchecked`, split on
whitespace the same way `parse` does so `as_directive` behaves identically either way.

- [ ] **Step 5: Add the test that keeps `Default` honest**

```rust
#[test]
fn every_default_would_survive_its_own_parser() {
    // Default bypasses `parse` (it cannot fail), so this is what stops a
    // default drifting outside the bounds the UI enforces — a value the user
    // could never type but the app ships with.
    let d = WebServerSettings::default();
    assert!(WorkerConnections::parse(d.worker_connections.get()).is_ok());
    assert!(Seconds::parse(d.keepalive_timeout.get()).is_ok());
    assert!(Seconds::parse(d.fastcgi_connect_timeout.get()).is_ok());
    assert!(Seconds::parse(d.fastcgi_send_timeout.get()).is_ok());
    assert!(Seconds::parse(d.fastcgi_read_timeout.get()).is_ok());
    assert!(GzipLevel::parse(d.gzip_comp_level.get()).is_ok());
    assert!(BodySize::parse(d.client_max_body_size.as_str()).is_ok());
    assert!(GzipTypes::parse(&d.gzip_types.as_directive()).is_ok());
}
```

- [ ] **Step 6: Export and run**

Add `pub mod settings;` to `crates/openvhost-conf/src/lib.rs` and re-export `WebServerSettings`
plus the newtypes next to the existing exports.

Run: `cargo test -p openvhost-conf settings`
Expected: PASS — 9 tests.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/openvhost-conf
git commit -s -m "feat(conf): validated nginx settings with development defaults

Each value is private behind a parse, because all of them end up inside a
generated config file. gzip_types is tokenised and checked one token at a
time: passed through, a crafted value becomes real directives that nginx -t
accepts."
```

---

## Task 2: Persist them in state.db

**Files:**
- Create: `crates/openvhost-core/src/db/migrations/0002_web_server_settings.sql`
- Create: `crates/openvhost-core/src/settings_repo.rs`
- Modify: `crates/openvhost-core/src/lib.rs`
- Modify: `.sqlx/` (regenerated)
- Test: `crates/openvhost-core/src/settings_repo.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `openvhost_conf::WebServerSettings` and its newtypes (Task 1); `Db`, `now_ms` from `crate::db`; `CoreError`.
- Produces:
```rust
  pub trait WebServerSettingsRepository: Send + Sync {
      /// The stored settings, or the defaults when no row exists. Does NOT write.
      fn get(&self) -> impl std::future::Future<Output = Result<WebServerSettings, CoreError>> + Send;
      /// Insert or replace the singleton row.
      fn save(&self, s: &WebServerSettings) -> impl std::future::Future<Output = Result<(), CoreError>> + Send;
  }
  pub struct SqliteWebServerSettings<'a>(&'a SqlitePool);
  impl<'a> SqliteWebServerSettings<'a> { pub fn new(db: &'a Db) -> Self; }
```
  Follow `crates/openvhost-core/src/site/repo.rs` for the trait shape — it uses RPITIT rather than `async_trait`, and re-validates rows through the newtypes on read so a hand-edited `state.db` cannot feed an unvalidated value downstream. Do the same here.

- [ ] **Step 1: Write the migration**

`crates/openvhost-core/src/db/migrations/0002_web_server_settings.sql`:

```sql
-- SPDX-License-Identifier: GPL-3.0-or-later
-- A singleton: `id` can only ever be 1, so "which row is the real one" is not
-- a question any query has to answer.
CREATE TABLE web_server_settings (
    id                       INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    worker_connections       INTEGER NOT NULL,
    client_max_body_size     TEXT    NOT NULL,
    keepalive_timeout        INTEGER NOT NULL,
    tcp_nodelay              INTEGER NOT NULL,
    fastcgi_connect_timeout  INTEGER NOT NULL,
    fastcgi_send_timeout     INTEGER NOT NULL,
    fastcgi_read_timeout     INTEGER NOT NULL,
    gzip                     INTEGER NOT NULL,
    gzip_comp_level          INTEGER NOT NULL,
    gzip_types               TEXT    NOT NULL,
    updated_at               INTEGER NOT NULL
) STRICT;
```

No seed row: a fresh install reads the defaults (spec §4).

- [ ] **Step 2: Write the failing tests**

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_fresh_database_reads_the_defaults_without_writing_a_row() {
        // Seeding on read would mean every launch writes to state.db before the
        // user has touched anything, and a failure there would surface as a
        // startup error for a value nobody changed.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        assert_eq!(repo.get().await.unwrap(), WebServerSettings::default());

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM web_server_settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "reading must not insert");
    }

    #[tokio::test]
    async fn a_saved_value_survives_a_round_trip() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        let mut s = WebServerSettings::default();
        s.fastcgi_read_timeout = Seconds::parse(900).unwrap();
        s.gzip = OnOff::new(true);
        repo.save(&s).await.unwrap();

        let back = repo.get().await.unwrap();
        assert_eq!(back.fastcgi_read_timeout.get(), 900);
        assert!(back.gzip.is_on());
    }

    #[tokio::test]
    async fn saving_twice_replaces_rather_than_accumulating() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        repo.save(&WebServerSettings::default()).await.unwrap();
        let mut s = WebServerSettings::default();
        s.keepalive_timeout = Seconds::parse(30).unwrap();
        repo.save(&s).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM web_server_settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(repo.get().await.unwrap().keepalive_timeout.get(), 30);
    }

    #[tokio::test]
    async fn a_hand_edited_row_that_breaks_a_bound_is_rejected_on_read() {
        // state.db is a file on the user's disk. The repo re-validates on read
        // for the same reason SiteRepository does: nothing unparsed may reach a
        // template, whatever wrote the row.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        repo.save(&WebServerSettings::default()).await.unwrap();
        sqlx::query("UPDATE web_server_settings SET gzip_comp_level = 99 WHERE id = 1")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(repo.get().await.is_err());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-core settings_repo`
Expected: FAIL — the module does not exist.

- [ ] **Step 4: Implement**

Booleans are stored as `INTEGER` (0/1) and read back through `OnOff::new(v != 0)`. Everything
else goes through its newtype's `parse` on read, mapping a failure to
`CoreError::Validation { field, reason }` so a corrupt row names the column.

Use `sqlx::query!`/`query_as!` so the queries are compile-time-checked, matching `site/repo.rs`.
`save` is a single `INSERT INTO … VALUES (1, …) ON CONFLICT(id) DO UPDATE SET …`.

- [ ] **Step 5: Regenerate the offline query cache**

Per CLAUDE.md — the build and CI run offline against the committed cache:

```bash
DATABASE_URL="sqlite://$PWD/target/_prepare.db" sqlx database create && \
  sqlx migrate run --source crates/openvhost-core/src/db/migrations && \
  cargo sqlx prepare --workspace
```

If `sqlx-cli` cannot be installed offline, build the crate once with a live `DATABASE_URL`
against a migrated temp database instead (unset `SQLX_OFFLINE`) — sqlx writes `.sqlx/` as a
side effect. **Commit the updated `.sqlx/`.**

- [ ] **Step 6: Run and commit**

Run: `cargo test -p openvhost-core settings_repo`
Expected: PASS — 4 tests.

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/openvhost-core .sqlx
git commit -s -m "feat(core): persist the nginx settings as a singleton row

Reads return the defaults when no row exists and deliberately do not write
one: seeding on read would touch state.db on every launch before the user
has changed anything. Rows re-validate through the newtypes on read, so a
hand-edited database cannot feed an unvalidated value into a template."
```

---

## Task 3: Render them

**Files:**
- Modify: `crates/openvhost-conf/templates/nginx/main.conf.tera`
- Modify: `crates/openvhost-conf/src/webserver.rs`
- Test: `crates/openvhost-conf/src/webserver.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `WebServerSettings` (Task 1).
- Produces: `fn generate_main_config(&self, home: &Path, settings: &WebServerSettings) -> Result<GeneratedFile, ConfError>` — the trait method gains a parameter. `WebServerAdapter::validate` also calls it; update that call site.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn worker_connections_lands_inside_the_events_block() {
    // Scope matters: nginx rejects worker_connections anywhere else.
    let c = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"), &WebServerSettings::default())
        .unwrap()
        .contents;
    let events = c.find("events {").expect("events block");
    let http = c.find("http {").expect("http block");
    let wc = c.find("worker_connections").expect("worker_connections");
    assert!(wc > events && wc < http, "worker_connections is outside events");
}

#[test]
fn the_http_level_settings_are_all_rendered() {
    let c = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"), &WebServerSettings::default())
        .unwrap()
        .contents;
    assert!(c.contains("client_max_body_size 256m;"));
    assert!(c.contains("keepalive_timeout 65;"));
    assert!(c.contains("tcp_nodelay on;"));
    assert!(c.contains("fastcgi_connect_timeout 60;"));
    assert!(c.contains("fastcgi_send_timeout 300;"));
    assert!(c.contains("fastcgi_read_timeout 300;"));
    assert!(c.contains("gzip off;"));
}

#[test]
fn a_changed_setting_changes_the_output() {
    // Guards the whole point of the slice: if the template ignored the struct
    // and kept its literals, every other test here would still pass.
    let mut s = WebServerSettings::default();
    s.fastcgi_read_timeout = Seconds::parse(900).unwrap();
    let c = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"), &s)
        .unwrap()
        .contents;
    assert!(c.contains("fastcgi_read_timeout 900;"));
    assert!(!c.contains("fastcgi_read_timeout 300;"));
}

#[test]
fn gzip_directives_appear_only_when_gzip_is_on() {
    let off = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"), &WebServerSettings::default())
        .unwrap()
        .contents;
    assert!(off.contains("gzip off;"));
    assert!(!off.contains("gzip_types"), "no point listing types with gzip off");

    let mut s = WebServerSettings::default();
    s.gzip = OnOff::new(true);
    let on = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"), &s)
        .unwrap()
        .contents;
    assert!(on.contains("gzip on;"));
    assert!(on.contains("gzip_comp_level 1;"));
    assert!(on.contains("gzip_types text/plain"));
}

#[test]
fn generation_stays_deterministic() {
    let a = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"), &WebServerSettings::default())
        .unwrap();
    let b = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"), &WebServerSettings::default())
        .unwrap();
    assert_eq!(a.contents, b.contents);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-conf generate_main_config`
Expected: FAIL — the method takes one argument.

- [ ] **Step 3: Update the template**

In `crates/openvhost-conf/templates/nginx/main.conf.tera`, replace `events {}` with:

```
events {
    worker_connections {{ worker_connections }};
}
```

and add inside `http {`, after the `access_log` line:

```
    client_max_body_size {{ client_max_body_size }};
    keepalive_timeout {{ keepalive_timeout }};
    tcp_nodelay {{ tcp_nodelay }};
    fastcgi_connect_timeout {{ fastcgi_connect_timeout }};
    fastcgi_send_timeout {{ fastcgi_send_timeout }};
    fastcgi_read_timeout {{ fastcgi_read_timeout }};
    gzip {{ gzip }};
{{ gzip_extra }}
```

`gzip_extra` is composed in Rust — empty when gzip is off, otherwise the `gzip_comp_level` and
`gzip_types` lines. Composing it in Rust rather than with a Tera `{% if %}` keeps the pattern
this crate already uses for the platform branch: **decisions live in Rust, the template only
interpolates.** Omit `gzip_types` entirely when the list is empty rather than emitting a bare
directive, which nginx rejects.

- [ ] **Step 4: Update the adapter**

Change the trait method and `NginxAdapter`'s implementation to take `&WebServerSettings`,
insert each value into the Tera context, and update `WebServerAdapter::validate`, which calls
`generate_main_config` and must now pass `&WebServerSettings::default()` — validation is about
whether the *shape* is valid, not about the user's stored values.

- [ ] **Step 5: Run, including against real nginx**

```bash
cargo test -p openvhost-conf
cargo test -p openvhost-conf --test validate_live
```
Expected: PASS. Then extend `validate_live.rs` with a non-default case — 900s timeouts, gzip
on, a custom type list — because a value that parses but nginx rejects is exactly what this
layer exists to prevent.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/openvhost-conf
git commit -s -m "feat(conf): render the nginx settings into the main config"
```

---

## Task 4: Feed them through the apply pipeline

**Files:**
- Modify: `crates/openvhost-core/src/site/apply/mod.rs`
- Test: `crates/openvhost-core/src/site/apply/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `WebServerSettings` (Task 1); the changed `generate_main_config` (Task 3).
- Produces: `ApplyInput` gains `pub settings: WebServerSettings`. Every construction site must set it — `crates/openvhost-core/src/site/apply/tests_support.rs` and the desktop app's `apply_input`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn changing_a_setting_changes_exactly_the_main_config() {
    // The whole architecture in one assertion: settings feed the generator, so
    // the existing plan/diff/validate/rollback pipeline covers them with no
    // second path. If this ever needs more than one file, something has leaked.
    let base = input(vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
    let mut changed = base.clone();
    changed.settings.fastcgi_read_timeout =
        openvhost_conf::Seconds::parse(900).unwrap();

    let before = render_set(&base).unwrap();
    let after = render_set(&changed).unwrap();

    let differing: Vec<String> = before
        .iter()
        .zip(after.iter())
        .filter(|(a, b)| a.contents != b.contents)
        .map(|(a, _)| a.path.display().to_string())
        .collect();
    assert_eq!(differing.len(), 1, "got {differing:?}");
    assert!(differing[0].ends_with("nginx/nginx.conf"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core apply`
Expected: FAIL — `ApplyInput` has no `settings` field.

- [ ] **Step 3: Implement**

Add the field to `ApplyInput`, pass it to `generate_main_config` in `render_set`, and give
`tests_support`'s builders `WebServerSettings::default()`.

- [ ] **Step 4: Run and commit**

```bash
cargo test --workspace
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
git add crates/openvhost-core
git commit -s -m "feat(core): settings feed render_set, so the existing pipeline covers them"
```

---

## Task 5: The IPC surface, and two honest renames

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src/lib/ipc/bindings.ts` (regenerated)
- Modify: `apps/desktop/src/lib/ipc/index.ts`
- Modify: every frontend caller of the two renamed commands
- Test: `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src/lib/ipc/ipc.test.ts`

**Interfaces:**
- Consumes: `WebServerSettingsRepository` (Task 2); the changed `ApplyInput` (Task 4).
- Produces:
```rust
  /// `#[serde(rename_all = "camelCase")]` like every other DTO here, so the
  /// TypeScript side is `fastcgiReadTimeout`, `clientMaxBodySize` and so on —
  /// the names Task 6's tests use.
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
  #[tauri::command] pub async fn web_server_settings(...) -> Result<WebServerSettingsDto, IpcError>;
  #[tauri::command] pub async fn save_web_server_settings(input: WebServerSettingsDto, ...) -> Result<(), IpcError>;
```
  Renames: `plan_site_apply` → `plan_config_apply`, `apply_sites` → `apply_config`. DTOs and behaviour unchanged.
  TS: `webServerSettings()`, `saveWebServerSettings(input)`, `planConfigApply()`, `applyConfig()`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_bad_setting_reaches_the_ui_marked_on_its_own_field() {
    // The form marks one input; a flattened Core error would mark none.
    let e: IpcError = openvhost_conf::GzipLevel::parse(99).unwrap_err().into();
    match e {
        IpcError::Validation { field, .. } => assert_eq!(field, "gzip_comp_level"),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn a_malformed_gzip_type_names_the_offending_token() {
    let e: IpcError = openvhost_conf::GzipTypes::parse("text/html; } server {")
        .unwrap_err()
        .into();
    match e {
        IpcError::Validation { field, message } => {
            assert_eq!(field, "gzip_types");
            assert!(message.contains("text/html;"), "got {message}");
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn the_dto_round_trips_through_the_domain_type() {
    let dto = WebServerSettingsDto {
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
    };
    let domain: openvhost_conf::WebServerSettings = dto.clone().try_into().unwrap();
    let back = WebServerSettingsDto::from(domain);
    assert_eq!(back, dto);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-desktop web_server_settings`
Expected: FAIL — the DTO does not exist.

- [ ] **Step 3: Implement**

Add `impl From<ConfError> for IpcError` mapping `ConfError::InvalidField { field, value, reason }`
to `IpcError::Validation { field: field.to_string(), message: format!("{value:?} {reason}") }`
and everything else to `IpcError::Core`. **Check `commands.rs` first** — if a `ConfError`
conversion already exists, extend it rather than adding a second.

`TryFrom<WebServerSettingsDto> for WebServerSettings` runs every `parse`, so the boundary is one
place. `From<WebServerSettings> for WebServerSettingsDto` is the read direction.

`web_server_settings` reads through the repo. `save_web_server_settings` parses, saves, and
returns — it does **not** apply. Applying is the user's next, explicit step.

Then rename the two commands. `grep -rn "plan_site_apply\|apply_sites\|planSiteApply\|applySites" apps/desktop`
to find every caller, including the frontend stores.

- [ ] **Step 4: Regenerate bindings and update callers**

```bash
cargo test -p openvhost-desktop export_bindings
git diff --stat apps/desktop/src/lib/ipc/bindings.ts
```
No diff means the export test did not run. Update `index.ts` wrappers and every store that
called the old names.

- [ ] **Step 5: Run everything**

```bash
cargo test --workspace && pnpm -C apps/desktop test
```

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
git add apps/desktop
git commit -s -m "feat(desktop): read and save the nginx settings over IPC

Also renames plan_site_apply/apply_sites to plan_config_apply/apply_config:
the pipeline no longer covers only sites, and the old names would mislead
the next reader about what an apply includes."
```

---

## Task 6: The form

**Files:**
- Create: `apps/desktop/src/lib/websettings.svelte.ts` + `.test.ts`
- Create: `apps/desktop/src/lib/components/WebServerSettingsForm.svelte` + `.test.ts`
- Modify: `apps/desktop/src/routes/web-server/+page.svelte`
- Modify: `apps/desktop/src/lib/components/WebServerPanel.svelte`

**Interfaces:**
- Consumes: `webServerSettings()`, `saveWebServerSettings(dto)`, `planConfigApply()`, `applyConfig()`, `WebServerSettingsDto`.
- Produces:
```ts
  export class WebSettingsStore {
      values: WebServerSettingsDto | null;
      fieldErrors: Record<string, string>;
      error: string;
      saving: boolean;
      dirty: boolean;
      load(): Promise<void>;
      save(): Promise<boolean>;   // saves, then plans; true when the diff is ready
  }
```

- [ ] **Step 1: Write the failing store tests**

```ts
it('loads the stored values', async () => {
	const s = new WebSettingsStore(api({ values: dto({ fastcgiReadTimeout: 900 }) }));
	await s.load();
	expect(s.values?.fastcgiReadTimeout).toBe(900);
});

it('marks the offending field and leaves the others clean', async () => {
	// A whole-form error would make the user hunt for which input was wrong.
	const s = new WebSettingsStore({
		webServerSettings: async () => dto({}),
		saveWebServerSettings: async () => {
			throw { kind: 'validation', field: 'gzip_comp_level', message: '"99" must be between 1 and 9' };
		},
		planConfigApply: async () => ({ changes: [] })
	});
	await s.load();
	expect(await s.save()).toBe(false);
	expect(s.fieldErrors.gzip_comp_level).toContain('between 1 and 9');
	expect(s.fieldErrors.keepalive_timeout).toBeUndefined();
	expect(s.error).toBe('');
});

it('clears a field error once that field is saved successfully', async () => {
	let fail = true;
	const s = new WebSettingsStore({
		webServerSettings: async () => dto({}),
		saveWebServerSettings: async () => {
			if (fail) throw { kind: 'validation', field: 'gzip_comp_level', message: 'bad' };
		},
		planConfigApply: async () => ({ changes: [] })
	});
	await s.load();
	await s.save();
	expect(s.fieldErrors.gzip_comp_level).toBeDefined();
	fail = false;
	await s.save();
	expect(s.fieldErrors.gzip_comp_level).toBeUndefined();
});

it('refuses a second save while one is in flight', async () => {
	let calls = 0;
	const s = new WebSettingsStore({
		webServerSettings: async () => dto({}),
		saveWebServerSettings: async () => {
			calls += 1;
			await new Promise((r) => setTimeout(r, 5));
		},
		planConfigApply: async () => ({ changes: [] })
	});
	await s.load();
	await Promise.all([s.save(), s.save()]);
	expect(calls).toBe(1);
});

it('plans after saving so the diff reflects what was stored', async () => {
	// Planning from the form's local values instead would show a diff for
	// something that failed to save.
	const order: string[] = [];
	const s = new WebSettingsStore({
		webServerSettings: async () => dto({}),
		saveWebServerSettings: async () => {
			order.push('save');
		},
		planConfigApply: async () => {
			order.push('plan');
			return { changes: [] };
		}
	});
	await s.load();
	await s.save();
	expect(order).toEqual(['save', 'plan']);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -C apps/desktop test websettings`
Expected: FAIL — cannot resolve `./websettings.svelte`.

- [ ] **Step 3: Implement the store**

Follow `apps/desktop/src/lib/apply.svelte.ts` for shape. `IpcError`'s validation variant carries
`field` — route it into `fieldErrors` keyed by that field and leave `error` for everything else.
The re-entrancy guard lives in the store, not on a `disabled` attribute.

- [ ] **Step 4: Write the failing form tests**

```ts
it('renders every editable setting with its stored value', () => {
	const body = renderForm({ values: dto({ fastcgiReadTimeout: 900, gzip: true }) });
	expect(body).toContain('value="900"');
	expect(body).toContain('data-testid="field-gzip"');
});

it('marks only the field that failed', () => {
	const body = renderForm({ values: dto({}), fieldErrors: { gzip_comp_level: 'must be between 1 and 9' } });
	expect(body).toMatch(/data-testid="error-gzip_comp_level"/);
	expect(body).not.toMatch(/data-testid="error-keepalive_timeout"/);
});

it('shows the Phase 3 fields disabled with a reason rather than hiding them', () => {
	// A missing field reads as an oversight; a disabled one with a reason tells
	// the user the product knows.
	const body = renderForm({ values: dto({}) });
	expect(body).toContain('data-testid="field-http-port"');
	expect(body).toMatch(/data-testid="field-http-port"[^>]*disabled|disabled[^>]*data-testid="field-http-port"/);
	expect(body).toMatch(/privileged helper|Phase 3/i);
});

it('disables Save while a save is in flight', () => {
	expect(renderForm({ values: dto({}), saving: true })).toContain('disabled');
	expect(renderForm({ values: dto({}), saving: false })).not.toContain('disabled');
});
```

The `disabled` assertions in the last two tests can collide — the Phase 3 inputs are always
disabled, so a bare `toContain('disabled')` would pass regardless of `saving`. Scope the Save
assertion to the Save button's own testid.

- [ ] **Step 5: Run to verify failure**

Run: `pnpm -C apps/desktop test WebServerSettingsForm`
Expected: FAIL — component does not exist.

- [ ] **Step 6: Build the form and wire it in**

Group as ServBay does: connection limits, timeouts, compression. Booleans are switches, numbers
are `type="number"`, `gzip_types` is a textarea. Each field carries `data-testid="field-<name>"`
and renders `data-testid="error-<name>"` when `fieldErrors` has it.

Phase 3 fields (HTTP port, HTTPS port, SSL protocol, `ssl_prefer_server_ciphers`, HTTP/2,
HTTP/3) render disabled with one shared line explaining they need the privileged helper and
local CA.

Save calls `store.save()`, then opens `ApplyDialog` with the plan — reuse it rather than
growing a second diff renderer.

In `WebServerPanel.svelte`, drop "Read-only" from the subtitle; the page is no longer read-only.

- [ ] **Step 7: Run the frontend gate**

```bash
pnpm -C apps/desktop test
pnpm -C apps/desktop lint
pnpm -C apps/desktop exec svelte-check
```

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src
git commit -s -m "feat(ui): edit the nginx settings from the Web server page

Save stores the values, then shows the diff of nginx.conf and applies it
there — the same pipeline the Sites page uses, reached from where the
settings live. Port and SSL fields render disabled with their reason
rather than being omitted."
```

---

## Definition of Done

- [ ] Raising the FastCGI read timeout on the Web server page shows a one-file diff of `nginx.conf` and applies it there.
- [ ] The generated config carries every setting, at its default or otherwise, and passes `nginx -t` with non-default values.
- [ ] A bad value marks its own field and no other; a malformed gzip type names the offending token.
- [ ] `text/html; } server { listen 9999; root /; } http {` is refused.
- [ ] A fresh install reads the defaults and writes no row until the first save.
- [ ] A hand-edited out-of-range row is refused on read.
- [ ] Port and SSL fields are visible, disabled, and explain why.
- [ ] `plan_site_apply`/`apply_sites` no longer exist under those names.
- [ ] Full gate green: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop lint`, `pnpm -C apps/desktop exec svelte-check`.
