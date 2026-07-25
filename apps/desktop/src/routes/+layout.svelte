<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import { onServiceState, type IpcError } from '$lib/ipc';
	import { servicesStore } from '$lib/services.shared.svelte';

	let { children } = $props();

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
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>
{@render children()}
