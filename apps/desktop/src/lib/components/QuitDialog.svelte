<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import Button from './Button.svelte';
	import { formatNameList } from '$lib/services.derive';

	let {
		pending,
		installingMajor = null,
		quitting = false,
		error = '',
		onCancel,
		onConfirm
	}: {
		/** Display names of services a quit would stop. Empty = nothing to lose. */
		pending: readonly string[];
		/** The PHP major currently installing via Homebrew, if any. A build in
		 *  progress is invisible to `pending` — it is not a supervised service —
		 *  so without this a quit would silently discard it mid-build. */
		installingMajor?: string | null;
		/** True while `confirmQuit` is in flight — services are being stopped. */
		quitting?: boolean;
		/** A failed quit attempt, rendered in place rather than as a page banner:
		 *  the dialog is modal, so a banner behind the scrim would be unreachable. */
		error?: string;
		onCancel: () => void;
		onConfirm: () => void;
	} = $props();

	const hasPending = $derived(pending.length > 0);
	const hasInstall = $derived(installingMajor !== null);
	/** "Stop and quit" whenever there is something to stop — a running service
	 *  OR an install in flight — otherwise a button promising to stop nothing
	 *  is a small lie the user notices. */
	const confirmLabel = $derived(hasPending || hasInstall ? 'Stop and quit' : 'Quit');

	let dialog = $state<HTMLElement | null>(null);

	onMount(() => {
		// Focus the SAFE choice, not the destructive one: a stray Enter or Space
		// arriving right after the dialog opens must not quit the app. Cancel is
		// first in DOM order, and queried rather than bound because `Button` exposes
		// no element ref — `bind:this` on a wrapping <span> would hand back an
		// unfocusable node and `.focus()` would silently no-op onto <body>.
		dialog?.querySelector<HTMLElement>('button')?.focus();
	});

	/**
	 * Deliberately smaller than `SiteDrawer`'s focus trap, which handles a form
	 * full of inputs, a folder picker and a combobox popup. This dialog has
	 * exactly two focusable controls, so the trap it needs is "wrap Tab between
	 * the first and last button" — reusing the drawer's machinery would mean
	 * either duplicating it or refactoring a component this change does not
	 * otherwise touch. If a third dialog appears, extract the drawer's version
	 * then rather than growing a second copy here.
	 */
	function onKeydown(e: KeyboardEvent): void {
		if (e.key === 'Escape') {
			// Escape cancels even mid-quit: the services are already stopping and
			// the window will go away, but refusing the key would look frozen.
			e.preventDefault();
			onCancel();
			return;
		}
		if (e.key !== 'Tab' || dialog === null) return;
		const els = Array.from(dialog.querySelectorAll<HTMLElement>('button:not(:disabled)'));
		if (els.length === 0) return;
		const first = els[0];
		const last = els[els.length - 1];
		if (e.shiftKey && document.activeElement === first) {
			e.preventDefault();
			last.focus();
		} else if (!e.shiftKey && document.activeElement === last) {
			e.preventDefault();
			first.focus();
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<!-- `aria-hidden` + no handler: clicking the scrim does NOT cancel. Every other
     scrim in this app closes on click, but this one guards the app's own exit —
     and the same stray click that would dismiss a drawer harmlessly would here
     dismiss a decision the user has not made yet. Escape and Cancel are the
     ways out, both explicit. -->
<div class="quit-backdrop" aria-hidden="true"></div>

<div
	class="quit-dialog"
	role="dialog"
	aria-modal="true"
	aria-labelledby="quit-title"
	aria-describedby="quit-body"
	bind:this={dialog}
	data-testid="quit-dialog"
>
	<h2 id="quit-title">Quit OpenVHost?</h2>
	<p id="quit-body" class="body">
		{#if hasPending}
			<span class="mono">{formatNameList(pending)}</span>
			{pending.length === 1 ? 'is' : 'are'} running. Quitting stops
			{pending.length === 1 ? 'it' : 'them'} first, so nothing is left serving in the background.
		{:else if !hasInstall}
			No services are running. Nothing will be interrupted.
		{/if}
		{#if hasInstall}
			{hasPending ? ' ' : ''}PHP <span class="mono">{installingMajor}</span> is still installing. Quitting
			stops it immediately and discards the download/build in progress — there is no resuming it, only
			starting over.
		{/if}
	</p>

	{#if error !== ''}
		<p class="quit-error" role="alert" data-testid="quit-error">{error}</p>
	{/if}

	<div class="actions">
		<Button variant="quiet" disabled={quitting} onclick={onCancel}>Cancel</Button>
		<Button variant="primary" disabled={quitting} onclick={onConfirm}>
			{quitting ? 'Stopping services…' : confirmLabel}
		</Button>
	</div>
</div>

<style>
	/* Backdrop + centred card. Ported from docs/design/mock.css's `.drawer-backdrop`
	   (same scrim colour and blur, so the two overlays read as one system), with the
	   panel centred instead of edge-anchored: this is a decision to make, not a form
	   to fill in, and the mock has no precedent for a centred dialog.

	   `--vh-z-dialog-backdrop` / `--vh-z-dialog` (60/70) were already in tokens.css,
	   provisioned above the drawer's 40/50 and unused until now — this is the dialog
	   they were reserved for, so it sits over an open drawer rather than under it. */
	.quit-backdrop {
		position: fixed;
		inset: 0;
		background: var(--vh-scrim);
		backdrop-filter: blur(2px);
		z-index: var(--vh-z-dialog-backdrop);
	}
	.quit-dialog {
		position: fixed;
		/* `translate` on a fixed element, not `margin: auto` in a flex parent: the
		   dialog is a direct child of the layout, which owns the app's grid. */
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		z-index: var(--vh-z-dialog);
		width: min(420px, calc(100vw - 2 * var(--vh-space-6)));
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		box-shadow: var(--vh-shadow-overlay);
		padding: var(--vh-space-6);
	}
	.quit-dialog h2 {
		font-size: var(--vh-text-section);
		font-weight: 600;
		margin: 0;
	}
	.quit-dialog .body {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		line-height: 1.6;
		margin: var(--vh-space-2) 0 0;
	}
	.quit-dialog .mono {
		font-family: var(--vh-font-mono);
		color: var(--vh-text);
	}
	.quit-error {
		margin: var(--vh-space-3) 0 0;
		color: var(--vh-fail);
		font-size: var(--vh-text-table);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: var(--vh-space-2);
		margin-top: var(--vh-space-6);
	}
</style>
