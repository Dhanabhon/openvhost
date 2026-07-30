<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		listLogSources,
		readLogWindow,
		revealLogFolder,
		type LogLevel,
		type LogSourceDto
	} from '$lib/ipc';
	import AppShell from '$lib/components/AppShell.svelte';
	import LogSourcePicker from '$lib/components/LogSourcePicker.svelte';
	import LogToolbar from '$lib/components/LogToolbar.svelte';
	import LogBody from '$lib/components/LogBody.svelte';
	import LogStatusLine from '$lib/components/LogStatusLine.svelte';
	import { runningCount } from '$lib/services.derive';
	import { servicesStore } from '$lib/services.shared.svelte';
	import {
		encodeLogSource,
		groupSources,
		parseSourceParam,
		siteSource,
		sourceDomain
	} from '$lib/logs.derive';
	import { LogsStore } from '$lib/logs.svelte';

	// Page-local, not shared (`services.shared.svelte.ts`'s reasoning does not
	// apply here — nothing outside `/logs` needs this state): a fresh store
	// per mount, the same convention `routes/+page.svelte` uses for
	// `SitesStore`. This is also what makes teardown simple — the store is
	// gone the instant the component is, with nothing left to leak.
	const store = new LogsStore({ listLogSources, readLogWindow, revealLogFolder });

	const running = $derived(runningCount(servicesStore.services));
	const grouped = $derived(groupSources(store.sources));
	// Scoped to ring rows only — see LogSourcePicker.svelte's own doc comment
	// on why a php-fpm pool chip cannot be joined to its ServiceState from
	// this catalogue alone.
	const failedServiceIds = $derived(
		new Set(servicesStore.services.filter((s) => s.state.kind === 'failed').map((s) => s.id))
	);
	const filtered = $derived(store.needle !== '' || store.minLevel !== null);

	function onSelect(source: LogSourceDto): void {
		void store.selectSource(source);
	}

	/** The toolbar's Access/Error toggle: switch which stream of the SAME
	 *  domain is showing. A no-op if nothing site-scoped is selected — the
	 *  toolbar only renders this control in that case, but a defensive guard
	 *  costs nothing. */
	function onSelectStream(stream: 'access' | 'error'): void {
		const domain = sourceDomain(store.selected);
		if (domain === null) return;
		void store.selectSource(siteSource(domain, stream));
	}

	function onNeedle(v: string): void {
		void store.setNeedle(v);
	}
	function onCaseSensitive(v: boolean): void {
		void store.setCaseSensitive(v);
	}
	function onMinLevel(v: LogLevel | null): void {
		void store.setMinLevel(v);
	}
	function onJumpToLatest(): void {
		void store.jumpToLatest();
	}
	function onRevealFolder(): void {
		void store.revealFolder();
	}

	/** Spec D6: "scrolling away turns [Follow] off" — `LogBody`'s scroll
	 *  listener reports whether the viewport is still near the bottom; only
	 *  scrolling AWAY changes anything here. Re-engaging Follow is the
	 *  user's own explicit choice (the toolbar's switch, or Jump to latest),
	 *  never restored just because they scrolled back down on their own —
	 *  that would fight a user deliberately re-reading the tail. */
	function onScroll(nearBottom: boolean): void {
		if (!nearBottom) store.setFollow(false);
	}

	onMount(() => {
		let disposed = false;

		void (async () => {
			await store.loadSources();
			if (disposed) return;
			const requested = parseSourceParam(window.location.search);
			await store.selectFromDeepLink(requested);
			if (disposed) return;
			// Reflect the RESOLVED selection back into the address bar —
			// `history.replaceState`, not `pushState`: this is a correction
			// of the current entry (a bare `/logs` becomes `/logs?source=…`
			// once a default is picked), not a new navigation step Back
			// should stop at.
			if (store.selected !== null) {
				// Plain string building, not `new URL(...)`: eslint's
				// `svelte/prefer-svelte-reactivity` flags the built-in URL
				// class as a reactivity footgun (mutating it would not
				// trigger Svelte updates) — moot here (a one-shot,
				// non-reactive browser-API call inside `onMount`, never read
				// back), but there is nothing this needs `URL` for anyway.
				const query = new URLSearchParams({
					source: encodeLogSource(store.selected)
				}).toString();
				window.history.replaceState(null, '', `${window.location.pathname}?${query}`);
			}
		})();

		// Poll gate: mounted AND the window is visible — mirrors
		// `+layout.svelte`'s identical `StatsStore` wiring exactly. See
		// `logs.svelte.ts`'s file header for why `follow` (the auto-scroll
		// toggle) is deliberately NOT a third gate here.
		const onVisibility = (): void => {
			if (document.visibilityState === 'visible') store.start();
			else store.stop();
		};
		onVisibility();
		document.addEventListener('visibilitychange', onVisibility);

		// The teardown spec D3 calls a "tested requirement": route change
		// (this cleanup function, run on unmount) and blur (the listener
		// above, via `onVisibility`) can never leave the interval running
		// past either.
		return () => {
			disposed = true;
			document.removeEventListener('visibilitychange', onVisibility);
			store.stop();
		};
	});
</script>

<AppShell runningCount={running} active="logs">
	<div class="page-head">
		<h1>Logs</h1>
	</div>
	<LogSourcePicker
		services={grouped.services}
		siteDomains={grouped.siteDomains}
		selected={store.selected}
		{failedServiceIds}
		{onSelect}
	/>
	<LogToolbar
		needle={store.needle}
		caseSensitive={store.caseSensitive}
		minLevel={store.minLevel}
		follow={store.follow}
		newRowsWhilePaused={store.newRowsWhilePaused}
		selected={store.selected}
		{onNeedle}
		{onCaseSensitive}
		{onMinLevel}
		onSetFollow={(v) => store.setFollow(v)}
		{onJumpToLatest}
		{onSelectStream}
	/>
	<LogBody
		selected={store.selected}
		requestedUnavailable={store.requestedUnavailable}
		readError={store.readError}
		exists={store.exists}
		rows={store.rows}
		{filtered}
		reset={store.reset}
		follow={store.follow}
		{onRevealFolder}
		{onScroll}
	/>
	<LogStatusLine
		selected={store.selected}
		requestedUnavailable={store.requestedUnavailable}
		sizeBytes={store.sizeBytes}
		truncatedLines={store.truncatedLines}
		scanBoundReached={store.scanBoundReached}
		follow={store.follow}
		{onRevealFolder}
	/>
</AppShell>

<style>
	/* Same page-head recipe as SitesPanel.svelte's own (mock.css's `.page-head`) —
	   this route composes its panels directly rather than through a wrapping
	   "LogsPanel" component, so the heading lives here instead. */
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
</style>
