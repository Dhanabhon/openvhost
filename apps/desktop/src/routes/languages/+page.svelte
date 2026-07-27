<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { installPhp, onPhpInstallLog, phpEnvironment, rescanPhpRuntimes } from '$lib/ipc';
	import { LanguagesStore } from '$lib/languages.svelte';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { runningCount } from '$lib/services.derive';
	import AppShell from '$lib/components/AppShell.svelte';
	import LanguageRow from '$lib/components/LanguageRow.svelte';
	import LanguagesEmpty from '$lib/components/LanguagesEmpty.svelte';

	const store = new LanguagesStore({ phpEnvironment, rescanPhpRuntimes, installPhp });
	const running = $derived(runningCount(servicesStore.services));

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
		await store.install(major);
	}

	/**
	 * Reads running state from the shared services store rather than keeping a
	 * second copy — two sources for one fact is how they disagree. `null`
	 * covers a row with no pool yet (not installed, or installed but never
	 * started so the supervisor has no entry for it).
	 */
	function isRunning(serviceId: string | null): boolean {
		if (serviceId === null) return false;
		const svc = servicesStore.services.find((s) => s.id === serviceId);
		return svc !== undefined && svc.state.kind !== 'stopped' && svc.state.kind !== 'failed';
	}

	onMount(() => {
		let unlisten: (() => void) | null = null;
		let disposed = false;

		void (async () => {
			try {
				const stop = await onPhpInstallLog((ev) => store.appendLog(ev.major, ev.line));
				// Mirrors services/+page.svelte's onServiceLog wiring: this page can
				// unmount while the listener registration is still in flight.
				if (disposed) {
					stop();
					return;
				}
				unlisten = stop;
				await store.refresh();
			} catch (e) {
				store.fail(e);
			}
		})();

		return () => {
			disposed = true;
			unlisten?.();
			unlisten = null;
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

	     Task 7's empty states: `LanguagesEmpty` reads `store.brewFound` and
	     `store.anyInstalled` to distinguish "no Homebrew at all" (the rowlist below
	     is hidden entirely in that case — every Install button in it would just
	     fail with no brew to run) from "Homebrew found, nothing installed yet" (it
	     renders ABOVE the rowlist as one clear invitation, and the rowlist stays
	     visible below with its own working per-version Install buttons — the
	     invitation does not replace them). Once a version is installed,
	     `LanguagesEmpty` renders nothing and the rowlist is the whole UI, same as
	     before this task. -->
	<section class="panel languages-panel" aria-label="PHP" data-testid="languages">
		{#if store.error !== '' && store.env === null}
			<div class="empty">
				<div class="title">Could not read the PHP environment</div>
				<p>{store.error}</p>
			</div>
		{:else if store.env}
			<LanguagesEmpty
				brewFound={store.brewFound}
				anyInstalled={store.anyInstalled}
				brewSearched={store.env.brewSearched}
				onRescan={() => void store.rescan()}
			/>
			{#if store.brewFound}
				<div class="rowlist">
					{#each store.env.runtimes as runtime (runtime.major)}
						<LanguageRow
							row={runtime}
							running={isRunning(runtime.serviceId)}
							installing={store.installing}
							log={store.logFor(runtime.major)}
							error={runtime.major === lastAttempted ? store.error : ''}
							outcome={store.outcome}
							onInstall={(major) => void onInstall(major)}
							onStart={(id) => void servicesStore.start(id)}
							onStop={(id) => void servicesStore.stop(id)}
						/>
					{/each}
				</div>
			{/if}
		{/if}
	</section>
</AppShell>

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
</style>
