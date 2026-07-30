<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Filter/case/level/follow controls + the stream toggle ("then the stream",
  spec D6) + the privacy note (spec D5). Every value that is NOT free text
  arrives as a prop and leaves as a callback (WebServerSettingsForm's
  convention); the filter text is the one exception — see `draft` below.
-->
<script lang="ts">
	import type { LogLevel, LogSourceDto } from '$lib/ipc';
	import { sourceDomain, sourceStream } from '$lib/logs.derive';
	import { privacyNoteCopy } from '$lib/logs.copy';

	let {
		needle,
		caseSensitive,
		minLevel,
		follow,
		newRowsWhilePaused,
		selected,
		onNeedle,
		onCaseSensitive,
		onMinLevel,
		onSetFollow,
		onJumpToLatest,
		onSelectStream
	}: {
		needle: string;
		caseSensitive: boolean;
		minLevel: LogLevel | null;
		follow: boolean;
		newRowsWhilePaused: boolean;
		selected: LogSourceDto | null;
		onNeedle: (v: string) => void;
		onCaseSensitive: (v: boolean) => void;
		onMinLevel: (v: LogLevel | null) => void;
		onSetFollow: (v: boolean) => void;
		onJumpToLatest: () => void;
		onSelectStream: (stream: 'access' | 'error') => void;
	} = $props();

	/** How long to wait after the user stops typing before actually
	 *  restarting the search (`LogsStore.setNeedle` discards accumulated
	 *  rows on every call — firing it per keystroke would flash the log
	 *  body empty mid-word and fire a request per character). Component-
	 *  owned, DOM-timer-based UX polish, the same untestable-under-SSR shape
	 *  as `Select.svelte`'s own typeahead-reset timer (that component's file
	 *  header states the identical carve-out). */
	const FILTER_DEBOUNCE_MS = 300;

	/** The input's live displayed value: a WRITABLE `$derived` (Svelte 5).
	 *  Reads as `needle` until the user types, at which point
	 *  `onFilterInput` assigns it directly — Svelte keeps that override
	 *  until `needle` itself changes for a reason OTHER than this
	 *  component's own debounced echo (a fresh source selection resets it
	 *  server-side, spec D6), at which point the override is dropped and
	 *  `draft` tracks `needle` again. One declaration replaces the
	 *  seed-once-plus-sync-effect pattern `SiteDrawer.svelte`'s older
	 *  `$state(untrack(...))` idiom needs — eslint's
	 *  `svelte/prefer-writable-derived` is pointing at exactly this. */
	let draft = $derived(needle);
	let debounceTimer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		return () => {
			if (debounceTimer !== undefined) clearTimeout(debounceTimer);
		};
	});

	function onFilterInput(v: string): void {
		draft = v;
		if (debounceTimer !== undefined) clearTimeout(debounceTimer);
		debounceTimer = setTimeout(() => onNeedle(draft), FILTER_DEBOUNCE_MS);
	}

	const LEVEL_OPTIONS: { value: 'all' | LogLevel; label: string }[] = [
		{ value: 'all', label: 'All levels' },
		{ value: 'warn', label: 'Warnings +' },
		{ value: 'error', label: 'Errors only' }
	];
	const levelValue = $derived(minLevel ?? 'all');
	function onLevelChange(v: string): void {
		onMinLevel(v === 'all' ? null : (v as LogLevel));
	}

	const domain = $derived(sourceDomain(selected));
	const stream = $derived(sourceStream(selected));
	// Deliberately shown only when a follow toggle already OFF, never merely
	// on `newRowsWhilePaused` alone — `LogsStore` never sets both true at
	// once (setFollow(true) clears the flag), but the component stays
	// defensive rather than trusting that invariant silently.
	const showJump = $derived(!follow && newRowsWhilePaused);
</script>

<div class="toolbar">
	<input
		class="input"
		type="search"
		maxlength="256"
		placeholder="Filter lines… (e.g. ERROR, port)"
		aria-label="Filter log lines"
		data-testid="log-filter"
		value={draft}
		oninput={(e) => onFilterInput(e.currentTarget.value)}
	/>
	<button
		type="button"
		class="switch"
		role="switch"
		aria-checked={caseSensitive}
		data-testid="log-case-sensitive"
		onclick={() => onCaseSensitive(!caseSensitive)}
	>
		<span class="track"><span class="thumb"></span></span>
		Match case
	</button>
	<label class="level-label" for="log-level">Level</label>
	<select
		id="log-level"
		class="input"
		data-testid="log-level"
		value={levelValue}
		onchange={(e) => onLevelChange(e.currentTarget.value)}
	>
		{#each LEVEL_OPTIONS as opt (opt.value)}
			<option value={opt.value} selected={opt.value === levelValue}>{opt.label}</option>
		{/each}
	</select>
	{#if domain !== null}
		<div class="seg" role="group" aria-label="Log stream" data-testid="log-stream-toggle">
			<button
				type="button"
				aria-pressed={stream === 'error'}
				data-testid="log-stream-error"
				onclick={() => onSelectStream('error')}>Error</button
			>
			<button
				type="button"
				aria-pressed={stream === 'access'}
				data-testid="log-stream-access"
				onclick={() => onSelectStream('access')}>Access</button
			>
		</div>
	{/if}
	<div class="grow"></div>
	{#if showJump}
		<button type="button" class="jump" data-testid="log-jump-to-latest" onclick={onJumpToLatest}>
			Jump to latest
		</button>
	{/if}
	<button
		type="button"
		class="switch"
		role="switch"
		aria-checked={follow}
		data-testid="log-follow"
		onclick={() => onSetFollow(!follow)}
	>
		<span class="track"><span class="thumb"></span></span>
		Follow tail
	</button>
</div>
<p class="privacy-note" data-testid="log-privacy-note">{privacyNoteCopy()}</p>

<style>
	/* Ported from docs/design/mock.css's `.toolbar` region, wrapping instead
	   of a fixed row so the control set stays usable at the project's 380px
	   panel floor. */
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--vh-space-3);
		padding: var(--vh-space-3) var(--vh-space-6);
	}
	.grow {
		flex: 1;
	}
	.input {
		font: inherit;
		color: var(--vh-text);
		background: var(--vh-surface);
		border: 1px solid var(--vh-border-strong);
		border-radius: var(--vh-radius-control);
		padding: 7px 10px;
		min-width: 0;
		transition: border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.input:hover {
		border-color: color-mix(in oklab, var(--vh-text) 40%, transparent);
	}
	.input:focus-visible {
		border-color: var(--vh-focus-ring);
		outline-offset: 0;
	}
	input.input[type='search'] {
		flex: 1 1 220px;
	}
	.level-label {
		font-size: var(--vh-text-table);
		color: var(--vh-text-2);
	}
	select.input {
		flex: none;
	}

	/* Same switch recipe as WebServerSettingsForm's `.switch` — reused
	   verbatim rather than re-derived, so a follow/case toggle looks like
	   every other on/off control in this app. */
	.switch {
		display: inline-flex;
		align-items: center;
		gap: var(--vh-space-2);
		font: inherit;
		font-size: var(--vh-text-table);
		color: var(--vh-text-2);
		background: transparent;
		border: 0;
		padding: 2px 0;
		cursor: pointer;
	}
	.switch .track {
		position: relative;
		width: 38px;
		height: 22px;
		border-radius: var(--vh-radius-pill);
		background: color-mix(in oklab, var(--vh-ink) 18%, transparent);
		border: 1px solid var(--vh-border-strong);
		transition: background var(--vh-dur-fast) var(--vh-ease-out);
	}
	.switch .thumb {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		background: var(--vh-surface);
		box-shadow: 0 1px 2px rgb(23 26 33 / 0.25);
		transition: transform var(--vh-dur-fast) var(--vh-ease-out);
	}
	.switch[aria-checked='true'] {
		color: var(--vh-accent);
	}
	.switch[aria-checked='true'] .track {
		background: var(--vh-accent);
		border-color: var(--vh-accent);
	}
	.switch[aria-checked='true'] .thumb {
		transform: translateX(16px);
	}
	.switch:hover .track {
		border-color: color-mix(in oklab, var(--vh-text) 45%, transparent);
	}

	/* Segmented Access/Error control — ported from mock.css's `.seg`. */
	.seg {
		display: inline-flex;
		border: 1px solid var(--vh-border-strong);
		border-radius: var(--vh-radius-control);
		overflow: hidden;
	}
	.seg button {
		font: inherit;
		font-weight: 500;
		font-size: var(--vh-text-table);
		padding: 6px 12px;
		background: var(--vh-surface);
		color: var(--vh-text-2);
		border: 0;
		cursor: pointer;
	}
	.seg button + button {
		border-left: 1px solid var(--vh-border);
	}
	.seg button[aria-pressed='true'] {
		background: var(--vh-accent);
		color: var(--vh-accent-contrast);
	}

	.jump {
		font: inherit;
		font-weight: 600;
		font-size: var(--vh-text-table);
		padding: 6px 14px;
		border-radius: var(--vh-radius-pill);
		border: 1px solid var(--vh-accent);
		background: var(--vh-accent);
		color: var(--vh-accent-contrast);
		cursor: pointer;
	}
	.jump:hover {
		background: var(--vh-accent-hover);
	}

	.privacy-note {
		margin: 0 var(--vh-space-6) var(--vh-space-2);
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
</style>
