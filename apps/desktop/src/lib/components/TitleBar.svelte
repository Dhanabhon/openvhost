<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import StatusPill from './StatusPill.svelte';

	let { runningCount }: { runningCount: number } = $props();
</script>

<!-- `data-tauri-drag-region="deep"` (NOT the bare attribute): tauri's drag script only starts a
     drag for a bare/`true` attribute when the click target IS this exact element, and
     `.titlebar-name` (flex: 1) covers nearly the whole strip — so bare left the window
     undraggable except the flex gap. "deep" makes the whole subtree a drag region; clickable
     descendants still block dragging on their own, so a future titlebar button keeps working.
     See titlebar.drag.test.ts for the full contract. -->
<div class="titlebar" data-tauri-drag-region="deep">
	<div class="titlebar-name"><b>OpenVHost</b></div>
	<StatusPill kind="running" label="{runningCount} running" />
</div>

<style>
	/* Ported from docs/design/mock.css (.titlebar, .titlebar-name). The mock's `.pill`/
	   `.pill-running`/`.dot` rules are NOT re-declared here — StatusPill.svelte already owns that
	   recipe, so this titlebar renders a `<StatusPill kind="running">` with a count label instead
	   of maintaining a second copy that could drift from it.
	   Adapted for the real macOS Overlay titlebar: the mockup's fake `.traffic` dots (a DOM
	   simulation for the static HTML mock) are dropped entirely — macOS draws the real traffic
	   lights over this strip — and padding-left insets the content so the brand clears them:
	   env(titlebar-area-x) if the webview ever supports it, a fixed 72px fallback otherwise. */
	.titlebar {
		display: flex;
		align-items: center;
		gap: var(--vh-space-3);
		padding: 10px var(--vh-space-4) 10px env(titlebar-area-x, 72px);
		background: var(--vh-surface-2);
		border-bottom: 1px solid var(--vh-border);
		user-select: none;
	}
	.titlebar-name {
		flex: 1;
		text-align: center;
		font-family: var(--vh-font-display);
		font-weight: 500;
		font-size: var(--vh-text-table);
		letter-spacing: -0.01em;
		color: var(--vh-text-2);
	}
	.titlebar-name b {
		color: var(--vh-text);
		font-weight: 500;
	}
</style>
