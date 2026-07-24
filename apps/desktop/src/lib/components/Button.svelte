<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '../utils/cn';

	let {
		variant,
		size,
		disabled = false,
		onclick,
		children
	}: {
		variant?: 'primary' | 'quiet';
		size?: 'sm';
		disabled?: boolean;
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
