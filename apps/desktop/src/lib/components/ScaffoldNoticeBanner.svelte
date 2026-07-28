<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { ScaffoldOutcomeDto } from '$lib/ipc';
	import { scaffoldNotice } from '$lib/sites.derive';
	import Button from './Button.svelte';

	let {
		siteName,
		docroot,
		outcome,
		onDismiss
	}: {
		siteName: string;
		docroot: string;
		outcome: ScaffoldOutcomeDto;
		onDismiss: () => void;
	} = $props();

	// All copy/tone/role decisions live in the helper — this template renders
	// its output only, so a fourth ScaffoldOutcomeDto variant fails the
	// helper's typecheck rather than silently rendering nothing here.
	const notice = $derived(scaffoldNotice(siteName, docroot, outcome));
</script>

<div
	class="scaffold-notice"
	data-tone={notice.tone}
	role={notice.role}
	data-testid="scaffold-notice"
>
	<p>{notice.text}</p>
	<Button variant="quiet" size="sm" onclick={onDismiss}>Dismiss</Button>
</div>

<style>
	/* Same box recipe as the sibling banners (PendingChangesBanner.svelte /
	   ErrorBanner.svelte): margin/padding/radius/font-size all reused verbatim so this
	   reads as "one more banner in the same stack", not a one-off.

	   Background is deliberately left at plain `--vh-surface` rather than the
	   9%-into-surface tint those siblings use for their (single) fail/accent tone.
	   Measured against the actual state-text colours this banner needs — `--vh-run`
	   AND `--vh-start` for the `warn` tone specifically — that recipe's 9% mix only
	   clears WCAG AA (>=4.5:1) for the green case (4.53:1) and FAILS it for the amber
	   `warn` case (4.36:1, computed via the same OKLab math `color-mix(in oklab, …)`
	   performs). `--vh-run`/`--vh-start` on plain `--vh-surface` is the pairing the
	   brand guidelines' own "text-safe on light" column already vouches for (also how
	   StatusPill.svelte uses them), clearing 4.88:1 / 4.68:1 with real margin — so the
	   tone lives in the border + text colour, the same two-variable vocabulary
	   `.pill-failed` already uses on an otherwise plain-surface pill, rather than in an
	   unaudited new background tint. (Dark theme is still the empty, reserved block in
	   tokens.css — light is the only theme actually rendered today, so there is only
	   one theme to have verified this against; whoever fills in the dark palette owns
	   re-checking these same two pairs there.) */
	.scaffold-notice {
		display: flex;
		align-items: center;
		gap: var(--vh-space-4);
		margin: var(--vh-space-3) var(--vh-space-6) 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		background: var(--vh-surface);
		border-radius: var(--vh-radius-card);
		font-size: var(--vh-text-table);
	}
	.scaffold-notice p {
		flex: 1;
		margin: 0;
	}
	.scaffold-notice[data-tone='ok'] {
		border: 1px solid color-mix(in oklab, var(--vh-run) 35%, transparent);
		color: var(--vh-run);
	}
	.scaffold-notice[data-tone='warn'] {
		border: 1px solid color-mix(in oklab, var(--vh-start) 35%, transparent);
		color: var(--vh-start);
	}
</style>
