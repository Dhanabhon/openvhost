<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Start/stop on the Web server page — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put a working Start/Stop control on the Web server page, and make sure that when starting does not work the page says why.

**Architecture:** No new machinery. `stack.rs` already registers nginx as a supervised service, `start_service`/`stop_service` already exist as IPC commands, and `servicesStore.start`/`.stop` already wrap them. Four of the five tasks are about the failure surface: a new `config_exists` fact from the backend, a disabled-with-a-reason Start, nginx's own stderr rendered on the row, and a warning when a site's php-fpm pool is down.

**Tech Stack:** Rust (Tauri 2 commands, tauri-specta bindings), SvelteKit + Svelte 5 runes, vitest SSR (`svelte/server`, no DOM).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-28-p1-webserver-start-stop-design.md`. Read it before Task 1.
- SPDX header `// SPDX-License-Identifier: GPL-3.0-or-later` (or `<!-- ... -->` in `.svelte`) on every new file.
- `git commit -s` (DCO sign-off) on every commit. Conventional Commits.
- Rust: `-D warnings`; `clippy::unwrap_used` / `expect_used` denied outside tests; clippy compiles the lib without `cfg(test)`, so a test-only import must be `#[cfg(test)]`-gated.
- `openvhost-core` depends on `openvhost-conf`, never the reverse.
- specta rejects `usize`/`isize`. Rust structs use `#[serde(rename_all = "camelCase")]`, so `config_exists` crosses as `configExists`.
- UI tests are SSR render-to-string in the `node` vitest project. There is no DOM and no layout engine.
- **Copy rule, verbatim from spec §4:** the disabled Start's reason is exactly `No config generated yet — apply your changes first.`
- **Copy rule, spec §4:** `config_exists` reports existence, not validity. No copy may imply the config is known-good.
- Run before every commit: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop lint`, `pnpm -C apps/desktop check`.
- If `node_modules` is missing or stale: `pnpm install --offline --frozen-lockfile` first.

---

## Correction to the spec, found while planning

Spec §4 says `config_exists` is "one `Path::exists()`". **Use `tokio::fs::try_exists` instead**, computed in `list_web_servers` and passed into `web_server_rows`.

The reason is written into the file you are editing. `read_web_server_config`'s doc comment explains at length that a synchronous `std::fs` call here would pin a tokio *worker* if `OPENVHOST_HOME` sits on a stalled network mount, stalling the supervisor event pump and every in-flight command with it. `Path::exists()` is exactly such a call. Keeping the stat async and leaving `web_server_rows` a pure function also keeps that function unit-testable, which Task 1 relies on.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `apps/desktop/src-tauri/src/commands.rs` | `WebServerDto.config_exists`; `web_server_rows` takes it; `list_web_servers` stats the path | 1 |
| `apps/desktop/src/lib/ipc/bindings.ts` | regenerated — `configExists` on the DTO | 1 |
| `apps/desktop/src/lib/components/webserver.panel.test.ts` | existing fixtures gain `configExists` | 1 |
| `apps/desktop/src/lib/webservers.derive.ts` | `startStopFor(...)` — the state→control decision, as data | 2 |
| `apps/desktop/src/lib/webservers.derive.test.ts` | its table | 2 |
| `apps/desktop/src/lib/components/WebServerRow.svelte` | renders the control, the disabled reason, and the stderr backstop | 3, 4 |
| `apps/desktop/src/lib/components/WebServerPanel.svelte` | passes `onStart`/`onStop` through | 3 |
| `apps/desktop/src/routes/web-server/+page.svelte` | wires the shared services store's `start`/`stop`; loads sites | 3, 5 |
| `apps/desktop/src/lib/webservers.derive.ts` | `stoppedPoolsFor(...)` — the 502 warning, as data | 5 |

---

## Task 1: The backend learns whether the config is there

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs` — `WebServerDto`, `WebServerDto::apache()`, `web_server_rows`, `list_web_servers`
- Modify: `apps/desktop/src/lib/ipc/bindings.ts` (regenerated, not hand-edited)
- Modify: `apps/desktop/src/lib/components/webserver.panel.test.ts` (fixtures)

**Interfaces:**
- Produces: `WebServerDto.config_exists: bool` → `configExists: boolean` in TS. `fn web_server_rows(p: &StackPaths, version: Option<String>, config_exists: bool) -> Vec<WebServerDto>`.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod list_web_servers_tests` in `commands.rs`:

```rust
#[test]
fn the_nginx_row_reports_whether_its_config_is_actually_there() {
    // The page disables Start on this fact, so a row that claims a config
    // exists when it does not sends the user at a service that will exit
    // immediately — and one that claims the opposite hides a working button.
    let p = StackPaths {
        home: PathBuf::from("/x/.openvhost"),
        nginx_bin: PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx"),
        nginx_conf: PathBuf::from("/x/.openvhost/config/generated/nginx/nginx.conf"),
    };

    let present = web_server_rows(&p, None, true);
    let nginx = present.iter().find(|r| r.id == "nginx").expect("an nginx row");
    assert!(nginx.config_exists, "true must reach the nginx row");

    let absent = web_server_rows(&p, None, false);
    let nginx = absent.iter().find(|r| r.id == "nginx").expect("an nginx row");
    assert!(!nginx.config_exists, "false must reach the nginx row");
}

#[test]
fn apache_never_claims_a_config() {
    // Apache is unsupported and has no config path at all. Reporting `true`
    // here would be the row asserting something about a file it cannot name.
    let apache = WebServerDto::apache();
    assert_eq!(apache.config_path, None);
    assert!(!apache.config_exists);
}
```

`StackPaths` may already be constructed by a helper in that test module — reuse it if so rather than repeating the literal.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p openvhost-desktop list_web_servers_tests 2>&1 | tail -20
```

Expected: FAIL to compile — `no field config_exists on type WebServerDto`, and `web_server_rows` takes 2 arguments not 3.

- [ ] **Step 3: Add the field and thread it through**

In `commands.rs`, add to `WebServerDto` (after `config_path`):

```rust
    /// Whether a file exists at `config_path` right now.
    ///
    /// EXISTENCE, NOT VALIDITY. nginx is registered to spawn with
    /// `-c <config_path>`, so a missing file means Start exits immediately —
    /// and on a fresh install the file is genuinely absent, because
    /// `provision_home` seeds directories and the welcome page but writes no
    /// config (pinned by `provisioning_no_longer_writes_any_config`). The page
    /// disables Start on this and says why, rather than letting the user find
    /// out by pressing it. A config that exists can still be refused by nginx;
    /// that case is the row's stderr block, not this flag.
    pub config_exists: bool,
```

In `apache()`, add `config_exists: false,`.

Change the signature and the nginx row:

```rust
fn web_server_rows(p: &StackPaths, version: Option<String>, config_exists: bool) -> Vec<WebServerDto> {
```

and inside the nginx literal, after `config_path`:

```rust
            config_exists,
```

In `list_web_servers`, before the `Ok(...)`:

```rust
    // `tokio::fs`, not `Path::exists()`: a sync stat pins a tokio WORKER, and an
    // OPENVHOST_HOME on a stalled network mount would take the supervisor event
    // pump down with it — the same hazard `read_web_server_config` documents
    // below. `unwrap_or(false)` because a stat that ERRORS is not evidence the
    // file is there, and the row must not claim it is.
    let config_exists = tokio::fs::try_exists(&p.nginx_conf).await.unwrap_or(false);
    Ok(web_server_rows(p, version, config_exists))
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p openvhost-desktop list_web_servers_tests 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Regenerate the bindings and fix the fixtures**

`bindings.ts` is committed and regenerated by a test — never hand-edited:

```bash
cargo test -p openvhost-desktop export_bindings
grep -n "configExists" apps/desktop/src/lib/ipc/bindings.ts
```

Expected: the test passes, and `grep` returns one hit inside `export type WebServerDto`. (The export also runs at dev-time from `lib.rs:121`; the test at `lib.rs:352` is the one to invoke deliberately.)

Then `pnpm -C apps/desktop check` will fail on the two fixtures in `webserver.panel.test.ts`. Add `configExists: true` to the `nginx` fixture and `configExists: false` to the `apache` fixture.

- [ ] **Step 6: Prove the test is not vacuous**

Change `list_web_servers` to hardcode `true` instead of stat-ing. Confirm `the_nginx_row_reports_whether_its_config_is_actually_there` still passes (it tests `web_server_rows`, not the command) — **this is expected, and it is the point of the next line.** Then confirm the honest gap: there is no test covering the *command's* stat. Write it down in your report as a known gap rather than pretending the unit test covers it. Restore the stat.

- [ ] **Step 7: Run all gates and commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop check
git add apps/desktop/src-tauri/src/commands.rs apps/desktop/src/lib/ipc/bindings.ts apps/desktop/src/lib/components/webserver.panel.test.ts
git commit -s -m "feat(desktop): report whether the nginx config is actually there"
```

---

## Task 2: The state→control decision, as data

**Files:**
- Modify: `apps/desktop/src/lib/webservers.derive.ts`
- Modify: `apps/desktop/src/lib/webservers.derive.test.ts`

**Interfaces:**
- Consumes: `statusFor(services, serviceId): ServiceStatus['state']['kind'] | null` (already in this file).
- Produces:

```ts
export type StartStopControl =
    | { kind: 'none' }
    | { kind: 'start'; disabled: boolean; reason: string }
    | { kind: 'retry' }
    | { kind: 'stop' };

export function startStopFor(
    statusKind: ServiceStatus['state']['kind'] | null,
    configExists: boolean
): StartStopControl;
```

This is a pure function in the `derive` module so the decision can be tested as a table rather than through eight rendered strings. `WebServerRow.svelte` then only has to render what it is told.

- [ ] **Step 1: Write the failing test**

Append to `apps/desktop/src/lib/webservers.derive.test.ts`:

```ts
describe('startStopFor', () => {
	it('renders no control at all while the state is unknown', () => {
		// `statusFor` returns null for the first frame of EVERY visit — the route
		// fires load() and the shared subscription resolves after the first paint.
		// A Start button here would be the page asserting nginx is stopped before
		// it has asked, and the user would be one click from starting something
		// whose state they were never shown.
		expect(startStopFor(null, true)).toEqual({ kind: 'none' });
		expect(startStopFor(null, false)).toEqual({ kind: 'none' });
	});

	it('offers Start when stopped with a config to start against', () => {
		expect(startStopFor('stopped', true)).toEqual({
			kind: 'start',
			disabled: false,
			reason: ''
		});
	});

	it('disables Start with a reason when there is no config yet', () => {
		// nginx spawns with `-c <config>`; without the file it exits immediately.
		expect(startStopFor('stopped', false)).toEqual({
			kind: 'start',
			disabled: true,
			reason: 'No config generated yet — apply your changes first.'
		});
	});

	it('offers Retry after a failure, and does not re-disable it', () => {
		// A failed service HAS been started, so a config existed at least once.
		// Disabling Retry on a stale `configExists: false` would strand the user
		// on a row whose own error text is telling them to try again.
		expect(startStopFor('failed', true)).toEqual({ kind: 'retry' });
		expect(startStopFor('failed', false)).toEqual({ kind: 'retry' });
	});

	it('offers Stop while running or still starting', () => {
		// `starting` gets Stop, not nothing: a start that hangs must be
		// interruptible, or the only way out is quitting the app.
		expect(startStopFor('running', true)).toEqual({ kind: 'stop' });
		expect(startStopFor('starting', true)).toEqual({ kind: 'stop' });
	});

	it('never disables Stop on a missing config', () => {
		// The process is running. Whether a file is on disk has no bearing on
		// whether the user may stop it.
		expect(startStopFor('running', false)).toEqual({ kind: 'stop' });
		expect(startStopFor('starting', false)).toEqual({ kind: 'stop' });
	});
});
```

Add `startStopFor` to the file's existing import from `./webservers.derive`.

- [ ] **Step 2: Run the test to verify it fails**

```bash
pnpm -C apps/desktop vitest run src/lib/webservers.derive.test.ts 2>&1 | tail -15
```

Expected: FAIL — `startStopFor is not a function`.

- [ ] **Step 3: Implement it**

Append to `apps/desktop/src/lib/webservers.derive.ts`:

```ts
/** The reason a Start button is disabled. Spec §4 fixes this string; the form
 *  and its test both read it from here so they cannot drift apart. It names the
 *  next step rather than only the problem — "no config" alone leaves the user
 *  to guess that Apply is what produces one. */
export const NO_CONFIG_REASON = 'No config generated yet — apply your changes first.';

/** What the row's service control should be right now.
 *
 *  A discriminated union rather than a pile of booleans on the component: the
 *  choice is a decision, it is testable as a table here, and the component is
 *  left with nothing to decide. */
export type StartStopControl =
	| { kind: 'none' }
	| { kind: 'start'; disabled: boolean; reason: string }
	| { kind: 'retry' }
	| { kind: 'stop' };

/**
 * `statusKind === null` means the supervisor snapshot has NOT ARRIVED, which is
 * not the same as "stopped" — see the test. It renders nothing, the same rule
 * the status pill already follows (`{#if statusKind}` in WebServerRow.svelte).
 *
 * `configExists` only ever gates `start`. A `failed` service has already been
 * started once, and a `running` one is a live process; neither decision has
 * anything to do with a file being on disk right now.
 */
export function startStopFor(
	statusKind: ServiceStatus['state']['kind'] | null,
	configExists: boolean
): StartStopControl {
	if (statusKind === null) return { kind: 'none' };
	if (statusKind === 'failed') return { kind: 'retry' };
	if (statusKind === 'stopped') {
		return configExists
			? { kind: 'start', disabled: false, reason: '' }
			: { kind: 'start', disabled: true, reason: NO_CONFIG_REASON };
	}
	return { kind: 'stop' };
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
pnpm -C apps/desktop vitest run src/lib/webservers.derive.test.ts 2>&1 | tail -10
```

Expected: PASS, all six cases.

- [ ] **Step 5: Prove the tests are not vacuous**

Make `startStopFor` return `{ kind: 'start', disabled: false, reason: '' }` for `null`. Confirm **`renders no control at all while the state is unknown` FAILS** and report its actual output. Restore.

Then make the `stopped` branch ignore `configExists` and always return `disabled: false`. Confirm **`disables Start with a reason when there is no config yet` FAILS**. Restore, confirm green.

- [ ] **Step 6: Run gates and commit**

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop check
git add apps/desktop/src/lib/webservers.derive.ts apps/desktop/src/lib/webservers.derive.test.ts
git commit -s -m "feat(ui): decide the web server row's service control as data"
```

---

## Task 3: The control on the row

**Files:**
- Modify: `apps/desktop/src/lib/components/WebServerRow.svelte`
- Modify: `apps/desktop/src/lib/components/WebServerPanel.svelte`
- Modify: `apps/desktop/src/routes/web-server/+page.svelte`
- Modify: `apps/desktop/src/lib/components/webserver.panel.test.ts`

**Interfaces:**
- Consumes: `startStopFor(statusKind, configExists): StartStopControl` from Task 2 — the component reads `control.reason` and does **not** import `NO_CONFIG_REASON`, so the string has exactly one home; `WebServerDto.configExists` from Task 1; `servicesStore.start(id)` / `.stop(id)` (already exist, `apps/desktop/src/lib/services.svelte.ts:104-110`).
- Produces: `WebServerRow` and `WebServerPanel` both gain `onStart: (serviceId: string) => void` and `onStop: (serviceId: string) => void`.

- [ ] **Step 1: Write the failing tests**

Append to `apps/desktop/src/lib/components/webserver.panel.test.ts`. The file's existing `html()` helper renders the panel — match its signature, and pass `onStart`/`onStop` as no-ops where the case does not care.

```ts
describe('the service control', () => {
	it('renders nothing at all before the services snapshot arrives', () => {
		// An empty services array is what the very first frame of every visit
		// looks like. Neither a Start nor a Stop may appear.
		const out = html({ servers: [nginx], services: [] });
		expect(out).not.toContain('data-testid="ws-start-nginx"');
		expect(out).not.toContain('data-testid="ws-stop-nginx"');
	});

	it('offers Start when nginx is stopped and a config exists', () => {
		const out = html({ servers: [nginx], services: [svc('nginx', { kind: 'stopped' })] });
		expect(out).toContain('data-testid="ws-start-nginx"');
		// Asserting the REASON is absent, not that the word "disabled" is absent
		// anywhere in the panel: this is a whole-panel string, and any other
		// control acquiring a `disabled` attribute later would fail this case for
		// a reason that has nothing to do with Start.
		expect(out).not.toContain('ws-start-reason-nginx');
	});

	it('disables Start and says why when no config has been generated', () => {
		const out = html({
			servers: [{ ...nginx, configExists: false }],
			services: [svc('nginx', { kind: 'stopped' })]
		});
		expect(out).toContain('data-testid="ws-start-nginx"');
		expect(out).toContain('disabled');
		expect(out).toContain('ws-start-reason-nginx');
		expect(out).toContain('No config generated yet');
	});

	it('offers Stop while running', () => {
		const out = html({ servers: [nginx], services: [svc('nginx', { kind: 'running' })] });
		expect(out).toContain('data-testid="ws-stop-nginx"');
		expect(out).not.toContain('data-testid="ws-start-nginx"');
	});

	it('gives Apache no service control, since it supervises nothing', () => {
		const out = html({ servers: [apache], services: [] });
		expect(out).not.toContain('ws-start-apache');
		expect(out).not.toContain('ws-stop-apache');
	});
});
```

- [ ] **Step 2: Run to verify they fail**

```bash
pnpm -C apps/desktop vitest run src/lib/components/webserver.panel.test.ts 2>&1 | tail -20
```

Expected: FAIL — the `ws-start-nginx` testid is absent.

- [ ] **Step 3: Render the control**

In `WebServerRow.svelte`, add to the props block (alongside `onShowConfig`/`onValidate`):

```ts
		onStart,
		onStop
```

and to its type:

```ts
		onStart: (serviceId: string) => void;
		onStop: (serviceId: string) => void;
```

Import the deriver in the same `<script>`:

```ts
	import { startStopFor } from '$lib/webservers.derive';
```

and derive:

```ts
	// `server.serviceId` is null for a brand OpenVHost does not supervise
	// (Apache), which is a different "no control" from "state not yet known" —
	// both render nothing, but only one of them can ever change.
	const control = $derived(
		server.serviceId === null ? { kind: 'none' as const } : startStopFor(statusKind, server.configExists)
	);
```

In the `.row-actions` group, **before** the existing Show config and Validate buttons — those are diagnostics, this is the action the page exists for:

```svelte
	{#if control.kind === 'start'}
		<Button
			variant="quiet"
			size="sm"
			testId="ws-start-{server.id}"
			ariaLabel="Start {server.displayName}"
			disabled={control.disabled}
			onclick={() => onStart(server.serviceId ?? '')}>Start</Button
		>
	{:else if control.kind === 'retry'}
		<Button
			variant="quiet"
			size="sm"
			testId="ws-retry-{server.id}"
			ariaLabel="Retry {server.displayName}"
			onclick={() => onStart(server.serviceId ?? '')}>Retry</Button
		>
	{:else if control.kind === 'stop'}
		<Button
			variant="quiet"
			size="sm"
			testId="ws-stop-{server.id}"
			ariaLabel="Stop {server.displayName}"
			onclick={() => onStop(server.serviceId ?? '')}>Stop</Button
		>
	{/if}
```

Below the head line, render the reason when there is one:

```svelte
	{#if control.kind === 'start' && control.disabled}
		<!-- The disabled button alone is a dead end: it says "not now" without
		     saying when. This names the action that produces a config. -->
		<p class="unavailable" data-testid="ws-start-reason-{server.id}">{control.reason}</p>
	{/if}
```

`.unavailable` already exists in this file's `<style>`.

- [ ] **Step 4: Thread the callbacks through**

In `WebServerPanel.svelte`, add `onStart` and `onStop` to the props and their types (same shape as `onShowConfig`), and pass `{onStart}` `{onStop}` to `<WebServerRow>`.

In `apps/desktop/src/routes/web-server/+page.svelte`, on `<WebServerPanel>`:

```svelte
	onStart={(id) => void servicesStore.start(id)}
	onStop={(id) => void servicesStore.stop(id)}
```

`servicesStore` is already imported on this route. Its `start`/`stop` route failures onto `servicesStore.error`, which `AppShell` already renders — so a rejected command is already surfaced and needs nothing new here.

- [ ] **Step 5: Run to verify they pass**

```bash
pnpm -C apps/desktop vitest run src/lib/components/webserver.panel.test.ts 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 6: Prove the tests are not vacuous**

Change the `control` derivation to `startStopFor(statusKind ?? 'stopped', server.configExists)`. Confirm **`renders nothing at all before the services snapshot arrives` FAILS**, and report the output — that fallback is the single most likely regression here, because "not running" and "not known" look interchangeable in a diff.

Restore. Then drop `disabled={control.disabled}` from the Start button and confirm **`disables Start and says why...` FAILS**. Restore, confirm green.

- [ ] **Step 7: Run gates and commit**

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop check
git add apps/desktop/src/lib/components/WebServerRow.svelte apps/desktop/src/lib/components/WebServerPanel.svelte apps/desktop/src/routes/web-server/+page.svelte apps/desktop/src/lib/components/webserver.panel.test.ts
git commit -s -m "feat(ui): start and stop nginx from the Web server page"
```

---

## Task 4: The backstop — nginx's own words

**Files:**
- Modify: `apps/desktop/src/lib/components/WebServerRow.svelte`
- Modify: `apps/desktop/src/lib/components/webserver.panel.test.ts`

**Interfaces:**
- Consumes: the `services` prop already carries the whole `ServiceState`; `failed` carries `{ exit: number | null, stderrTail: string[] }`.
- Produces: nothing later tasks depend on.

**Why this task exists.** Task 3's guard prevents one predictable failure. This covers every unpredictable one: a directive nginx rejects, a port already bound, a permission problem, a docroot deleted since Apply, or the config file vanishing between the page load and the click. Without it the pill flips to `failed` and the page says nothing — the dead-end shape this project keeps rediscovering.

The row currently receives only `statusKind` (a kind, not the state), so it **cannot** see `stderrTail`. That is the first thing to change.

- [ ] **Step 1: Write the failing test**

```ts
describe('a failed nginx', () => {
	it("shows nginx's own words, not just a failed pill", () => {
		// The whole point. Asserting on the CONTENT, not on the presence of a
		// block: an empty <pre> would satisfy a weaker assertion and tell the
		// user nothing about why their web server did not start.
		const out = html({
			servers: [nginx],
			services: [
				svc('nginx', {
					kind: 'failed',
					exit: 1,
					stderrTail: ['nginx: [emerg] bind() to 0.0.0.0:8080 failed (48: Address already in use)']
				})
			]
		});
		expect(out).toContain('Address already in use');
		expect(out).toContain('data-testid="ws-failed-nginx"');
	});

	it('offers Retry rather than Start after a failure', () => {
		const out = html({
			servers: [nginx],
			services: [svc('nginx', { kind: 'failed', exit: 1, stderrTail: ['boom'] })]
		});
		expect(out).toContain('data-testid="ws-retry-nginx"');
	});

	it('says a failure happened even when nginx said nothing', () => {
		// A service killed by a signal has an empty tail. Rendering only the
		// <pre> would leave a failed row that looks identical to a healthy one.
		const out = html({
			servers: [nginx],
			services: [svc('nginx', { kind: 'failed', exit: null, stderrTail: [] })]
		});
		expect(out).toContain('data-testid="ws-failed-nginx"');
		expect(out).toContain('nginx failed');
	});
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm -C apps/desktop vitest run src/lib/components/webserver.panel.test.ts 2>&1 | tail -20
```

Expected: FAIL — no `ws-failed-nginx` testid.

- [ ] **Step 3: Pass the whole state to the row, not just its kind**

In `WebServerPanel.svelte`, replace the `statusKind={statusFor(...)}` prop with the whole entry, keeping the kind derived inside the row:

```svelte
					serviceState={services.find((s) => s.id === server.serviceId)?.state ?? null}
```

In `WebServerRow.svelte`, replace the `statusKind` prop with:

```ts
		serviceState
```

typed as:

```ts
		/** The whole supervised state, not just its kind: `failed` carries the
		 *  `stderrTail` this row renders, and a kind alone cannot express it.
		 *  `null` when the brand is unsupervised OR the snapshot has not
		 *  arrived — both render no control (see `startStopFor`). */
		serviceState: ServiceStatus['state'] | null;
```

and derive the kind locally so the existing pill and `startStopFor` call are unchanged in meaning:

```ts
	const statusKind = $derived(serviceState?.kind ?? null);
```

Import `ServiceStatus` as a type from `$lib/ipc` if it is not already imported.

`statusFor` in `webservers.derive.ts` may now have no callers. Check with `git grep -n statusFor` — if the panel was its only user, delete it and its tests in this commit rather than leaving a dead export; if the Services page uses it, leave it alone.

- [ ] **Step 4: Render the failure**

After the existing `{#if report}` block in `WebServerRow.svelte`:

```svelte
{#if serviceState?.kind === 'failed'}
	<!-- Same recipe as ServiceRow.svelte's `fail-detail`, and for the same
	     reason: the supervisor's captured stderr is the only thing that
	     explains why a start did not take. Verbatim — an nginx [emerg] line
	     names the file and line number, and summarising it would throw away
	     the part that fixes the problem. -->
	<div class="report report-fail" role="status" data-testid="ws-failed-nginx">
		<p class="headline">
			{server.displayName} failed{#if serviceState.exit !== null}&nbsp;(exit {serviceState.exit}){/if}
		</p>
		{#if serviceState.stderrTail.length > 0}
			<pre>{serviceState.stderrTail.join('\n')}</pre>
		{/if}
	</div>
{/if}
```

The testid is `ws-failed-nginx` for the nginx row because `server.id` is `nginx` — use `data-testid="ws-failed-{server.id}"` so it is right for any brand.

`.report`, `.report-fail`, `.headline` and `.report pre` already exist in this file's `<style>`.

- [ ] **Step 5: Run to verify they pass**

```bash
pnpm -C apps/desktop vitest run src/lib/components/webserver.panel.test.ts 2>&1 | tail -10
```

Expected: PASS. Also re-run Task 3's cases — the prop change touches them.

- [ ] **Step 6: Prove the tests are not vacuous**

Remove the `<pre>` line. Confirm **`shows nginx's own words, not just a failed pill` FAILS** on the missing `Address already in use`. Report the output.

Restore. Then wrap the whole block in `{#if serviceState.stderrTail.length > 0}` so an empty tail renders nothing, and confirm **`says a failure happened even when nginx said nothing` FAILS**. Restore, confirm green.

- [ ] **Step 7: Run gates and commit**

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop check
git add apps/desktop/src/lib/components/WebServerRow.svelte apps/desktop/src/lib/components/WebServerPanel.svelte apps/desktop/src/lib/webservers.derive.ts apps/desktop/src/lib/webservers.derive.test.ts apps/desktop/src/lib/components/webserver.panel.test.ts
git commit -s -m "fix(ui): say why nginx failed instead of only that it did"
```

---

## Task 5: Closing the 502

**Files:**
- Modify: `apps/desktop/src/lib/webservers.derive.ts`
- Modify: `apps/desktop/src/lib/webservers.derive.test.ts`
- Modify: `apps/desktop/src/routes/web-server/+page.svelte`
- Modify: `apps/desktop/src/lib/components/WebServerPanel.svelte`
- Modify: `apps/desktop/src/lib/components/webserver.panel.test.ts`

**Interfaces:**
- Consumes: `SiteDto { phpVersion: string; enabled: boolean; ... }` from `$lib/ipc`; `listSites(): Promise<SiteDto[]>` (`apps/desktop/src/lib/ipc/index.ts:139`); the pool service id format `php-fpm-<major>` (`apps/desktop/src-tauri/src/stack.rs:79`).
- Produces: `stoppedPoolsFor(sites, services, nginxRunning): string[]` — the PHP majors to warn about, sorted, deduped.

**Before writing code:** confirm `SiteDto.phpVersion` holds the major (`"8.4"`) that `php_fpm_spec` builds its id from, and not a full version (`"8.4.13"`). Check how the Sites editor stores it. If it is a full version, the mapping needs the major extracted — say so in your report and implement it, do not assume.

- [ ] **Step 1: Write the failing test**

```ts
describe('stoppedPoolsFor', () => {
	const site = (phpVersion: string, enabled = true): SiteDto => ({
		id: `s-${phpVersion}-${enabled}`,
		name: 'x',
		domain: 'x.localhost',
		docroot: '/x',
		webServer: 'nginx',
		phpVersion,
		enabled,
		createdAt: 0,
		updatedAt: 0
	});

	it('names a pool an enabled site needs that is not running', () => {
		// The 502 this exists to prevent: nginx up, pool down, site dead, and
		// nothing on screen connecting the three.
		expect(
			stoppedPoolsFor([site('8.4')], [svc('php-fpm-8.4', { kind: 'stopped' })], true)
		).toEqual(['8.4']);
	});

	it('stays quiet when the pool is running', () => {
		expect(
			stoppedPoolsFor([site('8.4')], [svc('php-fpm-8.4', { kind: 'running' })], true)
		).toEqual([]);
	});

	it('ignores disabled sites', () => {
		// Warning about a pool nothing is serving would train the user to
		// ignore this line, and then it fails when it matters.
		expect(
			stoppedPoolsFor([site('8.4', false)], [svc('php-fpm-8.4', { kind: 'stopped' })], true)
		).toEqual([]);
	});

	it('stays quiet while nginx itself is stopped', () => {
		// The user has not asked to serve anything yet. A pool warning here is
		// noise about a problem they do not have.
		expect(
			stoppedPoolsFor([site('8.4')], [svc('php-fpm-8.4', { kind: 'stopped' })], false)
		).toEqual([]);
	});

	it('names a pool that is missing from the snapshot entirely', () => {
		// A PHP major with no registered service is not running by definition —
		// this is the never-installed case, and it is the one most likely to
		// bite a new user.
		expect(stoppedPoolsFor([site('8.4')], [], true)).toEqual(['8.4']);
	});

	it('names each version once, in order, however many sites share it', () => {
		expect(
			stoppedPoolsFor([site('8.4'), site('8.3'), site('8.4')], [], true)
		).toEqual(['8.3', '8.4']);
	});
});
```

Import `SiteDto` as a type in the test file.

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm -C apps/desktop vitest run src/lib/webservers.derive.test.ts 2>&1 | tail -15
```

Expected: FAIL — `stoppedPoolsFor is not a function`.

- [ ] **Step 3: Implement it**

Append to `apps/desktop/src/lib/webservers.derive.ts`:

```ts
/**
 * The PHP majors an enabled site needs whose php-fpm pool is not running.
 *
 * A PHP site needs nginx AND a pool. Starting nginx alone leaves the site
 * answering 502 with nothing on screen connecting the two — the page names the
 * gap instead of letting the user find it in a browser.
 *
 * Only while nginx is RUNNING: with nginx stopped the user has not asked to
 * serve anything, and a pool warning would be noise about a problem they do not
 * have. Only ENABLED sites: a disabled site's pool is genuinely not needed, and
 * warning about it teaches the user to ignore this line.
 *
 * A major missing from the snapshot counts as not running. That is the
 * never-installed case, which is the one most likely to bite a new user, and
 * treating "absent" as "fine" would hide exactly that.
 */
export function stoppedPoolsFor(
	sites: readonly SiteDto[],
	services: readonly ServiceStatus[],
	nginxRunning: boolean
): string[] {
	if (!nginxRunning) return [];
	const needed = new Set(sites.filter((s) => s.enabled).map((s) => s.phpVersion));
	const down = [...needed].filter(
		(major) => services.find((s) => s.id === `php-fpm-${major}`)?.state.kind !== 'running'
	);
	return down.sort();
}
```

Add `SiteDto` to this file's type imports from `$lib/ipc`.

- [ ] **Step 4: Run to verify it passes**

```bash
pnpm -C apps/desktop vitest run src/lib/webservers.derive.test.ts 2>&1 | tail -10
```

Expected: PASS, all six cases.

- [ ] **Step 5: Render it and wire the sites load**

In `WebServerPanel.svelte`, add a `stoppedPools: readonly string[]` prop (default `[]`) and render above the `.rowlist`, inside the panel:

```svelte
	{#if stoppedPools.length > 0}
		<p class="pool-warning" role="status" data-testid="ws-pool-warning">
			nginx is running, but {stoppedPools.length === 1 ? 'the pool' : 'the pools'} your sites need
			{stoppedPools.length === 1 ? 'is' : 'are'} not: PHP {stoppedPools.join(', ')}. A PHP site will
			answer 502 until {stoppedPools.length === 1 ? 'it starts' : 'they start'}. Start
			{stoppedPools.length === 1 ? 'it' : 'them'} on the Languages page.
		</p>
	{/if}
```

with a style following the file's existing tokens:

```css
	.pool-warning {
		margin: 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		border-bottom: 1px solid var(--vh-border);
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
	}
```

In `apps/desktop/src/routes/web-server/+page.svelte`, hold the sites as a plain `$state` list rather than constructing a `SitesStore`. That store's constructor demands `createSite`, `updateSite`, `deleteSite` and `openSite` (`routes/+page.svelte:30`), and this page has no business holding four mutation commands to render one warning.

```ts
	import { listSites, /* …existing… */ } from '$lib/ipc';
	import type { SiteDto } from '$lib/ipc';
	import { statusFor, stoppedPoolsFor } from '$lib/webservers.derive';

	// Read-only, and only to answer "which php-fpm pools do the sites need".
	// Deliberately NOT a SitesStore: that carries create/update/delete/open, and
	// this page must not be able to change a site.
	let sites = $state<SiteDto[]>([]);

	const stoppedPools = $derived(
		stoppedPoolsFor(
			sites,
			servicesStore.services,
			statusFor(servicesStore.services, 'nginx') === 'running'
		)
	);
```

In the existing `onMount`, alongside the other loads:

```ts
		// state.db only — spawns nothing, so it is cheap enough to fire with the
		// rest. A failure leaves `sites` empty, which suppresses the pool warning
		// rather than blanking the page: a missing hint is a smaller harm than a
		// page that will not render, and the row's own state is unaffected.
		void listSites()
			.then((s) => (sites = s))
			.catch(() => {});
```

Then pass `{stoppedPools}` to `<WebServerPanel>`.

- [ ] **Step 6: Add the panel-level test**

```ts
it('warns that a site will 502 when its pool is down', () => {
	const out = html({
		servers: [nginx],
		services: [svc('nginx', { kind: 'running' }), svc('php-fpm-8.4', { kind: 'stopped' })],
		stoppedPools: ['8.4']
	});
	expect(out).toContain('data-testid="ws-pool-warning"');
	expect(out).toContain('502');
	expect(out).toContain('8.4');
});
```

- [ ] **Step 7: Prove it is not vacuous**

Make `stoppedPoolsFor` return `[]` unconditionally. Confirm **four of the six cases FAIL**. Restore.

Then drop the `nginxRunning` guard. Confirm **`stays quiet while nginx itself is stopped` FAILS**. Restore, confirm green.

- [ ] **Step 8: Run all gates and commit**

```bash
cargo test --workspace
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop check
git add apps/desktop/src/lib/webservers.derive.ts apps/desktop/src/lib/webservers.derive.test.ts apps/desktop/src/routes/web-server/+page.svelte apps/desktop/src/lib/components/WebServerPanel.svelte apps/desktop/src/lib/components/webserver.panel.test.ts
git commit -s -m "feat(ui): name the stopped pool that would 502 a site"
```

---

## Definition of Done

- Start, Stop and Retry work on the Web server page against the real supervisor.
- Start is disabled with `No config generated yet — apply your changes first.` when no config exists, and live when one does.
- A failed nginx renders its own stderr on the row.
- With nginx running and a required pool stopped, the page names the version and points at Languages.
- No control renders at all before the services snapshot arrives.
- All gates green: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop test`, `lint`, `check`.
- Every guard has a recorded vacuity check with its actual failure output.

**Owed human click-through** (this repo cannot drive the real GUI — see the `sandbox-cannot-verify-gui` note):

1. On a home with no generated config: Start is greyed with the reason beneath it.
2. Apply once, return: Start is live. Press it — the pill goes `running`.
3. Press Stop — the pill returns to `stopped`.
4. With nginx running and a PHP site whose pool is stopped: the 502 warning names that version.
5. Break the config on purpose (add `zzz;` to the generated `nginx.conf`), press Start: the row shows nginx's `[emerg]` line naming the file and line.
