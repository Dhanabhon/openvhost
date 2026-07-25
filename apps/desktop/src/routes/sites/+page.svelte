<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import SitesPanel from '$lib/components/SitesPanel.svelte';
	import { createSite, deleteSite, listSites, updateSite, type SiteDto } from '$lib/ipc';
	import { SitesStore } from '$lib/sites.svelte';

	const store = new SitesStore({ listSites, createSite, updateSite, deleteSite });

	onMount(() => {
		void store.load();
	});

	// The editor drawer lands in Task 4; until then Add/Edit are inert hooks. The `site` param
	// stays unused until then, so it is disabled for this one line rather than loosening the
	// project's `@typescript-eslint/no-unused-vars` (which has no `argsIgnorePattern`).
	function onAdd(): void {}
	// eslint-disable-next-line @typescript-eslint/no-unused-vars -- filled in by Task 4's drawer
	function onEdit(site: SiteDto): void {}
</script>

<AppShell runningCount={0} active="sites">
	{#if store.error}
		<div class="banner-error" role="alert" data-testid="sites-error">
			<strong>Command failed ({store.error.kind})</strong>
			<span>{'message' in store.error ? store.error.message : ''}</span>
		</div>
	{/if}
	<SitesPanel sites={store.sites} {onAdd} {onEdit} />
</AppShell>

<style>
	/* .banner-error: the same token-based failure-surface treatment as the Services page
	   (routes/+page.svelte) — reuses mock.css's `.fail-detail` recipe so an IPC error reads as
	   the same "failure" semantic everywhere in the product. No extra `<h1 class="sr-only">`
	   here (unlike the Services page): SitesPanel already renders a real, visible `<h1>Sites</h1>`
	   as part of its page head, so a second hidden h1 would just duplicate the landmark. */
	.banner-error {
		margin: var(--vh-space-3) var(--vh-space-6) 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		background: var(--vh-fail-tint);
		border-radius: var(--vh-radius-control);
		color: var(--vh-fail);
		font-size: var(--vh-text-table);
	}
	.banner-error strong {
		display: block;
		margin-bottom: 2px;
	}
</style>
