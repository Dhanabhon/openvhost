# P1 Tray / Menu-bar Quick Controls — Design

- **Date:** 2026-07-31
- **Status:** Approved under the owner's standing delegation. Two items need an explicit owner decision (brand icon; see §Owner calls) — both are surfaced, neither blocks the build.
- **Roadmap line:** "System tray/menu-bar quick controls: start/stop all, per-service toggle."
- **Design process:** high-stakes (app lifecycle + the hard-won quit path) — two blind independent designs (deep-reasoner, Codex), synthesized here. Agreements adopted as-is; divergences resolved with rationale and, where they hinged on a claim, verified against the code before deciding.
- **Plan:** `docs/superpowers/plans/2026-07-31-p1-tray.md`

## What ships (end-to-end demonstrable)

Close the window; the app keeps running with its Dock icon. From the menu bar: see at a glance whether the stack is up, start or stop everything, toggle one service, and — when a tray-started service fails — get the real stderr in front of you without hunting. Reopen the window from the tray or the Dock. Quit still confirms and still leaves no child processes behind.

## Decisions

### D1 — Hide on close; stay a Regular Dock app (both designers agreed)

- **Red button / ⌘W** → `api.prevent_close()` **after** successfully hiding the window. App and services keep running.
- **Dock click / tray "Open OpenVHost" / tray icon activation** → show, unminimize, focus. Dock click is `RunEvent::Reopen`, which requires switching `lib.rs`'s `.run(ctx)` to `.build(ctx)?.run(|h, e| …)`. **Verified present in the resolved tauri 2.11.3** (`app.rs:279`, fed by `applicationShouldHandleReopen`) — not assumed from docs.
- **⌘Q / app-menu Quit / tray Quit** → the existing confirmation → install-abort → shared 18 s stop-all → destroy flow, unchanged.
- **Activation policy stays Regular; no `Accessory`/LSUIElement.** Both designers rejected it independently: it removes the standard Dock and app-switcher path back into the app, turning a *secondary* control into the product's only entrance.

**Required change to `quit.rs` — the sharpest interaction in the slice.** `request_quit` must `show()` + `set_focus()` the main window **before** emitting `QuitRequestedEvent`. Without it, ⌘Q while the app is frontmost-but-hidden (newly reachable once close-hides) emits a confirmation into an invisible webview: the dialog is never seen and the app silently refuses to quit. Both designers identified this independently.

**Effect on the documented "prevent_close makes the UI load-bearing" exposure: it decreases.** The ordinary close path no longer consults the webview at all (hide is pure Rust), so the most-used path stops depending on the frontend. Explicit Quit remains exposed exactly as documented today — and the tray adds a webview-independent escape hatch: tray Quit checks the UI-ready flag and, when the frontend never came up, calls `perform_quit` directly. A broken frontend becomes quittable without Force Quit, which is strictly better than today.

### D2 — Menu content and update strategy (both agreed)

Open OpenVHost · *(disabled)* aggregate summary · Start all · Stop all · — · one row per registered service · — · Quit OpenVHost.

Derive the model from `Supervisor::snapshot()`. On every state event, **recompute from a fresh snapshot** and diff against the last applied model; never apply event deltas. That is idempotent and immune to the broadcast channel's `Lagged` arm. Rebuild the whole menu only when the **set of service ids** changes.

**Both designers independently found the same gap: `Supervisor::register` emits no event**, so services registered after launch (a newly installed PHP major, an initialized MySQL) never reach any observer — the Services page has the same bug today. Fix it once at the source: add `SupervisorEvent::Registered { status }` and let both the tray and the existing UI store react. deep-reasoner's alternative (call a tray refresh at the three known register call sites) was rejected: it fixes the tray only and leaves the next call site to rediscover the bug.

**Mutation must happen on the main thread, in one batch.** `MenuItem::set_text`/`set_enabled` are blocking main-thread round-trips; called one-by-one from the event task they would block a tokio worker for as long as the user holds the menu open. Apply all diffs inside a single `AppHandle::run_on_main_thread` closure.

**Per-service rows are action + state** (`Start nginx — Stopped`, `Stop nginx — Running`, `Retry nginx — Failed`), never a checkmark: a `CheckMenuItem` is a boolean where a four-state enum belongs — the exact collapse this codebase has hit three times.

**Staleness is closed by construction:** the click handler ignores the label it was rendered with, re-reads `snapshot()`, and derives the action from the *current* state via a pure, exhaustively-matched function.

### D3 — `Starting` rows are disabled this slice, and the unmet contract is named

A row in `Starting` renders **disabled**. This is honest about a real limitation rather than offering an affordance that silently does nothing.

Codex correctly found that `stop()` during `Starting` is **queued until readiness completes** — for MySQL that is up to 15 s of the user clicking Stop with nothing appearing to happen — and proposed adding `ServiceState::Stopping` plus making readiness waits select on the stop channel. That diagnosis is right, and it is more than a nicety: `docs/design/README.md`'s original P0-3 consumer contract states **"starting state must be cancel-safe"**, so this is an *unmet existing requirement*, not a new wish.

It is nonetheless **deferred to its own slice**. It is a state-machine change to `openvhost-proc` — the crate carrying the orphan reaper and the hermetic E2E harness, where every prior change needed its own regression proof — and this slice's value does not depend on it. Bundling would put quick controls at the mercy of a supervisor refactor. The gap predates the tray and the tray does not worsen it: a disabled row is the truthful rendering of a service that cannot currently be cancelled. **Follow-up recorded: cancel-safe `Starting` (+ `Stopping` state), citing the P0-3 contract line.**

### D4 — Failure surfacing: native dialog with the stderr, plus a persistent channel (synthesis)

When a service **started from the tray** reaches `Failed`:

1. **Immediately** — a native error dialog (the `tauri-plugin-dialog` already in the dependency list; no new plugin, no new permission) carrying the service name, exit status, and the bounded `stderr_tail` **verbatim**, with *Open OpenVHost* and *Dismiss*.
2. **Durably** — the tray icon holds the attention state and the row stays `Retry … — Failed` until the state changes.

Codex proposed the dialog; deep-reasoner proposed the persistent icon/label plus auto-opening the window to `/services`. Adopting both halves is the actual synthesis: a dialog is transient and a user can dismiss it into oblivion, while an icon alone makes them hunt for the reason. Together the failure is unmissable *and* survives dismissal.

**Auto-opening the window is rejected** (Codex's reasoning wins): the dialog already makes the failure unavoidable, so raising the window on top of it is focus-stealing — and it would need a new frontend event, which the dialog does not. Native notifications are rejected by both: a new plugin and OS permission, silently suppressible by the user, and a poor carrier for multi-line stderr.

### D5 — Icon: template glyphs, four states, matching the documented aggregate precedence

`docs/design/README.md` already defines the app's aggregate contract — **`failed > starting > running > stopped`** — and names the tray as a future consumer of it. The tray therefore ships **four** template glyphs matching that precedence, not the three deep-reasoner proposed: consistency with the titlebar aggregate is worth one extra asset, and inventing a smaller set here would make two surfaces disagree about the same stack.

Template (`icon_as_template(true)`) so macOS tints them for light/dark/tinted menu bars. State is encoded by **shape**, not colour. Assets at 36×36 px — `tray-icon` scales to 18 pt, so 36 px is exactly 2× for Retina; an 18 px asset would be blurry. Stored under `apps/desktop/src-tauri/icons/tray/`, embedded, **not** added to `bundle.icon`.

**This narrows the brand guideline and needs the owner** — see §Owner calls.

### D6 — Zero new Tauri commands (both agreed)

The tray lives entirely in Rust: it reads the managed `Arc<Supervisor>` and calls `start`/`stop` directly. No new commands, no `capabilities/default.json` change, no JS tray API. Both designers noted this is also a *security improvement*: start-all/stop-all are unreachable from a compromised webview.

Bulk primitives live in one place: **`stop_all` is shared with `quit.rs` verbatim** (its 18 s budget already covers MySQL's 15 s grace) so Quit and tray Stop-all cannot drift; `start_all` is added beside it in the same closure-injected shape that made `stop_all_with` testable.

**`start_all` must not start two services that share an endpoint.** Verified in the code: every `mysql-<major>` declares the literal `127.0.0.1:3306` (`stack.rs:171` says so explicitly), so a naive start-all on a multi-major machine guarantees an "Address already in use" failure. Bulk start selects at most one service per distinct endpoint, skips non-terminal states, and skips `demo-ticker` (see D8).

**The Rail's permanently-disabled "Stop all" button is removed** (`Rail.svelte:204`). Wiring it would need a real bulk command and break the zero-new-commands story; leaving a dead control next to a working tray equivalent is worse than removing it. The PR says where bulk control now lives.

### D7 — Concurrency: narrow, verified (deep-reasoner adopted over Codex)

**Per-service toggles take no new lock.** Verified in `supervisor.rs`: `start()` returns `Ok(())` early when the state is already `Starting|Running`, and `stop()` when already `Stopped|Failed`, both **inside the entries mutex**. A tray/UI race is therefore a no-op, never a double-spawn.

Codex proposed a shared `ControlGate` wrapping tray actions, the **existing** `start_service`/`stop_service` commands, and `apply_config`. Rejected: it changes the behaviour of already-shipped, already-audited paths to solve a race the code shows cannot happen, which is regression risk bought with no safety.

**Bulk actions do take a lock** — they are long-running loops, unlike the synchronous per-service dispatch: a `BulkLock` plus a `try_lock` on the existing apply lock. **Reject, never queue** ("another operation is in progress"), with the tray model disabling Start all / Stop all while held. Queuing would flap the stack with no user intent behind it. `InstallLock` needs nothing: an install registers only new, stopped rows on completion, which the new `Registered` event reconciles.

Stop-all during MySQL's 15 s grace needs no change: `stop` re-flags, the full control channel discards the duplicate, and the loop simply polls to `Stopped`.

### D8 — `demo-ticker` is gated behind `debug_assertions`

`lib.rs:251` registers `demo-ticker` **unconditionally**, including in release builds — a service that deliberately fails after 45 ticks. Both designers flagged it. A faithful tray would list "demo-ticker — Failed" to every user. Codex called it an owner decision; it is not — shipping a deliberately-failing fake service in a menu is obviously wrong. One-line `#[cfg(debug_assertions)]` in this slice, and it is excluded from bulk start regardless.

### D9 — Testing (both agreed)

Rust-testable, and the seams are chosen to maximise it:
- `tray_model(&[ServiceStatus], BulkState) -> TrayModel` — labels, enablement, aggregate summary, icon state. Pure.
- `toggle_action(state) -> Action` — exhaustive match, no wildcard.
- `bulk_start_ids(&[ServiceStatus])` — two MySQL majors collapse to one id; two PHP majors keep both; `demo-ticker` excluded; non-terminal states skipped.
- `apply(old, new, &dyn TraySink)` against a recording fake — this is what makes "an event actually changes the menu" testable without AppKit.
- `handle_tray_menu_id(&AppHandle, &str)` under the existing `mock_builder` harness with a real managed `Supervisor` — so give the router that signature rather than one taking a `MenuEvent`.

**You cannot construct a real tray or muda menu in tests** (NSStatusItem/NSMenu, main thread, no NSApp) — do not try. Native ordering, clicks, hide/reopen, focus, dialog presentation, dark menu bar and Retina sharpness go on the human click-list.

### D10 — Slice cut

**In:** tray construction + the four icons; hide-on-close + `Reopen` + the `request_quit` show-first fix; the live menu with snapshot reconciliation; `SupervisorEvent::Registered`; per-service toggle; `start_all` + `BulkLock` + shared `stop_all`; the failure dialog + persistent tray state; `demo-ticker` debug-gating; Rail dead-button removal; tests; spec.

**Out, named:** cancel-safe `Starting` / `ServiceState::Stopping` (D3 follow-up, cites the P0-3 contract); Windows; Accessory/LSUIElement mode; launch-at-login; native notifications; per-site tray entries; deep links to pages other than the window itself; a frontend bulk-control command; a first-run "still running in the menu bar" hint.

## Owner calls

1. **Brand guideline §7.3 vs macOS reality — RESOLVED 2026-07-31: option (a), shape-coding.** §7.3 specified a system-tinted template bracket **with a colour-coded state dot** and called it the one place the mark's dot changes colour. Not achievable: `setTemplate(true)` draws the image as a **mask**, so all colour is discarded. The owner chose (a) — amend the guideline to shape-coding — over (b) funding native AppKit compositing at the cost of losing automatic menu-bar tinting. `docs/OPENVHOST_BRAND_GUIDELINES.md` §3.3 and §7.3 are amended on this branch with a dated note recording the constraint and the decision. Windows keeps the coloured dot (its tray icons are not template images).
2. **Hide-on-close discoverability.** No first-run hint ships; the Dock icon is the macOS-conventional "still running" signal. If you want a one-time notice, it is a clean fast-follow (a flag in state.db).

## Verification owed to a human (click-list)

1. Tray icon appears; its glyph matches the stack's aggregate state.
2. Red button and ⌘W hide the window; `curl` to a site still serves; the Dock icon remains.
3. Dock click and tray "Open OpenVHost" both restore and focus the window.
4. Toggle one service from the tray; the Services page agrees when reopened.
5. Start all / Stop all with the window hidden; verify with `curl` and `pgrep`.
6. Occupy port 80 externally, then tray-start nginx → the dialog shows the real stderr; dismissing it leaves the icon in the attention state and the row on `Retry`.
7. Install a PHP major while the app runs → its row appears in the tray without a restart (the `Registered` event).
8. ⌘Q while the window is hidden → the window shows and the confirmation appears (the `request_quit` fix); confirm → no `nginx`/`php-fpm`/`mysqld` survives.
9. Dark and light menu bars: the icon is legible in both; Retina shows no blur.
