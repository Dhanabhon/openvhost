<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import SiteDrawer from '$lib/components/SiteDrawer.svelte';
	import SitesPanel from '$lib/components/SitesPanel.svelte';
	import { createSite, deleteSite, listSites, openSite, updateSite, type SiteDto } from '$lib/ipc';
	import { runningCount } from '$lib/services.derive';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { SitesStore } from '$lib/sites.svelte';

	const store = new SitesStore({ listSites, createSite, updateSite, deleteSite, openSite });
	// The titlebar's "N running" belongs to every route, so it reads the shared
	// supervisor state that `routes/+layout.svelte` subscribes to — this page used to
	// pass a hardcoded 0, which announced "0 running" even with services up.
	const running = $derived(runningCount(servicesStore.services));

	onMount(() => {
		void store.load();
	});

	let editing = $state<SiteDto | null>(null);
	let drawerOpen = $state(false);

	function onAdd(): void {
		store.clearErrors();
		editing = null;
		drawerOpen = true;
	}
	function onEdit(site: SiteDto): void {
		store.clearErrors();
		editing = site;
		drawerOpen = true;
	}
</script>

<AppShell runningCount={running}>
	{#if store.error}
		<div class="banner-error" role="alert" data-testid="sites-error">
			<strong>Command failed ({store.error.kind})</strong>
			<span>{'message' in store.error ? store.error.message : ''}</span>
		</div>
	{/if}
	<SitesPanel
		sites={store.sites}
		{onAdd}
		{onEdit}
		busy={store.busy}
		rowErrors={store.rowError}
		onToggleEnabled={(site, enabled) => void store.setEnabled(site, enabled)}
		onOpen={(id) => void store.open(id)}
		onDelete={(id) => void store.removeRow(id)}
	/>
	{#if drawerOpen}
		<SiteDrawer
			site={editing}
			fieldErrors={store.fieldErrors}
			onSave={(id, input) => store.save(id, input)}
			onDelete={(id) => store.remove(id)}
			onClose={() => (drawerOpen = false)}
		/>
	{/if}
</AppShell>

<style>
	/* .banner-error: the same token-based failure-surface treatment as the Services page
	   (routes/services/+page.svelte) — reuses mock.css's `.fail-detail` recipe so an IPC error reads as
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
