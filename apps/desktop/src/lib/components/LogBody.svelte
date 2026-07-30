<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  The log pane itself: the distinct rendered states (spec D6 — empty,
  not-yet-created, permission-denied, unavailable, rotated,
  scan-bound-reached lives in LogStatusLine) plus the row list, the reset
  notice, auto-scroll-on-follow, and the scroll listener that disengages
  Follow when the user scrolls away.
-->
<script lang="ts">
	import type { IpcError, LogResetDto, LogRowDto, LogSourceDto } from '$lib/ipc';
	import { errorMessage } from '$lib/errors';
	import { describeSource, logBodyState } from '$lib/logs.derive';
	import {
		emptyCopy,
		genericReadErrorCopy,
		noSelectionCopy,
		notYetCreatedCopy,
		permissionDeniedCopy,
		resetNoticeCopy,
		unavailableSourceCopy
	} from '$lib/logs.copy';
	import LogLevelBadge from './LogLevelBadge.svelte';

	let {
		selected,
		requestedUnavailable,
		readError,
		exists,
		rows,
		filtered,
		reset,
		follow,
		onRevealFolder,
		onScroll
	}: {
		selected: LogSourceDto | null;
		requestedUnavailable: LogSourceDto | null;
		readError: IpcError | null;
		exists: boolean;
		rows: readonly LogRowDto[];
		/** Whether a needle or a level floor is currently active — `emptyCopy`
		 *  reads this to say "no lines match" instead of "this log is empty"
		 *  when the file genuinely has content the filter is hiding. */
		filtered: boolean;
		reset: LogResetDto | null;
		follow: boolean;
		onRevealFolder: () => void;
		/** `nearBottom`: false is what turns Follow off (spec D6 — "scrolling
		 *  away turns it off"); the store, not this component, owns that
		 *  decision (`LogsStore.setFollow`), so this only reports the fact. */
		onScroll: (nearBottom: boolean) => void;
	} = $props();

	const state = $derived(
		logBodyState({ selected, requestedUnavailable, readError, exists, rowCount: rows.length })
	);

	let logEl: HTMLDivElement | undefined;

	// Auto-follow: whenever `rows` is replaced (a poll, a fresh selection)
	// AND the caller says we are following, jump to the bottom. Reading
	// `rows` in the condition registers the reactive dependency, the same
	// convention `LogPane.svelte`'s identical effect already documents.
	$effect(() => {
		if (follow && rows && logEl) logEl.scrollTop = logEl.scrollHeight;
	});

	/** How close to the bottom still counts as "at the bottom" — a user
	 *  resting the wheel a few pixels short of the very last line should not
	 *  read as having scrolled away. */
	const NEAR_BOTTOM_PX = 24;

	function handleScroll(): void {
		if (!logEl) return;
		const nearBottom = logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight <= NEAR_BOTTOM_PX;
		onScroll(nearBottom);
	}
</script>

{#if reset !== null}
	<div class="reset-notice" role="status" data-testid="log-reset-notice">
		{resetNoticeCopy(reset)}
	</div>
{/if}

<div class="log" data-testid="log-body" bind:this={logEl} onscroll={handleScroll}>
	{#if state === 'unavailable'}
		<div class="state-msg" role="alert" data-testid="log-state-unavailable">
			{unavailableSourceCopy(
				requestedUnavailable !== null ? describeSource(requestedUnavailable) : ''
			)}
		</div>
	{:else if state === 'no-selection'}
		<div class="state-msg" data-testid="log-state-no-selection">{noSelectionCopy()}</div>
	{:else if state === 'permission-denied'}
		<div class="state-msg" role="alert" data-testid="log-state-permission-denied">
			<p>{permissionDeniedCopy()}</p>
			<button
				type="button"
				class="link-btn"
				data-testid="log-reveal-folder-inline"
				onclick={onRevealFolder}>Open log folder</button
			>
		</div>
	{:else if state === 'error'}
		<div class="state-msg" role="alert" data-testid="log-state-error">
			{genericReadErrorCopy(errorMessage(readError))}
		</div>
	{:else if state === 'not-yet-created'}
		<div class="state-msg" data-testid="log-state-not-yet-created">{notYetCreatedCopy()}</div>
	{:else if state === 'empty'}
		<div class="state-msg" data-testid="log-state-empty">{emptyCopy(filtered)}</div>
	{:else}
		{#each rows as row, i (i)}
			<div class="line">
				<LogLevelBadge level={row.level} />
				<span class="msg">{row.text}</span>
			</div>
		{/each}
	{/if}
</div>

<style>
	.reset-notice {
		margin: 0 var(--vh-space-6) var(--vh-space-2);
		padding: 6px 10px;
		border: 1px solid color-mix(in oklab, var(--vh-start) 35%, transparent);
		border-radius: var(--vh-radius-control);
		color: var(--vh-start);
		font-size: var(--vh-text-table);
		/* Border + text only, NO background tint — the standing lesson this
		   task carries forward (ScaffoldNoticeBanner.svelte's own comment): a
		   color-mix(…, var(--vh-surface)) tint at the usual ~9% measured
		   4.36:1 for this exact amber pairing, short of WCAG AA. This recipe
		   is the one already verified safe in this repo (4.68:1 on
		   --vh-surface) — reused, not re-derived. */
	}
	.log {
		flex: 1;
		margin: 0 var(--vh-space-6) var(--vh-space-4);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		background: var(--vh-log-bg);
		overflow: auto;
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-log);
		line-height: 1.7;
	}
	.line {
		display: grid;
		grid-template-columns: 56px 1fr;
		gap: 12px;
		padding: 0 14px;
		white-space: pre-wrap;
	}
	.line:hover {
		background: color-mix(in oklab, var(--vh-text) 4%, transparent);
	}
	.msg {
		color: var(--vh-text);
	}
	.state-msg {
		padding: var(--vh-space-6);
		text-align: center;
		color: var(--vh-text-2);
		font-family: var(--vh-font-ui);
		font-size: var(--vh-text-table);
		max-width: 56ch;
		margin: 0 auto;
	}
	.state-msg p {
		margin: 0 0 var(--vh-space-2);
	}
	.link-btn {
		font: inherit;
		font-weight: 500;
		font-size: var(--vh-text-table);
		color: var(--vh-link);
		background: none;
		border: 0;
		padding: 0;
		cursor: pointer;
	}
	.link-btn:hover {
		text-decoration: underline;
	}
</style>
