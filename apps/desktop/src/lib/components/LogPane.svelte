<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { UiLog } from '../services.svelte';

	let { logs }: { logs: UiLog[] } = $props();
	let logEl: HTMLDivElement | undefined;

	// Blunt auto-follow (v0, ported from the previous +page.svelte): whenever the log feed is
	// replaced (every new line replaces `logs` with a new array upstream in ServicesStore),
	// jump the pane to the bottom. Reading `logs` in the condition is what registers the
	// reactive dependency so this effect reruns on each push.
	$effect(() => {
		if (logs && logEl) logEl.scrollTop = logEl.scrollHeight;
	});

	function levelClass(level: string): string {
		return level === 'error' ? 'lvl-error' : level === 'warn' ? 'lvl-warn' : 'lvl-info';
	}
	function fmtTs(t: number): string {
		return new Date(t).toLocaleTimeString(undefined, { hour12: false });
	}
</script>

<h2 class="section-label">Log</h2>
<div class="log" data-testid="log" bind:this={logEl}>
	{#each logs as l, i (i)}
		<div class="line">
			<span class="ts num">{fmtTs(l.tsMs)}</span>
			<span class="lvl {levelClass(l.level)}">{l.level}</span>
			<span class="msg">{l.line}</span>
		</div>
	{/each}
</div>

<style>
	/* Ported from docs/design/mock.css (.log, .log .line, .log .ts, .log .lvl,
	   .log .lvl-info/.lvl-warn/.lvl-error, .log .msg). Grid widths/gap match the mock's log
	   line layout (96px timestamp / 56px level / 1fr message) rather than the previous
	   Tailwind-arbitrary-value grid this replaces. `.section-label` is deliberately absent —
	   ServicesPanel needs the identical heading, so it lives once in the base layer
	   (lib/styles/tokens.css) rather than as a scoped copy in each panel. */
	.log {
		flex: 1;
		margin: 0 var(--vh-space-6) var(--vh-space-4);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		background: var(--vh-log-bg);
		overflow: auto;
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-log);
		line-height: 1.7;
	}
	.log .line {
		display: grid;
		grid-template-columns: 96px 56px 1fr;
		gap: 12px;
		padding: 0 14px;
		white-space: pre-wrap;
	}
	.log .line:hover {
		background: color-mix(in oklab, var(--vh-text) 4%, transparent);
	}
	.log .ts {
		color: var(--vh-text-disabled);
	}
	.log .lvl {
		font-weight: 700;
	}
	.log .lvl-info {
		color: var(--vh-text-2);
	}
	.log .lvl-warn {
		color: var(--vh-start);
	}
	.log .lvl-error {
		color: var(--vh-fail);
	}
	.log .msg {
		color: var(--vh-text);
	}
</style>
