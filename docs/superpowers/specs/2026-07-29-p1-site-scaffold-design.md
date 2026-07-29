# P1 Site Scaffold on Create — Design

- **Date:** 2026-07-29
- **Status:** Approved (owner approved the product design in-session on 2026-07-29 and
  delegated the remaining approvals to the orchestrator/subagents; the two flagged
  owner's-calls below are recorded with who decided them)
- **Slice:** Phase 1 UI — Sites
- **Plan:** `docs/superpowers/plans/2026-07-29-p1-site-scaffold.md`

## Problem

`Docroot::parse` (crates/openvhost-core/src/site/model.rs:167) validates charset and
absoluteness only — it deliberately does not check existence. A user can therefore save a
site whose docroot is empty or does not exist; after Apply, nginx serves 403/404 and
nothing tells them whether the site is *broken* or just *not set up yet*. Every comparable
tool (Laravel Valet/Herd, Local, MAMP) closes this loop with a starter page.

The owner's ask: after picking a parent folder and saving, create a folder named after the
site and generate a placeholder `index.html`, so opening the site proves end-to-end that
it works.

## Product design (owner-approved)

- **Opt-in checkbox, create mode only.** In `SiteDrawer.svelte`'s Project-folder field, a
  checkbox reading **“Create a site folder inside this folder”**. When checked, a live
  preview of the final path renders beneath it (`<parent>/<name>`, or “Enter a name to see
  the final path” while the name is empty). The checkbox is **not rendered at all** in
  edit mode (`{#if site === null}` — same precedent as the danger zone).
- **Default unchecked.** *(Flagged as owner's call; decided by orchestrator under
  delegated authority.)* Checked-by-default would silently turn the common “browse to my
  existing `~/Sites/myapp`, name it `myapp`” flow into docroot `~/Sites/myapp/myapp`.
  Off keeps the field meaning exactly what it means today; the live preview makes the
  consequence visible the instant it is turned on.
- **On save with the box checked:** the stored docroot becomes `<parent>/<name>`
  (re-validated through `Docroot::parse` at ingress); the folder is created; a placeholder
  `index.html` is generated **unless** an `index.*` entry point already exists there.
- **Scaffold outcome is a state, not a boolean** (this UI has been bitten by
  boolean-collapse three times): `Created` / `KeptExisting { existing }` /
  `Failed { step, reason }`. **Scaffold failure never fails the save** — the site row
  persists and the UI shows a non-blocking, dismissible notice.
- **The notice renders for all three outcomes**, not only failure. *(Flagged as owner's
  call; decided by orchestrator.)* Success visibility is the point of the feature — the
  original ask was “so the user knows it works”. `Failed` renders `role="alert"`; the
  other two `role="status"`.
- **Placeholder is static, self-contained HTML.** No `phpinfo()` (that is a separate open
  owner question — this slice must not pre-empt it), no external assets (the machine may
  be offline). It shows site name, domain, docroot, and next-steps copy, styled
  intentionally for both light and dark (`prefers-color-scheme`).
- **Renaming a site later never moves or renames the folder.**
- **The post-save “now Apply” nudge is the existing pending-changes flow.** A new site
  always yields ≥1 generated config, so `PendingChangesBanner` appears on its own; the
  scaffold notice renders directly above it and reads as “folder ready → now apply”. No
  extra drawer copy (the drawer closes on success anyway).

## Technical decisions

Settled by `deep-reasoner` on 2026-07-29 (dispatch and full rationale in session; the
decisive points are recorded here).

### D1 — Core logic lives in `crates/openvhost-core/src/site/scaffold.rs`

```rust
pub fn scaffold_path(parent: &Docroot, name: &SiteName) -> Result<Docroot, CoreError>; // pure join + re-parse
pub fn scaffold(docroot: &Docroot, name: &SiteName, domain: &Domain) -> ScaffoldOutcome; // NOT Result

pub enum ScaffoldOutcome {
    Created,
    KeptExisting { existing: String }, // names the file found, e.g. "index.php"
    Failed { step: ScaffoldStep, reason: String },
}
pub enum ScaffoldStep { CreateDir, Inspect, WritePlaceholder }
```

`scaffold` returning `ScaffoldOutcome` rather than `Result` makes “scaffold failure never
fails the save” a **type-level guarantee** instead of a call-site convention someone can
forget. `step` gives the UI a stable discriminator so it never parses English out of
`reason`. Three variants, not four — whether the folder pre-existed is not actionable and
the copy is identical. `scaffold_path` is separate and pure so the over-length case
(parent + `/` + up-to-63-byte name can exceed `DOCROOT_MAX_LEN`) fails as a
`docroot` validation error **before** anything is created.

The core enum carries no serde/specta derives — openvhost-core stays free of tauri,
specta, and tera. The app layer mirrors it as a DTO (`ScaffoldOutcomeDto`), the same seam
`Site` → `SiteDto` already uses.

### D2 — Extend `create_site`; do not add a second command

```rust
pub async fn create_site(db, input: SiteInput, create_folder: bool)
    -> Result<CreateSiteResult, IpcError>;

pub struct CreateSiteResult { site: SiteDto, scaffold: Option<ScaffoldOutcomeDto> }
```

Order inside the command: ingress guard (`SiteInput → NewSite`) → if `create_folder`,
`scaffold_path` replaces the docroot → **DB insert** → **then** `scaffold(...)` → return
both. Insert-first is what makes “site persists, scaffold warns” structural rather than a
rollback dance — and a UNIQUE violation on name/domain leaves no orphan folder on disk.

A separate `scaffold_site_folder` command was rejected: it would hand the renderer a
standalone “mkdir + write a file at this path” primitive, whereas here the filesystem
effect is welded to “a row was just created with exactly this docroot” — the strictly
smaller surface for the security audit. `create_folder` is a **parameter, not a
`SiteInput` field**, because `SiteInput` is shared with `update_site`, where scaffolding
must never happen. (A bool is correct here: a checkbox is a genuinely binary intent, not
a collapsed state.) `scaffold: None` means “not requested” — not a fourth outcome.

### D3 — Atomic write: extract the hardened helper, don't duplicate it

`atomic_write_with_suffix`/`atomic_write` move out of
`crates/openvhost-core/src/site/apply/commit.rs` into a crate-internal
`crates/openvhost-core/src/atomicfile.rs` returning a neutral
`AtomicWriteError { op, path, source }`; `commit.rs` keeps thin wrappers mapping into
`ApplyError::Io`, scaffold maps into `ScaffoldStep::WritePlaceholder`. That helper is the
*hardened* one — same-directory temp (rename is atomic only within one filesystem),
`O_CREAT|O_EXCL` so a pre-planted symlink is never written through, random suffix. The
`_with_suffix` seam survives because commit.rs's pre-planted-symlink regression test pins
the suffix; that test must keep exercising the moved body. The other copy in the codebase
(`platform/macos/demo_stack.rs`, plain `std::fs::write`) follows symlinks — not reused.

### D4 — Filesystem semantics

- **`create_dir`, not `create_dir_all`**: silently materialising a missing parent chain
  from a stale path the user never approved is wrong; `NotFound` is the honest TOCTOU
  signal (parent deleted between Browse and Save → `Failed { step: CreateDir }`).
- On `AlreadyExists`: `symlink_metadata` (lstat, no follow) — real directory proceeds;
  a file or symlink at the docroot is `Failed { step: CreateDir, reason: "... already
  exists and is not a folder" }`.
- **Entry-point check**: one `read_dir`; any non-directory entry whose file stem is
  `index` (compared `eq_ignore_ascii_case`) blocks generation → `KeptExisting`. Covers
  `index.html` / `index.htm` / `index.php` / `INDEX.HTML`, and behaves identically on
  APFS case-insensitive (default) and case-sensitive volumes, which a naive
  `join("index.html").exists()` does not. `index.php` must count: the nginx template's
  directive is `index index.php index.html;`, so a pre-existing `index.php` wins anyway.
  `read_dir` failure → `Failed { step: Inspect }` — never a blind write.

Amended 2026-07-29 (owner-approved follow-up to PR #34): the entry-point check now
blocks only `index.{html,htm,php}` stems+extensions (both ASCII case-insensitive).
Non-web `index.*` files (`.js`, `.ts`, `.css`, …) no longer suppress generation — they
produced a green "kept existing" notice over a 404. `.htm` remains blocked because
generating `index.html` beside a user's `index.htm` would silently shadow it.

### D5 — No canonicalization, no symlinked-parent rejection

There is no privilege boundary: the scaffold runs as the user, in a folder the user
picked, and can write nothing they could not write in Finder. Canonicalizing would
rewrite the stored docroot to a path the user never typed (`/tmp` → `/private/tmp`),
diverging from the drawer's live preview for zero security gain; rejecting symlinked
parents breaks the common `~/Projects` → external-volume setup. For the auditor: the name
is a `[a-z0-9-]` slug so no traversal; the temp file uses `O_CREAT|O_EXCL`; the final
swap is `rename`, which replaces a symlink rather than writing through it. The only
refusal is `<parent>/<name>` already existing as a non-directory (D4).

### D6 — Placeholder is `include_str!` + marker substitution, not Tera

`const PLACEHOLDER_HTML: &str = include_str!("placeholder.html")` with three markers
(`{{name}}`, `{{domain}}`, `{{docroot}}`) replaced in Rust, each passed through a local
`html_escape` (`& < > " '`) **unconditionally**. Decisive: openvhost-conf's Tera engine
calls `autoescape_on(vec![])` — escaping is off *globally and deliberately* because it
renders `.conf` files; an HTML template there would ship unescaped (and `Docroot::parse`
rejects only `"`, `$`, and control bytes — `& < > '` all pass, so a docroot can otherwise
inject markup) or force a security-posture change on every nginx template. Also
`openvhost-core` does not depend on tera and must not start to. Escaping lives in
scaffold.rs, not the newtypes — those are charset guards, not encoders (carry-forward).
Unlike the demo stack's file, this one gets **no “GENERATED — DO NOT EDIT” banner**: it
is written once, never rewritten, and the user is meant to edit it.

### D7 — UI integration

- Checkbox + preview render inside the Project-folder `.field`, after the input group;
  the preview hint's id joins `rootDescribedBy` **last** (error-first ordering, matching
  the existing pattern).
- The path join for the preview is a pure exported `scaffoldPreview(parent, name)` in
  `apps/desktop/src/lib/sites.derive.ts` (tested in its existing test file). Truth is
  the returned `SiteDto.docroot`; the TS/Rust join duplication is accepted the same way
  the drawer's charset filters are (“typing affordances only”), with trailing-slash
  normalization matching on both sides.
- `onSave` becomes `(id, input, createFolder) => Promise<boolean>`; the edit path passes
  `false`.
- **Notice surface:** new dismissible `ScaffoldNoticeBanner.svelte` on `routes/+page.svelte`
  directly **above** `PendingChangesBanner`, fed by new `SitesStore` state
  `lastScaffold = $state<{ siteName; docroot; outcome } | null>(null)`, set on the create
  path, cleared on `reset()` and on dismiss. Not `ErrorBanner` (that is `IpcError`-shaped
  and fail-red; the site *did* save). No toast — none exists in this codebase and
  inventing one is scope creep. Copy derivation is a pure TS helper with an exhaustive
  `switch` on `outcome.kind` and **no `default:`**, so a fourth variant fails typecheck
  instead of rendering nothing.

## Security posture

- User-space only; no privileged helper involvement.
- The Tauri command surface changes (`create_site` signature + new response type), so
  **security-auditor review is mandatory before merge** (golden rule 2).
- Traversal: impossible via `name` (strict slug); the joined path is re-validated by
  `Docroot::parse` before insert.
- Writes: hardened atomic helper only (D3); the `atomicfile.rs` extraction must be a pure
  move + `map_err`, and the pre-planted-symlink test must still exercise the moved code —
  otherwise the refactor silently deletes the coverage that finding bought.
- Injection: all three interpolated values HTML-escaped unconditionally (D6).

## Out of scope

- Apply refusing (or warning on) sites whose docroot does not exist — today
  `render_set` wraps the docroot in a `PathBuf` without checking, Apply succeeds, nginx
  404s. Acceptable given the notice; a separate slice if the owner wants more.
- Any PHP entry point in the placeholder (phpinfo remains an open owner question).
- A toast/notification system.
- Windows behavior (macOS-first per project scope).

## Verification owed to a human (GUI click-list)

The sandbox cannot drive the real Tauri app (TCC). After merge, the owner should:

1. New site, checkbox **off** (default): save → no folder created, no notice.
2. New site, checkbox **on**, empty parent folder: preview shows `<parent>/<name>`;
   save → folder + styled placeholder exist; notice (status tone) above the pending
   banner; Apply → Open shows the placeholder page in the browser.
3. New site, checkbox on, parent already containing an `index.php`: notice says it kept
   `index.php`; no `index.html` added.
4. New site, checkbox on, parent deleted after Browse but before Save: site saves, red
   (alert) notice explains the folder failure.
5. Edit an existing site: no checkbox rendered.
6. Notice dismiss button clears it; opening the drawer again does not resurrect it.
