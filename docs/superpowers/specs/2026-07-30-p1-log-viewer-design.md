# P1 Live Log Viewer — Design

- **Date:** 2026-07-30
- **Status:** Approved under the owner's standing delegation (orchestrator picks the slice, subagents make approvals, check-in per phase). Two owner-call items are surfaced at the end — both ship with a default-closed choice and are reversible.
- **Roadmap line:** "Live log viewer (per service + per site), search/filter, tail-follow."
- **Why this slice:** OpenVHost can create and run a site but cannot help you debug one. Per-site logs **do not exist** — `site.conf.tera` sets no `access_log`/`error_log`, so every site's errors land in the global log — and `phpruntime.rs:54` points **every** php-fpm major at one shared `logs/php-fpm.log`, so a line cannot be attributed to a pool. That is the gap between demo and daily driver, and it compounds with every slice already shipped.
- **Design process:** high-stakes (new file-read IPC surface + config-template change) — two blind independent designs (deep-reasoner, Codex), synthesized here. Agreements adopted as-is; divergences resolved with rationale.
- **Mock:** `docs/design/log-viewer.html` (followed for chrome and controls; its flat tab strip is deliberately replaced — see D6).
- **Plan:** `docs/superpowers/plans/2026-07-30-p1-log-viewer.md`

## What ships (end-to-end demonstrable)

A site returns 500 → the user opens that site's logs from its row → the site **error** log shows nginx's `FastCGI sent in stderr: "PHP message: …"` → filtering narrows to the fatal → Follow shows the next request live → fixing the code and re-requesting shows a 200 in the access log.

## Decisions

### D1 — Log inventory: files for durable diagnostics, ring for live process output (both agreed)

On disk (per master plan §3.2), all directories `0700`:

- `<home>/logs/sites/<domain>/access.log` and `error.log` — **NEW**, the heart of the slice
- `<home>/logs/services/php-fpm-<major>/error.log` — **fixes the shared-file bug**; the pool is per-major, so master/pool lines are honestly labelled per major, never per site
- nginx globals stay exactly where they are (`logs/nginx.error.log`, `logs/nginx.access.log`)

In the ring only (unchanged): child stdout/stderr, supervisor lifecycle lines, and **mysqld** — `log-error` stays unset so failures keep reaching `Failed { stderr_tail }` (MySQL spec D4 preserved deliberately).

**nginx globals are NOT relocated.** Codex proposed moving them under `logs/services/nginx/`; deep-reasoner argued against and wins: `nginx.error.log` is named 19 times across 6 files, a wrong `-e` makes nginx write into its compiled-in prefix, and §3.2 is marked 🟡 Proposed. The auditability that motivated the move is bought instead by **D1a**.

**D1a — `openvhost_core::logs::LogPaths` becomes the single owner of every log path**, and the existing hardcoded call sites are rewired through it (same paths, no behavior change). This is what makes D5's confinement argument checkable at a glance rather than distributed across six files.

**Per-site PHP error log — conditional, non-load-bearing.** Codex proposed `logs/sites/<domain>/php-error.log` fed by a per-site `PHP_ADMIN_VALUE`. The pool is shared per major, so this only works if php-fpm honors an `error_log` override arriving as a per-request FastCGI param — **which must be proven live before it is built** (implementation task step). If it does not work, drop it: the fallback is already sufficient and is what the live proof relies on — a PHP fatal surfaces in the site's **nginx error log** as `FastCGI sent in stderr: "PHP message: …"`, and the per-major pool log (D1) carries the full trace.

### D2 — Template + apply-pipeline integration (both agreed)

Adding per-site log directives makes every enabled site render `Modified`; the existing pending-changes banner fires and the user runs the normal Apply (diff preview → `nginx -t` → restart). No migration code, no automatic background apply — Apply's preview/rollback contract is deliberate project behavior.

**One hard requirement:** nginx and php-fpm create log *files* but not their *directories*, and `nginx -t` fails on a missing one. `ApplyPlan` gains `log_dirs: Vec<PathBuf>` derived purely in `plan()` (which stays read-only), and `commit()` `create_dir_all`s them **before** validation. `provision_home` seeds `logs/sites` and `logs/services`. Rollback leaving empty directories is harmless.

### D3 — Read architecture: bounded Rust reads, frontend polling (both agreed; limits from Codex)

One query command with an opaque cursor. Server behavior:

- `cursor: None` → seek to `max(0, len - WINDOW)`, discard the first partial line
- cursor present → read forward from the offset; a trailing line without `\n` is neither returned nor counted
- `len < cursor.len` or inode change → `reset: rotated|truncated`, restart from a fresh tail
- missing file → `exists: false` as a **state**, not an error; stays pollable until created

Limits (Codex's, adopted): **500 rows**, **512 KiB payload**, **16 KiB per line** (truncate + mark), **16 MiB scanned per request**, **256-byte query**. 500 rows is the safe number precisely because there is no virtualization (D6); 16 MiB bounds server-side filtering (D4).

"Follow" = a 500 ms poll, alive only while the Logs route is mounted **and** Follow is on. Teardown on route change/blur is a tested requirement, not an assumption — an orphaned interval is a permanent battery wakeup.

FSEvents/`notify` push lost in both designs, for the same three reasons: coalesced events still require a re-stat after rotation, a watcher session keyed to UI subscriptions is the most bug-prone thing available, and push has **no backpressure** — an access log under load would flood the webview, whereas polling is self-limiting.

### D4 — Filtering: literal, in Rust, during the bounded scan (Codex adopted over deep-reasoner)

Case-insensitive literal substring by default with a case-sensitive toggle, plus a level filter. The predicate is applied **server-side during the scan**, and the cursor advances across non-matches, so filtering reaches back through the file rather than only the loaded window.

This is the sharpest divergence and it decides the feature's usefulness. deep-reasoner argued for filtering in JS over the loaded window (simple, composes with follow for free, honest "12 of 2,000 loaded" status). But the error a developer is hunting is usually **older than the last window** — on a busy access log, 256 KiB is minutes. A filter that cannot see past the tail answers the wrong question, and shipping it would make the real feature ("find it") look done. The 16 MiB scan bound keeps the cost honest, and the UI states plainly when a scan stopped at the bound.

**No regex.** The pattern comes from the renderer, and JS `RegExp` backtracking (`(a+)+$`) freezes the UI; a Rust `regex` engine would be linear-time but is a new dependency and a bigger surface. Deferring regex is clean precisely because the literal path already carries the cursor protocol.

**Exactly one level classifier is used for file lines** (in core's logs module). The supervisor's existing ring classifier keeps its own path — different input shape, already shipped — and this asymmetry is documented at both sites so nobody "unifies" them by accident.

### D5 — Security: the IPC surface cannot name a file (both agreed; enum from deep-reasoner)

The renderer never supplies a path. A **typed source enum** crosses IPC — `NginxError | NginxAccess | PhpFpm{major} | SiteAccess{domain} | SiteError{domain} | ServiceRing{id}` — whose `domain`/`major` are parsed at ingress into the existing `Domain`/`PhpVersion` newtypes (`Domain` is dot-joined `[a-z0-9-]` labels, so `..`, `/`, `$` are unrepresentable), after which `LogPaths` derives the path.

Codex's opaque server-issued ID registry lost: it needs per-session state and invalidation whenever sites change, whereas the enum is stateless and its safety is a **type property**. The enum also mirrors `read_web_server_config`'s `WebServerBrand::parse → live_config_path` shape, which this project's auditor has already accepted. Codex's better half is adopted: the source is additionally **checked against the live catalogue** (site exists in state.db / runtime installed) before any path is derived, so a deleted site's log is not readable.

Confinement, mirroring `plan.rs`'s established discipline:
- **No `canonicalize`** (it follows symlinks, and a missing log cannot canonicalize at all).
- `symlink_metadata` on the final path; refuse anything that is not a regular file. The derived path is safe, but the *file there* could have been replaced by a link to `~/.ssh/id_rsa` — the Docroot carry-forward lesson.
- A `starts_with(<home>/logs)` post-condition as the one-line assertion a reviewer can verify at a glance.
- Log directories `0700` explicitly (not merely inherited from the 0700 home), so a future change to the home's mode cannot silently open logs up. Log *files* are created by nginx/php-fpm under their own umask — we cannot control that without a race; stated, not papered over.

**Privacy — access logs omit query strings.** `main.conf.tera` gains an explicit `log_format` built on `$uri` (plus method, protocol, status, bytes) instead of `$request`, so `?token=…` / `?api_key=…` never reach a file the UI renders and users screenshot. Default-closed, matching the MySQL slice's redaction discipline. **Owner call** — see below. No heuristic redaction is promised for error logs (PHP/nginx errors can carry anything); the UI carries a plain "these are local logs, they may contain credentials" note instead of a false guarantee.

### D6 — UI: one `/logs` page, grouped sources, deep links (both agreed; both rejected the mock's flat tabs)

The inert rail item goes live. Source selection is **grouped** (Services / Sites, then the stream) rather than the mock's flat tab strip, which does not survive 40 sites — a deliberate, documented deviation from `docs/design/log-viewer.html`. Site and failed-service rows gain a "View logs" action that **deep-links** (`?source=…`) into the same page rather than embedding duplicate viewers; the deep link is the point of the slice.

**No virtualization**: the server already caps the window at 500 rows, so the rendered set is bounded by construction. Virtualization + follow + wrapped variable-height rows is the most bug-prone combination available, for a five-line alternative.

Follow is on by default; **scrolling away turns it off** and reveals "Jump to latest" when new lines have arrived (fixing v0's fight-the-user scroll). Distinct rendered states for: empty, not-yet-created, permission-denied, rotated, unavailable source, and scan-bound-reached.

**The v0 `LogPane` stays** on Services for live per-service output — you must be able to see why the thing you just clicked failed without navigating away — but its mixed first-service feed is scoped to the selected service and it gains "Open in Logs". If scoping proves larger than a small change, keep v0 as-is plus the deep link and defer the scoping (say so in the report). The row renderer is extracted so level colours cannot drift between the two surfaces.

### D7 — IPC: three queries, no new event stream (both agreed)

- `list_log_sources() -> Vec<LogSourceDto>` — `{ source, label, kind: "file"|"ring", exists, sizeBytes, serviceId }`
- `read_log_window(input) -> LogWindowDto` — `{ rows, cursor, exists, reset, hasMore, sizeBytes, scannedBytes, truncatedLines }`
- `reveal_log_folder(source) -> ()` — path derived in Rust, fixed target

Ring sources appear in `list_log_sources` for one unified picker but are read through the **existing** `service_log_tail` + `service-log` push path — unifying them behind the poll would add 500 ms to output that is instant today. Two mechanisms, deliberately, documented at the seam.

Placement: `openvhost-core/src/logs/` owns `LogPaths` + the pure bounded reader (tempfile-testable, no tauri); commands are ingress-parse → catalogue-check → delegate. `u64` byte fields carry the established `dangerously_cast_bigints_to_number` note.

### D8 — Retention: none this slice (both agreed; Codex's warning adopted)

Memory and the wire are bounded (D3); **disk growth is not**. The status line shows file size and a prominent warning above 100 MiB, plus "Open log folder" so the user can act. No Clear/Delete button — truncating in place makes nginx write at the old offset into a sparse file, and correct rotation needs coordinated rename + `SIGUSR1` reopen signalling to nginx and php-fpm plus a policy surface. That is a slice of its own, not something to improvise inside this one. **Owner call** — see below.

## Security posture

- New IPC surface reads files → **security-auditor review is mandatory before merge** (golden rule 2).
- No caller-supplied paths anywhere; typed enum + newtype ingress + catalogue check + `LogPaths` derivation + non-following `symlink_metadata` refusal + `starts_with(<home>/logs)` post-condition.
- Query strings kept out of access logs by construction; no false redaction promises for error logs.
- Log directories `0700`; the reader never loads a whole file (asserted by test, not by intent).

## Deferred (recorded)

Rotation/retention/clear · regex and whole-file search UI (paging, match positions, jump-to-context) · virtualization · merged multi-source timeline · per-line timestamp parsing · php-fpm slow log · relocating nginx's globals · mysqld `log-error` to a file · a user-facing `log_format` setting · log export/download · Windows.

## Owner calls (shipping with a default; both reversible)

1. **Access logs omit query strings** (`$uri`, not `$request`). Default-closed for privacy — a `?token=` in a log the UI renders is the largest new leak class this slice creates. Some developers expect to see query strings; if you want them back, it becomes a Web-server-page setting later and nothing else in this design changes.
2. **On-disk log growth is unbounded this slice**, with a 100 MiB warning and a folder shortcut instead of rotation. Recommended: accept for now, make rotation a pre-beta requirement. Say the word and rotation becomes a merge blocker instead — it is a separate slice's worth of work.

## Verification owed to a human (GUI click-list)

1. Create a site whose `index.php` fatals → Apply (diff shows every site's log directives being added) → `curl -i` returns 500.
2. Site row → View logs → the site's **error** log shows the PHP fatal via nginx's stderr capture; the `php-fpm <major>` source shows the trace.
3. Filter for the function name → matches found from **earlier than the visible tail** (proving server-side filtering).
4. Follow ON → re-request → new lines appear; scroll up → Follow disengages and "Jump to latest" appears.
5. Fix the fatal → re-request → access log shows 200; the access line contains **no query string** even when requested with `?token=abc`.
6. `mv` the log aside while following → the pane resets cleanly (no freeze, no double-print); delete it → "not yet created" state; `chmod 000` → permission state.
7. Loop-curl ~30 s to grow the access log to several MB → memory and CPU stay flat.
8. Services page still shows live output for the selected service and its "Open in Logs" link lands on the right source.
