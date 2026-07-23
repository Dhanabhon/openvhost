# Phase 1 UI · Slice A — Designed Main Window (shell + design system + Services) — design

**Status:** approved design, 2026-07-24. First Phase 1 UI slice. Owner: tauri-frontend-engineer.

## 1. Goal & context

Turn the functional-but-plain P0-3 Services page into the *designed* OpenVHost main window per `docs/design/main-window.html`, and lay the reusable design-system foundation every later UI slice builds on — **wired to the backend that already ships**, faking no data.

Current UI: a single SvelteKit route `apps/desktop/src/routes/+page.svelte` renders a bare Tailwind Services list bound to the real supervisor via `apps/desktop/src/lib/ipc/` and the `ServicesStore` (`src/lib/services.svelte.ts`). Backend IPC available today (from `src-tauri/src/commands.rs`): `core_info`, `list_services`, `start_service`, `stop_service`, `service_log_tail`, plus `ServiceStateEvent`/`ServiceLogEvent`. `ServiceStatus` = `{ id, displayName, endpoint?, pid?, state }`; `ServiceState` = `Stopped | Starting | Running | Failed{ exit, stderrTail }`. Stack: SvelteKit + Svelte 5 (runes) + Tailwind 4 (`@tailwindcss/vite`) + `adapter-static` + tauri-specta typed bindings.

Sites, MySQL/MariaDB, and package management shown in the mockup are Phase 1 **backend** work that does not exist yet — **out of this slice** (see §6).

## 2. Approved decisions

- **First slice = shell + design system + Services** (owner pick). Sites/Logs/Settings appear in the rail as visible-but-inert placeholders until their own slices land.
- **shadcn-svelte: foundation now, components just-in-time.** Init the shadcn-svelte scaffolding (the `cn` util = `clsx` + `tailwind-merge`, `components.json`, `src/lib/components/ui/` dir, its Tailwind conventions). Do NOT pull in any shadcn component yet — slice A's pieces are bespoke against the tokens; later slices (Sites forms, package dropdowns, dialogs) add real shadcn components on demand.
- **Dark mode: theme-ready tokens, ship light.** Structure the token layer so theming needs no retrofit (`:root` light values + `@media (prefers-color-scheme: dark)` and `:root[data-theme="…"]` hooks present but light-valued). No dark palette and no theme toggle in this slice — that is the fast follow-up.
- **Version column dropped** (mockup shows `nginx 1.27.4`; backend `ServiceStatus` has no version field). Render name, endpoint, state, and the action button; add a version column when the backend grows one.
- **Titlebar: macOS `titleBarStyle: Overlay`** — a custom title strip (brand + a live "N running" pill) with the native traffic lights floating over it, matching the mockup's integrated chrome; content left-inset clears the traffic lights. Windows keeps the native frame for now (macOS-first). *Low-risk fallback if Overlay proves fiddly:* native frame everywhere + an in-app header bar — documented so the slice never blocks on window chrome.

## 3. Components & files

**Design-system foundation** (`apps/desktop/src/lib/styles/`, `src/lib/utils/`):
- `styles/tokens.css` — adopted from `docs/design/tokens.css` (`--vh-*` colour/type/space scale), restructured theme-ready per §2; wired into the Tailwind 4 theme (`@theme`/CSS) so utilities and components share one scale.
- `styles/fonts.css` + `src/lib/assets/fonts/*.woff2` — **self-hosted** IBM Plex Sans, JetBrains Mono, Space Grotesk via `@font-face` + `font-display: swap`. NOT the mockup's Google Fonts CDN (Tauri is offline; the perf/security rules forbid external font hosts). Fonts are OFL (GPL-compatible).
- `utils/cn.ts` — the `clsx`+`tailwind-merge` helper; `components.json` + `components/ui/` from `shadcn-svelte` init.

**Window & panels** (`src/lib/components/`, recomposing `routes/+page.svelte`; `routes/+layout.svelte`/`app.html` for font + theme root):
- `TitleBar.svelte` — brand + live running-count pill (derived from the store), Overlay-aware inset.
- `AppShell.svelte` — titlebar + rail/content grid per `main-window.html`.
- `Rail.svelte` — Sites / Services / Logs / Settings nav + brand + version + "Stop all"; **Services the only live destination**, others inert placeholders (disabled, or a small "coming in a later update" empty state).
- `ServicesPanel.svelte` / `ServiceRow.svelte` / `StatusPill.svelte` / `Button.svelte` — the mockup's service-row design (name, endpoint, status pill, Start/Stop/Retry) + the **Failed-state detail** expander showing `stderrTail` (design rule: a Failed service is never rendered silently — mirror the mockup's fail-detail panel).
- `LogPane.svelte` — the existing log area carried over, lightly restyled to the tokens (the full `log-viewer.html` redesign is slice B).

**Tauri config:** `src-tauri/tauri.conf.json` window `titleBarStyle: "Overlay"` (macOS) + transparent-title handling. This is the slice's only non-frontend change — window chrome config, not business logic.

## 4. Data flow — unchanged

Reuse `ServicesStore` (`lib/services.svelte.ts`) and `lib/ipc/` verbatim. Slice A is a **visual re-skin + shell**, not a data-layer change: components bind to the store's reactive `services`/`logs`, the running-count derives from `services`, actions call the existing `startService`/`stopService` wrappers, and live `onServiceState`/`onServiceLog` subscriptions stay as in `+page.svelte`. **No Rust/IPC changes** beyond the `tauri.conf.json` titlebar setting.

## 5. Error handling & states

Every service state renders meaningfully (design rule + brand): `Running`/`Starting`/`Stopped` pills, and `Failed` expands the stderr-tail detail with a Retry action. The command-failure banner (IPC `IpcError`) is preserved, restyled to the tokens. Empty state (no services) and the placeholder nav destinations render intentional empty content, never a blank pane.

## 6. Non-goals (own future slices)

Sites panel + Site CRUD (needs Site model + state.db); package-manager UI (needs pkg IPC over P0-6); MySQL/MariaDB lifecycle (needs DB provisioning); full log-viewer redesign (`log-viewer.html`, slice B); the **dark palette + theme toggle** (fast follow-up — this slice only makes the tokens dark-ready); Windows custom chrome; tray/menu-bar; Settings content; the service **version** column.

## 7. Testing & verification

- **Unit (vitest):** keep `lib/ipc/ipc.test.ts` green; unit-test new pure logic (e.g. the running-count / state→pill derivation). Highly-visual components lean on visual regression over brittle markup assertions (web testing rule).
- **Visual:** screenshot the three real service states (running / stopped / failed) and the empty state via the dev server; verify the window at a couple of sensible desktop sizes; confirm no horizontal overflow.
- **Accessibility:** nav landmarks + `aria-current`, keyboard reachability of every action, visible focus, light-theme contrast; respect `prefers-reduced-motion` for any transition.
- **No-regression:** `pnpm -C apps/desktop lint && check && test && build` all green; the JS license gate passes the new deps (`clsx`, `tailwind-merge`, `shadcn-svelte` — MIT; fonts OFL); Rust workspace untouched (only `tauri.conf.json`).

## 8. Delivery constraints

- Branch `feat/p1-ui-shell-services` off `main` (after P0-9 merges). SPDX `<!-- SPDX-License-Identifier: GPL-3.0-or-later -->` / `// SPDX-License-Identifier: GPL-3.0-or-later` on new source files (Svelte/TS/CSS as the repo already does for `.svelte`). DCO `git commit -s`, no `Co-Authored-By`, Conventional Commits. Typed IPC only — no stringly `invoke("…")`. No security-auditor gate (no helper/cert/download/hosts/IPC-ACL surface). Owner: tauri-frontend-engineer. CI disabled → local frontend gates are the merge gate.
