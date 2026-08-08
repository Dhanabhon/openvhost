<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';
	import type { ReadinessNotice } from '$lib/site-readiness.derive';

	// The whole notice arrives decided (`site-readiness.derive.ts`): which
	// requirements are missing, the title, and each line's sentence. This
	// component picks no copy — the single- vs multi-missing wording is chosen
	// alongside the title in one place, so the two cannot drift apart.
	let { notice }: { notice: ReadinessNotice } = $props();
</script>

<!-- role="status", not "alert": nothing has failed and nothing is urgent. It is
     the state of a fresh machine, and an assertive live region on every launch
     of an un-set-up install would interrupt a screen-reader user for a fact they
     are about to read anyway. Matches the banner this replaces. -->
<div class="banner-info" role="status" data-testid="site-readiness-banner">
	<strong>{notice.title}</strong>
	{#if notice.lines.length === 1}
		<!-- One missing requirement: the title already named it, so the line says
		     why it matters and offers the way out — verbatim how the PHP-only
		     banner has read since it shipped. -->
		<span data-testid="readiness-{notice.lines[0].id}"
			>{notice.lines[0].text}
			<a href={resolve(notice.lines[0].route)}>{notice.lines[0].linkText}</a>.</span
		>
	{:else}
		<!-- More than one: a summary title cannot carry both facts, so each line
		     states its own. Still ONE banner (design D1) — a list inside it, never
		     a second banner stacked underneath. -->
		<ul>
			{#each notice.lines as line (line.id)}
				<li data-testid="readiness-{line.id}">
					{line.text}
					<a href={resolve(line.route)}>{line.linkText}</a>.
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	/* .banner-info: an accent-tinted pointer, not a failure — the same recipe as
	   PendingChangesBanner.svelte's own `.banner` (accent-tinted surface over
	   `--vh-surface`, distinct from `.banner-error`'s failure tint), carried over
	   unchanged from the inline banner this component replaces in
	   routes/+page.svelte. */
	.banner-info {
		margin: var(--vh-space-3) var(--vh-space-6) 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		border: 1px solid color-mix(in oklab, var(--vh-accent) 35%, transparent);
		background: color-mix(in oklab, var(--vh-accent) 8%, var(--vh-surface));
		border-radius: var(--vh-radius-card);
		font-size: var(--vh-text-table);
	}
	.banner-info strong {
		display: block;
		margin-bottom: 2px;
	}
	.banner-info a {
		color: var(--vh-link);
	}
	/* Real markers, with just enough indent to hold them. Two alternatives were
	   rejected: `list-style: none` drops the list from VoiceOver's a11y tree in
	   Safari (and the <ul> is here FOR those semantics), and `list-style-type: ''`
	   — which suppresses the glyph while keeping them — needs Safari 17.2+, while
	   `tauri.conf.json` sets no `minimumSystemVersion` and so inherits Tauri's
	   macOS 10.13 default. On an older WebKit that declaration is simply invalid,
	   the marker comes back, and with `padding: 0` it renders OUTSIDE the content
	   box against the banner's edge. A visible bullet everywhere beats one that
	   is fine here and clipped on someone else's machine.

	   1.1em of indent is affordable even in the 380px panel this UI must stay
	   usable at: the sentences are short and the remedy link is what has to stay
	   on one line, not the whole item. */
	.banner-info ul {
		margin: var(--vh-space-1) 0 0;
		padding-inline-start: 1.1em;
	}
	.banner-info li + li {
		margin-top: var(--vh-space-1);
	}
</style>
