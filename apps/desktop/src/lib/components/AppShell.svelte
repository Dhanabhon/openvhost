<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import TitleBar from './TitleBar.svelte';
	import Rail from './Rail.svelte';

	let { runningCount, children }: { runningCount: number; children: import('svelte').Snippet } =
		$props();
</script>

<div class="window">
	<TitleBar {runningCount} />
	<div class="shell">
		<Rail active="services" />
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
	   "grid/flex children ignore the parent's height" — so .content's overflow can do its job. */
	.shell {
		display: grid;
		grid-template-columns: 216px 1fr;
		min-height: 0;
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
