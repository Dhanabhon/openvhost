<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import ApplyDialog from '$lib/components/ApplyDialog.svelte';
	import ApplyErrorBanner from '$lib/components/ApplyErrorBanner.svelte';
	import ErrorBanner from '$lib/components/ErrorBanner.svelte';
	import PendingChangesBanner from '$lib/components/PendingChangesBanner.svelte';
	import WebServerPanel from '$lib/components/WebServerPanel.svelte';
	import WebServerSettingsForm from '$lib/components/WebServerSettingsForm.svelte';
	import {
		applyConfig,
		listSites,
		planConfigApply,
		saveWebServerSettings,
		webServerSettings
	} from '$lib/ipc';
	import type { SiteDto } from '$lib/ipc';
	import { ApplyStore } from '$lib/apply.svelte';
	import { runningCount } from '$lib/services.derive';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { statusFor, stoppedPoolsFor } from '$lib/webservers.derive';
	import { webServersStore as store } from '$lib/webservers.svelte';
	import { WebSettingsStore } from '$lib/websettings.svelte';

	// The titlebar's "N running" belongs to every route and comes from the shared
	// supervisor state the layout subscribes to — never a literal.
	const running = $derived(runningCount(servicesStore.services));

	// Read-only, and only to answer "which php-fpm pools do the sites need".
	// Deliberately NOT a SitesStore: that carries create/update/delete/open, and
	// this page must not be able to change a site just to render one warning line.
	let sites = $state<SiteDto[]>([]);

	const stoppedPools = $derived(
		stoppedPoolsFor(
			sites,
			servicesStore.services,
			statusFor(servicesStore.services, 'nginx') === 'running'
		)
	);

	// Same pair the Sites page wires, for the same pipeline: the settings and the
	// sites are ONE config set, so this page reaches `plan_config_apply` /
	// `apply_config` rather than growing an apply path of its own.
	const applyStore = new ApplyStore({ planConfigApply, applyConfig });

	const settings = new WebSettingsStore({
		webServerSettings,
		saveWebServerSettings,
		/**
		 * Deliberately NOT the bare `planConfigApply` IPC function.
		 *
		 * The dialog, the pending-changes banner and the settings save must all
		 * describe the SAME plan. Calling the command directly here would leave
		 * `applyStore.changes` holding whatever the last refresh saw, and Save
		 * would open a dialog showing a stale diff — or an empty one — right
		 * after storing new values. Routing the store's plan through
		 * `applyStore.refresh()` gives this page one change list.
		 *
		 * `refresh()` reports a failed plan on `applyStore.error` instead of
		 * throwing, which is what we want: the save itself succeeded, the dialog
		 * opens, and it renders that error in place of a diff (see
		 * `ApplyDialog`'s own handling) rather than the save reporting a failure
		 * that did not happen.
		 */
		planConfigApply: async () => {
			await applyStore.refresh();
			return { changes: applyStore.changes };
		}
	});

	let applyDialogOpen = $state(false);

	/**
	 * Save, then show the diff HERE. Not "saved — now go to the Sites page and
	 * press Apply", which would leave a Save button that visibly does nothing on
	 * the page the user is actually on.
	 */
	async function onSave(): Promise<void> {
		if (await settings.save()) applyDialogOpen = true;
	}

	onMount(() => {
		// Fire-and-forget: the shell and the page head paint immediately and the rows
		// appear when the list resolves. That matters here because `list_web_servers`
		// probes the version, which SPAWNS `nginx -v` server-side — awaiting it before
		// rendering would show the user an empty window for the length of a process
		// launch. Failures land on `store.error` and render in the banner below.
		void store.load();
		// The settings read touches state.db only, and the plan spawns nothing —
		// both are cheap enough to fire alongside the list.
		void settings.load();
		void applyStore.refresh();
		// state.db only — spawns nothing, so it is cheap enough to fire with the
		// rest. A failure leaves `sites` empty, which suppresses the pool warning
		// rather than blanking the page: a missing hint is a smaller harm than a
		// page that will not render, and the row's own state is unaffected.
		void listSites()
			.then((s) => (sites = s))
			.catch(() => {});
	});
</script>

<AppShell runningCount={running} active="web-server">
	<h1 class="sr-only">OpenVHost — Web server</h1>
	<!-- AppShell renders the SERVICES store's banner (supervisor failures, on every
	     route). This one is the web-server list's own page-level failure — the whole
	     `list_web_servers` call failing — which nothing else would surface. Per-row
	     failures are NOT here: they render on their own row so one brand's problem
	     cannot blank the page. -->
	<ErrorBanner error={store.error} />
	{#if applyStore.error !== '' && !applyDialogOpen}
		<!-- A failed plan or a failed apply, for the same reasons the Sites page
		     renders this: without it, a `plan_config_apply` that fails outright
		     leaves no pending count, no dialog, and no explanation. Suppressed
		     while the dialog is open, which renders the same string itself — a
		     banner behind the scrim is an error the user cannot reach.
		     No `missing`/`onEditSite`: this page has no site list to derive a PHP
		     remedy from and no drawer to open one in. -->
		<ApplyErrorBanner error={applyStore.error} />
	{/if}
	<!-- Reachable from here, not only from Sites: a user who saved settings and
	     closed the dialog without applying would otherwise have to guess that the
	     way back to their pending change is on another page. The count is the
	     whole config set's, which is what it has always been. -->
	<PendingChangesBanner count={applyStore.pendingCount} onReview={() => (applyDialogOpen = true)} />
	<WebServerPanel
		servers={store.servers}
		services={servicesStore.services}
		configText={store.configText}
		configError={store.configError}
		reports={store.reports}
		validating={store.validating}
		{stoppedPools}
		onShowConfig={(id) => void store.showConfig(id)}
		onValidate={(id) => void store.validate(id)}
		onStart={(id) => void servicesStore.start(id)}
		onStop={(id) => void servicesStore.stop(id)}
	/>
	<WebServerSettingsForm
		values={settings.values}
		fieldErrors={settings.fieldErrors}
		error={settings.error}
		saving={settings.saving}
		dirty={settings.dirty}
		canSave={settings.canSave}
		onNumber={(key, raw) => settings.setNumber(key, raw)}
		onBool={(key, value) => settings.setBool(key, value)}
		onText={(key, value) => settings.setText(key, value)}
		onSave={() => void onSave()}
	/>
	{#if applyDialogOpen}
		<!-- The SAME dialog the Sites page opens, on the same plan — reused rather
		     than a second diff renderer, which is how two views of one change set
		     start disagreeing. -->
		<ApplyDialog
			changes={applyStore.changes}
			applying={applyStore.applying}
			error={applyStore.error}
			outcome={applyStore.outcome}
			onApply={() => void applyStore.run()}
			onClose={() => (applyDialogOpen = false)}
		/>
	{/if}
</AppShell>
