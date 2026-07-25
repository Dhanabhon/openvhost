<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { SiteDto } from '$lib/ipc';
	import Button from './Button.svelte';
	import SiteListRow from './SiteListRow.svelte';

	let {
		sites,
		onAdd,
		onEdit
	}: {
		sites: readonly SiteDto[];
		onAdd: () => void;
		onEdit: (site: SiteDto) => void;
	} = $props();

	const enabledCount = $derived(sites.filter((s) => s.enabled).length);
</script>

<div class="page-head">
	<div>
		<h1>Sites</h1>
		<p class="sub">
			{sites.length}
			{sites.length === 1 ? 'site' : 'sites'} · {enabledCount} enabled
		</p>
	</div>
	<div class="grow"></div>
	<Button variant="primary" onclick={onAdd}>Add site</Button>
</div>

<section class="panel" aria-label="Sites" data-testid="sites">
	{#if sites.length === 0}
		<div class="empty">
			<p class="primary">No sites yet</p>
			<p class="meta">
				Add a site to serve a project folder at a <span class="mono">.localhost</span> domain.
			</p>
		</div>
	{:else}
		<div class="rowlist">
			{#each sites as site (site.id)}
				<SiteListRow {site} {onEdit} />
			{/each}
		</div>
	{/if}
</section>

<style>
	/* Ported from docs/design/main-window.html lines 60-70 (page head) + mock.css (.page-head,
	   .page-head h1, .page-head .sub, .page-head .grow, .panel, .rowlist, .empty). `.empty`'s
	   own contents (.primary/.meta/.mono) have no direct mock.css precedent — mock.css defines
	   `.empty` itself but never demonstrates its inner markup on any screen — so this reuses the
	   same "primary" (bold heading) / "meta" (muted detail) / "mono" (monospace fragment)
	   vocabulary the row already uses, scoped under `.empty` the same way ServiceRow scopes
	   `.row .primary`/`.row .meta`. `.mono` itself is the global utility class from
	   lib/styles/tokens.css, not redefined here. */
	.page-head {
		display: flex;
		align-items: center;
		gap: var(--vh-space-4);
		padding: 20px var(--vh-space-6) var(--vh-space-3);
	}
	.page-head h1 {
		font-size: var(--vh-text-page);
		font-weight: 600;
	}
	.page-head .sub {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		margin-top: 2px;
	}
	.page-head .grow {
		flex: 1;
	}
	.panel {
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		margin: 0 var(--vh-space-6);
		overflow: hidden;
	}
	.rowlist {
		display: flex;
		flex-direction: column;
	}
	.empty {
		padding: var(--vh-space-8) var(--vh-space-6);
		text-align: center;
		color: var(--vh-text-2);
	}
	.empty .primary {
		font-weight: 600;
		color: var(--vh-text);
	}
	.empty .meta {
		font-size: var(--vh-text-table);
		margin: 4px 0 0;
	}
</style>
