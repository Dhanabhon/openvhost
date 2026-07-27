<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { SiteDto } from '$lib/ipc';
	import Button from './Button.svelte';
	import SiteListRow from './SiteListRow.svelte';

	let {
		sites,
		installed,
		onAdd,
		onEdit,
		onToggleEnabled,
		onOpen,
		onDelete,
		busy = {},
		rowErrors = {}
	}: {
		sites: readonly SiteDto[];
		/** PHP majors actually installed on this machine, forwarded to every row
		 * so `SiteListRow`'s missing-runtime badge can compare against it. See
		 * that component's `installed` prop doc for why. */
		installed: readonly string[];
		onAdd: () => void;
		onEdit: (site: SiteDto) => void;
		onToggleEnabled: (site: SiteDto, enabled: boolean) => void;
		onOpen: (id: string) => void;
		onDelete: (id: string) => void;
		/** Both keyed by site id, so a row's state cannot be read off a neighbour. */
		busy?: Record<string, boolean>;
		rowErrors?: Record<string, string>;
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
	<!-- `focusFallback`: this page's primary action, so `SiteDrawer.svelte` can hand focus here
	     deterministically if the drawer's own "restore focus to whatever opened it" target no
	     longer exists (e.g. deleting a site removes that row's Edit button from the DOM before
	     the drawer unmounts). See that component's onMount cleanup. -->
	<Button variant="primary" focusFallback onclick={onAdd}>Add site</Button>
</div>

<section class="panel" aria-label="Sites" data-testid="sites">
	{#if sites.length === 0}
		<div class="empty">
			<div class="title">No sites yet</div>
			<p class="meta">
				Add a site to serve a project folder at a <span class="mono">.localhost</span> domain.
			</p>
		</div>
	{:else}
		<div class="rowlist">
			{#each sites as site (site.id)}
				<SiteListRow
					{site}
					{installed}
					{onEdit}
					{onToggleEnabled}
					{onOpen}
					{onDelete}
					busy={busy[site.id] === true}
					rowError={rowErrors[site.id] ?? ''}
				/>
			{/each}
		</div>
	{/if}
</section>

<style>
	/* Ported from docs/design/main-window.html lines 60-70 (page head) + mock.css (.page-head,
	   .page-head h1, .page-head .sub, .page-head .grow, .panel, .rowlist, .empty, .empty .title).
	   `.empty .title` (heading) mirrors mock.css:445 and the same convention
	   ServicesPanel.svelte already uses for its own empty state. The detail line has no direct
	   mock.css precedent — mock.css defines `.empty` itself but never demonstrates its inner
	   markup on any screen — so it reuses the row's "meta" (muted detail) / "mono" (monospace
	   fragment) vocabulary, scoped under `.empty` the same way the row scopes `.row .meta`.
	   `.mono` itself is the global utility class from lib/styles/tokens.css, not redefined
	   here. */
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
	.empty .title {
		font-weight: 600;
		color: var(--vh-text);
		margin-bottom: 4px;
	}
	.empty .meta {
		font-size: var(--vh-text-table);
		margin: 4px 0 0;
	}
</style>
