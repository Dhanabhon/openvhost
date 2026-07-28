<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { InstallOutcomeDto, PhpRuntimeDto, ServiceStatus } from '../ipc';
	import type { UiLog } from '../languages.svelte';
	import Button from './Button.svelte';
	import LogPane from './LogPane.svelte';
	import StatusPill from './StatusPill.svelte';

	let {
		row,
		serviceState,
		installing = '',
		log = [],
		error = '',
		outcome = null,
		onInstall,
		onStart,
		onStop
	}: {
		row: PhpRuntimeDto;
		/** The whole supervised state, not just whether it is running: `failed`
		 *  carries the `stderrTail` this row renders, and a boolean cannot express
		 *  it. Read from the shared services store by the caller, never tracked a
		 *  second time here.
		 *
		 *  `null` means the snapshot has not arrived yet, OR this row has no pool
		 *  at all. Both render no service control — but the not-installed row is
		 *  caught earlier by the `!row.installed` branch, which renders Install. */
		serviceState: ServiceStatus['state'] | null;
		/** The major installing anywhere on the page, '' when idle. Disables
		 *  every install button, not just this row's — only one install can run
		 *  at a time (`LanguagesStore.install`'s own re-entrancy guard). */
		installing?: string;
		log?: UiLog[];
		error?: string;
		/** The last install attempt's outcome, whichever major it was for — this
		 *  row only renders it once `outcome.major` matches its own. */
		outcome?: InstallOutcomeDto | null;
		onInstall: (major: string) => void;
		onStart: (serviceId: string) => void;
		onStop: (serviceId: string) => void;
	} = $props();

	const isInstalling = $derived(installing === row.major);
	const rowOutcome = $derived(outcome && outcome.major === row.major ? outcome : null);
	/** Brew exited 0 but the version was not found afterwards — `detected` exists
	 *  precisely for this case. Silence here is the failure it prevents: without
	 *  this message the user just presses Install again with nothing explaining
	 *  why nothing happened. */
	const notFound = $derived(
		rowOutcome !== null && rowOutcome.exitCode === 0 && !rowOutcome.detected
	);
	const justInstalled = $derived(rowOutcome !== null && rowOutcome.detected && row.installed);
	/** C1 audit finding: `install_php` returns `Ok(InstallOutcomeDto { exit_code:
	 *  Some(1), detected: false })` for a brew run that genuinely failed — a
	 *  non-zero exit (or `None`, killed by a signal) is an OUTCOME to render, not
	 *  a thrown error, so `error` above never carries it. `exitCode !== 0` covers
	 *  both: `1` (or any other non-zero code) and `null` (no code at all) are both
	 *  "not a clean exit". Checked before `notFound`/`justInstalled` in the
	 *  markup below — those only make sense once `exitCode === 0`. */
	const installFailed = $derived(rowOutcome !== null && rowOutcome.exitCode !== 0);
</script>

<div class="row lang-row" data-testid="lang-row-{row.major}">
	<!-- The label is its own element rather than a bare text node so it can carry
	     `white-space: nowrap`. As a bare node it was the only flexible thing in
	     this cell, so the badge's width came straight out of it and "PHP 8.5"
	     broke across two lines — on the recommended row only, which is why every
	     other row looked fine. -->
	<div class="primary">
		<span class="version">PHP {row.major}</span>
		{#if row.recommended}
			<span class="badge recommended">Recommended</span>
		{/if}
	</div>

	<!-- Renders nothing when the state is unknown, the same `{#if}` guard
	     WebServerRow uses: an absent snapshot is not a state to name. -->
	<div class="pill-cell">
		{#if serviceState}
			<StatusPill kind={serviceState.kind} testId="lang-pill-{row.major}" />
		{/if}
	</div>

	<!-- `title` so the full value stays reachable when the cell ellipsizes, same
	     reasoning as ServiceRow.svelte's endpoint column. -->
	<div class="meta mono path" title={row.path ?? undefined}>{row.path ?? '—'}</div>

	<!-- The socket is the first thing anyone needs when a site 502s — plain
	     selectable text rather than a copy-button-only affordance, so it works
	     even before any "copy" UI exists. -->
	<div class="meta mono socket" title={row.socketPath ?? undefined}>{row.socketPath ?? '—'}</div>

	<div class="row-actions">
		{#if !row.installed}
			<Button
				variant="primary"
				size="sm"
				testId="install-{row.major}"
				ariaLabel="Install PHP {row.major}"
				disabled={installing !== ''}
				onclick={() => onInstall(row.major)}
			>
				{isInstalling ? 'Installing…' : 'Install'}
			</Button>
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
	</div>
</div>

{#if log.length > 0}
	<!-- C1 fix: no longer gated on `isInstalling`. `install()`'s `finally` resets
	     `installing` to '' BEFORE this row re-renders with the settled outcome, so
	     gating on it made the log vanish at the exact moment — a failed or killed
	     install — a user most needs to read it. `log` is already scoped to this
	     row's own major (`store.logFor`) and only cleared at the START of this
	     row's NEXT attempt, so it safely survives here on its own. -->
	<LogPane logs={log} />
{/if}

{#if error !== ''}
	<!-- `white-space: pre-wrap` ALSO set inline, duplicating the scoped `.error`
	     rule below — same reason as ApplyDialog.svelte: the SSR test harness
	     (`svelte/server`) never sees scoped styles, only this inline copy. -->
	<p class="error" role="alert" style="white-space: pre-wrap">{error}</p>
{/if}

{#if installFailed}
	<!-- C1 fix: brew's own non-zero exit (or a signal kill, `exitCode === null`)
	     used to render NOTHING — no error (nothing threw), no `notFound` (that
	     branch requires `exitCode === 0`), and by then the log had already been
	     hidden by the `isInstalling` gate above. Spec §6 calls this a "Failed" row
	     state; this is it. -->
	<p class="error" role="alert">
		{#if rowOutcome?.exitCode !== null && rowOutcome?.exitCode !== undefined}
			brew exited with code {rowOutcome.exitCode} while installing PHP {row.major}.
		{:else}
			brew was killed before installing PHP {row.major} finished.
		{/if}
		Check the log above for what brew actually did.
	</p>
{:else if notFound}
	<p class="warn" role="alert">
		Homebrew reported success installing PHP {row.major}, but the version was not found afterwards.
		Check the log above for what brew actually did.
	</p>
{/if}

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

{#if justInstalled}
	<p class="ok" role="status">
		Installed PHP {row.fullVersion ?? row.major}. Apply your sites to start its pool.
	</p>
{/if}

<style>
	/* Same recipe as ServiceRow.svelte's `.row`/`.row-actions` — four data columns
	   (version / path / socket) plus a fixed action column, instead of that row's
	   three, because an installed PHP row has more to show than a running service
	   does. */
	.row {
		display: grid;
		align-items: center;
		gap: var(--vh-space-4);
		padding: 10px var(--vh-space-4);
		border-bottom: 1px solid var(--vh-border);
	}
	.row:last-child {
		border-bottom: 0;
	}
	.row:hover {
		background: color-mix(in oklab, var(--vh-text) 3%, var(--vh-surface));
	}
	.lang-row {
		/* First column min raised 120px -> 190px: it has to hold "PHP 8.5", an
		   8px gap and the ~106px Recommended badge without either wrapping. At
		   120px the badge's width was taken out of the label. The path and
		   socket columns already ellipsize, so the space comes from there. */
		grid-template-columns:
			minmax(190px, 0.6fr) 120px minmax(180px, 1.4fr) minmax(180px, 1.4fr)
			auto;
	}
	.pill-cell {
		min-width: 0;
	}
	.primary {
		font-weight: 600;
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
	}
	.primary .version {
		white-space: nowrap;
	}
	.badge.recommended {
		display: inline-flex;
		align-items: center;
		/* Never absorb the shortfall by squashing: if this cell is ever too
		   narrow the badge keeps its size and the row scrolls, rather than the
		   badge silently collapsing into an unreadable sliver. */
		flex-shrink: 0;
		white-space: nowrap;
		padding: 1px 8px;
		border-radius: var(--vh-radius-pill);
		font-size: var(--vh-text-caption);
		font-weight: 600;
		color: var(--vh-accent);
		background: var(--vh-selected);
		border: 1px solid color-mix(in oklab, var(--vh-accent) 35%, transparent);
	}
	.meta {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.row-actions {
		display: flex;
		justify-content: flex-end;
	}
	.error {
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		font-size: var(--vh-text-table);
	}
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
	.warn {
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		font-size: var(--vh-text-table);
	}
	.ok {
		color: var(--vh-run);
		background: var(--vh-add-tint);
		border: 1px solid color-mix(in oklab, var(--vh-run) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		font-size: var(--vh-text-table);
	}
</style>
