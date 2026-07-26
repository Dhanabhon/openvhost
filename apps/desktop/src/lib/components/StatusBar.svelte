<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { UNKNOWN, formatBytes, formatProcessCount } from '$lib/stats.derive';

	let {
		servicesBytes,
		processCount,
		homeBytes,
		homePending = false
	}: {
		/** `null` = unknown. Never pass 0 to mean unknown. */
		servicesBytes: number | null;
		processCount: number | null;
		homeBytes: number | null;
		/** True only while the FIRST home walk is in flight. */
		homePending?: boolean;
	} = $props();

	const memory = $derived(servicesBytes === null ? UNKNOWN : formatBytes(servicesBytes));
	const processes = $derived(processCount === null ? UNKNOWN : formatProcessCount(processCount));
	// Three states, not two: a walk in progress is not a failure, and saying "—"
	// for it would be as wrong as saying "measuring…" for a read that failed.
	const home = $derived(
		homeBytes !== null ? formatBytes(homeBytes) : homePending ? 'measuring…' : UNKNOWN
	);
</script>

<!-- No `aria-live`: this updates every 2 seconds and a live region would have a
     screen reader announce resource figures over whatever the user is doing. It is
     a labelled region they can visit deliberately instead. -->
<div class="statusbar" aria-label="Resource usage" data-testid="statusbar">
	<span>services <span class="num">{memory}</span></span>
	<span class="sep" aria-hidden="true">·</span>
	<span class="num">{processes}</span>
	<span class="sep" aria-hidden="true">·</span>
	<span class="mono">~/.openvhost</span>
	<span class="num">{home}</span>
</div>

<style>
	/* Ported from docs/design/mock.css's `.statusline` (flex row, --vh-space-4 gaps,
	   --vh-text-2, --vh-text-caption, values in .num/.mono), promoted from the log
	   viewer's pane-level strip to window level. Two adaptations: a fixed height and
	   a `border-top`, because at window level it is a chrome edge rather than a
	   trailing line inside a scrolling pane, and horizontal-only padding since the
	   height now does the vertical spacing. */
	.statusbar {
		display: flex;
		align-items: center;
		gap: var(--vh-space-4);
		height: 26px;
		padding: 0 var(--vh-space-6);
		border-top: 1px solid var(--vh-border);
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
		/* The strip must never be the reason the window scrolls: it is a fixed grid
		   row, and a long value ellipsizes rather than pushing the row wider. */
		white-space: nowrap;
		overflow: hidden;
	}
	.statusbar .mono {
		font-family: var(--vh-font-mono);
	}
	/* Values in the app's foreground colour against the muted labels, so the eye
	   lands on the numbers. `.num` is the global tabular-nums utility from
	   tokens.css — redeclaring the colour here does not override that. */
	.statusbar .num {
		color: var(--vh-text);
	}
	/* Decorative separator: `aria-hidden` in the markup, and the faintest colour
	   here so it reads as punctuation rather than content. */
	.statusbar .sep {
		color: var(--vh-border);
	}
</style>
