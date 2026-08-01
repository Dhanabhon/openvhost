<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import Button from './Button.svelte';
	import { formatNameList } from '$lib/services.derive';
	import { pendingOperationCopy, type PendingOperation } from '$lib/quit.derive';

	let {
		pending,
		pendingInstall = null,
		quitting = false,
		error = '',
		onCancel,
		onConfirm
	}: {
		/** Display names of services a quit would stop. Empty = nothing to lose. */
		pending: readonly string[];
		/** The package run currently occupying `InstallLock`'s single slot, if
		 *  any — a PHP or MySQL install, a MySQL datadir initialization, or a
		 *  `brew uninstall`. None of them is a supervised service, so all of them
		 *  are invisible to `pending`; without this the quit confirmation would
		 *  say "nothing will be interrupted" while one was about to be killed
		 *  mid-work.
		 *
		 *  `operation` is REQUIRED, not optional, and that is the fix for the
		 *  branch review's HIGH: this prop used to be `{ kind, label }`, Rust had
		 *  been sending `operation` end to end for a while, and TypeScript
		 *  accepted the extra field in silence (no excess-property check on a
		 *  variable) — so an uninstall was narrated with the install sentence,
		 *  every clause of it false. Requiring the field makes `+layout.svelte`'s
		 *  hand-off of the generated `PendingInstallDto` a compile-time seam.
		 *
		 *  The sentence itself lives in `quit.derive.ts` (including why PHP's
		 *  label needs a leading word and MySQL's does not) so it can be asserted
		 *  as words, and so no two operations can collapse onto one wording —
		 *  there are three since the security audit gave an initialization its
		 *  own `PackageOperation`. */
		pendingInstall?: PendingOperation | null;
		/** True while `confirmQuit` is in flight — services are being stopped. */
		quitting?: boolean;
		/** A failed quit attempt, rendered in place rather than as a page banner:
		 *  the dialog is modal, so a banner behind the scrim would be unreachable. */
		error?: string;
		onCancel: () => void;
		onConfirm: () => void;
	} = $props();

	const hasPending = $derived(pending.length > 0);
	const hasInstall = $derived(pendingInstall !== null);
	/** "Stop and quit" whenever there is something to stop — a running service
	 *  OR a Homebrew run in flight — otherwise a button promising to stop
	 *  nothing is a small lie the user notices. Deliberately the same label for
	 *  an install and an uninstall: both are stopped, and only the CONSEQUENCE
	 *  differs, which is what the sentence above the buttons explains. */
	const confirmLabel = $derived(hasPending || hasInstall ? 'Stop and quit' : 'Quit');
	/** The in-flight sentence, split at the label so it can keep the mono face.
	 *  Every word of it comes from `quit.derive.ts`; this component chooses only
	 *  where the three parts sit. */
	const installCopy = $derived(
		pendingInstall === null ? null : pendingOperationCopy(pendingInstall)
	);

	let dialog = $state<HTMLElement | null>(null);

	onMount(() => {
		// Focus the DIALOG, not a button inside it.
		//
		// This used to focus Cancel, reasoning that a stray Enter or Space right
		// after the dialog opens must not quit the app. That safety property is
		// preserved and improved here — the container is not a button, so a stray
		// Enter or Space activates nothing at all.
		//
		// What focusing Cancel also did was put a focus ring on a button the user
		// had never navigated to. Whether `:focus-visible` matches an element
		// focused by script is a browser heuristic — Chromium, for one, carries it
		// over from whatever was focused before — so the ring appeared or not
		// depending on how the user reached the dialog, which is worse than either
		// outcome consistently. Reported three times; restyling the ring never
		// addressed it, because the ring was not the problem. Putting focus on a
		// control was.
		//
		// Focusing the container sidesteps the heuristic entirely rather than
		// betting on it: no button is focused, so no button can be ringed.
		//
		// Focusing the container is the ordinary modal pattern and is better for
		// screen readers too: this element carries `aria-labelledby`/`aria-describedby`,
		// so landing on it announces the title and the body rather than just the
		// word "Cancel". Tab from here moves to Cancel and rings it properly,
		// because that IS keyboard navigation.
		dialog?.focus();
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
		// Focus starts on the dialog CONTAINER now, not on a button, so Shift+Tab
		// from that starting position matched neither branch below and the browser
		// moved focus backwards — out of a dialog that claims `aria-modal="true"`.
		// Only reachable as the very first key pressed, which is exactly when a
		// keyboard user is most likely to press it.
		//
		// Plain Tab from the container needs no branch: it is already heading INTO
		// the dialog, and letting the browser do it is what puts focus on Cancel.
		if (e.shiftKey && document.activeElement === dialog) {
			e.preventDefault();
			last.focus();
		} else if (e.shiftKey && document.activeElement === first) {
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
	tabindex="-1"
>
	<h2 id="quit-title">Quit OpenVHost?</h2>
	<p id="quit-body" class="body">
		{#if hasPending}
			<span class="mono">{formatNameList(pending)}</span>
			{pending.length === 1 ? 'is' : 'are'} running. Quitting stops
			{pending.length === 1 ? 'it' : 'them'} first, so nothing is left serving in the background.
		{:else if !pendingInstall}
			No services are running. Nothing will be interrupted.
		{/if}
		{#if installCopy}
			<!-- No branching on `kind` or `operation` here, on purpose: this
			     template used to pick the leading word itself and hardcode the
			     rest of the sentence, which is how an uninstall came to be
			     narrated as an install. The three parts arrive already decided
			     (`quit.derive.ts`), and the only choice left in markup is that
			     the label wears the mono face. -->
			{hasPending ? ' ' : ''}{installCopy.lead}<span class="mono">{installCopy.label}</span
			>{installCopy.rest}
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
	/* The container takes focus on open (see `onMount`), and the global
	   `:focus-visible` in tokens.css would then draw a ring around the whole
	   dialog — trading a ring on one button for a much larger one. Suppressed
	   here and ONLY here: this element is `tabindex="-1"`, so it is not reachable
	   by keyboard and has no focus state a user needs to see. Every control
	   inside it keeps the global ring, which is what a keyboard user actually
	   navigates between. */
	.quit-dialog:focus,
	.quit-dialog:focus-visible {
		outline: none;
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
