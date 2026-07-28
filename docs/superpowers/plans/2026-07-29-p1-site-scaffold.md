# Site Scaffold on Create — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When creating a site with the new opt-in checkbox, create `<parent>/<name>/`, generate a styled placeholder `index.html` (unless an index entry point exists), store the joined path as the docroot, and surface the outcome as a dismissible notice — without ever letting scaffold failure fail the save.

**Architecture:** Pure scaffold logic + outcome enum in `openvhost-core` (`site/scaffold.rs`), reusing the hardened atomic-write helper extracted from the apply pipeline. The existing `create_site` Tauri command gains a `create_folder` parameter and returns `CreateSiteResult { site, scaffold }` (DB insert first, scaffold after). UI: checkbox + live path preview in `SiteDrawer` (create mode only), new `ScaffoldNoticeBanner` above `PendingChangesBanner`.

**Tech Stack:** Rust (std fs, no new deps), Tauri v2 + specta bindings, Svelte 5 runes + SSR-only vitest tests.

**Spec:** `docs/superpowers/specs/2026-07-29-p1-site-scaffold-design.md` — read it before starting any task.

## Global Constraints

- SPDX header (`// SPDX-License-Identifier: GPL-3.0-or-later`) on every new file; commit with `git commit -s`; Conventional Commits format.
- `openvhost-core` must not gain dependencies on tauri, specta, or tera. No `unwrap`/`expect` outside tests.
- Every NEW test must be demonstrated to fail first: run it before the implementation exists (RED), or if written against existing code, temporarily revert/neuter the code under test and watch it fail, then restore. State the vacuity check in your commit or report.
- Svelte component tests are SSR-only (`render` from `svelte/server`, node vitest project — see the header comment of `SiteDrawer.svelte.test.ts`). No jsdom. Interactive behavior goes on the manual click-list in the task report.
- Field-validation errors flow through the existing snake_case `fieldErrors` seam (`CoreError` validation with `field: "docroot"` → drawer's `fieldErrors.docroot`).
- No sqlx query or migration changes are expected in this slice; do not touch `.sqlx/`. If you believe you need a query change, STOP and report.
- If working in a fresh worktree: run `pnpm -C apps/desktop install --offline --frozen-lockfile` before any desktop build/test, or gates fail with a bogus "Cannot find package".
- Commands below assume repo root as cwd.

---

## Phase A — core (Tasks 1–3)

### Task 1: Extract the hardened atomic-write helper into `atomicfile.rs`

**Files:**
- Create: `crates/openvhost-core/src/atomicfile.rs`
- Modify: `crates/openvhost-core/src/site/apply/commit.rs` (the `atomic_write_with_suffix` / `atomic_write` bodies, currently around lines 40–105)
- Modify: `crates/openvhost-core/src/lib.rs` (register `mod atomicfile;`)

**Interfaces:**
- Consumes: nothing new.
- Produces (later tasks rely on these exact names):
  - `pub(crate) struct AtomicWriteError { pub op: &'static str, pub path: PathBuf, pub source: std::io::Error }`
  - `pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), AtomicWriteError>`
  - `pub(crate) fn write_atomic_with_suffix(path: &Path, contents: &str, suffix: &str) -> Result<(), AtomicWriteError>`

This is a **pure move + `map_err`** of security-audited code. Behavior must not change; the existing commit.rs tests (including the pre-planted-symlink test that pins the suffix) are the coverage and must keep exercising the moved body. No new tests; the vacuity rule does not apply to a pure refactor.

- [ ] **Step 1: Create `crates/openvhost-core/src/atomicfile.rs`**

Move the two functions from commit.rs verbatim (keep the full doc comments, including the same-directory-temp and `O_CREAT|O_EXCL` rationale), changing only the error type:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened atomic file write shared by the apply pipeline and the site
//! scaffold. Moved verbatim from `site/apply/commit.rs`; callers map
//! `AtomicWriteError` into their own error types.

use std::path::{Path, PathBuf};

/// A failed atomic write: which operation failed, on which path.
#[derive(Debug)]
pub(crate) struct AtomicWriteError {
    pub op: &'static str,
    pub path: PathBuf,
    pub source: std::io::Error,
}

pub(crate) fn write_atomic_with_suffix(
    path: &Path,
    contents: &str,
    suffix: &str,
) -> Result<(), AtomicWriteError> {
    // ... body moved verbatim from commit.rs::atomic_write_with_suffix,
    // with each `ApplyError::Io { op, path, source }` construction replaced
    // by `AtomicWriteError { op, path, source }`.
}

pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), AtomicWriteError> {
    write_atomic_with_suffix(path, contents, &uuid::Uuid::new_v4().simple().to_string())
}
```

- [ ] **Step 2: Shrink commit.rs to thin wrappers**

```rust
impl From<crate::atomicfile::AtomicWriteError> for ApplyError {
    fn from(e: crate::atomicfile::AtomicWriteError) -> Self {
        ApplyError::Io { op: e.op, path: e.path, source: e.source }
    }
}

fn atomic_write_with_suffix(path: &Path, contents: &str, suffix: &str) -> Result<(), ApplyError> {
    Ok(crate::atomicfile::write_atomic_with_suffix(path, contents, suffix)?)
}

fn atomic_write(path: &Path, contents: &str) -> Result<(), ApplyError> {
    Ok(crate::atomicfile::write_atomic(path, contents)?)
}
```

(Adapt the `From` placement to wherever `ApplyError` lives if commit.rs is not it. Keep the wrapper names so every existing call site and test compiles unchanged.)

- [ ] **Step 3: Register the module** in `crates/openvhost-core/src/lib.rs`: `mod atomicfile;` (crate-private).

- [ ] **Step 4: Verify green, including the symlink regression test**

Run: `cargo test -p openvhost-core`
Expected: PASS, same test count as before the move. Then confirm the pre-planted-symlink test still bites the moved code: temporarily change `write_atomic_with_suffix`'s `create_new(true)` to `create(true)`, re-run the commit.rs symlink test, watch it FAIL, revert. This proves the move did not orphan that coverage.

- [ ] **Step 5: Commit**

```bash
git add crates/openvhost-core/src/atomicfile.rs crates/openvhost-core/src/site/apply/commit.rs crates/openvhost-core/src/lib.rs
git commit -s -m "refactor(core): extract hardened atomic write into atomicfile module"
```

---

### Task 2: `ScaffoldOutcome` types + `scaffold_path`

**Files:**
- Create: `crates/openvhost-core/src/site/scaffold.rs` (types + `scaffold_path` only; `scaffold()` itself is Task 3)
- Modify: `crates/openvhost-core/src/site/mod.rs` (add `pub mod scaffold;` or re-export per the module's existing style)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Docroot`, `SiteName`, `CoreError` from `crate::site::model` (note `Docroot::parse`, `Docroot::as_str`, `SiteName::parse`; the `invalid(field, msg)` constructor pattern in model.rs).
- Produces (Tasks 3–4 rely on these exact shapes):

```rust
pub fn scaffold_path(parent: &Docroot, name: &SiteName) -> Result<Docroot, CoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScaffoldOutcome {
    Created,
    KeptExisting { existing: String },
    Failed { step: ScaffoldStep, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaffoldStep { CreateDir, Inspect, WritePlaceholder }
```

No serde/specta derives in core — the app layer mirrors these as DTOs (Task 4).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::site::model::{Docroot, SiteName};

    fn d(s: &str) -> Docroot { Docroot::parse(s).unwrap() }
    fn n(s: &str) -> SiteName { SiteName::parse(s).unwrap() }

    #[test]
    fn scaffold_path_joins_parent_and_name() {
        assert_eq!(scaffold_path(&d("/Users/x/Downloads"), &n("my-site")).unwrap().as_str(),
            "/Users/x/Downloads/my-site");
    }

    #[test]
    fn scaffold_path_normalizes_trailing_slash() {
        assert_eq!(scaffold_path(&d("/Users/x/Downloads/"), &n("my-site")).unwrap().as_str(),
            "/Users/x/Downloads/my-site");
    }

    #[test]
    fn scaffold_path_handles_root_parent() {
        assert_eq!(scaffold_path(&d("/"), &n("a")).unwrap().as_str(), "/a");
    }

    #[test]
    fn scaffold_path_rejects_over_length_join() {
        // A parent that is itself valid but whose join with the name exceeds
        // DOCROOT_MAX_LEN must fail as a docroot validation error, before
        // anything touches the filesystem. Read DOCROOT_MAX_LEN from model.rs
        // and size the parent so parent + "/" + name is one byte over.
        let parent = format!("/{}", "a".repeat(/* DOCROOT_MAX_LEN - 63 - 1 */ 0));
        let err = scaffold_path(&d(&parent), &n(&"b".repeat(63))).unwrap_err();
        // Assert the error is the validation kind with field "docroot",
        // matching how model.rs tests assert on CoreError.
    }
}
```

Fill in the over-length arithmetic and the `CoreError` assertion by reading `model.rs`'s own tests for the established pattern — do not invent a new assertion style.

- [ ] **Step 2: Run to verify RED** — `cargo test -p openvhost-core scaffold` → FAIL (module/function missing).

- [ ] **Step 3: Implement**

```rust
/// Pure join of the picked parent folder and the site name, re-validated as a
/// `Docroot` so the over-length case fails before anything is created.
pub fn scaffold_path(parent: &Docroot, name: &SiteName) -> Result<Docroot, CoreError> {
    let joined = format!("{}/{}", parent.as_str().trim_end_matches('/'), name.as_str());
    Docroot::parse(&joined)
}
```

(`"/"` trims to `""`, so root joins to `"/a"` — covered by the test.)

- [ ] **Step 4: Run to verify GREEN** — `cargo test -p openvhost-core scaffold` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/openvhost-core/src/site/scaffold.rs crates/openvhost-core/src/site/mod.rs
git commit -s -m "feat(core): scaffold outcome types and docroot join for site scaffold"
```

---

### Task 3: `scaffold()` + placeholder page

**Files:**
- Modify: `crates/openvhost-core/src/site/scaffold.rs`
- Create: `crates/openvhost-core/src/site/placeholder.html`
- Test: `scaffold.rs` `#[cfg(test)]` (tempdir-based; commit.rs's tests show the crate's temp-dir convention — reuse it)

**Interfaces:**
- Consumes: Task 1's `crate::atomicfile::write_atomic`; Task 2's types; `Domain::as_str` from model.
- Produces: `pub fn scaffold(docroot: &Docroot, name: &SiteName, domain: &Domain) -> ScaffoldOutcome` — **returns the enum, never `Result`** (spec D1: type-level guarantee that scaffold failure cannot fail the save).

- [ ] **Step 1: Write failing tests** (names are the contract; bodies follow the crate's tempdir style):

```rust
#[test] fn scaffold_creates_dir_and_placeholder() {
    // fresh tempdir parent; docroot = parent/my-site via scaffold_path
    // → ScaffoldOutcome::Created
    // → dir exists; index.html exists and contains the site name, the domain,
    //   and the docroot path
}
#[test] fn scaffold_second_run_keeps_existing() {
    // run scaffold twice → second returns KeptExisting { existing: "index.html" }
    // and the file's mtime/content is untouched
}
#[test] fn scaffold_keeps_existing_index_php() {
    // pre-create dir with index.php → KeptExisting { existing: "index.php" },
    // and NO index.html is written
}
#[test] fn scaffold_keeps_existing_uppercase_index() {
    // pre-create dir with INDEX.HTML → KeptExisting (works on case-sensitive
    // volumes too, where a naive exists() check would miss it)
}
#[test] fn scaffold_ignores_directory_named_index() {
    // pre-create dir containing a SUBDIRECTORY named "index" → Created
    // (only non-directory entries block generation)
}
#[test] fn scaffold_fails_when_parent_missing() {
    // docroot under a nonexistent parent → Failed { step: CreateDir, .. }
    // (create_dir, not create_dir_all: the missing parent is the honest
    // TOCTOU signal, never silently materialized)
}
#[test] fn scaffold_fails_when_target_is_a_file() {
    // plain file at the docroot path → Failed { step: CreateDir, reason }
    // with reason containing "not a folder"
}
#[test] fn scaffold_fails_when_target_is_a_symlink() {
    // (unix) symlink at the docroot path — even one pointing at a real dir —
    // → Failed { step: CreateDir, .. }: lstat, no follow
}
#[test] fn placeholder_html_escapes_interpolations() {
    // docroot containing `<b>&'x` (all legal per Docroot::parse — only " $ and
    // control bytes are rejected) → rendered file contains &lt;b&gt;&amp;&#39;x
    // and does NOT contain the raw `<b>`
}
```

- [ ] **Step 2: RED** — `cargo test -p openvhost-core scaffold` → new tests FAIL.

- [ ] **Step 3: Implement**

```rust
const PLACEHOLDER_HTML: &str = include_str!("placeholder.html");

/// Create the docroot folder and starter page. Infallible by design: every
/// failure is data (`ScaffoldOutcome::Failed`), because the caller has already
/// persisted the site row and must not roll it back over a filesystem problem.
pub fn scaffold(docroot: &Docroot, name: &SiteName, domain: &Domain) -> ScaffoldOutcome {
    let dir = docroot.as_path();
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // lstat, no follow: a file or symlink squatting on the docroot is
            // refused rather than written into.
            match std::fs::symlink_metadata(dir) {
                Ok(md) if md.is_dir() => {}
                Ok(_) => {
                    return ScaffoldOutcome::Failed {
                        step: ScaffoldStep::CreateDir,
                        reason: format!("{} already exists and is not a folder", dir.display()),
                    }
                }
                Err(e) => {
                    return ScaffoldOutcome::Failed {
                        step: ScaffoldStep::CreateDir,
                        reason: format!("{}: {e}", dir.display()),
                    }
                }
            }
        }
        Err(e) => {
            return ScaffoldOutcome::Failed {
                step: ScaffoldStep::CreateDir,
                reason: format!("{}: {e}", dir.display()),
            }
        }
    }

    match existing_index(dir) {
        Ok(Some(existing)) => return ScaffoldOutcome::KeptExisting { existing },
        Ok(None) => {}
        Err(e) => {
            return ScaffoldOutcome::Failed {
                step: ScaffoldStep::Inspect,
                reason: format!("{}: {e}", dir.display()),
            }
        }
    }

    let html = render_placeholder(name, domain, docroot);
    match crate::atomicfile::write_atomic(&dir.join("index.html"), &html) {
        Ok(()) => ScaffoldOutcome::Created,
        Err(e) => ScaffoldOutcome::Failed {
            step: ScaffoldStep::WritePlaceholder,
            reason: format!("{}: {}", e.path.display(), e.source),
        },
    }
}

/// Any non-directory entry whose file stem is `index` (ASCII case-insensitive)
/// blocks generation: covers index.html / index.htm / index.php / INDEX.HTML,
/// identically on case-insensitive (APFS default) and case-sensitive volumes.
fn existing_index(dir: &std::path::Path) -> std::io::Result<Option<String>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            continue;
        }
        let fname = entry.file_name();
        let Some(fname) = fname.to_str() else { continue };
        if std::path::Path::new(fname)
            .file_stem()
            .is_some_and(|s| s.eq_ignore_ascii_case("index"))
        {
            return Ok(Some(fname.to_string()));
        }
    }
    Ok(None)
}

/// Escaping lives HERE, unconditionally, for all three values — the newtypes
/// are charset guards, not encoders, and Docroot legally contains & < > '.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn render_placeholder(name: &SiteName, domain: &Domain, docroot: &Docroot) -> String {
    PLACEHOLDER_HTML
        .replace("{{name}}", &html_escape(name.as_str()))
        .replace("{{domain}}", &html_escape(domain.as_str()))
        .replace("{{docroot}}", &html_escape(docroot.as_str()))
}
```

- [ ] **Step 4: Create `placeholder.html`**

Self-contained, no external assets (the machine may be offline), no JS, both themes, system font stack. **No "GENERATED — DO NOT EDIT" banner — the user is meant to edit this file.** Use this content:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{name}} — ready</title>
<style>
  :root {
    --bg: #f6f4ef; --card: #fffdf9; --ink: #1f1d1a; --muted: #6f6a61;
    --accent: #1a7f5a; --edge: #e4e0d6; --mono-bg: #efece4;
  }
  @media (prefers-color-scheme: dark) {
    :root {
      --bg: #171614; --card: #201f1c; --ink: #ece9e2; --muted: #9b968b;
      --accent: #3fbf8c; --edge: #33312c; --mono-bg: #2a2925;
    }
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh; display: grid; place-items: center;
    background: var(--bg); color: var(--ink);
    font: 16px/1.6 ui-sans-serif, -apple-system, "Segoe UI", sans-serif;
  }
  main {
    background: var(--card); border: 1px solid var(--edge); border-radius: 14px;
    padding: 2.5rem 2.75rem; max-width: 34rem; margin: 2rem;
    box-shadow: 0 20px 50px -30px rgb(0 0 0 / .35);
  }
  .ok {
    display: inline-flex; align-items: center; gap: .5ch;
    color: var(--accent); font-weight: 600; font-size: .8rem;
    letter-spacing: .08em; text-transform: uppercase;
  }
  .ok::before { content: "●"; font-size: .7em; }
  h1 { margin: .5rem 0 .25rem; font-size: 2rem; letter-spacing: -.02em; }
  .domain { color: var(--muted); margin: 0 0 1.5rem; }
  code {
    font: .82em ui-monospace, "SF Mono", Menlo, monospace;
    background: var(--mono-bg); border-radius: 5px; padding: .15em .45em;
    overflow-wrap: anywhere;
  }
  ol { margin: 1.25rem 0 0; padding-left: 1.25rem; color: var(--muted); }
  ol li { margin: .35rem 0; }
  ol li::marker { color: var(--accent); font-weight: 600; }
  footer { margin-top: 2rem; font-size: .78rem; color: var(--muted); }
</style>
</head>
<body>
<main>
  <span class="ok">Site is serving</span>
  <h1>{{name}}</h1>
  <p class="domain">{{domain}}</p>
  <p>This starter page was generated by OpenVHost in<br><code>{{docroot}}</code></p>
  <ol>
    <li>Replace this <code>index.html</code> with your own entry point.</li>
    <li>Or drop your project files into the folder above.</li>
    <li>PHP files work here too — <code>index.php</code> takes precedence.</li>
  </ol>
  <footer>OpenVHost · local development server</footer>
</main>
</body>
</html>
```

- [ ] **Step 5: GREEN** — `cargo test -p openvhost-core scaffold` → PASS. Also run `cargo test -p openvhost-core` (whole crate still green).

- [ ] **Step 6: Commit**

```bash
git add crates/openvhost-core/src/site/scaffold.rs crates/openvhost-core/src/site/placeholder.html
git commit -s -m "feat(core): scaffold site folder with escaped placeholder page"
```

---

## Phase B — IPC + UI (Tasks 4–6)

### Task 4: `create_site` gains `create_folder` + `CreateSiteResult`

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs` (`create_site` at ~line 397; DTO section wherever `SiteDto` lives)
- Regenerate: the tauri-specta TS bindings (find the repo's mechanism — grep `export` / `collect_commands` in `src-tauri/src`; typically regenerated by a dev build or a dedicated test)
- Modify: `apps/desktop/src/lib/ipc/index.ts` (`createSite` wrapper at ~line 142; re-export the new types)
- Modify: `apps/desktop/src/lib/sites.svelte.ts` (`SitesApi.createSite` signature at line 9; the `save` path around line 79; new `lastScaffold` state)
- Test: `apps/desktop/src/lib/sites.svelte.test.ts` (mocks + new cases)

**Interfaces:**
- Consumes (from Tasks 2–3): `openvhost_core::site::scaffold::{scaffold, scaffold_path, ScaffoldOutcome, ScaffoldStep}` (adjust path to the actual re-export).
- Produces (Tasks 5–6 rely on these):
  - Rust: `create_site(db, input: SiteInput, create_folder: bool) -> Result<CreateSiteResult, IpcError>`
  - TS (generated): `CreateSiteResult = { site: SiteDto; scaffold: ScaffoldOutcomeDto | null }`, `ScaffoldOutcomeDto = { kind: "created" } | { kind: "keptExisting"; existing: string } | { kind: "failed"; step: ScaffoldStepDto; reason: string }`, `ScaffoldStepDto = "createDir" | "inspect" | "writePlaceholder"`
  - TS wrapper: `createSite(input: SiteInput, createFolder: boolean): Promise<CreateSiteResult>`
  - Store: `SitesStore.save(id: string | null, input: SiteInput, createFolder: boolean): Promise<boolean>`; `SitesStore.lastScaffold: { siteName: string; docroot: string; outcome: ScaffoldOutcomeDto } | null`; `SitesStore.dismissScaffold(): void`

- [ ] **Step 1: DTOs in commands.rs** (mirror seam, same as `Site` → `SiteDto`):

```rust
/// Scaffold outcome crossing IPC. Mirrors `openvhost_core`'s enum — the core
/// crate stays serde/specta-free. `kind` is the discriminator the UI switches
/// on exhaustively.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScaffoldOutcomeDto {
    Created,
    KeptExisting { existing: String },
    Failed { step: ScaffoldStepDto, reason: String },
}

#[derive(Debug, Clone, Copy, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ScaffoldStepDto { CreateDir, Inspect, WritePlaceholder }

impl From<ScaffoldOutcome> for ScaffoldOutcomeDto { /* mechanical match */ }
impl From<ScaffoldStep> for ScaffoldStepDto { /* mechanical match */ }

/// `scaffold: None` means "not requested" — it is NOT a fourth outcome.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateSiteResult {
    pub site: SiteDto,
    pub scaffold: Option<ScaffoldOutcomeDto>,
}
```

- [ ] **Step 2: The command.** Order is load-bearing (spec D2): ingress guard → join → **insert** → **then** scaffold. A UNIQUE violation must leave no folder behind.

```rust
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
```

If `NewSite`'s fields are not assignable from the app crate, adapt (rebuild the struct, or join before `try_into` — but the joined path MUST pass through `Docroot::parse`); the requirement is the ingress-validated join, not the exact mutation style. Check whether commands.rs has an existing command-test harness (`grep -n "#\[cfg(test)\]" apps/desktop/src-tauri/src/commands.rs`); if one exists, add a case asserting insert-before-scaffold ordering (UNIQUE violation → no folder created) and joined-docroot storage; if none exists, do NOT invent a harness — record the gap in your task report for the review phase.

- [ ] **Step 3: Regenerate bindings; update the `ipc/index.ts` wrapper:**

```ts
export async function createSite(
	input: SiteInput,
	createFolder: boolean
): Promise<CreateSiteResult> {
	return unwrap(commands.createSite(input, createFolder));
}
```

Re-export `CreateSiteResult`, `ScaffoldOutcomeDto` (and step type) alongside the existing type re-exports so UI code imports from `$lib/ipc`, matching every other type's import path.

- [ ] **Step 4: Store.** In `sites.svelte.ts`: update `SitesApi.createSite(input, createFolder): Promise<CreateSiteResult>`; add state + dismissal:

```ts
/** Outcome of the most recent create-with-folder, for the notice banner.
 *  null = nothing to show (never requested, or dismissed). */
lastScaffold = $state<{ siteName: string; docroot: string; outcome: ScaffoldOutcomeDto } | null>(null);

dismissScaffold(): void {
	this.lastScaffold = null;
}
```

In the `save` path: `save(id, input, createFolder)`; on the create branch, call `this.api.createSite(input, createFolder)` and set `lastScaffold` from the result (`siteName: input.name`, `docroot: result.site.docroot`, only when `result.scaffold !== null`); clear it in the existing `reset()`. Edit branch passes nothing new.

- [ ] **Step 5: Tests (RED first).** Update the `SitesApi` mocks in `sites.svelte.test.ts` to the new signature/return shape (mechanical). New cases:

```ts
it('passes createFolder through to the api and stores the scaffold outcome', …);
it('leaves lastScaffold null when scaffold was not requested', …);
it('dismissScaffold clears the notice', …);
it('reset clears a stale scaffold notice', …);
```

Write them against the not-yet-updated store where possible (RED), or neuter the new store lines and watch them fail, then restore.

- [ ] **Step 6: GREEN + build.**

Run: `pnpm -C apps/desktop test` → PASS. `cargo build -p openvhost-desktop` (or the src-tauri package name) → compiles; bindings diff committed.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src-tauri apps/desktop/src/lib/ipc apps/desktop/src/lib/sites.svelte.ts apps/desktop/src/lib/sites.svelte.test.ts
git commit -s -m "feat(ipc): create_site optionally scaffolds the site folder"
```

---

### Task 5: Drawer checkbox + live preview

**Files:**
- Modify: `apps/desktop/src/lib/sites.derive.ts` (+ its test file)
- Modify: `apps/desktop/src/lib/components/SiteDrawer.svelte` (Project-folder field ~lines 496–516; `rootDescribedBy` ~line 352; `onSave` prop type; state block ~line 130)
- Modify: `apps/desktop/src/routes/+page.svelte` (the `onSave` handler, ~line 115 — now forwards `createFolder` to `store.save`)
- Test: `apps/desktop/src/lib/components/SiteDrawer.svelte.test.ts`

**Interfaces:**
- Consumes: `SitesStore.save(id, input, createFolder)` from Task 4.
- Produces: `scaffoldPreview(parent: string, name: string): string | null` in `$lib/sites.derive`; `SiteDrawer`'s `onSave: (id: string | null, input: SiteInput, createFolder: boolean) => Promise<boolean>`.

- [ ] **Step 1: `scaffoldPreview` — failing tests first** in the derive test file:

```ts
describe('scaffoldPreview', () => {
	it('joins parent and name', () =>
		expect(scaffoldPreview('/Users/x/Downloads', 'my-site')).toBe('/Users/x/Downloads/my-site'));
	it('normalizes trailing slashes', () =>
		expect(scaffoldPreview('/Users/x/Downloads//', 'my-site')).toBe('/Users/x/Downloads/my-site'));
	it('handles the root parent', () => expect(scaffoldPreview('/', 'a')).toBe('/a'));
	it('returns null while name is empty', () => expect(scaffoldPreview('/x', '')).toBeNull());
	it('returns null while parent is blank', () => expect(scaffoldPreview('  ', 'a')).toBeNull());
});
```

- [ ] **Step 2: RED** — `pnpm -C apps/desktop test sites.derive` → FAIL.

- [ ] **Step 3: Implement** (normalization MUST match Rust's `trim_end_matches('/')` — spec D7 accepts the TS/Rust duplication as a typing affordance; truth is the returned `SiteDto.docroot`):

```ts
/** Live preview of the docroot the create-folder checkbox will produce.
 *  null = not previewable yet (blank parent or no name typed). */
export function scaffoldPreview(parent: string, name: string): string | null {
	if (parent.trim() === '' || name === '') return null;
	return `${parent.replace(/\/+$/, '')}/${name}`;
}
```

- [ ] **Step 4: GREEN** — `pnpm -C apps/desktop test sites.derive` → PASS.

- [ ] **Step 5: Drawer.** In `SiteDrawer.svelte`:
  - State: `let createFolder = $state(false);` (default unchecked — spec, decided).
  - In the Project-folder `.field`, after the `.input-group` and BEFORE the existing error paragraphs, create-mode only:

```svelte
{#if site === null}
	<label class="check" for="f-root-create">
		<input id="f-root-create" type="checkbox" bind:checked={createFolder} />
		Create a site folder inside this folder
	</label>
	{#if createFolder}
		<p class="hint mono" id="f-root-preview">
			{scaffoldPreview(docroot, name) ?? 'Enter a name to see the final path'}
		</p>
	{/if}
{/if}
```

  Match the drawer's existing class/markup conventions for checkboxes if one exists (grep for `type="checkbox"` first); otherwise style `.check` consistently with the drawer's field styling. Append `'f-root-preview'` **last** in the `rootDescribedBy` computation (line ~352), only when it renders — error ids stay first, same ordering rule the name field uses.
  - `onSave` prop type gains the third parameter; the drawer's save handler passes `createFolder` (state is unreachable/false in edit mode since the control never renders — pass it unconditionally).
  - `+page.svelte`: forward to `sitesStore.save(id, input, createFolder)`.

- [ ] **Step 6: SSR tests (RED first — write, run, watch fail, then wire).** In `SiteDrawer.svelte.test.ts`:

```ts
it('create mode renders the create-folder checkbox unchecked', () => {
	// render with site: null → body contains id="f-root-create", no `checked`
});
it('edit mode renders no create-folder control at all', () => {
	// render with a SiteDto → body does not contain "f-root-create"
});
```

SSR cannot toggle the checkbox, so the preview line and the `aria-describedby` join while checked are NOT assertable here (initial state is unchecked). The preview LOGIC is covered by the derive tests above; list the interactive half (check box → preview appears and updates live; describedby order) in your task report's manual click-list — same convention as the file's "WHAT THIS FILE CANNOT COVER" header.

- [ ] **Step 7: GREEN** — `pnpm -C apps/desktop test SiteDrawer` → PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src/lib/sites.derive.ts apps/desktop/src/lib/sites.derive.test.ts apps/desktop/src/lib/components/SiteDrawer.svelte apps/desktop/src/lib/components/SiteDrawer.svelte.test.ts apps/desktop/src/routes/+page.svelte
git commit -s -m "feat(ui): create-folder checkbox with live path preview in site drawer"
```

(Adjust the derive test filename to the actual one on disk.)

---

### Task 6: `ScaffoldNoticeBanner`

**Files:**
- Modify: `apps/desktop/src/lib/sites.derive.ts` (+ test) — the copy helper
- Create: `apps/desktop/src/lib/components/ScaffoldNoticeBanner.svelte`
- Create: `apps/desktop/src/lib/components/ScaffoldNoticeBanner.svelte.test.ts`
- Modify: `apps/desktop/src/routes/+page.svelte` (render above `<PendingChangesBanner>`)

**Interfaces:**
- Consumes: `SitesStore.lastScaffold` / `dismissScaffold()` from Task 4; `ScaffoldOutcomeDto` from `$lib/ipc`.
- Produces: `scaffoldNotice(siteName, docroot, outcome)` helper; banner component with props `{ siteName: string; docroot: string; outcome: ScaffoldOutcomeDto; onDismiss: () => void }`.

- [ ] **Step 1: Copy helper — failing tests first.** The exhaustiveness lives HERE, in TS, where the compiler can enforce it (spec D7):

```ts
export type ScaffoldNotice = { tone: 'ok' | 'warn'; role: 'status' | 'alert'; text: string };

/** Exhaustive over ScaffoldOutcomeDto — NO default arm, so a fourth variant
 *  fails typecheck instead of rendering nothing. */
export function scaffoldNotice(
	siteName: string,
	docroot: string,
	outcome: ScaffoldOutcomeDto
): ScaffoldNotice {
	switch (outcome.kind) {
		case 'created':
			return {
				tone: 'ok', role: 'status',
				text: `Folder ready — added a starter page at ${docroot}/index.html.`
			};
		case 'keptExisting':
			return {
				tone: 'ok', role: 'status',
				text: `Folder ready — using your existing ${outcome.existing} in ${docroot}.`
			};
		case 'failed':
			return {
				tone: 'warn', role: 'alert',
				text: `${siteName} was saved, but its folder couldn't be set up: ${outcome.reason}`
			};
		default: {
			const unreachable: never = outcome;
			return unreachable;
		}
	}
}
```

(The `never`-typed default is the standard exhaustiveness idiom — it is a compile-time arm, not a runtime path; if the codebase has an established `assertNever` helper, use that instead.) Tests: one per variant asserting tone/role/text substrings, RED before the helper exists.

- [ ] **Step 2: Banner component.** Dismissible; render from the helper only (no copy in the template):

```svelte
<script lang="ts">
	import type { ScaffoldOutcomeDto } from '$lib/ipc';
	import { scaffoldNotice } from '$lib/sites.derive';
	let { siteName, docroot, outcome, onDismiss }: {
		siteName: string; docroot: string; outcome: ScaffoldOutcomeDto; onDismiss: () => void;
	} = $props();
	const notice = $derived(scaffoldNotice(siteName, docroot, outcome));
</script>

<div class="scaffold-notice" data-tone={notice.tone} role={notice.role}>
	<p>{notice.text}</p>
	<button type="button" onclick={onDismiss}>Dismiss</button>
</div>
```

Style to match the sibling banners (read `PendingChangesBanner.svelte` and reuse its spacing/typography; `ok` tone uses the app's success/neutral tokens, `warn` the warning tokens — NOT the fail-red of `ErrorBanner`, the site did save). Check contrast in both themes against the tokens (the status-bar slice shipped a WCAG failure; don't repeat it).

- [ ] **Step 3: Wire into `+page.svelte`**, directly ABOVE `<PendingChangesBanner …>`:

```svelte
{#if sitesStore.lastScaffold}
	<ScaffoldNoticeBanner
		siteName={sitesStore.lastScaffold.siteName}
		docroot={sitesStore.lastScaffold.docroot}
		outcome={sitesStore.lastScaffold.outcome}
		onDismiss={() => sitesStore.dismissScaffold()}
	/>
{/if}
```

- [ ] **Step 4: SSR tests (RED via neutering — component is new but wire order matters):**

```ts
it('created outcome renders role="status" with the starter-page path', …);
it('keptExisting outcome names the file it kept', …);
it('failed outcome renders role="alert" and the reason', …);
it('renders a dismiss button', …);
```

Run each against a deliberately broken copy of the helper call (e.g. hardcode tone) once to prove they bite, then restore.

- [ ] **Step 5: GREEN + full desktop suite** — `pnpm -C apps/desktop test` → PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/lib/sites.derive.ts apps/desktop/src/lib/sites.derive.test.ts apps/desktop/src/lib/components/ScaffoldNoticeBanner.svelte apps/desktop/src/lib/components/ScaffoldNoticeBanner.svelte.test.ts apps/desktop/src/routes/+page.svelte
git commit -s -m "feat(ui): scaffold outcome notice above the pending-changes banner"
```

---

## Phase C — gates (Task 7)

### Task 7: Full gates, PR, merge

Run by the orchestrator with review subagents; listed here so the plan is complete.

- [ ] **Step 1: Local gates** (all must pass; CI is disabled on GitHub — these ARE the merge gate):

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings
cargo test --workspace
pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

- [ ] **Step 2: Whole-branch review** — rust-reviewer + typescript-reviewer over the full branch diff (not per-task: the assembled product is what ships), plus qa-test-engineer if coverage gaps were reported.
- [ ] **Step 3: security-auditor** over the branch (mandatory: Tauri command surface changed). BLOCK findings are fixed and re-audited before merge.
- [ ] **Step 4: PR** with summary, test evidence, and the manual click-list from the spec; merge to main once every gate is green.
