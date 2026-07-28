<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# php-fpm service state on the Languages page — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a php-fpm pool that fails to start say so, in php-fpm's own words, with a Retry — instead of showing a Start button identical to the one before the attempt.

**Architecture:** One root cause, three tasks. `isRunning` collapses four supervisor states onto a boolean and puts `failed` on the same value as `stopped`; deleting it and passing the whole `serviceState` to the row is what makes the failure surface, the correct control, and a status pill all possible. No new IPC, no Rust.

**Tech Stack:** SvelteKit + Svelte 5 runes, vitest SSR (`svelte/server`, no DOM).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-28-p1-languages-service-state-design.md`. Read it before Task 1.
- SPDX header `<!-- SPDX-License-Identifier: GPL-3.0-or-later -->` on any new `.svelte`, `// SPDX-…` on any new `.ts`.
- `git commit -s` (DCO) on every commit. Conventional Commits.
- UI tests are SSR render-to-string in the `node` vitest project. **There is no DOM and no layout engine** — never assert on computed width, position, or overlap.
- The pill track is **120px**, matching `ServiceRow` and `SiteListRow`. `StatusPill`'s own doc comment states it is written for a fixed 120px grid track.
- `StatusPill` API: `{ kind: StateKind, label?: string, testId?: string }`.
- **The `{#if !row.installed}` Install branch stays first**, ahead of every service-state branch (spec §5.1).
- **`fullVersion` the DTO field stays**; only its column goes (spec §8).
- Do not touch the install failure surface — brew exit codes, the not-detected case, the log pane (spec §9).
- Do not raise licensing: deferred project-wide until after v1.0.0.
- Run before every commit, from `apps/desktop`: `pnpm run test`, `pnpm run lint`, `pnpm run check`. On this machine `pnpm -C apps/desktop <script>` can hit a shim quirk; `cd apps/desktop && pnpm run <script>` works. If `node_modules` is missing, `pnpm install --offline --frozen-lockfile` first.

### The existing test file's helpers — use these, do not invent new ones

`apps/desktop/src/lib/components/LanguageRow.svelte.test.ts` already has:

- **`renderRow({ row, ... })`** — the render helper. It returns the SSR body string. The test code in this plan is written against it; there is no `html(...)` helper in this file.
- **`r(major, installed, overrides?)`** — the `PhpRuntimeDto` builder, e.g. `r('8.3', true, { serviceId: 'php-fpm-8.3' })`.

**Svelte appends a scoped-style hash to class attributes in SSR output**, so an exact-literal class match is unreliable. The file already handles this — see its `/<div class="meta mono[^"]*">/` regex. Match classes by regex with `[^"]*`, never by exact string.

**Existing tests pass `running:` to `renderRow`.** Task 1 changes that prop, so those call sites break. Update them as part of Task 1 — do not leave them, and do not weaken what they assert.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `apps/desktop/src/routes/languages/+page.svelte` | delete `isRunning`; pass the whole state | 1 |
| `apps/desktop/src/lib/components/LanguageRow.svelte` | the control (1), the failure block + rename (2), the pill + column (3) |
| `apps/desktop/src/lib/components/LanguageRow.svelte.test.ts` | all three tasks' assertions |
| `apps/desktop/src/routes/languages/languages-page.test.ts` | updated where it asserts on the old boolean wiring | 1 |

**No new deriver.** The nginx slice put its decision in `webservers.derive.ts` because it had a tri-state config flag to fold in; here the mapping is state → verb with nothing to combine, so a pure function would be indirection without a second caller. Noted as a follow-up in Task 3: three rows now map service state to a verb inline, and unifying them is its own slice.

---

## Task 1: Give the row the state, not a boolean

**Files:**
- Modify: `apps/desktop/src/routes/languages/+page.svelte` — delete `isRunning`, change the prop passed
- Modify: `apps/desktop/src/lib/components/LanguageRow.svelte` — the prop and the control block
- Modify: `apps/desktop/src/lib/components/LanguageRow.svelte.test.ts`
- Modify: `apps/desktop/src/routes/languages/languages-page.test.ts` (only if it asserts on the old wiring — check)

**Interfaces:**
- Consumes: `ServiceStatus['state']` from `$lib/ipc` — the union `{kind:'stopped'} | {kind:'starting'} | {kind:'running'} | {kind:'failed', exit: number|null, stderrTail: string[]}`.
- Produces: `LanguageRow` prop `serviceState: ServiceStatus['state'] | null` replacing `running?: boolean`. Testids `start-{serviceId}`, `stop-{serviceId}`, and the new `retry-{serviceId}`.

- [ ] **Step 1: Write the failing tests**

Add to `apps/desktop/src/lib/components/LanguageRow.svelte.test.ts`. Match the file's existing render helper and `PhpRuntimeDto` fixture rather than inventing new ones — read the top of the file first.

```ts
describe('the pool control', () => {
	it('offers Start when the pool is stopped', () => {
		const out = renderRow({ row: installed, serviceState: { kind: 'stopped' } });
		expect(out).toContain('data-testid="start-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="retry-php-fpm-8.4"');
	});

	it('offers Stop while running or still starting', () => {
		// `starting` gets Stop, not nothing: a start that hangs has to be
		// interruptible or the only way out is quitting the app.
		for (const kind of ['running', 'starting'] as const) {
			const out = renderRow({ row: installed, serviceState: { kind } });
			expect(out, kind).toContain('data-testid="stop-php-fpm-8.4"');
		}
	});

	it('offers Retry after a failure, not another Start', () => {
		// The whole point. `failed` used to collapse onto `stopped`, so the row
		// showed a Start button identical to the one the user had just pressed.
		const out = renderRow({
			row: installed,
			serviceState: { kind: 'failed', exit: 1, stderrTail: ['boom'] }
		});
		expect(out).toContain('data-testid="retry-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="start-php-fpm-8.4"');
	});

	it('renders no control at all while the state is unknown', () => {
		// `null` is the first frame of every visit. A Start button here asserts
		// the pool is stopped before the supervisor has answered.
		const out = renderRow({ row: installed, serviceState: null });
		expect(out).not.toContain('data-testid="start-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="stop-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="retry-php-fpm-8.4"');
	});

	it('still offers Install first when PHP is not installed', () => {
		// Spec §5.1: the not-installed branch outranks every service-state
		// branch. Reversing them would replace Install with nothing on exactly
		// the rows a new user needs it.
		const out = renderRow({ row: notInstalled, serviceState: null });
		expect(out).toContain('data-testid="install-8.4"');
	});
});
```

Define the two fixtures at the top of the new `describe` using the file's own builder, so they carry whatever defaults it already sets:

```ts
	const installed = r('8.4', true, { serviceId: 'php-fpm-8.4' });
	const notInstalled = r('8.4', false);
```

**Also update the existing tests in this file that pass `running:`** — at minimum `offers start and stop for an installed version`, which currently calls `renderRow({ row: r('8.3', true, { serviceId: 'php-fpm-8.3' }), running: false })`. Change `running: false` to `serviceState: { kind: 'stopped' }`, keeping what each one asserts. Search the file for `running` to find them all; the prop no longer exists, so any you miss will fail typecheck rather than silently pass.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd apps/desktop && pnpm exec vitest run src/lib/components/LanguageRow.svelte.test.ts
```

Expected: FAIL — no `retry-php-fpm-8.4` testid exists, and the `null` case still renders Start.

- [ ] **Step 3: Change the prop**

In `LanguageRow.svelte`, replace the `running` prop and its type. Remove:

```ts
		running = false,
```

and

```ts
		/** Whether this row's `serviceId` is currently running — read from the
		 *  shared services store by the caller, never tracked a second time here. */
		running?: boolean;
```

Add in their places:

```ts
		serviceState,
```

```ts
		/** The whole supervised state, not just whether it is running: `failed`
		 *  carries the `stderrTail` this row renders, and a boolean cannot express
		 *  it. Read from the shared services store by the caller, never tracked a
		 *  second time here.
		 *
		 *  `null` means the snapshot has not arrived yet, OR this row has no pool
		 *  at all. Both render no service control — but the not-installed row is
		 *  caught earlier by the `!row.installed` branch, which renders Install. */
		serviceState: ServiceStatus['state'] | null;
```

Add `ServiceStatus` to the file's type import from `../ipc`.

- [ ] **Step 4: Rewrite the control block**

Replace the whole `{:else if row.serviceId}` body — currently a `running` ternary between Stop and Start — with:

```svelte
		{:else if row.serviceId && serviceState !== null}
			{#if serviceState.kind === 'failed'}
				<Button
					variant="quiet"
					size="sm"
					testId="retry-{row.serviceId}"
					ariaLabel="Retry PHP {row.major}"
					onclick={() => onStart(row.serviceId ?? '')}>Retry</Button
				>
			{:else if serviceState.kind === 'stopped'}
				<Button
					variant="quiet"
					size="sm"
					testId="start-{row.serviceId}"
					ariaLabel="Start PHP {row.major}"
					onclick={() => onStart(row.serviceId ?? '')}>Start</Button
				>
			{:else}
				<Button
					variant="quiet"
					size="sm"
					testId="stop-{row.serviceId}"
					ariaLabel="Stop PHP {row.major}"
					onclick={() => onStop(row.serviceId ?? '')}>Stop</Button
				>
			{/if}
		{/if}
```

- [ ] **Step 5: Delete `isRunning` and pass the state**

In `apps/desktop/src/routes/languages/+page.svelte`, delete the whole `isRunning` function including its doc comment. It is the collapse this slice exists to undo, not an implementation worth repairing.

Replace the prop on `<LanguageRow>`:

```svelte
							running={isRunning(runtime.serviceId)}
```

with:

```svelte
							serviceState={runtime.serviceId === null
								? null
								: (servicesStore.services.find((s) => s.id === runtime.serviceId)?.state ?? null)}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd apps/desktop && pnpm exec vitest run src/lib/components/LanguageRow.svelte.test.ts src/routes/languages/languages-page.test.ts
```

Expected: PASS. If `languages-page.test.ts` fails, it is asserting on the old boolean wiring — update those assertions to the new prop; do not weaken them.

- [ ] **Step 7: Prove the tests are not vacuous**

Change the guard in Step 4 from `serviceState !== null` to `true`, and make the final `{:else}` branch render Start instead of Stop, i.e. reintroduce the old boolean behaviour. Confirm **`renders no control at all while the state is unknown` FAILS** and report its actual output — that is the regression a future refactor is most likely to reintroduce, because "not running" and "not known" look interchangeable in a diff.

Restore, confirm green.

- [ ] **Step 8: Run all gates and commit**

```bash
cd apps/desktop && pnpm run test && pnpm run lint && pnpm run check
git add apps/desktop/src/lib/components/LanguageRow.svelte apps/desktop/src/routes/languages/+page.svelte apps/desktop/src/lib/components/LanguageRow.svelte.test.ts apps/desktop/src/routes/languages/languages-page.test.ts
git commit -s -m "feat(ui): give the Languages row the pool's state, not a boolean"
```

---

## Task 2: Say why the pool failed

**Files:**
- Modify: `apps/desktop/src/lib/components/LanguageRow.svelte`
- Modify: `apps/desktop/src/lib/components/LanguageRow.svelte.test.ts`

**Interfaces:**
- Consumes: `serviceState` from Task 1; `serviceState.kind === 'failed'` carries `{ exit: number | null, stderrTail: string[] }`.
- Produces: testid `pool-failed-{serviceId}`.

**Why this task is separate.** Task 1 makes the control correct. This makes the failure *legible*. Without it the user gets a Retry button and still no idea what to change — better than before, but still a dead end.

- [ ] **Step 1: Make room for a second failure**

The row already binds `failed`, and it means **brew's install failed**. A second failure concept under the same name in the same component is how the two get crossed — and they render in different places, so crossing them would put an install error where a runtime error belongs.

Rename the existing binding and every use of it:

```ts
	const installFailed = $derived(rowOutcome !== null && rowOutcome.exitCode !== 0);
```

Its one use in the markup, `{#if failed}`, becomes `{#if installFailed}`. Leave its doc comment's content intact — it explains a real audit finding — but update the name where the comment refers to it.

Do **not** introduce a bare `failed` binding for the service. Read `serviceState.kind === 'failed'` at the point of use, so the two can never be confused by name again.

- [ ] **Step 2: Write the failing tests**

```ts
describe('a failed pool', () => {
	it("shows php-fpm's own words, not just a Retry button", () => {
		// Asserting on CONTENT, not on the presence of a block: an empty <pre>
		// would satisfy a weaker assertion and tell the user nothing about why
		// their pool did not start.
		const out = renderRow({
			row: installed,
			serviceState: {
				kind: 'failed',
				exit: 78,
				stderrTail: ['[08-Jul-2026 10:00:00] ERROR: unable to bind listening socket']
			}
		});
		expect(out).toContain('unable to bind listening socket');
		expect(out).toContain('data-testid="pool-failed-php-fpm-8.4"');
	});

	it('says a failure happened even when php-fpm said nothing', () => {
		// A pool killed by a signal has an empty tail. Rendering only the <pre>
		// would leave a failed row looking identical to a healthy one.
		const out = renderRow({
			row: installed,
			serviceState: { kind: 'failed', exit: null, stderrTail: [] }
		});
		expect(out).toContain('data-testid="pool-failed-php-fpm-8.4"');
		expect(out).toContain('PHP 8.4');
	});

	it('keeps a brew install failure and a pool failure apart', () => {
		// The two render in different places and mean different things. A row
		// showing both must show each in its own block, not one in place of the
		// other.
		const out = renderRow({
			row: installed,
			serviceState: { kind: 'failed', exit: 78, stderrTail: ['pool is broken'] },
			outcome: { major: '8.4', exitCode: 1, detected: false }
		});
		expect(out).toContain('brew exited with code 1');
		expect(out).toContain('pool is broken');
	});
});
```

Match the file's existing `InstallOutcomeDto` fixture shape for `outcome` — read it rather than assuming these field names.

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cd apps/desktop && pnpm exec vitest run src/lib/components/LanguageRow.svelte.test.ts
```

Expected: FAIL — no `pool-failed-php-fpm-8.4` testid.

- [ ] **Step 4: Render the failure**

Add after the existing `{#if installFailed}…{:else if notFound}…{/if}` block, so a pool failure sits below an install failure rather than replacing it:

```svelte
{#if serviceState?.kind === 'failed'}
	<!-- The supervisor's captured stderr is the only thing that explains why a
	     start did not take. Verbatim — a php-fpm startup error names the pool
	     file and the directive that broke, and summarising it would throw away
	     the part that fixes the problem. Same treatment WebServerRow gives a
	     failed nginx. -->
	<p class="error" role="alert" data-testid="pool-failed-{row.serviceId}">
		PHP {row.major}'s pool failed{#if serviceState.exit !== null}&nbsp;(exit {serviceState.exit}){/if}.
	</p>
	{#if serviceState.stderrTail.length > 0}
		<pre class="pool-stderr">{serviceState.stderrTail.join('\n')}</pre>
	{/if}
{/if}
```

Add the style beside the file's existing `.error` rule:

```css
	.pool-stderr {
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		padding: var(--vh-space-2) var(--vh-space-3);
		background: var(--vh-log-bg);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		color: var(--vh-text);
		font-size: var(--vh-text-log);
		line-height: 1.6;
		overflow: auto;
		max-height: 320px;
		white-space: pre-wrap;
	}
```

The `max-height` cap is the same one `WebServerRow` puts on config and stderr text: a long tail must scroll rather than push the next row off the screen.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd apps/desktop && pnpm exec vitest run src/lib/components/LanguageRow.svelte.test.ts
```

Expected: PASS, including Task 1's cases.

- [ ] **Step 6: Prove the tests are not vacuous**

Delete the `<pre class="pool-stderr">` line. Confirm **`shows php-fpm's own words, not just a Retry button` FAILS** on the missing `unable to bind listening socket`. Restore.

Then wrap the whole block in `{#if serviceState.stderrTail.length > 0}` so an empty tail renders nothing at all, and confirm **`says a failure happened even when php-fpm said nothing` FAILS**. Restore, confirm green.

Report both actual outputs.

- [ ] **Step 7: Run all gates and commit**

```bash
cd apps/desktop && pnpm run test && pnpm run lint && pnpm run check
git add apps/desktop/src/lib/components/LanguageRow.svelte apps/desktop/src/lib/components/LanguageRow.svelte.test.ts
git commit -s -m "fix(ui): say why a php-fpm pool failed instead of only that it did"
```

---

## Task 3: Make the state visible

**Files:**
- Modify: `apps/desktop/src/lib/components/LanguageRow.svelte`
- Modify: `apps/desktop/src/lib/components/LanguageRow.svelte.test.ts`

**Interfaces:**
- Consumes: `serviceState` from Task 1; `StatusPill` from `./StatusPill.svelte`, props `{ kind: StateKind, label?: string, testId?: string }`.
- Produces: testid `lang-pill-{major}`.

**Why this task is separate.** Tasks 1 and 2 fix what happens when things go wrong. This one makes the ordinary state readable at all — today the only way to tell a running pool from an unchecked one is to read the button's verb.

- [ ] **Step 1: Write the failing tests**

```ts
describe('the pool status pill', () => {
	it('names the state for a pool the supervisor knows about', () => {
		const out = renderRow({ row: installed, serviceState: { kind: 'running' } });
		expect(out).toContain('data-testid="lang-pill-8.4"');
		expect(out).toContain('running');
	});

	it('renders nothing while the state is unknown', () => {
		// Same rule the control follows: an absent snapshot is not a state.
		const out = renderRow({ row: installed, serviceState: null });
		expect(out).not.toContain('data-testid="lang-pill-8.4"');
	});

	it('drops the full-version column that never had anything to show', () => {
		// It rendered an em dash on EVERY row, installed or not, because no
		// patch-level prober exists. To a reader that is not absent data, it is
		// data that failed to load.
		//
		// Asserting on the CELL, not on the em dash: the path and socket cells
		// render `'—'` too when their values are null, so `not.toContain('—')`
		// would fail for reasons that have nothing to do with this column, and
		// would pass or fail on which fixture happened to be used. This is the
		// same regex the test being deleted used — the version cell was the only
		// `meta mono` with no third class; path and socket add their own.
		const out = renderRow({ row: { ...installed, fullVersion: null }, serviceState: null });
		expect(out).not.toMatch(/<div class="meta mono[^"]*">/);
	});

	it('still names the full version in the install-success message', () => {
		// The FIELD stays; only the column goes. This message is where it is
		// genuinely useful and degrades honestly to the major.
		const out = renderRow({
			row: { ...installed, fullVersion: '8.4.13' },
			serviceState: null,
			outcome: { major: '8.4', exitCode: 0, detected: true }
		});
		expect(out).toContain('8.4.13');
	});
});
```

- [ ] **Step 2: Run to verify they fail**

```bash
cd apps/desktop && pnpm exec vitest run src/lib/components/LanguageRow.svelte.test.ts
```

Expected: FAIL — no `lang-pill-8.4` testid, and the em dash is still rendered.

- [ ] **Step 3: Delete the test that pins the cell you are removing**

`LanguageRow.svelte.test.ts` contains `shows an em dash rather than repeating the major when the patch level is unknown`, which asserts the version cell renders `'—'`. It will fail the moment the cell is gone, and it *should* — it is pinning a cell that no longer exists.

**Delete that test**, and move the reasoning in its comment into the new `drops the full-version column…` test rather than losing it: the finding it recorded (falling back to `row.major` printed the major twice and implied a patch level had been fetched) is still the reason the column is worthless, which is why it is going.

Deleting a test needs a justification, and this is it: the behaviour it guarded — "if we render this cell, render an em dash, not a repeat of the major" — is now unreachable, because we render no cell. It is not being deleted for being inconvenient.

- [ ] **Step 4: Swap the column for the pill**

Import the pill in `LanguageRow.svelte`:

```ts
	import StatusPill from './StatusPill.svelte';
```

Delete the full-version cell entirely — the `<div class="meta mono">{row.fullVersion ?? '—'}</div>` line and the long comment above it, which documented why that cell printed an em dash. Both go; the comment describes a cell that no longer exists.

Put the pill in its place:

```svelte
	<!-- Renders nothing when the state is unknown, the same `{#if}` guard
	     WebServerRow uses: an absent snapshot is not a state to name. -->
	<div class="pill-cell">
		{#if serviceState}
			<StatusPill kind={serviceState.kind} testId="lang-pill-{row.major}" />
		{/if}
	</div>
```

- [ ] **Step 5: Re-tune the grid**

The track count is unchanged — the pill takes the column the full version had. Only its width changes, from the 90px that column used to the 120px `ServiceRow` and `SiteListRow` both use and that `StatusPill`'s own doc comment is written for.

Replace:

```css
		grid-template-columns: minmax(190px, 0.6fr) 90px minmax(180px, 1.4fr) minmax(180px, 1.4fr) auto;
```

with:

```css
		grid-template-columns: minmax(190px, 0.6fr) 120px minmax(180px, 1.4fr) minmax(180px, 1.4fr) auto;
```

Add:

```css
	.pill-cell {
		min-width: 0;
	}
```

- [ ] **Step 6: Run to verify they pass**

```bash
cd apps/desktop && pnpm exec vitest run src/lib/components/LanguageRow.svelte.test.ts
```

Expected: PASS, including Tasks 1 and 2's cases.

- [ ] **Step 7: Prove the tests are not vacuous**

Remove the `{#if serviceState}` guard so the pill renders unconditionally. Confirm **`renders nothing while the state is unknown` FAILS** — and note that with no guard the component would also throw on `serviceState.kind` for a `null` state, which is itself the point. Restore.

Then put the full-version cell back. Confirm **`drops the full-version column that never had anything to show` FAILS** on finding the em dash. Restore, confirm green.

Report both actual outputs.

- [ ] **Step 8: Record the follow-up you are leaving**

Three rows now map a service state to a control verb inline — `ServiceRow`, `WebServerRow`, and this one. That is real duplication and a real drift risk, and unifying them is its own slice with its own review. Do not do it here. Note it in your report so it reaches the PR body.

- [ ] **Step 9: Run all gates and commit**

```bash
cd apps/desktop && pnpm run test && pnpm run lint && pnpm run check
git add apps/desktop/src/lib/components/LanguageRow.svelte apps/desktop/src/lib/components/LanguageRow.svelte.test.ts
git commit -s -m "feat(ui): show the pool's state on the Languages row"
```

---

## Definition of Done

- A stopped pool offers Start; a running or starting one offers Stop; a **failed** one offers **Retry**.
- A failed pool renders php-fpm's own stderr, and still announces the failure when the tail is empty.
- An unknown state renders no control and no pill.
- A not-installed row still renders Install, ahead of everything above.
- A brew install failure and a pool failure render in their own blocks and cannot be mistaken for each other.
- The full-version column is gone; the `fullVersion` field still appears in the install-success message.
- All gates green: `pnpm run test`, `pnpm run lint`, `pnpm run check`.
- Every guard has a recorded vacuity check with its actual failure output.

**Owed human click-through** (this repo cannot drive the real GUI — see the `sandbox-cannot-verify-gui` note):

1. Languages page, an installed PHP with its pool stopped: the row shows a `stopped` pill and a Start button.
2. Press Start: the pill goes `running` and the button becomes Stop.
3. Break that pool's generated config (add a bogus directive to `<home>/config/generated/php/<major>/php-fpm.conf`) and press Start: the row shows a `failed` pill, a **Retry** button, and php-fpm's own error naming the file.
4. The row's columns still line up with the Services and Sites rows — the pill sits in a 120px track like theirs, and there is no em-dash column.
