<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { InstallOutcomeDto, PhpRuntimeDto } from '../ipc';
	import type { UiLog } from '../languages.svelte';
	import Button from './Button.svelte';
	import LogPane from './LogPane.svelte';

	let {
		row,
		running = false,
		installing = '',
		log = [],
		error = '',
		outcome = null,
		onInstall,
		onStart,
		onStop
	}: {
		row: PhpRuntimeDto;
		/** Whether this row's `serviceId` is currently running — read from the
		 *  shared services store by the caller, never tracked a second time here. */
		running?: boolean;
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
</script>

<div class="row lang-row" data-testid="lang-row-{row.major}">
	<div class="primary">
		PHP {row.major}
		{#if row.recommended}
			<span class="badge recommended">Recommended</span>
		{/if}
	</div>

	<div class="meta mono">{row.installed ? (row.fullVersion ?? row.major) : '—'}</div>

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
		{:else if row.serviceId}
			{#if running}
				<Button
					variant="quiet"
					size="sm"
					testId="stop-{row.serviceId}"
					ariaLabel="Stop PHP {row.major}"
					onclick={() => onStop(row.serviceId ?? '')}>Stop</Button
				>
			{:else}
				<Button
					variant="quiet"
					size="sm"
					testId="start-{row.serviceId}"
					ariaLabel="Start PHP {row.major}"
					onclick={() => onStart(row.serviceId ?? '')}>Start</Button
				>
			{/if}
		{/if}
	</div>
</div>

{#if isInstalling && log.length > 0}
	<LogPane logs={log} />
{/if}

{#if error !== ''}
	<!-- `white-space: pre-wrap` ALSO set inline, duplicating the scoped `.error`
	     rule below — same reason as ApplyDialog.svelte: the SSR test harness
	     (`svelte/server`) never sees scoped styles, only this inline copy. -->
	<p class="error" role="alert" style="white-space: pre-wrap">{error}</p>
{/if}

{#if notFound}
	<p class="warn" role="alert">
		Homebrew reported success installing PHP {row.major}, but the version was not found afterwards.
		Check the log above for what brew actually did.
	</p>
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
		grid-template-columns: minmax(120px, 0.6fr) 90px minmax(200px, 1.4fr) minmax(200px, 1.4fr) auto;
	}
	.primary {
		font-weight: 600;
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.badge.recommended {
		display: inline-flex;
		align-items: center;
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
