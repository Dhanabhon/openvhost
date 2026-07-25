<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { SiteDto } from '$lib/ipc';
	import { enabledPill } from '$lib/sites.derive';
	import Button from './Button.svelte';

	let { site, onEdit }: { site: SiteDto; onEdit: (site: SiteDto) => void } = $props();
	const pill = $derived(enabledPill(site.enabled));
</script>

<div class="row site-row" data-testid="site-{site.id}">
	<div>
		<div class="primary">{site.name}</div>
		<div class="meta mono">{site.domain}</div>
	</div>
	<div class="mono num">PHP {site.phpVersion}</div>
	<div class="meta">{site.webServer}</div>
	<span class="pill {pill.cls}" data-testid="site-pill-{site.id}">
		<span class="dot"></span>{pill.label}
	</span>
	<div class="row-actions">
		<Button variant="quiet" size="sm" ariaLabel="Edit {site.name}" onclick={() => onEdit(site)}
			>Edit</Button
		>
	</div>
</div>

<style>
	/* Ported from docs/design/mock.css (.row, .site-row, .primary/.meta/.mono under .row,
	   .pill/.dot/.pill-running/.pill-stopped, .row-actions). `.num` is NOT redefined here — it
	   is a global utility class already applied app-wide from lib/styles/tokens.css
	   (`.num { font-variant-numeric: tabular-nums }`), the same convention ServiceRow.svelte
	   relies on. `.mono`'s font-family is likewise the global utility (`code, pre, .mono {
	   font-family: var(--vh-font-mono) }`), but mock.css also sizes it per-row (`.row .mono {
	   font-size: var(--vh-text-table) }`), so that rule IS ported below to match the
	   domain/web-server cells' size.

	   Deliberate deviation from the mock (see the design spec's deviation table): a site has no
	   runtime state yet, so there is no status-running pill and no "Open" button — only the
	   stored `enabled` flag, rendered via the shared pill-running/pill-stopped classes as
	   Enabled/Disabled, and a single Edit action. `.pill-starting`/`.pill-failed` are therefore
	   not ported — `enabledPill` never produces them. */
	.row {
		display: grid;
		align-items: center;
		gap: var(--vh-space-4);
		padding: 10px var(--vh-space-4);
		border-bottom: 1px solid var(--vh-border);
		transition: background var(--vh-dur-fast) var(--vh-ease-out);
	}
	.row:last-child {
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
	.row .mono {
		font-size: var(--vh-text-table);
	}
	.site-row {
		grid-template-columns: minmax(220px, 1.4fr) 110px 90px 120px auto;
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
	.pill-stopped {
		color: var(--vh-stop);
	}
	.pill-stopped .dot {
		background: var(--vh-stop-dot);
	}
	.row-actions {
		display: flex;
		gap: 4px;
		justify-content: flex-end;
		opacity: 0.85;
	}
</style>
