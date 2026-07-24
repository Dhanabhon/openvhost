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
	<!-- `title` so the full value stays reachable when the cell ellipsizes: `endpoint` is
	     free-form text from the ServiceSpec, and the demo ticker's is a whole sentence. -->
	<div class="mono num meta endpoint" title={service.endpoint ?? undefined}>
		{service.endpoint ?? '—'}
	</div>
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

	   1b. The re-proportioning that followed from (1) was wrong on both axes and is corrected
	      at `.svc-row` below, where the reasoning lives.

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
	/* The ENDPOINT carries the flexible track, not the name.

	   The mock is 5 columns — `minmax(180px, 1fr) 90px 150px 120px auto` (name / version /
	   endpoint / state / action) — where 150px suited its short sample endpoints. Dropping the
	   version column (see deviation 1 above) originally re-proportioned this to
	   `minmax(200px, 1fr) 150px 120px auto`, which broke twice over:

	   - the name was the only `fr` track, so it absorbed ALL the slack. On a 1180px window
	     that left ~490px of dead space after "nginx" and squeezed the other three columns
	     against the right edge.
	   - 150px is narrower than `http://127.0.0.1:8080` renders in mono at
	     `--vh-text-table`, and a grid item defaults to `min-width: auto` — it refuses to
	     shrink below min-content — so the text did not clip, it SPILLED into the pill.

	   Now: the name takes a modest share, the endpoint takes the rest (it holds the variable
	   content), and the pill keeps a FIXED 120px on purpose — the state text changes width
	   between running/stopped/failed/starting, and a content-sized track would shift the
	   action button horizontally every time a service changed state. */
	.svc-row {
		grid-template-columns: minmax(140px, 0.8fr) minmax(210px, 1.6fr) 120px auto;
	}
	/* `min-width: 0` is the load-bearing half: it overrides the grid item's `auto` minimum so
	   the cell may shrink and the text ellipsizes instead of overflowing its track. Without
	   it the other three properties do nothing. */
	.row .endpoint {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* Same reason, so a long display name shrinks rather than pushing its neighbours. Service
	   names are ours and short today; this keeps the row structurally sound if one grows. */
	.row .primary {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
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
	/* The mock's `<pre>` keeps `white-space: pre` + `overflow-x: auto`, which technically scrolls
	   but hides most of a real stderr tail: an nginx `[emerg]` line citing an absolute
	   Application Support path measured 2069px of content in an 846px box — 59% of the error
	   text parked off-screen behind a macOS overlay scrollbar that gives no visible hint it is
	   scrollable. An error message is the last thing that should need discovery to read, so
	   wrap instead (matching LogPane's `.log .line`, which already wraps for the same reason).
	   `overflow-wrap: anywhere` covers stderr tokens with no space to break at (long paths,
	   base64, stack frames); `overflow-x: auto` stays as the backstop for anything still
	   unwrappable. */
	.fail-detail pre {
		margin: 0;
		padding: var(--vh-space-2) var(--vh-space-3);
		background: var(--vh-log-bg);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		font-size: var(--vh-text-caption);
		line-height: 1.6;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		overflow-x: auto;
		color: var(--vh-text);
	}
</style>
