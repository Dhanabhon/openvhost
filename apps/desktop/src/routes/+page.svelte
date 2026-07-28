<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { resolve } from '$app/paths';
	import AppShell from '$lib/components/AppShell.svelte';
	import ApplyDialog from '$lib/components/ApplyDialog.svelte';
	import ApplyErrorBanner from '$lib/components/ApplyErrorBanner.svelte';
	import PendingChangesBanner from '$lib/components/PendingChangesBanner.svelte';
	import SiteDrawer from '$lib/components/SiteDrawer.svelte';
	import SitesPanel from '$lib/components/SitesPanel.svelte';
	import {
		applyConfig,
		createSite,
		deleteSite,
		listSites,
		openSite,
		phpEnvironment,
		planConfigApply,
		updateSite,
		type PhpEnvironmentDto,
		type SiteDto,
		type SiteInput
	} from '$lib/ipc';
	import { runningCount } from '$lib/services.derive';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { findMissingRuntimeSite } from '$lib/sites.derive';
	import { ApplyStore } from '$lib/apply.svelte';
	import { SitesStore } from '$lib/sites.svelte';

	const store = new SitesStore({ listSites, createSite, updateSite, deleteSite, openSite });
	const applyStore = new ApplyStore({ planConfigApply, applyConfig });
	// The titlebar's "N running" belongs to every route, so it reads the shared
	// supervisor state that `routes/+layout.svelte` subscribes to — this page used to
	// pass a hardcoded 0, which announced "0 running" even with services up.
	const running = $derived(runningCount(servicesStore.services));

	// Threaded into SiteDrawer so its PHP-version picker offers only what this machine
	// actually has (Task 8): a hardcoded list let every option lead to an Apply the
	// backend refused. `null` until the first read settles; `phpEnvLoaded` distinguishes
	// that in-flight state from a genuinely empty result (see `noPhpInstalled` below).
	let phpEnv = $state<PhpEnvironmentDto | null>(null);
	let phpEnvLoaded = $state(false);
	// I2 audit finding: a FAILED read used to be indistinguishable from a
	// genuinely empty one — both left `phpEnv` at `null`, so `noPhpInstalled`
	// below fired for either, and every row's missing-runtime badge (fed by
	// `installedPhpVersions`, `[]` in both cases too) asserted "not installed"
	// as fact when the honest answer was "we don't know". Tracked separately so
	// the two cases can render differently.
	let phpEnvError = $state(false);

	const installedPhpVersions = $derived(
		(phpEnv?.runtimes ?? []).filter((r) => r.installed).map((r) => r.major)
	);

	// Whether this machine's PHP environment is actually KNOWN — i.e. the read
	// settled AND succeeded. `installedPhpVersions` reads `[]` both while this
	// is false (loading, or the read failed) and when it is genuinely empty;
	// `noPhpInstalled`/the row badges below must tell those apart rather than
	// treating "unknown" as "definitely none".
	const phpEnvKnown = $derived(phpEnvLoaded && !phpEnvError);

	// Gated on `phpEnvKnown`, not just `installedPhpVersions.length === 0`: `null`
	// (still loading), a failed read, AND a genuinely empty environment all produce
	// the same empty array, and flashing this banner for any of the first two would
	// be wrong on any machine that does have a version installed.
	const noPhpInstalled = $derived(phpEnvKnown && installedPhpVersions.length === 0);

	// The servable site (if any) whose PHP version this machine no longer has —
	// used only to decide whether the apply-error banner below has a "install
	// this" remedy to offer. Re-derived from state already on this page rather
	// than a structured IPC field; see `findMissingRuntimeSite`'s doc comment
	// and task-9-report.md for why that is the honest tradeoff here.
	const missingRuntimeSite = $derived(findMissingRuntimeSite(store.sites, installedPhpVersions));

	onMount(() => {
		void store.load();
		void applyStore.refresh();
		void loadPhpEnvironment();
	});

	async function loadPhpEnvironment(): Promise<void> {
		try {
			phpEnv = await phpEnvironment();
			phpEnvError = false;
		} catch {
			// I2 fix: a failed read is not evidence nothing is installed, but it is
			// also not the same fact as a genuinely empty result — `phpEnvError`
			// is what lets the markup below tell them apart instead of collapsing
			// both into "nothing installed".
			phpEnvError = true;
		} finally {
			phpEnvLoaded = true;
		}
	}

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
	async function onSave(
		id: string | null,
		input: SiteInput,
		createFolder: boolean
	): Promise<boolean> {
		const ok = await store.save(id, input, createFolder);
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
		     failed `plan_config_apply` (MissingRuntime / NotAPlainFile — fails the
		     WHOLE call, not just an empty change list) surfaces here with the
		     dialog unreachable (no pending count means no "Review and apply"
		     button), and a failed `apply_config` surfaces here too once the user
		     closes the dialog with the count still pending. Without this banner
		     the first case in particular would leave the user with no way to
		     learn why Apply never appears at all. The heading stays generic
		     across both rather than guessing which one happened.

		     Suppressed while the dialog is open: `ApplyDialog` renders this same
		     `applyStore.error` itself, and a page banner behind the dialog's scrim
		     would be the QuitDialog lesson in reverse — an error the user cannot
		     reach or read behind a blurred backdrop.

		     `missingRuntimeSite` — see its derivation above — is what turns this
		     from a dead end into a way out: this is exactly the plan_config_apply
		     failure a machine that lost a PHP version hits, and until now this
		     banner named the problem and offered nothing to press. -->
		<ApplyErrorBanner error={applyStore.error} missing={missingRuntimeSite} onEditSite={onEdit} />
	{/if}
	{#if phpEnvError}
		<!-- I2 fix: a failed `phpEnvironment()` read used to render the SAME
		     "nothing installed" banner as a genuinely empty environment — a false
		     claim about the machine, stated as fact. This is the honest version:
		     the read failed, so nothing below (this banner, Save in the drawer,
		     the row badges) can say anything about which PHP versions exist. -->
		<div class="banner-error" role="alert" data-testid="php-env-error-banner">
			<strong>Could not read the PHP environment</strong>
			<span
				>Site rows below cannot show whether their PHP version is installed until this succeeds.</span
			>
		</div>
	{:else if noPhpInstalled}
		<!-- Sites is the landing page (Rail.svelte's own comment: "`/`, not `/sites`"), so
		     this is where a first-time user — or one who has never installed PHP — lands
		     first. Pointing at Languages here, not only inside the drawer, means the guidance
		     is visible before they even open Add site. -->
		<div class="banner-info" role="status" data-testid="no-php-banner">
			<strong>No PHP version is installed yet</strong>
			<span
				>Sites need one to run. <a href={resolve('/languages')}
					>Install a version on the Languages page</a
				>.</span
			>
		</div>
	{/if}
	<PendingChangesBanner count={applyStore.pendingCount} onReview={() => (applyDialogOpen = true)} />
	<SitesPanel
		sites={store.sites}
		installed={phpEnvKnown ? installedPhpVersions : null}
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
			installed={installedPhpVersions}
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
	/* .banner-info: an accent-tinted pointer, not a failure — same recipe as
	   PendingChangesBanner.svelte's own `.banner` (accent-tinted surface over
	   `--vh-surface`, distinct from `.banner-error`'s failure tint above), reused here
	   as a block-level notice rather than that component's flex row + action button. */
	.banner-info {
		margin: var(--vh-space-3) var(--vh-space-6) 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		border: 1px solid color-mix(in oklab, var(--vh-accent) 35%, transparent);
		background: color-mix(in oklab, var(--vh-accent) 8%, var(--vh-surface));
		border-radius: var(--vh-radius-card);
		font-size: var(--vh-text-table);
	}
	.banner-info strong {
		display: block;
		margin-bottom: 2px;
	}
	.banner-info a {
		color: var(--vh-link);
	}
</style>
