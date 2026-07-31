<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Status facts below the log body (spec D6/D8): follow state, file size,
  the >100 MiB warning, the scan-bound honesty note, a truncated-lines
  count, and "Open log folder" — the one recourse this slice ships against
  unbounded on-disk growth (no rotation yet, spec D8's owner call).
-->
<script lang="ts">
	import type { LogSourceDto } from '$lib/ipc';
	import { SIZE_WARNING_BYTES, formatBytes } from '$lib/logs.derive';
	import { scanBoundCopy, sizeWarningCopy } from '$lib/logs.copy';

	let {
		selected,
		requestedUnavailable,
		sizeBytes,
		truncatedLines,
		scanBoundReached,
		follow,
		onRevealFolder
	}: {
		selected: LogSourceDto | null;
		requestedUnavailable: LogSourceDto | null;
		sizeBytes: number;
		truncatedLines: number;
		scanBoundReached: boolean;
		follow: boolean;
		onRevealFolder: () => void;
	} = $props();

	// A folder can only be revealed for a REAL, targeted file source — not
	// while nothing is selected, and not for an unavailable deep-link (there
	// is nothing derived to open; `resolve_log_path` never ran).
	const targeted = $derived(selected !== null && requestedUnavailable === null);
	const oversized = $derived(sizeBytes > SIZE_WARNING_BYTES);
</script>

{#if targeted}
	<div class="statusline" data-testid="log-status-line">
		<span>{follow ? 'Following' : 'Paused'}</span>
		<span class="mono num">{formatBytes(sizeBytes)}</span>
		{#if oversized}
			<span class="size-warning" role="status" data-testid="log-size-warning">
				{sizeWarningCopy()}
			</span>
		{/if}
		{#if scanBoundReached}
			<span class="scan-note" role="status" data-testid="log-scan-bound-note">
				{scanBoundCopy()}
			</span>
		{/if}
		{#if truncatedLines > 0}
			<span data-testid="log-truncated-note">
				{truncatedLines}
				{truncatedLines === 1 ? 'line was' : 'lines were'} too long and truncated
			</span>
		{/if}
		<div class="grow"></div>
		<button type="button" class="link-btn" data-testid="log-reveal-folder" onclick={onRevealFolder}>
			Open log folder
		</button>
	</div>
{/if}

<style>
	/* Ported from docs/design/mock.css's `.statusline`. */
	.statusline {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--vh-space-4);
		padding: 6px var(--vh-space-6) var(--vh-space-4);
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
	.grow {
		flex: 1;
	}
	/* Reuses the already-shipped --vh-fail / --vh-fail-tint pairing (e.g.
	   ServiceRow's .fail-detail, routes/+page.svelte's .banner-error) rather
	   than a fresh recipe — see this task's report for why a NEW tint recipe
	   is not introduced without re-measuring it (standing lesson). */
	.size-warning {
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: 2px 8px;
	}
	/* Border + text only on plain --vh-surface/--vh-bg, NOT a tinted
	   background — the amber sibling-tint recipe measured 4.36:1 in this
	   repo (ScaffoldNoticeBanner.svelte's comment); this pairing alone is
	   the one already verified to clear AA (4.68:1). */
	.scan-note {
		color: var(--vh-start);
	}
	.link-btn {
		font: inherit;
		font-weight: 500;
		font-size: var(--vh-text-caption);
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
