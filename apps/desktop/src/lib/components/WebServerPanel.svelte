<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { ServiceStatus, ValidationReportDto, WebServerDto } from '$lib/ipc';
	import WebServerRow from './WebServerRow.svelte';

	// PURELY PRESENTATIONAL: every piece of state arrives as a prop and every action
	// leaves as a callback. Nothing here reaches for `webServersStore` — that is what
	// lets the whole surface be asserted by an SSR test in this project's DOM-less
	// vitest project, and it keeps the one place that talks to IPC (the route) the
	// only place that can.
	let {
		servers,
		services,
		configText,
		configError,
		reports,
		validating,
		onShowConfig,
		onValidate,
		onStart,
		onStop
	}: {
		servers: readonly WebServerDto[];
		/** The SHARED supervisor snapshot. Status is correlated by `serviceId` rather
		 * than fetched again, so this page cannot drift from the Services page. */
		services: readonly ServiceStatus[];
		configText: Record<string, string>;
		/** Per-row failure, keyed by brand id. Separate from the page-level error the
		 * route renders, so one row's problem cannot blank the page. */
		configError: Record<string, string>;
		reports: Record<string, ValidationReportDto>;
		validating: Record<string, boolean>;
		onShowConfig: (id: string) => void;
		onValidate: (id: string) => void;
		onStart: (serviceId: string) => void;
		onStop: (serviceId: string) => void;
	} = $props();
</script>

<div class="page-head">
	<div>
		<!-- `<h2>`, not `<h1>`: the route already renders the page's `<h1>` (sr-only), the way
		     routes/services/+page.svelte does. -->
		<h2>Web server</h2>
		<!-- No longer "Read-only": the settings form below this panel edits how
		     nginx behaves. This line now describes what the panel itself shows,
		     and the form carries its own description of what saving does. -->
		<p class="sub">
			The binary OpenVHost runs, the config it reads, whether that config is valid — and the
			settings that shape it.
		</p>
	</div>
</div>

<section class="panel ws-panel" aria-label="Web servers" data-testid="web-servers">
	{#if servers.length === 0}
		<!-- Deliberately does NOT say "no web servers": the route paints before
		     `list_web_servers` resolves (it spawns `nginx -v` server-side), so this block
		     is on screen for a frame on EVERY visit. "No web servers listed" was a claim
		     about the user's system that the app had not checked yet. This copy is true
		     while the list is still loading AND once it has come back empty, which is the
		     only honest thing to say without a `loaded` flag. -->
		<div class="empty">
			<div class="title">Nothing to show yet</div>
			<p>
				OpenVHost lists one row per web server brand it knows about. If the list failed to load, the
				error is shown above.
			</p>
		</div>
	{:else}
		<div class="rowlist">
			{#each servers as server (server.id)}
				<!-- The per-id maps are indexed HERE so each row receives only its own slice.
				     The whole state is passed, not just its kind — `failed` carries the
				     `stderrTail` the row renders, and a kind alone cannot express it. -->
				<WebServerRow
					{server}
					serviceState={services.find((s) => s.id === server.serviceId)?.state ?? null}
					configText={configText[server.id]}
					configError={configError[server.id] ?? ''}
					report={reports[server.id] ?? null}
					validating={validating[server.id] === true}
					{onShowConfig}
					{onValidate}
					{onStart}
					{onStop}
				/>
			{/each}
		</div>
	{/if}
</section>

<style>
	/* Ported from docs/design/mock.css (.page-head, .page-head .sub, .page-head h1 — applied to
	   the `<h2>` this panel renders instead, .panel, .rowlist, .empty, .empty .title), the same
	   recipes SitesPanel.svelte and ServicesPanel.svelte already use, so a third page reads as
	   part of the same product rather than a new dialect. `.page-head` has no action button
	   here: this page is read-only, and this codebase does not render a control for something
	   that isn't wired (see Rail.svelte's Logs/Settings placeholders). */
	.page-head {
		display: flex;
		align-items: center;
		gap: var(--vh-space-4);
		padding: 20px var(--vh-space-6) var(--vh-space-3);
	}
	.page-head h2 {
		font-size: var(--vh-text-page);
		font-weight: 600;
	}
	.page-head .sub {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		margin-top: 2px;
	}
	.panel {
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		margin: 0 var(--vh-space-6);
		overflow: hidden;
	}
	.ws-panel {
		margin-bottom: var(--vh-space-6);
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
