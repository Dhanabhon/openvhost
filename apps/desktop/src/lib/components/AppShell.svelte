<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import TitleBar from './TitleBar.svelte';
	import Rail from './Rail.svelte';

	// Defaults to 'services' so the existing Services page (routes/+page.svelte), which never
	// passes `active`, needs no change.
	let {
		runningCount,
		active = 'services',
		children
	}: {
		runningCount: number;
		active?: 'services' | 'sites';
		children: import('svelte').Snippet;
	} = $props();
</script>

<div class="window">
	<TitleBar {runningCount} />
	<div class="shell">
		<Rail {active} />
		<main class="content">{@render children()}</main>
	</div>
</div>

<style>
	/* Ported from docs/design/mock.css (.window/.shell/.content), adapted to FILL the real Tauri
	   window instead of the mockup's fixed 1180x760 bordered frame (that frame simulated an OS
	   window floating on a page — border/border-radius/box-shadow all removed here because the
	   real macOS window already draws its own chrome; this div IS the window's content, not a
	   picture of one). Relies on `html, body { height: 100% }` in routes/layout.css so this
	   height: 100% has a definite ancestor height to resolve against. */
	.window {
		display: grid;
		grid-template-rows: auto 1fr;
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
</style>
