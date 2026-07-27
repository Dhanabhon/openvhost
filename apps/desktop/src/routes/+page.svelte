<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import ApplyDialog from '$lib/components/ApplyDialog.svelte';
	import PendingChangesBanner from '$lib/components/PendingChangesBanner.svelte';
	import SiteDrawer from '$lib/components/SiteDrawer.svelte';
	import SitesPanel from '$lib/components/SitesPanel.svelte';
	import {
		applySites,
		createSite,
		deleteSite,
		listSites,
		openSite,
		planSiteApply,
		updateSite,
		type SiteDto,
		type SiteInput
	} from '$lib/ipc';
	import { runningCount } from '$lib/services.derive';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { ApplyStore } from '$lib/apply.svelte';
	import { SitesStore } from '$lib/sites.svelte';

	const store = new SitesStore({ listSites, createSite, updateSite, deleteSite, openSite });
	const applyStore = new ApplyStore({ planSiteApply, applySites });
	// The titlebar's "N running" belongs to every route, so it reads the shared
	// supervisor state that `routes/+layout.svelte` subscribes to — this page used to
	// pass a hardcoded 0, which announced "0 running" even with services up.
	const running = $derived(runningCount(servicesStore.services));

	onMount(() => {
		void store.load();
		void applyStore.refresh();
	});

	let editing = $state<SiteDto | null>(null);
	let drawerOpen = $state(false);
	let applyDialogOpen = $state(false);

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

	// Every one of these mutates the generated site tree that Apply reads, so the
	// pending-changes banner would otherwise show a stale count (or none) after a
	// save, a delete, or a row toggle. `refresh()` is cheap by design (see
	// `ApplyStore`'s own doc comment) — it is fine to call after every mutation.
	async function onSave(id: string | null, input: SiteInput): Promise<boolean> {
		const ok = await store.save(id, input);
		if (ok) await applyStore.refresh();
		return ok;
	}
	async function onDrawerDelete(id: string): Promise<boolean> {
		const ok = await store.remove(id);
		if (ok) await applyStore.refresh();
		return ok;
	}
	async function onRowDelete(id: string): Promise<void> {
		if (await store.removeRow(id)) await applyStore.refresh();
	}
	async function onToggleEnabled(site: SiteDto, enabled: boolean): Promise<void> {
		if (await store.setEnabled(site, enabled)) await applyStore.refresh();
	}

	async function onDialogApply(): Promise<void> {
		await applyStore.run();
	}
</script>

<AppShell runningCount={running}>
	{#if store.error}
		<div class="banner-error" role="alert" data-testid="sites-error">
			<strong>Command failed ({store.error.kind})</strong>
			<span>{'message' in store.error ? store.error.message : ''}</span>
		</div>
	{/if}
	{#if applyStore.error !== '' && !applyDialogOpen}
		<!-- `applyStore.error` covers two distinct failures with one string: a
		     failed `plan_site_apply` (MissingRuntime / NotAPlainFile — fails the
		     WHOLE call, not just an empty change list) surfaces here with the
		     dialog unreachable (no pending count means no "Review and apply"
		     button), and a failed `apply_sites` surfaces here too once the user
		     closes the dialog with the count still pending. Without this banner
		     the first case in particular would leave the user with no way to
		     learn why Apply never appears at all. The heading stays generic
		     across both rather than guessing which one happened.

		     Suppressed while the dialog is open: `ApplyDialog` renders this same
		     `applyStore.error` itself, and a page banner behind the dialog's scrim
		     would be the QuitDialog lesson in reverse — an error the user cannot
		     reach or read behind a blurred backdrop. -->
		<div class="banner-error" role="alert" data-testid="apply-plan-error">
			<strong>Couldn't apply site changes</strong>
			<span>{applyStore.error}</span>
		</div>
	{/if}
	<PendingChangesBanner count={applyStore.pendingCount} onReview={() => (applyDialogOpen = true)} />
	<SitesPanel
		sites={store.sites}
		{onAdd}
		{onEdit}
		busy={store.busy}
		rowErrors={store.rowError}
		onToggleEnabled={(site, enabled) => void onToggleEnabled(site, enabled)}
		onOpen={(id) => void store.open(id)}
		onDelete={(id) => void onRowDelete(id)}
	/>
	{#if drawerOpen}
		<SiteDrawer
			site={editing}
			fieldErrors={store.fieldErrors}
			{onSave}
			onDelete={onDrawerDelete}
			onClose={() => (drawerOpen = false)}
		/>
	{/if}
	{#if applyDialogOpen}
		<ApplyDialog
			changes={applyStore.changes}
			applying={applyStore.applying}
			error={applyStore.error}
			outcome={applyStore.outcome}
			onApply={() => void onDialogApply()}
			onClose={() => (applyDialogOpen = false)}
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
