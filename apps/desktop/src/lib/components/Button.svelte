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
		variant?: 'primary' | 'quiet';
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
	.btn:disabled {
		color: var(--vh-text-disabled);
		border-color: var(--vh-border);
		background: transparent;
		cursor: not-allowed;
	}
	.btn-sm {
		padding: 4px 10px;
		font-size: var(--vh-text-table);
	}
</style>
