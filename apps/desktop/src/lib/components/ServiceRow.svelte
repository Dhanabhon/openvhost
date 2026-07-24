<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { ServiceStatus } from '../ipc';
	import Button from './Button.svelte';
	import StatusPill from './StatusPill.svelte';

	let {
		service,
		onStart,
		onStop
	}: {
		service: ServiceStatus;
		onStart: (id: string) => void;
		onStop: (id: string) => void;
	} = $props();
</script>

<div class="row svc-row">
	<div class="primary">{service.displayName}</div>
	<div class="mono num meta">{service.endpoint ?? '—'}</div>
	<StatusPill kind={service.state.kind} testId="pill-{service.id}" />
	<div class="row-actions">
		{#if service.state.kind === 'stopped'}
			<Button
				variant="quiet"
				size="sm"
				ariaLabel="Start {service.displayName}"
				onclick={() => onStart(service.id)}>Start</Button
			>
		{:else if service.state.kind === 'failed'}
			<Button
				variant="quiet"
				size="sm"
				ariaLabel="Retry {service.displayName}"
				onclick={() => onStart(service.id)}>Retry</Button
			>
		{:else}
			<Button
				variant="quiet"
				size="sm"
				ariaLabel="Stop {service.displayName}"
				onclick={() => onStop(service.id)}>Stop</Button
			>
		{/if}
	</div>
</div>

{#if service.state.kind === 'failed'}
	<div class="fail-detail" role="status" data-testid="failed-{service.id}">
		<div class="headline">
			{service.displayName} failed{#if service.state.exit != null}&nbsp;(exit {service.state
					.exit}){/if}
		</div>
		<pre>{service.state.stderrTail.join('\n')}</pre>
	</div>
{/if}

<style>
	/* Ported from docs/design/mock.css (.row, .primary, .meta, .row-actions, .svc-row,
	   .fail-detail, .fail-detail .headline, .fail-detail pre). Two deliberate deviations from
	   the mock, both from the task-3 brief / this codebase's own conventions:

	   1. `.svc-row` here is 4 columns (name / endpoint / state / action), not the mock's 5
	      (name / version / endpoint / state / action) — `ServiceStatus` carries no version
	      field, so the version column is dropped and the remaining columns re-proportioned.

	   2. The mock's `.fail-detail .actions` (the "View log" / "Change port" links) is NOT
	      ported: neither a per-service log route nor a port-editing UI exists anywhere in this
	      app yet, and this codebase's established convention (see Rail.svelte's Sites/Logs/
	      Settings placeholders) is to never render a fake `href="#"` control for a feature that
	      isn't wired. Add them back, wired, once those surfaces land. */
	.row {
		display: grid;
		align-items: center;
		gap: var(--vh-space-4);
		padding: 10px var(--vh-space-4);
		border-bottom: 1px solid var(--vh-border);
		transition: background var(--vh-dur-fast) var(--vh-ease-out);
	}
	/* A failed row's `.fail-detail` is a flat sibling rendered right after it (see markup
	   above), so when the failed service is last in the list, `.fail-detail` — not the
	   `.row` — is `.rowlist`'s actual last child, and `:last-child` alone misses the row.
	   Cover that case explicitly: a row immediately followed by the trailing `.fail-detail`
	   reads as the panel's last visible row and gets the same border suppression. */
	.row:last-child,
	.row:has(+ .fail-detail:last-child) {
		border-bottom: 0;
	}
	.row:hover {
		background: color-mix(in oklab, var(--vh-text) 3%, var(--vh-surface));
	}
	.row .primary {
		font-weight: 600;
	}
	.row .meta {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
	}
	.row-actions {
		display: flex;
		gap: 4px;
		justify-content: flex-end;
		opacity: 0.85;
	}
	.svc-row {
		grid-template-columns: minmax(200px, 1fr) 150px 120px auto;
	}
	.fail-detail {
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		background: var(--vh-fail-tint);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3) var(--vh-space-4);
	}
	.fail-detail .headline {
		font-weight: 600;
		color: var(--vh-fail);
		margin-bottom: 6px;
	}
	.fail-detail pre {
		margin: 0;
		padding: var(--vh-space-2) var(--vh-space-3);
		background: var(--vh-log-bg);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		font-size: var(--vh-text-caption);
		line-height: 1.6;
		overflow-x: auto;
		color: var(--vh-text);
	}
</style>
