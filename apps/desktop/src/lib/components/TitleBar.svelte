<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	let { runningCount }: { runningCount: number } = $props();
</script>

<div class="titlebar" data-tauri-drag-region>
	<div class="titlebar-name"><b>OpenVHost</b></div>
	<span class="pill pill-running"><span class="dot"></span>{runningCount} running</span>
</div>

<style>
	/* Ported from docs/design/mock.css (.titlebar, .titlebar-name, .pill, .pill-running, .dot).
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
	.pill {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 2px 10px 2px 7px;
		border-radius: var(--vh-radius-pill);
		font-size: var(--vh-text-caption);
		font-weight: 600;
		border: 1px solid var(--vh-border);
		background: var(--vh-surface);
		white-space: nowrap;
	}
	.pill .dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		flex: none;
	}
	.pill-running {
		color: var(--vh-run);
	}
	.pill-running .dot {
		background: var(--vh-run-dot);
	}
</style>
