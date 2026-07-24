<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { ServiceStatus } from '../ipc';
	import ServiceRow from './ServiceRow.svelte';

	let {
		services,
		onStart,
		onStop
	}: {
		services: readonly ServiceStatus[];
		onStart: (id: string) => void;
		onStop: (id: string) => void;
	} = $props();
</script>

<div class="strip-head">
	<h2 class="section-label">Services</h2>
</div>

<section class="panel services-panel" aria-label="Services" data-testid="services">
	{#if services.length === 0}
		<div class="empty">
			<div class="title">No services registered</div>
			<p>Services appear here once the supervisor reports them.</p>
		</div>
	{:else}
		<div class="rowlist">
			{#each services as service (service.id)}
				<ServiceRow {service} {onStart} {onStop} />
			{/each}
		</div>
	{/if}
</section>

<style>
	/* Ported from docs/design/main-window.html lines 129-176 + mock.css (.strip-head,
	   .section-label, .panel, .services-panel, .rowlist, .empty, .empty .title). The mock's
	   `.strip-head` also carries an unwired "Manage packages" link — intentionally dropped for
	   the same "no fake href='#' control" reason documented in ServiceRow.svelte; add it back,
	   wired, once package management ships. */
	.strip-head {
		display: flex;
		align-items: baseline;
	}
	.section-label {
		font-size: var(--vh-text-caption);
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--vh-text-2);
		padding: 0 var(--vh-space-6);
		margin: var(--vh-space-4) 0 var(--vh-space-2);
	}
	.panel {
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		margin: 0 var(--vh-space-6);
		overflow: hidden;
	}
	.services-panel {
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
