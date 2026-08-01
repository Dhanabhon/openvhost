<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '../utils/cn';

	let {
		variant,
		size,
		disabled = false,
		ariaLabel,
		focusFallback = false,
		testId,
		expanded,
		controls,
		onclick,
		children
	}: {
		variant?: 'primary' | 'quiet' | 'ghost';
		size?: 'sm';
		disabled?: boolean;
		/** Overrides the accessible name when the visible label alone is ambiguous out of
		 * context — e.g. a row of repeated "Start"/"Stop" buttons, one per service, needs
		 * each one to announce which service it acts on. Leaves the visible text untouched. */
		ariaLabel?: string;
		/** Marks this button's real DOM node as the app's deterministic focus-restoration
		 * fallback (`data-vh-focus-fallback`) — consumed by `SiteDrawer.svelte`'s focus-trap
		 * cleanup when the element that originally opened the drawer (e.g. a deleted row's
		 * Edit button) no longer exists in the DOM. Opt-in and defaults to `false`; every
		 * existing `<Button>` usage is unaffected. */
		focusFallback?: boolean;
		/** Test hook on the real `<button>`, following `StatusPill.svelte`'s `testId`
		 * precedent. Opt-in: omitted means no attribute is emitted at all. */
		testId?: string;
		/** Disclosure state for a button that reveals or hides a region — `aria-expanded`
		 * is a state `role="button"` supports, and it is the only thing that tells a
		 * screen-reader user whether the region is open. Omitted on plain buttons, which
		 * must NOT claim to be disclosures. */
		expanded?: boolean;
		/** Id of the region a disclosure button controls. Pass it only while that region
		 * is actually in the DOM — an `aria-controls` IDREF pointing at nothing is worse
		 * than none. `aria-controls` is a global ARIA property, so it is valid here. */
		controls?: string;
		onclick: () => void;
		children: Snippet;
	} = $props();
</script>

<button
	type="button"
	class={cn(
		'btn',
		variant === 'primary' && 'btn-primary',
		variant === 'quiet' && 'btn-quiet',
		variant === 'ghost' && 'btn-ghost',
		size === 'sm' && 'btn-sm'
	)}
	aria-label={ariaLabel}
	aria-expanded={expanded}
	aria-controls={controls}
	data-testid={testId}
	data-vh-focus-fallback={focusFallback ? '' : undefined}
	{disabled}
	{onclick}
>
	{@render children()}
</button>

<style>
	/* Ported from docs/design/mock.css (.btn, .btn-primary, .btn-quiet, .btn:disabled), plus
	   `.btn-sm`, which lives in docs/design/main-window.html's inline <style> override rather
	   than mock.css itself (mock.css has no `.btn-sm` rule of its own). */
	.btn {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		font: inherit;
		font-weight: 500;
		padding: 7px 14px;
		border-radius: var(--vh-radius-control);
		border: 1px solid transparent;
		cursor: pointer;
		transition:
			background var(--vh-dur-fast) var(--vh-ease-out),
			border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.btn-primary {
		background: var(--vh-accent);
		color: var(--vh-accent-contrast);
	}
	.btn-primary:hover {
		background: var(--vh-accent-hover);
	}
	.btn-quiet {
		background: transparent;
		color: var(--vh-text);
		border-color: var(--vh-border-strong);
	}
	.btn-quiet:hover {
		background: color-mix(in oklab, var(--vh-text) 6%, transparent);
	}
	/* The row-action level. `.btn-quiet` is the app's secondary button and draws a full
	   border at rest, which is right for a dialog's Cancel — one control, one decision — and
	   wrong for a control repeated once per row: five bordered boxes per row down a list
	   reads as a grid of frames rather than a place to act. This is the same button with the
	   resting border withheld until the pointer or focus arrives.

	   Identical geometry to `.btn-quiet` — only `border-color` differs, and the border stays
	   1px throughout — so a control can move between the two variants without reflowing, and
	   a ghost button sitting beside a quiet one lines up exactly. */
	.btn-ghost {
		background: transparent;
		color: var(--vh-text);
		border-color: transparent;
	}
	.btn-ghost:hover {
		background: color-mix(in oklab, var(--vh-text) 6%, transparent);
		border-color: var(--vh-border-strong);
	}
	/* The global `:focus-visible` in tokens.css puts a 2px ring 2px outside the
	   control. On a button with no border of its own that reads as one focus
	   indicator, which is what it is. On `.btn-quiet` — the only variant with a
	   VISIBLE border — it stacks three concentric edges: the grey 1px border,
	   the 2px gap, then the green ring. That reads as a doubled frame, and it
	   was reported twice on the quit dialog's Cancel button.

	   An earlier attempt moved the global offset from 1px to 2px. That treated
	   the symptom: it separated the two frames rather than removing one, so
	   there were still two.

	   Closing the gap and letting the border carry the ring's colour merges
	   them into a single 3px band whose radius matches the button. Only colour
	   and offset change — the border is still 1px, so nothing reflows and the
	   button does not move when focused.

	   NOT applied to `.btn-primary`: its border is already transparent, so it
	   has no doubling to fix, and a green ring flush against a green fill would
	   lose the contrast the 2px gap currently gives it against the page.

	   `.btn-ghost` joins for a different reason. It has nothing to double — its
	   resting border is transparent, like primary's — but it lives in the row
	   action strip next to `.icon-link`, which already copies this exact
	   treatment so the group reads as one family of controls. Letting ghost
	   take the global ring instead would split that group in two the moment
	   anyone tabbed through it. The band it produces is the same 3px. */
	.btn-quiet:focus-visible,
	.btn-ghost:focus-visible {
		border-color: var(--vh-focus-ring);
		outline-offset: 0;
	}
	.btn:disabled {
		color: var(--vh-text-disabled);
		border-color: var(--vh-border);
		background: transparent;
		cursor: not-allowed;
	}
	/* Must follow `.btn:disabled` — same specificity, so order decides. Without it a ghost
	   button POPS a border in at the instant it goes busy, which is a frame appearing out of
	   nowhere at exactly the moment the row should look calm. The dimmed label already
	   carries the state, and it is the same signal the other variants rely on. */
	.btn-ghost:disabled {
		border-color: transparent;
	}
	.btn-sm {
		padding: 4px 10px;
		font-size: var(--vh-text-table);
	}
</style>
