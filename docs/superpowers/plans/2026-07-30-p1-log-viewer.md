# Live Log Viewer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A site 500s → the user opens that site's logs from its row → the error is on screen, findable by filter even when it is older than the visible tail, and Follow shows the next request live.

**Architecture:** see the spec (`docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md`) — READ IT FIRST for every task; D1–D8 are binding. `openvhost-core/src/logs/` owns log paths + the pure bounded reader; templates gain per-site log directives and a query-string-free access format; the apply pipeline creates log dirs before validation; three query commands feed a new `/logs` page with grouped sources and deep links.

**Tech Stack:** Rust workspace + Tauri v2/SvelteKit, Tera, vitest SSR-only for Svelte.

## Global Constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By` trailer** (repo convention: attribution disabled).
- `openvhost-core` gains no tauri/specta dependency. No `unwrap`/`expect` outside tests. Atomic writes via `atomicfile::write_atomic` only.
- **The reader must never load a whole file** — this is asserted by a test against a large fixture, not merely intended.
- **No caller-supplied paths cross IPC, ever.** Typed source enum → newtype ingress → catalogue check → `LogPaths` derivation. `symlink_metadata` refusal of non-regular files; **no `canonicalize`**; `starts_with(<home>/logs)` post-condition.
- TDD with vacuity proof: every new test shown RED first (or neuter-proven if written against existing code); state the method per test group in your report.
- Svelte tests are SSR-only (`svelte/server` render, node project); interactive behavior goes on the manual click-list.
- Gates for every task: focused tests → whole crate/suite → `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` (+ `pnpm -C apps/desktop test` / `check` when desktop is touched).
- Mirror, don't invent: bounded reads follow `read_web_server_config`; confinement follows `site/apply/plan.rs`; command/DTO/event conventions follow the existing families; UI follows the Languages/Databases pages and `docs/design/log-viewer.html` (its flat tab strip is deliberately replaced — spec D6).
- Live/manual proof happens in Task 7 (controller-run), not inside unit tests.

---

## Phase A — core + conf (Tasks 1–3)

### Task 1: `LogPaths` — the single owner of every log path

**Files:** create `crates/openvhost-core/src/logs/mod.rs` (+ `paths.rs` if it reads better), register in `lib.rs`; rewire the existing hardcoded call sites.

**Interfaces (contract for Tasks 2–5):**
```rust
pub struct LogPaths { /* built from a home dir */ }
impl LogPaths {
    pub fn new(home: &Path) -> Self;
    pub fn nginx_error(&self) -> PathBuf;        // <home>/logs/nginx.error.log   (UNCHANGED value)
    pub fn nginx_access(&self) -> PathBuf;       // <home>/logs/nginx.access.log  (UNCHANGED value)
    pub fn php_fpm_error(&self, major: &PhpVersion) -> PathBuf; // <home>/logs/services/php-fpm-<major>/error.log
    pub fn site_dir(&self, domain: &Domain) -> PathBuf;         // <home>/logs/sites/<domain>/
    pub fn site_access(&self, domain: &Domain) -> PathBuf;
    pub fn site_error(&self, domain: &Domain) -> PathBuf;
    pub fn root(&self) -> PathBuf;               // <home>/logs — the confinement anchor
}
```
Path derivation from home + validated newtypes is this module's confinement argument — **state it in a doc comment** (Docroot lesson).

- [ ] **Step 1:** Find every hardcoded log path (`grep -rn "nginx.error.log\|nginx.access.log\|php-fpm.log" --include="*.rs" crates/ apps/` — expect ~19 hits across ~6 files) and list them in your report before changing anything.
- [ ] **Step 2:** Tests RED first: each accessor returns the expected path for a fixture home; `php_fpm_error` differs per major (the bug being fixed); `site_*` for two domains never collide; every accessor's output `starts_with(root())`.
- [ ] **Step 3:** Implement; rewire the call sites to `LogPaths` **with identical resulting values** — nginx's globals do NOT move (spec D1). Any call site you cannot rewire cleanly, leave and report why.
- [ ] **Step 4:** `cargo test -p openvhost-core` + workspace green (the existing nginx/apply/validate tests are the proof that values did not change); fmt/clippy clean.
- [ ] **Step 5:** Commit: `refactor(core): single owner for every log path`

### Task 2: the bounded reader

**Files:** `crates/openvhost-core/src/logs/read.rs` (+ tests in-module or `tests/`, mirroring crate style).

**Interfaces (contract for Task 5):**
```rust
pub struct LogCursor { /* opaque: file identity (inode/dev) + byte offset */ }
pub struct LogQuery { pub needle: Option<String>, pub case_sensitive: bool, pub min_level: Option<LogLevel> }
pub struct LogLimits { /* rows: 500, payload: 512 KiB, line: 16 KiB, scan: 16 MiB */ }
pub enum LogReset { Rotated, Truncated }
pub struct LogWindow {
    pub rows: Vec<LogRow>,          // { level, text }
    pub cursor: Option<LogCursor>,
    pub exists: bool,
    pub reset: Option<LogReset>,
    pub has_more: bool,
    pub size_bytes: u64,
    pub scanned_bytes: u64,
    pub truncated_lines: u32,
    pub scan_bound_reached: bool,
}
pub fn read_window(path: &Path, cursor: Option<LogCursor>, query: &LogQuery, limits: &LogLimits) -> Result<LogWindow, CoreError>;
pub fn classify_level(line: &str) -> LogLevel;  // the ONE classifier for file lines
```
Semantics per spec D3/D4: `cursor: None` → tail window, discard first partial line; forward reads from the offset; trailing line without `\n` neither returned nor counted; `len < cursor.len` or identity change → `reset` + fresh tail; missing file → `exists: false`, `Ok`, not an error; filter applied **during** the scan with the cursor advancing across non-matches, bounded by `scan` bytes (`scan_bound_reached` when hit). Refuse non-regular files via `symlink_metadata` (no `canonicalize`).

- [ ] **Step 1: Tests RED first** (tempfile fixtures; name the vacuity method per group):
  - tail window discards the leading partial line; forward read resumes exactly where it left off
  - a trailing line without `\n` is not returned, and is returned once the newline arrives
  - truncation → `reset: Truncated`; rename/replace (new inode) → `reset: Rotated`
  - missing file → `exists: false`, `Ok`
  - a symlink at the path → refused (`CoreError`), and the target is not read
  - over-long line → truncated at 16 KiB and counted in `truncated_lines`
  - **large-file bound:** build a fixture well over the scan bound and assert `scanned_bytes <= scan limit` and that the call is fast — the "never loads the whole file" guarantee, tested not intended
  - filtering: a match **older than the tail window** is found; non-matches advance the cursor; `scan_bound_reached` is set when the bound stops the scan; case-sensitive toggle behaves
  - `classify_level` on representative nginx/php lines (and a plain line → the neutral level)
- [ ] **Step 2:** RED → implement → GREEN; `cargo test -p openvhost-core`; fmt/clippy clean.
- [ ] **Step 3:** Commit: `feat(core): bounded log window reader with server-side filtering`

### Task 3: templates — per-site logs, per-major pool log, private access format

**Files:** `crates/openvhost-conf/templates/nginx/site.conf.tera`, `templates/nginx/main.conf.tera`, `templates/php-fpm/pool.conf.tera`, their context structs and golden tests in `crates/openvhost-conf/src/`.

Per spec D1/D5:
- site template gains `access_log "<site access path>";` and `error_log "<site error path>";` (paths fed in as plain values from the context, mirroring how the crate already receives paths — do **not** invert the dependency by importing core).
- `main.conf.tera` defines an explicit `log_format` built on `$uri` + method + protocol + status + bytes (**never `$request`, never `$args`**) and uses it for the global access log; site access logs use the same named format.
- pool template's `error_log` becomes the per-major path (fixing `phpruntime.rs:54`'s shared file).
- **Do NOT attempt the per-site PHP error log yet** — that is Task 7's live experiment (spec D1); this task leaves php error routing as-is.

- [ ] **Step 1:** Golden-file tests RED first: the rendered site conf contains both directives with the expected paths; the rendered main conf contains the `log_format` and **does not contain** `$request` or `$args` (assert absence explicitly — this is the privacy guarantee); the rendered pool conf's `error_log` differs between two majors.
- [ ] **Step 2:** Implement; `cargo test -p openvhost-conf` green (existing goldens will need updating — that diff IS the change, review it in your report).
- [ ] **Step 3:** Commit: `feat(conf): per-site nginx logs, per-major pool log, query-string-free access format`

## Phase B — apply + IPC + UI (Tasks 4–6)

### Task 4: apply pipeline creates log directories before validation

**Files:** `crates/openvhost-core/src/site/apply/{plan,commit}.rs` (+ their tests), `provision_home` in the macOS platform module.

Per spec D2: `ApplyPlan` gains `log_dirs: Vec<PathBuf>` derived **purely** in `plan()` (which must stay read-only — assert that); `commit()` `create_dir_all`s them **before** running validation, and chmods them `0700` (spec D5); `provision_home` seeds `logs/sites` and `logs/services` at `0700`.

- [ ] **Step 1:** Tests RED first: `plan()` populates `log_dirs` for enabled sites and creates nothing on disk; `commit()` creates them before validation (prove the ordering — e.g. a validator stub that asserts the dir exists when it runs, or a validation failure leaving the dirs already created); dirs are `0700`; re-apply is a no-op; rollback with dirs already created is harmless.
- [ ] **Step 2:** Implement; `cargo test -p openvhost-core` + workspace green.
- [ ] **Step 3:** Commit: `feat(core): apply creates site log directories before validation`

### Task 5: commands + bindings

**Files:** `apps/desktop/src-tauri/src/commands.rs` (+ `lib.rs` registration), bindings regen, `apps/desktop/src/lib/ipc/index.ts`.

Per spec D5/D7. Source enum crosses IPC as a tagged DTO; ingress parses `domain`/`major` into `Domain`/`PhpVersion`, **then checks the source against the live catalogue** (site exists in state.db / runtime installed) before deriving a path via `LogPaths`; the derived path gets the `starts_with(logs root)` post-condition. Commands: `list_log_sources`, `read_log_window`, `reveal_log_folder`. Ring sources are listed but read via the existing `service_log_tail`/`service-log` path (document the two-mechanism seam).

- [ ] **Step 1:** Command-harness tests RED first (the `tauri::test::mock_builder()` harness EXISTS — use it): unknown/deleted site → rejected without touching the filesystem; out-of-catalogue major → rejected; a symlink planted at a derived log path → refused; `list_log_sources` shape for a fixture home; `read_log_window` round-trips a cursor. Fake/tempdir fixtures only — no real nginx.
- [ ] **Step 2:** Implement; regenerate + commit bindings; add `ipc/index.ts` wrappers per convention.
- [ ] **Step 3:** `cargo test --workspace` + `pnpm -C apps/desktop test` + `check` green; fmt/clippy clean.
- [ ] **Step 4:** Commit: `feat(ipc): log source catalogue and bounded window reads`

### Task 6: the `/logs` page + deep links

**Files:** `apps/desktop/src/routes/logs/+page.svelte`, `apps/desktop/src/lib/logs.svelte.ts` (+ derive helpers and tests), `Rail.svelte` (activate the item), row actions in the Sites and Services surfaces, extracted row renderer shared with `LogPane.svelte`.

Per spec D6: grouped source selection (Services / Sites), **not** the mock's flat tabs (document the deviation); `?source=…` deep links from site rows and failed service rows; Follow on by default with scroll-away-disengages + "Jump to latest"; filter input (literal, ≤256 chars) + case toggle + level filter, all driving the **server-side** query; status line with file size, the >100 MiB warning, and a scan-bound notice; distinct states for empty / not-yet-created / permission-denied / rotated / unavailable / scan-bound. `LogPane` stays on Services scoped to the selected service, gains "Open in Logs"; if scoping is more than a small change, keep v0 and defer with a note. Poll teardown on route change/blur is a **tested** requirement. Contrast checked in both themes against tokens (standing lesson).

- [ ] **Step 1:** Store + derive tests RED first: cursor/append/reset handling (a `reset` clears rows rather than double-printing), follow toggling, filter round-trip through the api mock, poll teardown, deep-link param parsing, source grouping/labels.
- [ ] **Step 2:** SSR tests RED first: each rendered state; grouped picker; filter/case/level controls present; the privacy note; deep-link target rendering.
- [ ] **Step 3:** Implement; full desktop suite + `check` + lint green.
- [ ] **Step 4:** Commit: `feat(ui): logs page with grouped sources, filtering, and follow`

## Phase C — gates + live proof (Task 7)

- [ ] **Step 1:** Local gates: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && pnpm -C apps/desktop test && pnpm -C apps/desktop build`
- [ ] **Step 2: Live experiment — per-site PHP error log (spec D1).** With a real php-fpm running, test whether a per-request `PHP_ADMIN_VALUE[error_log]` FastCGI param from the site's nginx block actually redirects that site's PHP errors. If it works, add it (template + `LogPaths::site_php_error` + a source variant) as a small follow-up commit; if it does not, drop it and record the result in the spec. **Do not build it on assumption.**
- [ ] **Step 3: Live proof** (controller-run, the point of the slice): a site with a fatal `index.php` → Apply → `curl -i` 500 → the site error log shows the PHP fatal → filter finds a match older than the tail → Follow updates live → `?token=abc` request leaves **no query string** in the access log → `mv` the log while following resets cleanly → a multi-MB access log keeps memory/CPU flat. Record the output in the PR.
- [ ] **Step 4:** Whole-branch review (most capable model) with ledger Minor-triage + **security-auditor** (MANDATORY: new file-read IPC surface — confinement, catalogue check, symlink refusal, privacy format, log dir modes). Fix wave; re-run gates and the live proof after any fix that touches the read or template paths.
- [ ] **Step 5:** PR with test evidence, the click-list, and the two owner-call items; squash-merge on green.
