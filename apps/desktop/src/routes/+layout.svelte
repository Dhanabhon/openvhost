<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import {
		confirmQuit,
		onQuitRequested,
		onServiceState,
		quitDialogReady,
		type IpcError
	} from '$lib/ipc';
	import { errorMessage } from '$lib/errors';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { statsStore } from '$lib/stats.shared.svelte';
	import { pendingServiceNames } from '$lib/services.derive';
	import QuitDialog from '$lib/components/QuitDialog.svelte';

	let { children } = $props();

	// Quit confirmation lives HERE for the same reason the supervisor subscription
	// does: closing the window is not a property of whichever page happens to be
	// open, and the layout is the one component that outlives navigation.
	let quitOpen = $state(false);
	let quitting = $state(false);
	let quitError = $state('');
	// Read live, not snapshotted when the dialog opened: a service that stops while
	// the user reads the dialog should drop out of the sentence.
	const pending = $derived(pendingServiceNames(servicesStore.services));

	async function onConfirmQuit(): Promise<void> {
		if (quitting) return;
		quitting = true;
		quitError = '';
		try {
			await confirmQuit();
			// Reaching here means the window was NOT destroyed — the command
			// resolved without quitting. Surface it rather than leaving a dialog
			// stuck on "Stopping services…" forever.
			quitError = 'The quit did not complete. Close the window again to retry.';
		} catch (e) {
			quitError = errorMessage(e);
		} finally {
			quitting = false;
		}
	}

	function onCancelQuit(): void {
		// Not gated on `quitting`: the dialog's Cancel is disabled mid-quit, but
		// Escape is not, and a user who hits it deserves the dialog to go away.
		quitOpen = false;
		quitError = '';
	}

	// The supervisor wiring the TITLEBAR depends on lives here, not on a page: every
	// route shows "N running", and the layout is the one component that outlives
	// navigation — so the subscription is created exactly once and can never double
	// up as the user moves between Sites and Services. Page-specific log wiring
	// (`onServiceLog` + the log tail seed, which only the Services page renders)
	// deliberately stays on that page.
	onMount(() => {
		let unlisten: (() => void) | null = null;
		let disposed = false;

		void (async () => {
			try {
				// Subscribe BEFORE the snapshot, keeping the ordering the Services page used:
				// a state change landing mid-fetch at least reaches the listener.
				const stop = await onServiceState((ev) => servicesStore.applyState(ev));
				// `await` means teardown may already have run (dev HMR disposes this layout).
				// Drop the listener immediately rather than leaking it past the cleanup.
				if (disposed) {
					stop();
					return;
				}
				unlisten = stop;
				// Resolves rather than rejects — the store captures load failures on
				// `error`, which the Services page's banner renders.
				await servicesStore.loadServices();
			} catch (e) {
				// Only `onServiceState` can land here, and the ipc barrel normalizes it.
				servicesStore.fail(e as IpcError);
			}
		})();

		return () => {
			disposed = true;
			unlisten?.();
			unlisten = null;
		};
	});

	// Separate from the supervisor subscription above: that one must not tear this
	// one down if it fails, or a failed service-state listener would leave the app
	// unquittable (Rust prevents the close only when this listener is registered,
	// but a failure here would mean the dialog never opens).
	onMount(() => {
		let unlisten: (() => void) | null = null;
		let disposed = false;

		void (async () => {
			try {
				const stop = await onQuitRequested(() => {
					quitError = '';
					quitOpen = true;
				});
				if (disposed) {
					stop();
					return;
				}
				unlisten = stop;
				// Ack only AFTER the listener is live. Acking first would let Rust
				// prevent a close during the window where nothing could answer it.
				await quitDialogReady();
			} catch {
				// Nothing to render: with no listener the Rust side does not prevent
				// the close, so the window shuts as it did before this feature.
			}
		})();

		return () => {
			disposed = true;
			unlisten?.();
			unlisten = null;
		};
	});

	// Sampling is paused whenever the window is hidden. The master plan's first
	// principle is "lightweight always-on … idle RAM budget for the app itself
	// < 100 MB. This is why Tauri was chosen over Electron" — an app left open
	// behind an IDE all day must cost nothing while nobody is looking at it.
	//
	// The store owns the timers and the layout owns this listener, so the store
	// stays DOM-free and unit-testable with fake timers.
	onMount(() => {
		const sync = () => {
			if (document.visibilityState === 'visible') statsStore.start();
			else statsStore.stop();
		};
		sync();
		document.addEventListener('visibilitychange', sync);
		return () => {
			document.removeEventListener('visibilitychange', sync);
			statsStore.stop();
		};
	});
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>
{@render children()}
{#if quitOpen}
	<QuitDialog
		{pending}
		{quitting}
		error={quitError}
		onCancel={onCancelQuit}
		onConfirm={onConfirmQuit}
	/>
{/if}
