<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { DocrootRisk, DocrootWarningMode } from '$lib/sites.derive';
	import { docrootWarningText } from '$lib/sites.derive';

	let {
		risk,
		mode
	}: {
		/** Already-classified — see `docrootRisk` in `$lib/sites.derive`. This
		 *  component never classifies a raw path itself; `SiteDrawer.svelte` only
		 *  renders it at all inside `{#if}` once `docrootRisk(docroot)` is
		 *  non-null, the same "caller classifies, component renders" split
		 *  `ScaffoldNoticeBanner.svelte` uses for `ScaffoldOutcomeDto`. */
		risk: DocrootRisk;
		/** create vs edit — the one-click fix text genuinely differs (the
		 *  create-folder checkbox this points at only renders in create mode). */
		mode: DocrootWarningMode;
	} = $props();

	const text = $derived(docrootWarningText(risk, mode));
</script>

<!-- Associated with the Project-folder input via `aria-describedby`
     (`rootDescribedBy` in SiteDrawer.svelte), the same mechanism the field's
     errors/preview already use — not `role="alert"`: this is a permanent,
     non-dismissing caution (spec D3, "warn, never block"), not a transient
     event, and the field already has a working description-list convention. -->
<p class="docroot-risk" id="f-root-risk" data-testid="docroot-risk-warning">{text}</p>

<style>
	/* Amber "needs attention, not fixed" tone — the SAME vouched pairing
	   ScaffoldNoticeBanner.svelte / MysqlRow.svelte's `.note.warn` already use:
	   `--vh-start` text directly on the ambient `--vh-surface` background (no
	   tint), which measures 4.68:1 — clears WCAG AA with margin. Reused
	   verbatim rather than invented, per this task's own instruction. Dark
	   theme: tokens.css's `[data-theme='dark']` block is still empty/reserved
	   (see tokens.css header) — light is the only theme actually rendered
	   today, so there is only one theme to have verified this against; this
	   inherits the same "re-check when dark lands" note those two files carry. */
	.docroot-risk {
		margin: 0;
		padding: var(--vh-space-2) var(--vh-space-3);
		border: 1px solid color-mix(in oklab, var(--vh-start) 35%, transparent);
		border-radius: var(--vh-radius-control);
		color: var(--vh-start);
		font-size: var(--vh-text-caption);
	}
</style>
