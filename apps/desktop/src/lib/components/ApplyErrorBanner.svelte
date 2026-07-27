<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';
	import type { SiteDto } from '$lib/ipc';

	let {
		error,
		missing,
		onEditSite
	}: {
		/**
		 * `applyStore.error` verbatim — a failed `plan_config_apply` (e.g.
		 * `MissingRuntime`/`NotAPlainFile`, which fail the whole call) or a
		 * failed `apply_config` once the dialog is closed with changes still
		 * pending. Rendered as-is, never parsed: see `missing` below for why
		 * the "is this a missing runtime?" question is answered separately.
		 */
		error: string;
		/**
		 * The servable site Apply would reject for a missing PHP runtime, or
		 * `null` for any other failure a nginx -t syntax error, say). Computed by
		 * `$lib/sites.derive`'s `findMissingRuntimeSite` — NOT by parsing `error`,
		 * which is a human sentence nobody agreed to keep stable (see
		 * task-9-report.md for the full reasoning). `null` here means "no PHP
		 * remedy fits", not "nothing failed" — `error` is shown either way.
		 */
		missing: SiteDto | null;
		/** Opens `missing`'s site in the editor drawer — mirrors `SiteListRow`'s
		 * own `onEdit`, so a page wiring one already has the other. */
		onEditSite: (site: SiteDto) => void;
	} = $props();
</script>

<div class="banner-error" role="alert" data-testid="apply-plan-error">
	<strong>Couldn't apply site changes</strong>
	<!-- pre-wrap: `ValidationFailed`'s nginx stderr is multi-line and would run
	     off-screen as a single line otherwise (the ServiceRow lesson, also
	     applied in ApplyDialog.svelte's own copy of this same error). -->
	<span class="detail" style="white-space: pre-wrap">{error}</span>
	{#if missing !== null}
		<!-- Two concrete ways out, not just a restated problem: install the
		     version this site wants, or point the site at one already installed.
		     Either one clears `missing` on the next Apply. -->
		<div class="actions">
			<a
				class="action-link"
				href={resolve('/languages')}
				data-testid="go-install-{missing.phpVersion}"
			>
				Install PHP {missing.phpVersion}
			</a>
			<button
				type="button"
				class="action-link"
				data-testid="edit-site-{missing.name}"
				onclick={() => onEditSite(missing)}
			>
				Edit {missing.name}
			</button>
		</div>
	{/if}
</div>

<style>
	/* Same recipe as +page.svelte's other `.banner-error` usages (reuses mock.css's
	   `.fail-detail` failure-surface treatment) — this component only owns the
	   markup that used to be inline there. */
	.banner-error {
		margin: var(--vh-space-3) var(--vh-space-6) 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		background: var(--vh-fail-tint);
		border-radius: var(--vh-radius-control);
		color: var(--vh-fail);
		font-size: var(--vh-text-table);
	}
	.banner-error strong {
		display: block;
		margin-bottom: 2px;
	}
	.banner-error .detail {
		display: block;
	}
	.actions {
		display: flex;
		gap: var(--vh-space-3);
		margin-top: var(--vh-space-2);
	}
	/* Minimal link-styled control, matching SiteListRow's own documented deviation
	   for its danger button: `Button.svelte` renders only a `<button>`, so it
	   cannot become the `<a href>` that "Install PHP X" needs to actually
	   navigate, and mixing a `<Button>` with a hand-rolled `<a>` styled
	   differently would read as two different affordances for one "go do this
	   elsewhere" action. Both controls share this class so they read as the
	   same kind of action. */
	.action-link {
		display: inline-flex;
		align-items: center;
		font: inherit;
		font-weight: 600;
		color: var(--vh-fail);
		text-decoration: underline;
		background: none;
		border: none;
		padding: 0;
		cursor: pointer;
	}
	.action-link:hover {
		text-decoration: none;
	}
</style>
