# Phase 1 · Sites CRUD (typed IPC + Sites panel & editor drawer) — design

**Status:** approved design, 2026-07-25. Owners: rust-core-engineer (IPC boundary) + tauri-frontend-engineer (UI).

## 1. Goal & context

Turn the shipped `state.db`/`Site` backend into a usable feature: four typed IPC commands over `SiteRepository`, and the Sites panel + editor drawer from the mockups. This is the visible payoff of the state.db slice and the gateway to the Phase 1 headline (per-site PHP version selection).

**What already exists.** `openvhost-core` (merged `1c79bf3`, hardened `89471df`): `Db` (sqlx `SqlitePool` on `<home>/state.db`), `site::model` (the opaque newtypes — `SiteId`/`SiteName`/`Domain`/`PhpVersion`/`Docroot` with private fields and `parse` as the only constructor, `WebServer` enum; sealed from `site::repo` by being siblings), and `site::repo` (`SiteRepository` trait — `create(NewSite)->Site`, `get(&SiteId)->Option<Site>`, `list()->Vec<Site>`, `update(&Site)->Site`, `delete(&SiteId)->bool`, all async RPITIT — plus `SqliteSiteRepository::new(&Db)`). The desktop app already opens and `app.manage`s `Db` at startup (best-effort). The IPC surface today is 5 commands (`core_info`, `list_services`, `start_service`, `stop_service`, `service_log_tail`) + 2 events; the UI is the designed shell (`AppShell`/`Rail`/`TitleBar`) with a live Services panel and Sites/Logs/Settings as inert placeholder rail items.

**Design sources:** `docs/design/main-window.html` (the Sites list section) and `docs/design/site-editor.html` (the editor drawer), with `docs/design/mock.css` supplying `.drawer`, `.drawer-backdrop`, `.drawer-head/body/foot`, `.field`, `.input`, `.input-group`, `.input-suffix`, `.seg`, `.danger-zone`, `.consequence`, `.btn-danger`, `.btn-icon`, `.hint`.

## 2. Approved decisions

- **Store-only CRUD.** Reads/writes `state.db` only. No config generation, no nginx reload — the apply/diff pipeline (which has its own mockup, `diff-preview.html`) is a separate slice. The drawer's primary button reads **"Save"**, not the mockup's "Save & restart nginx".
- **Native folder picker** for the docroot via `tauri-plugin-dialog` (new Rust dep + npm package + a Tauri capability grant). The picker returns an absolute path, which is what `Docroot::parse` requires — no tilde expansion needed and the user cannot mistype a path.
- **Per-field validation errors.** `IpcError` gains `Validation { field, message }` so the form marks the offending input instead of showing a generic banner.
- **Security-auditor APPROVE is a MERGE GATE for this slice.** CLAUDE.md golden rule 2 lists IPC endpoints / the Tauri command surface as merge-blocked without it, and this slice adds four commands *and* a new capability grant. (The state.db slice had no such gate because it touched no IPC.)

## 3. IPC surface (`apps/desktop/src-tauri/src/commands.rs`)

Four `async` commands, registered in `collect_commands!` in `lib.rs`. No new events (the store refetches after each mutation).

```
list_sites(db: State<'_, Db>)                          -> Result<Vec<SiteDto>, IpcError>
create_site(db: State<'_, Db>, input: SiteInput)       -> Result<SiteDto, IpcError>
update_site(db: State<'_, Db>, id: String, input: SiteInput) -> Result<SiteDto, IpcError>
delete_site(db: State<'_, Db>, id: String)             -> Result<bool, IpcError>
```

Each builds `SqliteSiteRepository::new(&db)` per call — cheap (clones a pool handle) and avoids a second managed type. **If `state.db` failed to open at startup**, `Db` is not managed, Tauri's `State` extraction fails, and the frontend's existing `normalizeError` turns that into an `IpcError` the panel renders in its error banner — consistent with the startup log ("Sites features disabled this run").

`delete_site` returns the repository's `bool` (`false` = the row was already gone); the store treats either outcome as success and refetches, so a stale list self-heals instead of erroring.

`update_site` takes the id **separately from the input** and re-reads the row first (`get(&id)?`, `None` → `IpcError::Core` "site not found"), then rebuilds a `Site` from the existing `id`/`created_at` plus the newly validated fields. The client therefore **cannot rewrite `id`, `created_at`, or `updated_at`** — those are server-owned.

## 4. DTOs and the boundary conversion (the security core)

`Site` is deliberately not serializable: its fields are opaque newtypes whose whole purpose is that they can only be built by `parse`. So the IPC boundary gets explicit DTOs in `commands.rs`:

- **`SiteDto`** (outbound) — `id, name, domain, docroot, web_server, php_version: String`, `enabled: bool`, `created_at, updated_at: i64`. Derives `Serialize` + `specta::Type`, `#[serde(rename_all = "camelCase")]` (matching `ServiceStatus`'s convention). Built by `From<Site>` reading each newtype's `as_str()`.
- **`SiteInput`** (inbound) — `name, domain, docroot, web_server, php_version: String`, `enabled: bool`. Derives `Deserialize` + `specta::Type`.

**`TryFrom<SiteInput> for NewSite` runs every field through its `parse`** — `SiteName::parse`, `Domain::parse`, `Docroot::parse`, `WebServer::parse`, `PhpVersion::parse` — so a hostile or malformed IPC payload cannot reach `state.db`, exactly as `TryFrom<SiteRow>` guards reads. This is the same discipline in a third place (ingress-from-DB, ingress-from-IPC, and the domain constructors themselves).

`IpcError` gains:

```rust
/// A domain value failed validation; `field` names the offending input.
#[error("{message}")]
Validation { field: String, message: String },
```

and `From<CoreError>` maps `CoreError::Validation { field, reason }` → `IpcError::Validation { field: field.to_string(), message: reason }`, leaving every other `CoreError` on the existing `Core { message }` path. Because the repository already maps a UNIQUE violation on `name`/`domain` to `CoreError::Validation { field }`, "that name is already taken" surfaces on the right input for free.

## 5. UI

**Route + navigation.** Add a `/sites` route (`routes/sites/+page.svelte`); the rail's Sites item becomes a live link with `aria-current` when active, while Logs/Settings stay inert placeholders. `/` remains the Services page (no churn); making Sites the default landing view is a trivial follow-up once it has parity.

**Components** (`apps/desktop/src/lib/components/`):
- `SitesPanel.svelte` — the page head ("Sites", a count sub-line, an "Add site" primary button) + the list, ported from `main-window.html`'s Sites section. Empty state: an intentional "No sites yet" panel with the Add-site affordance, never blank.
- `SiteListRow.svelte` — name + domain (mono), PHP version, web server, an Enabled/Disabled pill, and an **Edit** action. (Named to avoid colliding with the existing services `ServiceRow.svelte`.)
- `SiteDrawer.svelte` — the `site-editor.html` drawer: `aside role="dialog" aria-modal="true"` + backdrop, labelled by its heading, **focus moved to the first field on open and restored to the trigger on close, Esc closes, focus trapped while open**. Fields: Name; Domain (subdomain input + fixed `.localhost` suffix, with the mockup's hint); Project folder (mono input + **Browse**); Web server (segmented nginx/apache, `role="group"`, `aria-pressed`); PHP version (select); **Enabled** (a checkbox/switch — see §6, an addition to the mockup); plus the danger zone.
- `sites.svelte.ts` — a `SitesStore` (runes `$state`) holding `sites`, plus `load()`, `create()`, `update()`, `remove()`; it refetches via `list_sites` after each mutation and exposes the last per-field validation error for the form. Mirrors the existing `ServicesStore` shape.
- `sites.derive.ts` — pure helpers: `composeDomain(sub) => `${sub}.localhost``, `splitDomain(domain) => sub`, and `enabledPill(enabled)`. Unit-tested. `splitDomain` strips exactly one trailing `.localhost` if present and otherwise returns the string unchanged; a stored domain without that suffix is only reachable by hand-editing `state.db` (the UI always composes it), and such a row simply shows its full value in the subdomain input — acceptable for this slice rather than adding a second domain-entry mode.

**Domain composition** happens in the frontend (`composeDomain`), and the backend validates the *full* domain via `Domain::parse`. This keeps the IPC API general — a future custom-TLD slice needs no command change — while validation stays centralized in the domain layer.

**Delete** lives in the drawer's danger zone as a two-step confirm: "Delete site…" reveals "Really delete `<name>`?" with Cancel + a `btn-danger` confirm. The mockup's reassurance copy stays: the project files on disk are not touched.

**Folder picker.** `@tauri-apps/plugin-dialog`'s `open({ directory: true })` from the Browse button; the returned absolute path fills the field. A cancelled picker leaves the field unchanged. The Rust side registers `tauri_plugin_dialog::init()` and the capability file grants only the dialog-open permission needed — no blanket filesystem access. The exact permission identifier must be confirmed against the installed plugin version at implementation time (master-plan version caveat).

## 6. Deliberate deviations from the mockups

Each because the backing slice does not exist yet; shipping honest UI beats faking data.

| Mockup | This slice | Reason |
|---|---|---|
| "Save & restart nginx" | "Save" | apply/reload is a separate slice |
| PHP select annotated "(installed)" / "— install first" | plain select of majors (8.1–8.4) | needs package IPC (openvhost-pkg) |
| Row status pill "running" | **Enabled / Disabled** from `Site.enabled` | sites have no runtime state yet; `enabled` is real stored data |
| Row "Open" button | omitted | nothing is served yet — a button that 404s is worse than none |
| `~/www/myshop` | absolute path from the picker | `Docroot::parse` requires absolute |
| *(no enabled control)* | **adds an Enabled toggle** | `Site.enabled` is stored and drives the row pill; without a control the pill could never change and would be decorative |

## 7. Error handling & states

Every failure renders: per-field `Validation` errors mark the input (name/domain taken, bad path, bad version); other `IpcError`s show the panel's error banner (including the state.db-unavailable case). Empty list renders intentional empty content. The drawer disables Save while a mutation is in flight and keeps the user's input on failure (never silently discards a filled form).

## 8. Testing

- **Rust:** `SiteDto` round-trip from a `Site`; `TryFrom<SiteInput> for NewSite` accepts a valid payload and **rejects hostile input with the right `field`** — quote/space in name, quote/space/`..` in domain, relative or quote-bearing docroot, bad `php_version`, unknown `web_server`; `CoreError::Validation` → `IpcError::Validation{field}` mapping; `update_site` ignores client-supplied timestamps (server-owned).
- **Frontend (vitest):** `composeDomain`/`splitDomain` round-trip (including a subdomain that already ends in `.localhost`), `enabledPill`, and the store's refetch-after-mutation + validation-error surfacing.
- **Bindings:** `export_bindings` regenerates with the 4 new commands + the new `IpcError` variant — the diff is expected and reviewed (unlike prior slices where an unchanged diff was the check).
- **Visual:** list / empty / drawer-open / per-field-error states; keyboard-only pass through the drawer (open → tab through fields → Esc → focus restored).
- **Gates:** `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh`, plus `pnpm -C apps/desktop lint && check && test && build`. All offline (`.sqlx/` is committed).

## 9. Non-goals (own future slices)

Config generation / diff preview / nginx reload (the apply pipeline); starting or serving a site; hosts-file editing (this slice relies on `*.localhost` resolving without privileges); the installed-PHP-version list and the package-manager UI; per-site custom directives, env vars, or non-standard TLDs; site-level logs; the other four domain entities; making Sites the default landing view.

## 10. Delivery constraints

Branch `feat/p1-sites-crud` off `main`. SPDX line 1 on every new file (`//` for `.rs`/`.ts`, `<!-- -->` for `.svelte`). No `unwrap()`/`expect()` outside `#[cfg(test)]`. `openvhost-core` is NOT modified by this slice (the DTOs live in the app's `commands.rs`; core stays tauri-free). Typed bindings only — no stringly `invoke("…")`. New deps: `tauri-plugin-dialog` (Rust) + `@tauri-apps/plugin-dialog` (npm), both MIT/Apache — the license gate must pass. Svelte 5 runes, TypeScript strict, no `console.log`. DCO `git commit -s`, no `Co-Authored-By`, Conventional Commits. **Security-auditor APPROVE is a merge gate** (IPC command surface + new capability grant); the auditor must specifically rule on the boundary conversion (can any IPC payload reach `state.db` unvalidated?) and the capability scope (is the dialog grant minimal?). CI is disabled → local gates are the merge gate.
