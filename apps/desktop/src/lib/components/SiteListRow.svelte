<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { SiteDto } from '$lib/ipc';
	import { enabledPill } from '$lib/sites.derive';
	import Button from './Button.svelte';

	let {
		site,
		onEdit,
		onToggleEnabled,
		onDelete,
		busy = false,
		rowError = ''
	}: {
		site: SiteDto;
		onEdit: (site: SiteDto) => void;
		onToggleEnabled: (site: SiteDto, enabled: boolean) => void;
		onDelete: (id: string) => void;
		busy?: boolean;
		rowError?: string;
	} = $props();

	const pill = $derived(enabledPill(site.enabled));

	/**
	 * Two-step delete confirm, held LOCALLY on purpose.
	 *
	 * `SitesPanel`'s `{#each sites as site (site.id)}` is keyed by id, so this component
	 * instance is bound to one site: Svelte matches by key and updates props rather than
	 * recreating, which means a list refetch mid-confirm keeps the confirm on the SAME
	 * row instead of moving it. Lifting this into the store and keying it by row index
	 * is the version that would put a red Delete under the wrong site.
	 */
	let confirming = $state(false);
</script>

<div class="row site-row" data-testid="site-{site.id}">
	<div>
		<div class="primary">{site.name}</div>
		<!-- `title` so an ellipsized domain stays readable: the cell now clips (see the CSS
		     note on the name cell) and the domain is the row's identifying detail. -->
		<div class="meta mono" title={site.domain}>{site.domain}</div>
	</div>
	<div class="mono num">PHP {site.phpVersion}</div>
	<div class="meta">{site.webServer}</div>
	<span class="pill {pill.cls}" data-testid="site-pill-{site.id}">
		<span class="dot"></span>{pill.label}
	</span>
	{#if confirming}
		<div class="row-actions confirm" data-testid="confirm-{site.id}">
			<!-- "this site", not the name: the name is already the first thing in the row, and
			     interpolating it here would make the confirm state's width vary with it, which
			     is exactly the column-shifting the pinned `.row-actions` width prevents. The
			     name IS in both buttons' aria-labels, where a screen-reader user — who cannot
			     see the row this replaced — actually needs it. -->
			<span class="confirm-q">Delete this site?</span>
			<Button
				variant="quiet"
				size="sm"
				ariaLabel="Keep {site.name}"
				onclick={() => (confirming = false)}>Cancel</Button
			>
			<!-- The only danger-tinted control in the row: the destructive step itself, never
			     the button that merely asks. Hand-rolled rather than `Button`, because
			     `Button` has no danger variant and its `.btn` rule is component-scoped — a
			     `<button class="btn btn-danger">` outside it renders completely unstyled. So
			     this carries a minimal local `.btn` subset, exactly as SiteDrawer's danger
			     zone already does for the same reason (see its deviation note). -->
			<button
				type="button"
				class="btn btn-danger btn-sm"
				disabled={busy}
				aria-label="Confirm deleting {site.name}"
				onclick={() => onDelete(site.id)}
			>
				Delete
			</button>
		</div>
	{:else}
		<div class="row-actions">
			<Button
				variant="quiet"
				size="sm"
				ariaLabel={site.enabled ? `Disable ${site.name}` : `Enable ${site.name}`}
				disabled={busy}
				onclick={() => onToggleEnabled(site, !site.enabled)}
				>{site.enabled ? 'Disable' : 'Enable'}</Button
			>
			<Button variant="quiet" size="sm" ariaLabel="Edit {site.name}" onclick={() => onEdit(site)}
				>Edit</Button
			>
			<Button
				variant="quiet"
				size="sm"
				ariaLabel="Delete {site.name}"
				onclick={() => (confirming = true)}>Delete</Button
			>
		</div>
	{/if}
</div>
{#if rowError !== ''}
	<!-- Flat sibling rather than a cell inside the grid, so a long message wraps under the
	     whole row instead of stretching one column. Mirrors ServiceRow's `.fail-detail`. -->
	<p class="row-error" role="alert" data-testid="row-error-{site.id}">{rowError}</p>
{/if}

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
		/* The action track is `auto` (content-sized), and this row now has three different
		   action states of three different natural widths — Disable/Edit/Delete,
		   Enable/Edit/Delete, and the confirm question + Cancel + Delete. Without a floor the
		   track resizes on every state change, which steals width from the `1.4fr` name
		   column and slides PHP, the web server and the pill sideways: pressing Delete
		   visibly jolted three columns left. This floor is wider than all three states, so
		   the track is constant and only the buttons themselves change.

		   232px is measured, not derived — it is the confirm state (the widest) plus a little
		   air at this font and size. If a fallback font renders wider, the states differ by a
		   constant rather than by the site's name length, which was the variance that
		   mattered. Verified against a 964px content width (1180px window minus the rail). */
		min-width: 232px;
		display: flex;
		gap: 4px;
		justify-content: flex-end;
		align-items: center;
		opacity: 0.85;
	}
	/* The name cell must clip, not push. `min-width: 0` overrides the grid item's `auto`
	   minimum (which refuses to shrink below min-content) and the children ellipsize, so a
	   long domain stays on ONE line and every row keeps the same height. Without this, pinning
	   the action width above made `my-really-long-project-name.localhost` wrap and that row
	   grow taller than its neighbours. Same mechanism as ServiceRow's `.endpoint`. */
	.site-row > :first-child {
		min-width: 0;
	}
	.site-row > :first-child > * {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* Extra air before Delete, so the destructive control is not flush against Edit and an
	   overshooting click lands on nothing rather than on the wrong action. */
	.row-actions > :last-child {
		margin-left: var(--vh-space-2);
	}
	.confirm .confirm-q {
		font-size: var(--vh-text-table);
		color: var(--vh-fail);
		white-space: nowrap;
		margin-right: 2px;
	}
	/* Minimal `.btn` subset for the danger confirm only — `Button.svelte` has no danger
	   variant and its own `.btn` rule is component-scoped, so a `.btn` used out here would
	   be unstyled. Ported from mock.css:114-135, matching SiteDrawer's danger zone. */
	.btn {
		display: inline-flex;
		align-items: center;
		font: inherit;
		font-weight: 500;
		border-radius: var(--vh-radius-control);
		border: 1px solid transparent;
		cursor: pointer;
		transition:
			background var(--vh-dur-fast) var(--vh-ease-out),
			border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.btn-sm {
		padding: 4px 10px;
		font-size: var(--vh-text-table);
	}
	.btn-danger {
		background: transparent;
		color: var(--vh-fail);
		border-color: color-mix(in oklab, var(--vh-fail) 45%, transparent);
	}
	.btn-danger:hover:not(:disabled) {
		background: var(--vh-fail-tint);
	}
	.btn:disabled {
		opacity: 0.55;
		cursor: default;
	}
	/* Flat sibling of the row, so a long message wraps under the full width instead of
	   stretching one grid column. Mirrors ServiceRow's `.fail-detail` placement. */
	.row-error {
		margin: 0;
		padding: 0 var(--vh-space-4) var(--vh-space-3);
		color: var(--vh-fail);
		font-size: var(--vh-text-table);
	}
</style>
