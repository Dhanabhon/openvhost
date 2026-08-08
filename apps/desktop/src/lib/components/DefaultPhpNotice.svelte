<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { DefaultPhpDto } from '$lib/ipc';
	import { DEFAULT_PHP_MISSING_TITLE, defaultPhpNotice } from '$lib/php-default.derive';

	let { resolved }: { resolved: DefaultPhpDto | null } = $props();

	// Every copy and every "does this render at all" decision lives in the
	// helper, which matches `DefaultPhpDto` exhaustively — so a fifth resolution
	// state fails ITS typecheck rather than silently rendering nothing here.
	// `null` (the environment has not been read yet) is a separate absence from
	// "there is nothing to say", and neither renders.
	const notice = $derived(resolved === null ? null : defaultPhpNotice(resolved));
</script>

{#if notice !== null}
	<!-- Spec claim 4: uninstalling the default must leave the state LEGIBLE, and
	     legible is not enough on its own — the sentence names both ways out
	     (reinstall the version you chose, or choose one that is here).

	     PAGE-LEVEL, not per-row, and that is forced rather than stylistic: the
	     major this names may have no row on the page at all. Rows are the
	     catalogue plus what is installed, so a preference for a hand-installed
	     `php@7.4` that has since been removed appears in neither — and that is
	     precisely the user who most needs telling.

	     `role="status"`, not `role="alert"`: nothing is broken and nothing is
	     failing. A version is being served, just not the one that was chosen —
	     interrupting a screen reader mid-sentence for that would be the wrong
	     urgency. The amber `warn` treatment carries the same message visually:
	     worth your attention, not an error. -->
	<div class="default-php-notice" role="status" data-testid="php-default-missing">
		<p class="title">{DEFAULT_PHP_MISSING_TITLE}</p>
		<p class="body">{notice}</p>
	</div>
{/if}

<style>
	/* Sits inside `.panel`'s own border and background (like the Languages page's
	   `.banner-error` beside it), so it is PADDED rather than margined — a margin
	   would leave a visible gutter against the panel edge that no other child of
	   that panel has.

	   Amber, using the `--vh-start` text colour on a plain `--vh-surface`
	   background, which is the pairing ScaffoldNoticeBanner.svelte measured at
	   4.68:1 (WCAG AA) and the brand guidelines' "text-safe on light" column
	   vouches for. Deliberately NOT the 9%-into-surface tint `.banner-error` uses:
	   that recipe was measured to FAIL AA for amber specifically (4.36:1). The
	   tone lives in the border and the text colour instead. Dark theme is still
	   the reserved empty block in tokens.css; whoever fills it owns re-checking
	   this pair there. */
	.default-php-notice {
		padding: var(--vh-space-3) var(--vh-space-6);
		background: var(--vh-surface);
		border-bottom: 1px solid var(--vh-border);
		font-size: var(--vh-text-table);
	}
	.title {
		margin: 0;
		font-weight: 600;
		color: var(--vh-start);
	}
	.body {
		margin: 4px 0 0;
		color: var(--vh-text-2);
		/* A major is a short token, but the sentence carries two of them plus a
		   hostname — let it wrap rather than push the panel wide at 380px. */
		min-width: 0;
		overflow-wrap: anywhere;
	}
</style>
