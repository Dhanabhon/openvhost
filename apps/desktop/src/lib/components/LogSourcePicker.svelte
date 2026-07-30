<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Grouped log-source picker (Services / Sites, then the stream) — spec D6's
  deliberate, documented replacement for docs/design/log-viewer.html's flat
  tab strip, which "does not survive 40 sites": 40 sites means 80 rows in
  that design (one tab per access/error pair) with no visual separation
  from the service rows sharing the same strip. Grouping collapses each
  site's access/error pair into ONE domain chip here — the Access/Error
  choice becomes "the stream" toggle in `LogToolbar.svelte`, shown only
  once a site-scoped source is selected, rather than a second flat row of
  tabs.
-->
<script lang="ts">
	import type { LogSourceDto, LogSourceRowDto } from '$lib/ipc';
	import {
		encodeLogSource,
		sameSource,
		siteSource,
		sourceDomain,
		sourceStream
	} from '$lib/logs.derive';

	let {
		services,
		siteDomains,
		selected,
		failedServiceIds,
		onSelect
	}: {
		/** Every non-site row from `list_log_sources`, in server order —
		 *  nginx's two globals, one row per installed PHP major, one ring
		 *  row per supervised service (`logs.derive.ts`'s `groupSources`). */
		services: readonly LogSourceRowDto[];
		/** Distinct site domains, already sorted (`groupSources`). */
		siteDomains: readonly string[];
		selected: LogSourceDto | null;
		/** Ring-service ids with a `state.kind === 'failed'` ServiceState —
		 *  see this file's own doc comment on why php-fpm pool chips are not
		 *  included (the DTO gives no non-fragile way to join them). */
		failedServiceIds: ReadonlySet<string>;
		onSelect: (source: LogSourceDto) => void;
	} = $props();

	/** Selecting a domain that is ALREADY the current one keeps whichever
	 *  stream is showing (switching streams is the toolbar's job); picking a
	 *  NEW domain defaults to its error log — the live-proof entry point
	 *  ("the site's error log shows the PHP fatal"). A re-click on the
	 *  already-selected exact source is a no-op, so it does not reset the
	 *  in-flight read for nothing. */
	function selectDomain(domain: string): void {
		const stream =
			sourceDomain(selected) === domain ? (sourceStream(selected) ?? 'error') : 'error';
		const target = siteSource(domain, stream);
		if (selected !== null && sameSource(selected, target)) return;
		onSelect(target);
	}
</script>

<nav class="source-picker" aria-label="Log sources" data-testid="log-sources">
	<div class="src-group">
		<h3 class="src-group-title">Services</h3>
		{#if services.length === 0}
			<p class="src-empty">No services registered.</p>
		{:else}
			<div class="chips">
				{#each services as row (encodeLogSource(row.source))}
					<button
						type="button"
						class="chip"
						aria-pressed={selected !== null && sameSource(selected, row.source)}
						data-testid="log-source-{encodeLogSource(row.source)}"
						onclick={() => onSelect(row.source)}
					>
						{row.label}
						{#if row.source.kind === 'serviceRing' && failedServiceIds.has(row.source.id)}
							<span
								class="chip-fail"
								role="status"
								data-testid="chip-fail-{row.source.id}"
								title="{row.label} has failed"
							></span>
						{/if}
					</button>
				{/each}
			</div>
		{/if}
	</div>
	<div class="src-group">
		<h3 class="src-group-title">Sites</h3>
		{#if siteDomains.length === 0}
			<p class="src-empty">No sites yet.</p>
		{:else}
			<div class="chips">
				{#each siteDomains as domain (domain)}
					<button
						type="button"
						class="chip"
						aria-pressed={sourceDomain(selected) === domain}
						data-testid="log-source-domain-{domain}"
						onclick={() => selectDomain(domain)}
					>
						{domain}
					</button>
				{/each}
			</div>
		{/if}
	</div>
</nav>

<style>
	/* Chips wrap via flex-wrap rather than a horizontal scroll strip (the
	   mock's `.tabs`), so a long catalogue degrades by growing taller
	   instead of needing horizontal scroll discovery — the same reasoning
	   ServiceRow's `.fail-detail pre` switched from `overflow-x: auto` to
	   `white-space: pre-wrap` for. Also what keeps this usable at the
	   project's 380px panel floor. */
	.source-picker {
		display: flex;
		flex-direction: column;
		gap: var(--vh-space-3);
		padding: var(--vh-space-3) var(--vh-space-6) 0;
	}
	.src-group-title {
		font-size: var(--vh-text-caption);
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--vh-text-2);
		margin: 0 0 var(--vh-space-2);
	}
	.src-empty {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		margin: 0 0 var(--vh-space-2);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	/* Same unselected/selected recipe as mock.css's `.seg button` (a
	   segmented control), reused here for a wrapping chip strip instead of a
	   fixed-count row — `.btn-primary`'s own accent/accent-contrast pairing,
	   already shipped and relied on elsewhere in this app, so no new
	   contrast math is needed for the selected state. */
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font: inherit;
		font-weight: 500;
		font-size: var(--vh-text-table);
		padding: 5px 12px;
		border-radius: var(--vh-radius-pill);
		border: 1px solid var(--vh-border-strong);
		background: var(--vh-surface);
		color: var(--vh-text-2);
		cursor: pointer;
		transition:
			background var(--vh-dur-fast) var(--vh-ease-out),
			border-color var(--vh-dur-fast) var(--vh-ease-out),
			color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.chip:hover {
		border-color: color-mix(in oklab, var(--vh-text) 40%, transparent);
		color: var(--vh-text);
	}
	.chip[aria-pressed='true'] {
		background: var(--vh-accent);
		border-color: var(--vh-accent);
		color: var(--vh-accent-contrast);
	}
	.chip-fail {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--vh-fail-dot);
		flex: none;
	}
</style>
