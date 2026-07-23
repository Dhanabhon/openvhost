# Phase 1 UI · Slice A — Designed Main Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the plain P0-3 Services page with the *designed* OpenVHost main window (window chrome + design-system foundation + re-skinned Services panel) wired to the real supervisor backend, per `docs/design/main-window.html`.

**Architecture:** Port the existing HTML/CSS mockup (`docs/design/main-window.html` + `mock.css` + `tokens.css`) into focused Svelte 5 components, bound to the **unchanged** `ServicesStore` + `lib/ipc` data layer. Establish the reusable foundation (design tokens made theme-ready, self-hosted fonts, shadcn-svelte scaffolding). Only non-frontend change is the macOS Overlay titlebar in `tauri.conf.json`.

**Tech Stack:** SvelteKit + Svelte 5 runes + Tailwind 4 (`@tailwindcss/vite`) + tauri-specta typed IPC + `@fontsource` self-hosted fonts + `clsx`/`tailwind-merge` (shadcn-svelte `cn`).

**Spec:** `docs/superpowers/specs/2026-07-24-p1-ui-shell-services-design.md`

## Global Constraints

- Branch `feat/p1-ui-shell-services` off `main`.
- **Authoritative visual source — PORT, do not re-invent:** `docs/design/main-window.html` (markup/structure), `docs/design/mock.css` (component styles: `.titlebar`, `.rail`, `.nav-item`, `.shell`, `.content`, `.panel`, `.rowlist`, `.row`/`.svc-row`, `.pill`/`.pill-*`, `.btn`/`.btn-*`, `.fail-detail`, `.section-label`, etc.), `docs/design/tokens.css` (the `--vh-*` scale). Components must match the mockup; copy its CSS rules into scoped component styles rather than approximating.
- SPDX line 1 of every NEW source file: `.svelte`/`.html` → `<!-- SPDX-License-Identifier: GPL-3.0-or-later -->`; `.ts` → `// SPDX-License-Identifier: GPL-3.0-or-later`; `.css` → `/* SPDX-License-Identifier: GPL-3.0-or-later */`.
- **Data layer is frozen:** reuse `apps/desktop/src/lib/services.svelte.ts` (`ServicesStore`) and `apps/desktop/src/lib/ipc/` verbatim. No new IPC command/event; typed bindings only (no stringly `invoke("…")`). No Rust change except `tauri.conf.json`.
- **Theme-ready, ship light:** tokens keep the light values; add the dark hooks (`@media (prefers-color-scheme: dark)` + `:root[data-theme="dark"]`) as structure only (no dark palette, no toggle — that is the fast follow-up).
- **shadcn-svelte: foundation only** (the `cn` util, `components.json`, `src/lib/components/ui/` dir). Do NOT add any shadcn component in this slice; slice A's components are bespoke ports.
- **Self-host fonts** via `@fontsource` npm packages — NO Google Fonts CDN (Tauri offline; perf/security rules). IBM Plex Sans, JetBrains Mono, Space Grotesk (OFL; GPL-compatible).
- Svelte 5 runes (`$state`/`$derived`/`$effect`/`$props`); TypeScript strict; no `console.log`; every service state renders meaningfully (Failed is never silent).
- DCO `git commit -s`, no `Co-Authored-By`, Conventional Commits. No security-auditor gate.
- CI disabled → the frontend gate is the merge gate: `pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build` (also keep `cargo test --workspace` green — Rust is untouched but `tauri.conf.json` must stay valid).

---

### Task 1: Design-system foundation (tokens, fonts, shadcn `cn`)

**Files:**
- Create: `apps/desktop/src/lib/styles/tokens.css`
- Create: `apps/desktop/src/lib/styles/app.css` (global entry: tailwind + tokens + fonts + base)
- Create: `apps/desktop/src/lib/utils/cn.ts`
- Create: `apps/desktop/components.json` (shadcn-svelte config)
- Create: `apps/desktop/src/lib/components/ui/.gitkeep`
- Modify: `apps/desktop/src/routes/+layout.svelte` (import `app.css` instead of `layout.css`)
- Modify: `apps/desktop/package.json` (deps)
- Delete: `apps/desktop/src/routes/layout.css` (superseded by `app.css`) — only after moving any rules still needed.

**Interfaces produced:** `$lib/utils/cn` → `cn(...classes) => string`; the `--vh-*` token layer + self-hosted fonts available app-wide; `$lib/components/ui/` scaffolded.

- [ ] **Step 1: Branch + install deps**

```bash
git checkout main && git pull --ff-only && git checkout -b feat/p1-ui-shell-services
pnpm -C apps/desktop add clsx tailwind-merge @fontsource/ibm-plex-sans @fontsource/jetbrains-mono @fontsource/space-grotesk
```

Expected: all resolve; they are MIT (packaging) + OFL (font data) — the license gate passes.

- [ ] **Step 2: Port the token layer, theme-ready**

Create `apps/desktop/src/lib/styles/tokens.css` = a copy of `docs/design/tokens.css` (the full `--vh-*` set + the light `:root` block + the base rules), with SPDX line 1, and append the theme-ready dark hooks as STRUCTURE ONLY (light values duplicated for now — a real dark palette is the follow-up):

```css
/* ---- theme-ready hooks (dark palette is a follow-up; values still light) ---- */
:root[data-theme="dark"] {
  /* Dark overrides land here in the dark-mode slice. Intentionally empty so
     the mechanism exists without shipping an unreviewed dark palette. */
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    /* Same: reserved for the dark-mode slice. */
  }
}
```

- [ ] **Step 3: Self-host fonts + the global entry**

Create `apps/desktop/src/lib/styles/app.css`:

```css
/* SPDX-License-Identifier: GPL-3.0-or-later */
@import 'tailwindcss';
@import '@fontsource/ibm-plex-sans/400.css';
@import '@fontsource/ibm-plex-sans/500.css';
@import '@fontsource/ibm-plex-sans/600.css';
@import '@fontsource/jetbrains-mono/400.css';
@import '@fontsource/jetbrains-mono/700.css';
@import '@fontsource/space-grotesk/500.css';
@import '@fontsource/space-grotesk/700.css';
@import './tokens.css';
```

(The `@fontsource` imports self-host the exact weights the mockup uses — `docs/design/main-window.html`'s Google Fonts `<link>` line 10. Verify any rule the old `routes/layout.css` still needs is preserved in `tokens.css`'s base block before deleting it.)

Modify `apps/desktop/src/routes/+layout.svelte` line 3: `import './layout.css';` → `import '$lib/styles/app.css';`. Then delete `apps/desktop/src/routes/layout.css`.

- [ ] **Step 4: Write the failing test for `cn`**

Create `apps/desktop/src/lib/utils/cn.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { cn } from './cn';

describe('cn', () => {
  it('joins truthy classes and drops falsy', () => {
    expect(cn('a', false && 'b', 'c')).toBe('a c');
  });
  it('later tailwind utility wins on conflict', () => {
    expect(cn('px-2', 'px-4')).toBe('px-4');
  });
});
```

Run: `pnpm -C apps/desktop test -- cn` → FAIL (`cn` not found).

- [ ] **Step 5: Implement `cn` + shadcn scaffolding**

`apps/desktop/src/lib/utils/cn.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

/** shadcn-svelte class combiner: clsx semantics + Tailwind conflict resolution. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
```

`apps/desktop/components.json` (shadcn-svelte foundation marker; components added on demand in later slices):

```json
{
  "$schema": "https://shadcn-svelte.com/schema.json",
  "tailwind": { "css": "src/lib/styles/app.css", "baseColor": "neutral" },
  "aliases": { "components": "$lib/components", "utils": "$lib/utils", "ui": "$lib/components/ui" },
  "typescript": true
}
```

Create `apps/desktop/src/lib/components/ui/.gitkeep` (empty).

- [ ] **Step 6: Green + gate + commit**

```bash
pnpm -C apps/desktop test -- cn
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop build
git add apps/desktop && git commit -s -m "feat(ui): design-system foundation — tokens (theme-ready), self-hosted fonts, shadcn cn"
```

Expected: `cn` tests pass; build succeeds with fonts self-hosted and tokens loaded; no CDN references remain (`grep -r "fonts.googleapis" apps/desktop/src` is empty).

---

### Task 2: App shell — TitleBar, Rail, AppShell + macOS Overlay titlebar

**Files:**
- Create: `apps/desktop/src/lib/components/TitleBar.svelte`
- Create: `apps/desktop/src/lib/components/Rail.svelte`
- Create: `apps/desktop/src/lib/components/AppShell.svelte`
- Modify: `apps/desktop/src-tauri/tauri.conf.json` (macOS titlebar)

**Interfaces:**
- Consumes: the token layer (Task 1); the mockup `.titlebar`/`.rail`/`.nav-item`/`.shell`/`.content` markup + `mock.css` rules.
- Produces: `AppShell` (props: `runningCount: number`, `totalCount: number`, snippet `children`) rendering the titlebar + rail + a content region; `Rail` (props: `active: 'services'`) with Sites/Logs/Settings as inert placeholders; `TitleBar` (props: `runningCount: number`).

- [ ] **Step 1: macOS Overlay titlebar**

In `apps/desktop/src-tauri/tauri.conf.json`, on the main window object add (macOS draws native traffic lights over your content; you inset for them):

```json
"titleBarStyle": "Overlay",
"hiddenTitle": true
```

Keep the existing `title`, `width`, `height`. (Windows ignores `titleBarStyle` → native frame, per macOS-first. If the app has no explicit window in `tauri.conf.json`, add the fields to the single window entry under `app.windows[0]`.)

- [ ] **Step 2: TitleBar component**

`apps/desktop/src/lib/components/TitleBar.svelte` — port `docs/design/main-window.html` lines 22-26 (`.titlebar`, brand, running-count `.pill.pill-running`) + the `.titlebar`/`.pill` rules from `mock.css` into a scoped `<style>`. macOS: left-pad the strip ~72px so it clears the traffic lights (`env(titlebar-area-x)` if available, else a fixed inset). Contract:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
  let { runningCount }: { runningCount: number } = $props();
</script>

<div class="titlebar" data-tauri-drag-region>
  <div class="titlebar-name"><b>OpenVHost</b></div>
  <span class="pill pill-running"><span class="dot"></span>{runningCount} running</span>
</div>

<style>
  /* Port .titlebar, .titlebar-name, .pill, .pill-running, .dot from docs/design/mock.css.
     Add: padding-left: 72px (clear macOS traffic lights); use var(--vh-*) tokens only. */
</style>
```

Note `data-tauri-drag-region` makes the strip draggable (Overlay windows need an explicit drag region).

- [ ] **Step 3: Rail component**

`apps/desktop/src/lib/components/Rail.svelte` — port `main-window.html` lines 29-57 (`.rail`, `.rail-brand`, `.nav-item`, `.rail-foot`) + `mock.css`. Services is `aria-current="page"`; **Sites / Logs / Settings are inert placeholders** — render them as `disabled`-styled items (`aria-disabled="true"`, not focusable, muted) so the nav shows the roadmap without dead links. Keep the brand SVG, `v0.1.0`, and a "Stop all" link (wire "Stop all" to nothing yet — render it disabled too, or omit; do not fake an action). Contract:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
  let { active = 'services' }: { active?: 'services' } = $props();
</script>
<!-- port the <nav class="rail"> markup; Services item gets aria-current when active==='services';
     Sites/Logs/Settings are aria-disabled placeholders. -->
```

- [ ] **Step 4: AppShell**

`apps/desktop/src/lib/components/AppShell.svelte` — port `.shell`/`.content` grid; compose `TitleBar` + `Rail` + a `{@render children()}` content slot. Contract:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
  import TitleBar from './TitleBar.svelte';
  import Rail from './Rail.svelte';
  let { runningCount, children }: { runningCount: number; children: import('svelte').Snippet } =
    $props();
</script>

<div class="window">
  <TitleBar {runningCount} />
  <div class="shell">
    <Rail active="services" />
    <main class="content">{@render children()}</main>
  </div>
</div>

<style>
  /* Port .window/.shell/.content layout from docs/design/mock.css. */
</style>
```

- [ ] **Step 5: Verify shell renders + gate + commit**

Temporarily render `<AppShell runningCount={3}>…</AppShell>` (a scratch page or the existing `+page.svelte`) and `pnpm -C apps/desktop dev`; confirm the titlebar (draggable, traffic lights clear the brand), rail (Services active, others muted), and content region match the mockup. Then:

```bash
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop build && cargo test --workspace
git add apps/desktop && git commit -s -m "feat(ui): app shell — Overlay titlebar, rail nav, layout per mockup"
```

Expected: build green; `tauri.conf.json` still valid (cargo build of the app unaffected).

---

### Task 3: Services panel re-skin + wiring (the working designed window)

**Files:**
- Create: `apps/desktop/src/lib/components/StatusPill.svelte`, `Button.svelte`, `ServiceRow.svelte`, `ServicesPanel.svelte`, `LogPane.svelte`
- Create: `apps/desktop/src/lib/services.derive.ts` (pure derivations + tests)
- Modify: `apps/desktop/src/routes/+page.svelte` (recompose using `AppShell` + `ServicesPanel`)

**Interfaces:**
- Consumes: `ServicesStore` (`services`, `logs`, `init`, `applyState`, `applyLog`), `lib/ipc` (`coreInfo`, `startService`, `stopService`, `onServiceState`, `onServiceLog`, types `ServiceStatus`/`ServiceState`/`CoreInfo`/`IpcError`), `AppShell` (Task 2). `ServiceStatus = { id, displayName, endpoint?, pid?, state }`; `state.kind ∈ {'running','starting','failed','stopped'}`, failed carries `exit`/`stderrTail`.
- Produces: the recomposed main window.

- [ ] **Step 1: Failing test for the pure derivations**

`apps/desktop/src/lib/services.derive.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { runningCount, pillClass } from './services.derive';
import type { ServiceStatus } from './ipc';

const svc = (id: string, kind: string): ServiceStatus =>
  ({ id, displayName: id, endpoint: null, pid: null, state: { kind } }) as unknown as ServiceStatus;

describe('runningCount', () => {
  it('counts only running services', () => {
    expect(runningCount([svc('a', 'running'), svc('b', 'stopped'), svc('c', 'running')])).toBe(2);
  });
});
describe('pillClass', () => {
  it('maps each state kind to its pill modifier', () => {
    expect(pillClass('running')).toBe('pill-running');
    expect(pillClass('starting')).toBe('pill-starting');
    expect(pillClass('failed')).toBe('pill-failed');
    expect(pillClass('stopped')).toBe('pill-stopped');
  });
});
```

Run: `pnpm -C apps/desktop test -- services.derive` → FAIL (module missing).

- [ ] **Step 2: Implement the derivations**

`apps/desktop/src/lib/services.derive.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import type { ServiceState, ServiceStatus } from './ipc';

export function runningCount(services: readonly ServiceStatus[]): number {
  return services.filter((s) => s.state.kind === 'running').length;
}

export type StateKind = ServiceState['kind'];
export function pillClass(kind: StateKind): string {
  return `pill-${kind}`;
}
```

Run: `pnpm -C apps/desktop test -- services.derive` → PASS.

- [ ] **Step 3: Presentational components (port from mockup)**

Port these from `docs/design/main-window.html` + `mock.css`, tokens only, into scoped styles:
- `StatusPill.svelte` — props `{ kind: StateKind }`; renders `<span class="pill {pillClass(kind)}"><span class="dot"></span>{kind}</span>` (port `.pill`/`.pill-*`/`.dot`).
- `Button.svelte` — props `{ variant?: 'primary' | 'quiet', size?: 'sm', disabled?, onclick, children }`; port `.btn`/`.btn-primary`/`.btn-quiet`/`.btn-sm`. Use `cn` for class composition.
- `ServiceRow.svelte` — props `{ service: ServiceStatus, onStart, onStop }`; port `.row.svc-row` (name, `endpoint` in `.mono.meta`, `StatusPill`, and the action: `stopped`→Start, `failed`→Retry(=start), else→Stop). When `state.kind==='failed'` render the `.fail-detail` block with `state.stderrTail.join('\n')` in a `<pre>` (port lines 158-167). **No version column** (backend has none).
- `LogPane.svelte` — props `{ logs: UiLog[] }`; port the existing `+page.svelte` log grid, restyled to `--vh-log-*` tokens; keep the auto-follow `$effect` (scroll to bottom on new lines).
- `ServicesPanel.svelte` — props `{ services, onStart, onStop }`; the `.section-label` "Services" head + a `.panel.services-panel` with a `ServiceRow` per service (port lines 129-176). Empty state: an intentional "No services registered" panel, never blank.

- [ ] **Step 4: Recompose the page**

Rewrite `apps/desktop/src/routes/+page.svelte` to keep ALL the existing store/IPC wiring (the `onMount` subscribe + `store.init()` + `coreInfo()` + the `act()` error handling + error banner) and render through the new components:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    coreInfo, startService, stopService, listServices, serviceLogTail,
    onServiceLog, onServiceState, type CoreInfo, type IpcError
  } from '$lib/ipc';
  import { ServicesStore } from '$lib/services.svelte';
  import { runningCount } from '$lib/services.derive';
  import AppShell from '$lib/components/AppShell.svelte';
  import ServicesPanel from '$lib/components/ServicesPanel.svelte';
  import LogPane from '$lib/components/LogPane.svelte';

  const store = new ServicesStore({ listServices, serviceLogTail });
  let info = $state<CoreInfo | null>(null);
  let error = $state<IpcError | null>(null);
  const running = $derived(runningCount(store.services));

  onMount(() => {
    let unsubs: Array<() => void> = [];
    (async () => {
      try {
        unsubs = await Promise.all([
          onServiceState((ev) => store.applyState(ev)),
          onServiceLog((ev) => store.applyLog(ev))
        ]);
        await store.init();
        info = await coreInfo();
      } catch (e) { error = e as IpcError; }
    })();
    return () => unsubs.forEach((u) => u());
  });

  async function act(fn: (id: string) => Promise<void>, id: string) {
    error = null;
    try { await fn(id); } catch (e) { error = e as IpcError; }
  }
</script>

<AppShell runningCount={running}>
  {#if error}
    <div class="banner-error" role="alert" data-testid="error-banner">
      <strong>Command failed ({error.kind})</strong>
      <span>{'message' in error ? error.message : ''}</span>
    </div>
  {/if}
  <ServicesPanel
    services={store.services}
    onStart={(id) => act(startService, id)}
    onStop={(id) => act(stopService, id)}
  />
  <LogPane logs={store.logs} />
  {#if info}
    <p class="coreinfo mono">OpenVHost {info.appVersion} · {info.os}/{info.arch} · {info.openvhostHome}</p>
  {/if}
</AppShell>

<style>
  /* .banner-error + .coreinfo styled with --vh-* tokens (port the mockup's fail tint + caption). */
</style>
```

Preserve the `data-testid` hooks (`error-banner`, and add `services`/`pill-{id}`/`failed-{id}`/`log` on the corresponding new elements) so the existing `ipc.test.ts` intent still holds.

- [ ] **Step 5: Run the app, verify the three states, gate, commit**

`pnpm -C apps/desktop dev`; with the supervisor running, confirm: services list styled per mockup, a running service shows the running pill + Stop, stopping shows Stop→Start, and a Failed service expands the stderr detail with Retry. Then:

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop build && cargo test --workspace
git add apps/desktop && git commit -s -m "feat(ui): re-skin Services panel per mockup, wired to the supervisor"
```

Expected: all green; the window matches `main-window.html`'s Services section against real data.

---

### Task 4: Verification, visual proof, PR

**Files:** none (verification + PR).

- [ ] **Step 1: Accessibility + reduced-motion pass**

Confirm: the rail is a `<nav aria-label="Main">` with `aria-current="page"` on Services; every action button is keyboard-reachable with a visible `:focus-visible` ring (tokens already define it); placeholder nav items are `aria-disabled`; contrast holds in light; `prefers-reduced-motion` is respected (tokens already gate transitions). Fix any gap.

- [ ] **Step 2: Visual proof (screenshots)**

With the app running (`pnpm -C apps/desktop dev`), capture the main window in three states — a **running** service, a **stopped** service, and a **failed** service (induce one, e.g. a service whose program exits non-zero) — plus the empty state. Save under the scratchpad and confirm each matches `main-window.html`. (These screenshots are the artifact the controller relays to the owner for visual sign-off.)

- [ ] **Step 3: Full frontend gate + license gate**

```bash
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
cargo test --workspace && cargo deny check licenses advisories
grep -rn "fonts.googleapis\|fonts.gstatic" apps/desktop/src && echo "CDN FONT LEAK" || echo "no CDN fonts"
```

Expected: all green; `cargo deny` exit 0 (the new npm deps are MIT/OFL — note: `cargo deny` covers Rust; the JS licenses are MIT/OFL and pass the project's npm license expectation); no CDN font references.

- [ ] **Step 4: Push + PR (do NOT merge)**

```bash
git push -u origin feat/p1-ui-shell-services
gh pr create --title "feat: Phase 1 UI slice A — designed main window (shell + design system + Services)" --body "Ports docs/design/main-window.html into Svelte 5 components wired to the real supervisor: window chrome (macOS Overlay titlebar + rail nav), the design-system foundation (tokens made theme-ready + shipping light, self-hosted @fontsource fonts, shadcn-svelte cn scaffolding), and the re-skinned Services panel (status pills, Start/Stop/Retry, Failed-state stderr detail). Sites/Logs/Settings are inert placeholder nav; the data layer (ServicesStore + lib/ipc) is unchanged; only tauri.conf.json changes on the Rust side.

Deferred to follow-ups: dark palette + toggle (tokens are already dark-ready), full log-viewer redesign, Sites/DB/package panels (need backend), Windows custom chrome, service version column. Frontend gates + cargo test green; no CDN fonts; no new IPC. Screenshots of running/stopped/failed states in the PR for visual sign-off. No security surface → no security-auditor gate.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

- [ ] **Step 5: Hand back to controller** — final whole-branch review, then relay the screenshots to the owner for visual sign-off + the merge decision (UI taste is the owner's call).

---

## Self-review (controller: verify before dispatching Task 1)

- **Spec coverage:** §2 decisions → T1 (shadcn foundation-only, theme-ready tokens, self-host fonts) + T2 (Overlay titlebar) + T3 (version-column-dropped); §3 components → T2 (TitleBar/Rail/AppShell) + T3 (StatusPill/Button/ServiceRow/ServicesPanel/LogPane); §4 data flow unchanged → T3 Step 4 reuses ServicesStore/ipc verbatim; §5 error/failed states → T3 (fail-detail + error banner); §6 non-goals honored (no Sites/DB/packages/dark-palette/version); §7 testing → T4 + the per-task vitest; §8 delivery → Global Constraints + T4. Every spec section maps to a task.
- **Type consistency:** `cn`, `runningCount(ServiceStatus[])→number`, `pillClass(StateKind)→string`, `ServiceStatus.state.kind`, `AppShell{runningCount,children}`, `Rail{active}`, `TitleBar{runningCount}` — consistent across tasks and matched to the real `lib/ipc` + `services.svelte.ts` exports.
- **Placeholder scan:** foundation/glue code is complete; the visual components are explicit PORTS of named `mock.css` rules + `main-window.html` line ranges (the authoritative source per Global Constraints), not vague "style it" steps — this is the correct handling for a mockup port and is not a placeholder.
- **Hazards flagged for implementers:** the data layer is frozen (reuse ServicesStore/ipc verbatim — do not re-fetch or restructure); `data-tauri-drag-region` is required on the Overlay titlebar or the window can't be dragged; macOS traffic-light inset (~72px) on the title strip; keep the `data-testid` hooks so existing tests hold; delete `routes/layout.css` only after its base rules are preserved in `tokens.css`; `pnpm check` (svelte-check) must stay clean (Svelte 5 runes + strict TS).
