<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';
	import type { UiLog } from '../services.svelte';
	import { logSourceQuery } from '../logs.derive';
	import LogLevelBadge from './LogLevelBadge.svelte';

	// `firstServiceId` is exactly which service `logs` was seeded from
	// (`ServicesStore.loadLogTail` picks `this.services[0]`) — v0's "mixed
	// first-service feed" is unchanged by this task (spec D6 allows keeping
	// it as-is and deferring the "scoped to the selected service" part; see
	// task-6-report.md). "Open in Logs" links honestly to what this pane
	// ALREADY shows rather than implying a per-service selector that does
	// not exist yet.
	//
	// Optional, defaulting to `null` (link omitted): `LogPane` has TWO other
	// call sites — `LanguageRow.svelte`/`MysqlRow.svelte`'s per-row INSTALL
	// log panels (`php-install-log`/`mysql-install-log`, streamed while a
	// version installs). Those logs have no matching `LogSourceDto` in
	// `list_log_sources`' catalogue at all — an "Open in Logs" link for them
	// would point at nothing real — so they simply do not pass this prop
	// rather than being forced to invent a meaningless value. Only
	// `routes/services/+page.svelte` (the ring-log feed this prop actually
	// describes) passes it.
	let { logs, firstServiceId = null }: { logs: UiLog[]; firstServiceId?: string | null } = $props();
	let logEl: HTMLDivElement | undefined;

	// Blunt auto-follow (v0, ported from the previous +page.svelte): whenever the log feed is
	// replaced (every new line replaces `logs` with a new array upstream in ServicesStore),
	// jump the pane to the bottom. Reading `logs` in the condition is what registers the
	// reactive dependency so this effect reruns on each push.
	$effect(() => {
		if (logs && logEl) logEl.scrollTop = logEl.scrollHeight;
	});

	function fmtTs(t: number): string {
		return new Date(t).toLocaleTimeString(undefined, { hour12: false });
	}
</script>

<div class="log-head">
	<h2 class="section-label">Log</h2>
	{#if firstServiceId !== null}
		<!-- resolve('/logs') genuinely runs (see the href below): the eslint rule's
		     expressionIsResolveCall only recognises a BARE resolve(...) call or an aliasing
		     variable, not one combined with a query string in a template literal (checked
		     against the rule's own source, eslint-plugin-svelte/lib/rules/
		     no-navigation-without-resolve.js) — a false positive for "resolved path + safe,
		     encoded query string" (logSourceQuery, $lib/logs.derive), not a raw/unresolved path.
		     A block disable, not `-next-line`: prettier is free to re-wrap this element across
		     any number of lines, and `-next-line` would then silently stop covering the `href`
		     attribute the moment it did. -->
		<!-- eslint-disable svelte/no-navigation-without-resolve -->
		<a
			class="open-in-logs"
			href={`${resolve('/logs')}${logSourceQuery({ kind: 'serviceRing', id: firstServiceId })}`}
			>Open in Logs</a
		>
		<!-- eslint-enable svelte/no-navigation-without-resolve -->
	{/if}
</div>
<div class="log" data-testid="log" bind:this={logEl}>
	{#each logs as l, i (i)}
		<div class="line">
			<span class="ts num">{fmtTs(l.tsMs)}</span>
			<LogLevelBadge level={l.level} />
			<span class="msg">{l.line}</span>
		</div>
	{/each}
</div>

<style>
	/* Ported from docs/design/mock.css (.log, .log .line, .log .ts, .log .msg). Grid
	   widths/gap match the mock's log line layout (96px timestamp / 56px level / 1fr
	   message) rather than the previous Tailwind-arbitrary-value grid this replaces.
	   `.section-label` is deliberately absent — ServicesPanel needs the identical heading,
	   so it lives once in the base layer (lib/styles/tokens.css) rather than as a scoped
	   copy in each panel. The level colours (`.lvl`/`.lvl-info`/`.lvl-warn`/`.lvl-error`)
	   moved to LogLevelBadge.svelte (spec D6's row-renderer extraction) — this file no
	   longer carries its own copy of that mapping. */
	.log-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
	}
	/* Right-aligned against the same page inset `.section-label` uses on the left
	   (lib/styles/tokens.css's `padding: 0 var(--vh-space-6)`) — this element sits
	   OUTSIDE that global rule, so it states its own matching inset rather than
	   inheriting one. */
	.open-in-logs {
		margin: var(--vh-space-4) var(--vh-space-6) var(--vh-space-2) 0;
		color: var(--vh-link);
		text-decoration: none;
		font-weight: 500;
		font-size: var(--vh-text-table);
	}
	.open-in-logs:hover {
		text-decoration: underline;
	}
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
	.log .msg {
		color: var(--vh-text);
	}
</style>
