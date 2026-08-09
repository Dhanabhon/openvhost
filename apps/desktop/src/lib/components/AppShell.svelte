<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import TitleBar from './TitleBar.svelte';
	import Rail from './Rail.svelte';
	import ErrorBanner from './ErrorBanner.svelte';
	import StoreUnavailableBanner from './StoreUnavailableBanner.svelte';
	import BootCheckFailedBanner from './BootCheckFailedBanner.svelte';
	import StatusBar from './StatusBar.svelte';
	import { bootStatusStore } from '$lib/boot-status.shared.svelte';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { statsStore } from '$lib/stats.shared.svelte';
	import { storeStatusStore } from '$lib/store-status.shared.svelte';

	// Defaults to 'sites', which is what `/` renders (routes/+page.svelte) — so the landing
	// page needs no `active` of its own, and a new route that forgets the prop highlights the
	// rail's default destination rather than an unrelated one. Services, Web server, Languages,
	// Databases and Logs pass it explicitly (routes/services/+page.svelte,
	// routes/web-server/+page.svelte, routes/languages/+page.svelte, routes/databases/+page.svelte,
	// routes/logs/+page.svelte). Keep this union and default in step with Rail.svelte's own.
	let {
		runningCount,
		active = 'sites',
		children
	}: {
		runningCount: number;
		active?: 'services' | 'sites' | 'web-server' | 'languages' | 'databases' | 'logs';
		children: import('svelte').Snippet;
	} = $props();
</script>

<div class="window">
	<TitleBar {runningCount} />
	<div class="shell">
		<Rail {active} />
		<main class="content">
			<!-- Rendered here rather than per-page so a supervisor failure is never silent on
			     whichever route happens to be showing. The layout performs the startup load,
			     and its failure would otherwise be visible only as an unexplained "0 running"
			     in the titlebar. Reads the shared store directly — AppShell already displays
			     supervisor-derived state (`runningCount`), so this is the same coupling, not a
			     new one, and routing it through a prop would let a new page forget it. -->
			<ErrorBanner error={servicesStore.error} />
			<!-- Rendered here for the same reason ErrorBanner is, and one more.
			     The condition IS app-level — `state.db` is down everywhere, not on
			     one page — and one banner covers Sites, Languages, Databases and
			     Logs at once (optional-state.db design D5). It lives in AppShell
			     rather than in `routes/+layout.svelte` itself because `.window` is
			     a `height: 100%` grid and a sibling banner would push the titlebar
			     and status strip out of the window; the LOAD is in the layout,
			     which is the component that outlives navigation. -->
			<StoreUnavailableBanner reason={storeStatusStore.reason} />
			<!-- Here rather than as a sibling of `{@render children()}` in
			     `routes/+layout.svelte`, for the reason StoreUnavailableBanner
			     gives one line up and no other: `.window` is a `height: 100%`
			     grid, and a banner rendered beside it would push the titlebar and
			     status strip out of the window. The layout still owns the ASK and
			     the gating decision — this is only where the answer's quietest
			     outcome becomes visible.

			     Reaching this component at all means the children ARE rendering,
			     which is the whole point: `boot_status` failing is not a reason to
			     blank a working app. -->
			<BootCheckFailedBanner error={bootStatusStore.askFailed} />
			{@render children()}
		</main>
	</div>
	<StatusBar
		servicesBytes={statsStore.servicesBytes}
		processCount={statsStore.processCount}
		homeBytes={statsStore.homeBytes}
		homePending={statsStore.homePending}
	/>
</div>

<style>
	/* Ported from docs/design/mock.css (.window/.shell/.content), adapted to FILL the real Tauri
	   window instead of the mockup's fixed 1180x760 bordered frame (that frame simulated an OS
	   window floating on a page — border/border-radius/box-shadow all removed here because the
	   real macOS window already draws its own chrome; this div IS the window's content, not a
	   picture of one). Relies on `html, body { height: 100% }` in routes/layout.css so this
	   height: 100% has a definite ancestor height to resolve against.
	   `auto 1fr auto`: titlebar, the shell, and the status strip. The strip is a
	   THIRD ROW rather than a child of `.content` because it reports window-level
	   state — putting it inside `.content` (the one scrolling region) would make it
	   scroll away with the page and read as part of it. */
	.window {
		display: grid;
		grid-template-rows: auto 1fr auto;
		height: 100%;
		width: 100%;
		background: var(--vh-bg);
	}
	/* Unchanged from mock.css: min-height: 0 lets this grid row actually shrink below its
	   content's natural size instead of forcing .window to overflow — the standard fix for
	   "grid/flex children ignore the parent's height" — so .content's overflow can do its job.
	   `position: relative` is an addition beyond mock.css's own `.shell` rule — the mock only
	   sets it as an inline `style=""` on docs/design/site-editor.html's specific demo page
	   (the one screen there that renders a drawer). This app's SiteDrawer is a Task-4 addition
	   rendered deep inside `.content` (via `{@render children()}`), not as a `.shell`-level
	   sibling like the flat mock DOM — so `.shell` needs its own real `position: relative` for
	   the drawer's `position: absolute; inset: 0` backdrop/aside to resolve against the shell
	   (rail + content, below the titlebar) instead of the viewport (which would incorrectly
	   cover the titlebar too). A property with no `top`/`left`/etc. offset of its own, so it
	   has zero visual effect on `.shell` or any page that never renders a drawer. */
	.shell {
		display: grid;
		grid-template-columns: 216px 1fr;
		min-height: 0;
		position: relative;
	}
	/* Unchanged from mock.css: this is the one scrolling region in the whole shell. */
	.content {
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
		overflow: auto;
	}
	/* Beyond mock.css, and the fix for a bug that hit every page at once.

	   `.content` is a COLUMN FLEX container, so everything a route renders into it is a
	   flex item with the default `flex-shrink: 1`. A flex item is normally protected from
	   shrinking below its own content by its automatic minimum size — but that protection
	   is switched off for any item whose `overflow` is not `visible`, and every panel in
	   this app sets `overflow: hidden` so its rounded corners clip the rows inside it
	   (SitesPanel, ServicesPanel, WebServerPanel, WebServerSettingsForm). Those panels are
	   therefore shrinkable all the way to zero, and once a page is taller than the window
	   the browser shrinks them and clips their content instead of scrolling it.

	   It showed up as the nginx card's "Version 1.31.3" sliced in half by the card's own
	   bottom edge. It was latent on the other pages for as long as they happened to fit;
	   the Web server page simply became the first one tall enough to expose it.

	   `flex-shrink: 0` makes every child keep its natural height, which is what turns the
	   excess into scrolling — the job `.content`'s `overflow: auto` was already there to
	   do and never got the chance. */
	.content > :global(*) {
		flex-shrink: 0;
	}
</style>
