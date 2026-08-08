<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		applyConfig,
		onPhpInstallLog,
		onPhpInstallProgress,
		openHomebrewSite,
		planConfigApply
	} from '$lib/ipc';
	import { ApplyStore } from '$lib/apply.svelte';
	import { subscribeLanguageEvents } from '$lib/languages.listeners';
	import { languagesStore as store } from '$lib/languages.shared.svelte';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { uninstallStore } from '$lib/uninstall.shared.svelte';
	import { runningCount } from '$lib/services.derive';
	import AppShell from '$lib/components/AppShell.svelte';
	import ApplyDialog from '$lib/components/ApplyDialog.svelte';
	import Button from '$lib/components/Button.svelte';
	import DefaultPhpNotice from '$lib/components/DefaultPhpNotice.svelte';
	import { isChosenDefault } from '$lib/php-default.derive';
	import LanguageRow from '$lib/components/LanguageRow.svelte';
	import LanguagesEmpty from '$lib/components/LanguagesEmpty.svelte';
	import UninstallDialog from '$lib/components/UninstallDialog.svelte';

	const running = $derived(runningCount(servicesStore.services));

	// The same pair the Sites and Web server pages wire, for the same pipeline:
	// the default major is part of ONE config set, so this page reaches
	// `plan_config_apply`/`apply_config` rather than growing an apply path of
	// its own.
	const applyStore = new ApplyStore({ planConfigApply, applyConfig });

	let applyDialogOpen = $state(false);

	/**
	 * Store the choice, then show what it changes. The dialog is not decoration:
	 * `set_default_php` writes a preference and rewrites nothing, so without this
	 * the badge would move while `localhost:8080` kept serving the old major
	 * until some later, unrelated Apply — and the button's own tooltip promises a
	 * diff. Mirrors `web-server/+page.svelte`'s `onSave`, whose comment gives the
	 * general form of the reason: otherwise you leave a control that visibly does
	 * nothing on the page the user is actually on.
	 */
	async function onMakeDefault(major: string): Promise<void> {
		if (!(await store.setDefault(major))) return;
		await applyStore.refresh();
		applyDialogOpen = true;
	}

	/**
	 * Which major the on-screen error belongs to. `LanguagesStore.installing`
	 * resets to '' the instant `install()` settles — success or failure — so
	 * without a separate marker the row that just finished would lose its
	 * error the moment the user most needs to read it. Set when an install
	 * starts and left alone afterwards (unlike `store.outcome`, which already
	 * carries its own `major`, and unlike the log, which `store.logFor` now
	 * attributes itself — `store.error` carries no major of its own, so this
	 * is still what scopes it to a row).
	 */
	let lastAttempted = $state('');

	async function onInstall(major: string): Promise<void> {
		lastAttempted = major;
		const installed = await store.install(major);
		if (installed) {
			// I1 audit finding: `Supervisor::register` used to emit no event, and
			// `ServicesStore.loadServices()` memoizes its first successful
			// fetch — so a major installed AFTER launch registered a real
			// supervisor row that the services store never learned about. The
			// row would offer Start, the click would genuinely start the
			// pool, and the row would keep saying Start forever (the Services
			// page would not list it either) until the app relaunched.
			//
			// The durable fix has since shipped (tray slice, Task 1): `register`
			// now emits `SupervisorEvent::Registered`, and the layout's
			// `onServiceRegistered` subscription calls
			// `ServicesStore.applyRegistered` on every route — so the row now
			// arrives here on its own. `reload()` stays anyway as a synchronous
			// guarantee right at the one moment this page KNOWS the
			// registered-service set may have changed, rather than depending on
			// how long the event takes to round-trip.
			await servicesStore.reload();
		}
	}

	/**
	 * The "Check again" button's handler — wraps `store.rescan()` with the same
	 * services-store reload as `onInstall`, for the same reason: a rescan can
	 * also register a newly-discovered major's supervisor row (see
	 * `rescan_into_state` on the Rust side), and that row is just as invisible
	 * to `servicesStore` as one `install_php` registers.
	 */
	async function onRescan(): Promise<void> {
		await store.rescan();
		await servicesStore.reload();
	}

	/**
	 * Opens the uninstall confirmation (design D6). Uninstalls nothing: the
	 * plan behind the dialog is a pure, spawn-free query, and `brew uninstall`
	 * only runs once the user confirms in the dialog itself.
	 */
	async function onUninstall(major: string): Promise<void> {
		await uninstallStore.request('php', major);
	}

	/**
	 * The dialog's confirm. Re-reads the PHP environment on success so the row
	 * flips back to "not installed" — the supervisor row disappearing is NOT
	 * this page's job: `SupervisorEvent::Unregistered` reaches
	 * `ServicesStore.applyUnregistered` through the layout's subscription, on
	 * every route (Task 1). `servicesStore.reload()` is kept for the same
	 * reason `onInstall` keeps it: a synchronous guarantee at the one moment
	 * this page KNOWS the registered set changed, independent of how long the
	 * event takes to round-trip.
	 */
	async function onConfirmUninstall(): Promise<void> {
		const uninstalled = await uninstallStore.confirm();
		if (uninstalled) {
			await store.refresh();
			await servicesStore.reload();
		}
	}

	onMount(() => {
		// Every subscription lives in `languages.listeners.ts`, NOT inline here —
		// the same move `routes/databases/+page.svelte` made and for the identical
		// reason: `onMount` does not run under `svelte/server`, so anything written
		// in this closure is untestable by construction, and a severed callback
		// leaves the whole suite green. That is not hypothetical here — the
		// packaged-install progress event shipped with no subscriber at all, and
		// nothing in the suite could have said so. What remains here is only the
		// part a DOM would be needed to test anyway: calling the subscriber, and
		// calling its own disposer.
		let release: (() => void) | null = null;
		let disposed = false;

		void (async () => {
			try {
				const stop = await subscribeLanguageEvents(
					{ onPhpInstallLog, onPhpInstallProgress },
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

		return () => {
			disposed = true;
			release?.();
			release = null;
		};
	});
</script>

<AppShell runningCount={running} active="languages">
	<h1 class="sr-only">OpenVHost — Languages</h1>

	<div class="strip-head">
		<h2 class="section-label">PHP</h2>
	</div>

	<!-- Grouped under a "PHP" heading (spec §6) even though PHP is the only language
	     today: a second runtime (Node.js, Python, Go — see ServBay's equivalent page)
	     becomes a new group here rather than a redesign of this page.

	     Task 7's empty states: `LanguagesEmpty` distinguishes "no route to any PHP
	     at all" from "a route exists, nothing installed yet", rendering ABOVE the
	     rowlist in both cases (never in place of it — see the rowlist's own
	     condition below) as one clear invitation/explanation rather than four
	     rows all failing the same way. Once a version is installed,
	     `LanguagesEmpty` renders nothing and the rowlist is the whole UI.

	     The rowlist itself is hidden only when there is truly nothing to show it
	     for, and that is now the SAME question the dead end asks — hence the same
	     predicate, negated (off-Homebrew slice 5C design D2). It used to be
	     `brewFound || anyInstalled`, which was right about two of its three cases
	     and wrong about the one this programme exists for: a machine with no
	     Homebrew and a packaged PHP available had its rows hidden while being
	     told it could not install PHP. `anyInstalled` still keeps the list
	     visible without brew — an already-running php-fpm pool does not need brew
	     to serve, or to Start/Stop, so a brew that went missing after setup must
	     not hide it from view — and `noRouteToAnyPhp` already folds that in. -->
	<section class="panel languages-panel" aria-label="PHP" data-testid="languages">
		{#if store.error !== '' && store.env === null}
			<div class="empty">
				<div class="title">Could not read the PHP environment</div>
				<p>{store.error}</p>
			</div>
		{:else if store.env}
			<!-- C3 fix: rendered whenever `store.error` is non-empty, INDEPENDENT of
			     whether the rowlist below renders — not only when `store.env` is
			     `null`. Before this, a failed rescan (a poisoned lock, a stack-paths
			     error, a probe spawn failure) set `store.error` and correctly kept
			     `env`, but nothing rendered it: on a no-brew machine the rowlist
			     below never mounts at all, so "Check again" — the one control whose
			     entire purpose is to unstick a user — appeared to do nothing. The
			     per-row `error` prop below is UNCHANGED and still scoped to
			     `lastAttempted`, so an install failure keeps its own row-level
			     message too. -->
			{#if store.error !== ''}
				<div class="banner-error" role="alert" data-testid="languages-page-error">
					{store.error}
				</div>
			{/if}
			<!-- A default that cannot be honoured, said once for the page rather than
			     on a row — the major it names may have NO row at all (a
			     hand-installed `php@7.4` since removed appears in neither the
			     catalogue nor the installed list), and that user is exactly the one
			     who needs telling. Renders nothing in every other state, which is
			     what keeps this invisible until someone chooses. -->
			<DefaultPhpNotice resolved={store.defaultPhp} />
			<LanguagesEmpty
				brewFound={store.brewFound}
				noRouteToAnyPhp={store.noRouteToAnyPhp}
				anyInstalled={store.anyInstalled}
				brewSearched={store.env.brewSearched}
				installing={store.installing}
				onRescan={() => void onRescan()}
				onOpenBrewSite={() => void openHomebrewSite().catch((e) => store.fail(e))}
			/>
			{#if !store.noRouteToAnyPhp}
				<!-- C2 fix: `LanguagesEmpty` only renders its OWN "Check again" button
				     in the dead-end branch, so wherever that branch does not render,
				     the rescan would be unreachable from the UI — yet that is exactly
				     what a user needs after running `brew install php@8.2` in a
				     terminal: discovering a version this page did not have at launch,
				     without a relaunch.

				     The condition is the EXACT COMPLEMENT of `LanguagesEmpty`'s
				     dead-end branch, so precisely one "Check again" is on screen in
				     every state. It used to be `store.brewFound`, which was the same
				     complement only while `!brewFound` was what triggered the dead
				     end. After design D2 it is not: a machine with no Homebrew and a
				     packaged PHP renders no dead end, and gating on `brewFound` there
				     would have left the page with no rescan control at all. -->
				<div class="check-again">
					<!-- A1 audit finding: `rescan_php_runtimes` takes `InstallLock` with
					     `.lock().await` (H1's fix), so this button now blocks for the
					     length of a running install with no feedback, and repeated
					     presses queue unbounded waiters on that mutex. Disabled while
					     `store.installing !== ''`, same condition `LanguageRow`'s own
					     Install button already uses. -->
					<Button
						size="sm"
						testId="languages-check-again-header"
						disabled={store.installing !== ''}
						onclick={() => void onRescan()}
					>
						Check again
					</Button>
				</div>
			{/if}
			{#if !store.noRouteToAnyPhp}
				<div class="rowlist">
					{#each store.env.runtimes as runtime (runtime.major)}
						<LanguageRow
							row={runtime}
							cataloged={runtime.cataloged}
							brewFound={store.brewFound}
							serviceState={runtime.serviceId === null
								? null
								: (servicesStore.services.find((s) => s.id === runtime.serviceId)?.state ?? null)}
							installing={store.installing}
							uninstalling={uninstallStore.uninstalling}
							log={store.logFor(runtime.major)}
							error={runtime.major === lastAttempted ? store.error : ''}
							outcome={store.outcome}
							installProgress={store.progressFor(runtime.major)}
							installTotal={store.installTotal}
							isDefault={isChosenDefault(store.env.defaultPhp, runtime.major)}
							offersDefault={store.offersDefaultChoice && runtime.installed}
							settingDefault={store.settingDefault}
							onInstall={(major) => void onInstall(major)}
							onUninstall={(major) => void onUninstall(major)}
							onMakeDefault={(major) => void onMakeDefault(major)}
							onStart={(id) => void servicesStore.start(id)}
							onStop={(id) => void servicesStore.stop(id)}
						/>
					{/each}
				</div>
			{/if}
		{/if}
	</section>
</AppShell>

<!-- Rendered at the page level, outside `AppShell`, like every other modal in
     this app: `uninstallStore` is shared with the Databases page, and only one
     of the two routes is ever mounted, so this can never double up. -->
{#if uninstallStore.isOpen}
	{#if applyDialogOpen}
		<ApplyDialog
			changes={applyStore.changes}
			applying={applyStore.applying}
			error={applyStore.error}
			outcome={applyStore.outcome}
			onApply={() => void applyStore.run()}
			onClose={() => (applyDialogOpen = false)}
		/>
	{/if}
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
	/* Same recipe as ServicesPanel.svelte's `.strip-head`/`.panel`/`.rowlist`/`.empty` —
	   `.section-label` lives once in lib/styles/tokens.css rather than as a scoped
	   copy here, same reasoning as that panel. */
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
		/* A row costs 940px and cannot get it here: `minWidth` is 960, the rail takes 216 and
		   this panel another 50 of margin and border, so the app's own smallest legal window
		   leaves a row ~694px. The grid overflowed into `.panel`'s `overflow: hidden` and put
		   Uninstall — and on a row that is not installed yet, Install — entirely off the right
		   edge. `LanguageRow` answers with a wrapped layout below that width and queries THIS
		   element rather than the viewport: the row's width comes from the panel, not the
		   screen. Same treatment as SitesPanel's `.rowlist`. */
		container-type: inline-size;
		container-name: langlist;
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
	/* Same failure palette as routes/+page.svelte's own `.banner-error` (Sites),
	   but padded rather than margined: THIS one lives inside `.panel`'s own
	   border/background (that page's sits directly in AppShell), so insetting
	   it with margin would leave a visible gap between the banner and the
	   panel's edge that every other child here (`.empty`, `.rowlist`'s rows)
	   avoids by padding instead. */
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
