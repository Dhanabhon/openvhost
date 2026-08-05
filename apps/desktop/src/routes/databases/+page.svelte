<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		onMariadbInitLog,
		onMariadbInstallLog,
		onMariadbInstallProgress,
		onMysqlInitLog,
		onMysqlInstallLog,
		onMysqlInstallProgress
	} from '$lib/ipc';
	import { subscribeDatabaseEvents } from '$lib/databases.listeners';
	import { subscribeMariadbEvents } from '$lib/mariadb.listeners';
	import { databasesStore as store } from '$lib/databases.shared.svelte';
	import { mariadbStore } from '$lib/mariadb.shared.svelte';
	import { MARIADB_SERIES, mariadbInstance } from '$lib/mariadb.svelte';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { uninstallStore } from '$lib/uninstall.shared.svelte';
	import { runningCount } from '$lib/services.derive';
	import { catalogedMajors } from '$lib/databases.derive';
	import { copyToClipboard } from '$lib/utils/clipboard';
	import AppShell from '$lib/components/AppShell.svelte';
	import Button from '$lib/components/Button.svelte';
	import DatabasesEmpty from '$lib/components/DatabasesEmpty.svelte';
	import MysqlRow from '$lib/components/MysqlRow.svelte';
	import UninstallDialog from '$lib/components/UninstallDialog.svelte';

	const running = $derived(runningCount(servicesStore.services));
	const catalogedMajorsList = $derived(store.env ? catalogedMajors(store.env.instances) : []);
	/** The single MariaDB row, adapted from the single-instance environment —
	 *  `null` before the first load settles, mirroring `store.env`'s own gate
	 *  on the MySQL rowlist below. */
	const mariadbRow = $derived(mariadbStore.env ? mariadbInstance(mariadbStore.env) : null);
	const mariadbServiceState = $derived(
		mariadbRow?.serviceId === null || mariadbRow?.serviceId === undefined
			? null
			: (servicesStore.services.find((s) => s.id === mariadbRow.serviceId)?.state ?? null)
	);

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
	 *  why that split exists). Review fix (MANDATORY): calls
	 *  `store.copyPassword`, NOT `store.reveal` — the latter also turns the
	 *  on-screen display gate on, and Copy must never un-mask the field (a
	 *  screen-share is exactly the scenario this avoids). */
	async function onCopyPassword(major: string): Promise<void> {
		const password = await store.copyPassword(major);
		if (password !== undefined) await copyToClipboard(password);
	}

	/**
	 * Opens the uninstall confirmation (design D6). Uninstalls nothing on its
	 * own — and for MySQL that dialog is the whole point of this slice: it is
	 * the only place the user is told the datadir and the stored root password
	 * survive (design D2), which is what makes the button safe to press.
	 */
	async function onUninstall(major: string): Promise<void> {
		await uninstallStore.request('mysql', major);
	}

	/**
	 * The dialog's confirm, shared with MariaDB's own Uninstall (below): the
	 * kind is captured BEFORE `confirm()` runs, because a successful confirm
	 * clears `uninstallStore.target` back to `null` — there is no way to ask
	 * afterwards which package it had just been open for. Re-reads only the
	 * store the uninstalled package actually belongs to; the supervisor row
	 * disappearing either way is handled by the layout's
	 * `onServiceUnregistered` subscription, not here.
	 */
	async function onConfirmUninstall(): Promise<void> {
		const kind = uninstallStore.target?.kind ?? null;
		const uninstalled = await uninstallStore.confirm();
		if (uninstalled) {
			if (kind === 'mariadb') {
				await mariadbStore.refresh();
			} else {
				await store.refresh();
			}
			await servicesStore.reload();
		}
	}

	/**
	 * Which MariaDB action the on-screen error belongs to (install vs
	 * initialize) — the single-instance mirror of `lastInstallAttempted`/
	 * `lastInitAttempted` above. Booleans rather than a major string: this
	 * build ships exactly one series, so there is no "which" left to track,
	 * only "was the LAST attempt on this store an install, an initialize, or
	 * neither" — without this gate an unrelated `rescan()` failure would
	 * render as if the last Install/Initialize press had failed.
	 */
	let lastMariadbInstallAttempted = $state(false);
	let lastMariadbInitAttempted = $state(false);

	async function onInstallMariadb(): Promise<void> {
		lastMariadbInstallAttempted = true;
		const installed = await mariadbStore.install();
		if (installed) {
			await servicesStore.reload();
		}
	}

	/** Mirrors `onRescan` for the MariaDB group's own "Check again". */
	async function onRescanMariadb(): Promise<void> {
		await mariadbStore.rescan();
		await servicesStore.reload();
	}

	async function onInitializeMariadb(): Promise<void> {
		lastMariadbInitAttempted = true;
		const initialized = await mariadbStore.initialize();
		if (initialized) {
			await servicesStore.reload();
		}
	}

	/** Mirrors `onCopyPassword`'s split — see that function's own doc comment
	 *  for why Copy calls `copyPassword`, never `reveal`. */
	async function onCopyMariadbPassword(): Promise<void> {
		const password = await mariadbStore.copyPassword();
		if (password !== undefined) await copyToClipboard(password);
	}

	/** Opens the SAME shared uninstall confirmation MySQL's rows use (design
	 *  D6/D5): MariaDB's uninstall goes through the identical
	 *  `PackageKind::Mariadb` path, so there is one dialog, not two. */
	async function onUninstallMariadb(): Promise<void> {
		await uninstallStore.request('mariadb', MARIADB_SERIES);
	}

	onMount(() => {
		// Every subscription lives in `databases.listeners.ts`/`mariadb.listeners.ts`,
		// NOT inline here: `onMount` does not run under `svelte/server`, so anything
		// written in this closure is untestable by construction — a neuter experiment
		// severed the progress callback and the whole suite stayed green. What
		// remains here is only the part a DOM would be needed to test anyway:
		// calling each subscriber, and calling its own disposer.
		let release: (() => void) | null = null;
		let disposed = false;

		void (async () => {
			try {
				const stop = await subscribeDatabaseEvents(
					{ onMysqlInstallLog, onMysqlInstallProgress, onMysqlInitLog },
					store,
					uninstallStore,
					() => disposed
				);
				release = stop;
				await store.refresh();
			} catch (e) {
				store.fail(e);
			}
		})();

		// A PARALLEL subscription, not a wider one (design D6): `MariadbStore`
		// holds scalars where `DatabasesStore` holds per-major maps, so this
		// page manages its own, independent disposer for it rather than folding
		// it into the block above.
		let releaseMariadb: (() => void) | null = null;
		let disposedMariadb = false;

		void (async () => {
			try {
				const stop = await subscribeMariadbEvents(
					{ onMariadbInstallLog, onMariadbInstallProgress, onMariadbInitLog },
					mariadbStore,
					uninstallStore,
					() => disposedMariadb
				);
				releaseMariadb = stop;
				await mariadbStore.refresh();
			} catch (e) {
				mariadbStore.fail(e);
			}
		})();

		return () => {
			disposed = true;
			release?.();
			release = null;
			disposedMariadb = true;
			releaseMariadb?.();
			releaseMariadb = null;
		};
	});
</script>

<AppShell runningCount={running} active="databases">
	<h1 class="sr-only">OpenVHost — Databases</h1>

	<div class="strip-head">
		<h2 class="section-label">MySQL</h2>
	</div>

	<!-- Grouped under a "MySQL" heading (mirrors Languages' own "PHP" grouping).
	     MariaDB (P1 MariaDB UI design) is the second group below, exactly as
	     this comment always said it would be — a new group here, not a
	     redesign of this page. -->
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
			<DatabasesEmpty anyInstalled={store.anyInstalled} />
			<!-- Unconditional since the move off Homebrew: `DatabasesEmpty` no
			     longer renders a rescan control of its own (it had one only inside
			     the no-brew guide, which is gone), so this can never sit beside a
			     duplicate. -->
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
			<!-- Also unconditional. It used to be gated on `brewFound`, which is
			     the exact gate this slice removes: a machine with no Homebrew can
			     now install MySQL, so hiding every row from it would hide the only
			     control that does the job. -->
			<div class="rowlist">
				{#each store.env.instances as instance (instance.major)}
					<MysqlRow
						{instance}
						installingMajor={store.installing}
						installProgress={store.installProgress}
						installTotal={store.installTotal}
						cancellingInstall={store.cancellingInstall}
						installOutcome={store.installOutcome}
						installError={instance.major === lastInstallAttempted ? store.error : ''}
						initializingMajor={store.initializing}
						initLog={store.initLogFor(instance.major)}
						initFailure={store.initFailureFor(instance.major)}
						initError={instance.major === lastInitAttempted ? store.error : ''}
						uninstallingMajor={uninstallStore.uninstalling}
						{catalogedMajorsList}
						serviceState={instance.serviceId === null
							? null
							: (servicesStore.services.find((s) => s.id === instance.serviceId)?.state ?? null)}
						password={store.passwords[instance.major]}
						revealed={store.revealed[instance.major] ?? false}
						revealing={store.revealing[instance.major] ?? false}
						passwordError={store.passwordError[instance.major] ?? ''}
						resetting={store.resetting[instance.major] ?? false}
						resetOutcome={store.resetOutcome[instance.major]}
						resetError={store.resetError[instance.major] ?? ''}
						verifying={store.verifying[instance.major] ?? false}
						verifyResult={store.verifyResult[instance.major]}
						verifyError={store.verifyError[instance.major] ?? ''}
						onInstall={(major) => void onInstall(major)}
						onCancelInstall={() => void store.cancelInstall()}
						onInitialize={(major) => void onInitialize(major)}
						onUninstall={(major) => void onUninstall(major)}
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
	</section>

	<div class="strip-head">
		<h2 class="section-label">MariaDB</h2>
	</div>

	<!-- The second group this page's own comment above always anticipated.
	     A single row, not an `{#each}`: this build ships exactly one series
	     (`MARIADB_SERIES`), so a list whose length is always 0 or 1 would
	     invent a key nothing can vary — the same reasoning
	     `MariadbInstanceRepo` gives for leaving `major` off `MariadbInstance`
	     (design D6). -->
	<section class="panel databases-panel" aria-label="MariaDB" data-testid="databases-mariadb">
		{#if mariadbStore.error !== '' && mariadbStore.env === null}
			<div class="empty">
				<div class="title">Could not read the MariaDB environment</div>
				<p>{mariadbStore.error}</p>
			</div>
		{:else if mariadbRow}
			{#if mariadbStore.error !== ''}
				<div class="banner-error" role="alert" data-testid="databases-mariadb-page-error">
					{mariadbStore.error}
				</div>
			{/if}
			<DatabasesEmpty engine="mariadb" anyInstalled={mariadbStore.anyInstalled} />
			<div class="check-again">
				<Button
					size="sm"
					testId="mariadb-check-again-header"
					disabled={mariadbStore.installing || mariadbStore.initializing}
					onclick={() => void onRescanMariadb()}
				>
					Check again
				</Button>
			</div>
			<div class="rowlist">
				<MysqlRow
					engine="mariadb"
					instance={mariadbRow}
					installingMajor={mariadbStore.installingMajor}
					installProgress={mariadbStore.installProgress}
					installTotal={mariadbStore.installTotal}
					cancellingInstall={mariadbStore.cancellingInstall}
					installOutcome={null}
					mariadbInstallOutcome={mariadbStore.installOutcome === null
						? null
						: { major: MARIADB_SERIES, result: mariadbStore.installOutcome.result }}
					installError={lastMariadbInstallAttempted ? mariadbStore.error : ''}
					initializingMajor={mariadbStore.initializingMajor}
					initLog={mariadbStore.initLog}
					initFailure={mariadbStore.initFailure}
					initError={lastMariadbInitAttempted ? mariadbStore.error : ''}
					uninstallingMajor={uninstallStore.uninstalling}
					catalogedMajorsList={[MARIADB_SERIES]}
					serviceState={mariadbServiceState}
					password={mariadbStore.password}
					revealed={mariadbStore.revealed}
					revealing={mariadbStore.revealing}
					passwordError={mariadbStore.passwordError}
					resetting={mariadbStore.resetting}
					resetOutcome={mariadbStore.resetOutcome}
					resetError={mariadbStore.resetError}
					verifying={mariadbStore.verifying}
					verifyResult={mariadbStore.verifyResult}
					verifyError={mariadbStore.verifyError}
					onInstall={() => void onInstallMariadb()}
					onCancelInstall={() => void mariadbStore.cancelInstall()}
					onInitialize={() => void onInitializeMariadb()}
					onUninstall={() => void onUninstallMariadb()}
					onStart={(id) => void servicesStore.start(id)}
					onStop={(id) => void servicesStore.stop(id)}
					onReveal={() => void mariadbStore.reveal()}
					onHide={() => mariadbStore.forgetPassword()}
					onCopyPassword={() => void onCopyMariadbPassword()}
					onReset={() => void mariadbStore.resetPassword()}
					onVerify={() => void mariadbStore.verifyConnection()}
				/>
			</div>
		{/if}
	</section>
</AppShell>

<!-- Same shared store the Languages page renders; only one of the two routes is
     ever mounted, so the dialog can never double up. -->
{#if uninstallStore.isOpen}
	<UninstallDialog
		plan={uninstallStore.plan}
		planning={uninstallStore.planning}
		uninstalling={uninstallStore.uninstalling !== ''}
		error={uninstallStore.error}
		log={uninstallStore.log}
		onCancel={() => uninstallStore.close()}
		onConfirm={() => void onConfirmUninstall()}
	/>
{/if}

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
