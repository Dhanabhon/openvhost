<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { onMysqlInitLog, onMysqlInstallLog, openHomebrewSite } from '$lib/ipc';
	import { databasesStore as store } from '$lib/databases.shared.svelte';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { runningCount } from '$lib/services.derive';
	import { catalogedMajors } from '$lib/databases.derive';
	import { copyToClipboard } from '$lib/utils/clipboard';
	import AppShell from '$lib/components/AppShell.svelte';
	import Button from '$lib/components/Button.svelte';
	import DatabasesEmpty from '$lib/components/DatabasesEmpty.svelte';
	import MysqlRow from '$lib/components/MysqlRow.svelte';

	const running = $derived(runningCount(servicesStore.services));
	const catalogedMajorsList = $derived(store.env ? catalogedMajors(store.env.instances) : []);

	/**
	 * Which major the on-screen INSTALL/INIT error belongs to. Mirrors
	 * `routes/languages/+page.svelte`'s own `lastAttempted`: `store.error`
	 * carries no major of its own — `installing`/`initializing` reset to '' the
	 * instant `install()`/`initialize()` settle (success or failure), so
	 * without a separate marker the row that just finished would lose its
	 * error the moment the user most needs to read it.
	 */
	let lastInstallAttempted = $state('');
	let lastInitAttempted = $state('');

	async function onInstall(major: string): Promise<void> {
		lastInstallAttempted = major;
		const installed = await store.install(major);
		if (installed) {
			// I1-style fix (Languages page's own `onInstall` carries the full
			// audit-finding comment this mirrors): `install_mysql` can register a
			// supervisor row directly (a pre-existing Initialized datadir found
			// right after the binary appears), and `ServicesStore.loadServices()`
			// memoizes its first successful fetch — so this is the one moment
			// this page KNOWS the registered-service set may have changed.
			await servicesStore.reload();
		}
	}

	/** Mirrors `onInstall` for the "Check again" affordance — a rescan can also
	 *  register a newly-discovered major's supervisor row (see
	 *  `rescan_mysql_into_state` on the Rust side). */
	async function onRescan(): Promise<void> {
		await store.rescan();
		await servicesStore.reload();
	}

	async function onInitialize(major: string): Promise<void> {
		lastInitAttempted = major;
		const initialized = await store.initialize(major);
		if (initialized) {
			// The USUAL way a MySQL supervisor row appears: `initialize_mysql`
			// registers it directly on a successful staged init.
			await servicesStore.reload();
		}
	}

	/** Fetch-if-needed then write to the clipboard — the store owns the
	 *  fetch-and-cache half (spec D3/D6: never fetched eagerly), this page
	 *  owns the browser-API half (see `MysqlCredentials.svelte`'s own note on
	 *  why that split exists). */
	async function onCopyPassword(major: string): Promise<void> {
		await store.reveal(major);
		const password = store.passwords[major];
		if (password !== undefined) await copyToClipboard(password);
	}

	onMount(() => {
		let unlistenInstall: (() => void) | null = null;
		let unlistenInit: (() => void) | null = null;
		let disposed = false;

		void (async () => {
			try {
				const stopInstall = await onMysqlInstallLog((ev) =>
					store.appendInstallLog(ev.major, ev.line)
				);
				const stopInit = await onMysqlInitLog((ev) => store.appendInitLog(ev.major, ev.line));
				// Mirrors languages/+page.svelte's onMount wiring: this page can
				// unmount while the listener registrations are still in flight.
				if (disposed) {
					stopInstall();
					stopInit();
					return;
				}
				unlistenInstall = stopInstall;
				unlistenInit = stopInit;
				await store.refresh();
			} catch (e) {
				store.fail(e);
			}
		})();

		return () => {
			disposed = true;
			unlistenInstall?.();
			unlistenInit?.();
			unlistenInstall = null;
			unlistenInit = null;
		};
	});
</script>

<AppShell runningCount={running} active="databases">
	<h1 class="sr-only">OpenVHost — Databases</h1>

	<div class="strip-head">
		<h2 class="section-label">MySQL</h2>
	</div>

	<!-- Grouped under a "MySQL" heading (mirrors Languages' own "PHP" grouping)
	     even though MySQL is the only database engine today — MariaDB (spec
	     Deferred: "same seams, next slice") becomes a new group here rather
	     than a redesign of this page. -->
	<section class="panel databases-panel" aria-label="MySQL" data-testid="databases">
		{#if store.error !== '' && store.env === null}
			<div class="empty">
				<div class="title">Could not read the MySQL environment</div>
				<p>{store.error}</p>
			</div>
		{:else if store.env}
			<!-- Rendered whenever `store.error` is non-empty, independent of
			     whether the rowlist below renders — same C3 fix Languages carries
			     (see that page's own comment): a failed rescan must stay visible
			     even on a no-brew machine, where the rowlist never mounts at all. -->
			{#if store.error !== ''}
				<div class="banner-error" role="alert" data-testid="databases-page-error">
					{store.error}
				</div>
			{/if}
			<DatabasesEmpty
				brewFound={store.brewFound}
				anyInstalled={store.anyInstalled}
				brewSearched={store.env.brewSearched}
				installing={store.installing || store.initializing}
				onRescan={() => void onRescan()}
				onOpenBrewSite={() => void openHomebrewSite().catch((e) => store.fail(e))}
			/>
			{#if store.brewFound}
				<!-- Shown for every brewFound state (both "brew, no MySQL yet" and
				     "brew, already installed") so it never sits next to
				     `DatabasesEmpty`'s own no-brew copy of the same control —
				     mirrors Languages' C2 fix exactly. -->
				<div class="check-again">
					<Button
						size="sm"
						testId="databases-check-again-header"
						disabled={store.installing !== '' || store.initializing !== ''}
						onclick={() => void onRescan()}
					>
						Check again
					</Button>
				</div>
			{/if}
			{#if store.brewFound || store.anyInstalled}
				<div class="rowlist">
					{#each store.env.instances as instance (instance.major)}
						<MysqlRow
							{instance}
							brewFound={store.brewFound}
							installingMajor={store.installing}
							installLog={store.installLogFor(instance.major)}
							installOutcome={store.installOutcome}
							installError={instance.major === lastInstallAttempted ? store.error : ''}
							initializingMajor={store.initializing}
							initLog={store.initLogFor(instance.major)}
							initFailure={store.initFailureFor(instance.major)}
							initError={instance.major === lastInitAttempted ? store.error : ''}
							{catalogedMajorsList}
							serviceState={instance.serviceId === null
								? null
								: (servicesStore.services.find((s) => s.id === instance.serviceId)?.state ?? null)}
							password={store.passwords[instance.major]}
							revealing={store.revealing[instance.major] ?? false}
							passwordError={store.passwordError[instance.major] ?? ''}
							resetting={store.resetting[instance.major] ?? false}
							resetOutcome={store.resetOutcome[instance.major]}
							resetError={store.resetError[instance.major] ?? ''}
							verifying={store.verifying[instance.major] ?? false}
							verifyResult={store.verifyResult[instance.major]}
							verifyError={store.verifyError[instance.major] ?? ''}
							onInstall={(major) => void onInstall(major)}
							onInitialize={(major) => void onInitialize(major)}
							onStart={(id) => void servicesStore.start(id)}
							onStop={(id) => void servicesStore.stop(id)}
							onReveal={(major) => void store.reveal(major)}
							onHide={(major) => store.forgetPassword(major)}
							onCopyPassword={(major) => void onCopyPassword(major)}
							onReset={(major) => void store.resetPassword(major)}
							onVerify={(major) => void store.verifyConnection(major)}
						/>
					{/each}
				</div>
			{/if}
		{/if}
	</section>
</AppShell>

<style>
	/* Same recipe as ServicesPanel.svelte's/languages/+page.svelte's own
	   `.strip-head`/`.panel`/`.rowlist`/`.empty` — `.section-label` lives once
	   in lib/styles/tokens.css rather than as a scoped copy here, same
	   reasoning as that panel. */
	.strip-head {
		display: flex;
		align-items: baseline;
	}
	.panel {
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		margin: 0 var(--vh-space-6) var(--vh-space-6);
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
	.empty p {
		margin: 4px 0 0;
	}
	.banner-error {
		padding: var(--vh-space-3) var(--vh-space-6);
		background: var(--vh-fail-tint);
		color: var(--vh-fail);
		font-size: var(--vh-text-table);
		border-bottom: 1px solid var(--vh-border);
	}
	.check-again {
		display: flex;
		justify-content: flex-end;
		padding: var(--vh-space-3) var(--vh-space-4);
		border-bottom: 1px solid var(--vh-border);
	}
</style>
