<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import {
		confirmQuit,
		onQuitRequested,
		onServiceRegistered,
		onServiceState,
		onServiceUnregistered,
		pendingInstall,
		quitDialogReady,
		type IpcError,
		type PendingInstallDto
	} from '$lib/ipc';
	import { errorMessage } from '$lib/errors';
	import { bootStatusStore } from '$lib/boot-status.shared.svelte';
	import { bootRendering } from '$lib/boot-status.svelte';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { statsStore } from '$lib/stats.shared.svelte';
	import { storeStatusStore } from '$lib/store-status.shared.svelte';
	import { pendingServiceNames } from '$lib/services.derive';
	import BootTakeover from '$lib/components/BootTakeover.svelte';
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
	// What this window should be showing at all (degraded-boot design D2). The
	// decision is a pure function of the two things the store knows, so the four
	// cases can be argued in `boot-status.svelte.ts` and tested without a DOM;
	// the template below only routes them.
	const rendering = $derived(bootRendering(bootStatusStore.status, bootStatusStore.askFailed));
	// An install (PHP or MySQL alike — review fix wave, Important 1) in
	// progress is invisible to `pending` — it is not a supervised service —
	// so it is fetched separately, once, at the moment the dialog is about
	// to open. Not reactive/polled: unlike services (which push state
	// changes the layout already subscribes to), there is no live event for
	// "an install just started/finished", and asking on every open is cheap
	// enough that polling continuously would only add complexity for no
	// benefit.
	let pendingInstallInfo = $state<PendingInstallDto | null>(null);

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
	//
	// `onServiceRegistered` is wired alongside `onServiceState` for the same
	// reason: a service registered after launch (a PHP major installed at
	// runtime, a freshly initialized MySQL major) must reach `servicesStore`
	// without a relaunch, on every route — not just the Languages/Databases
	// page that happened to trigger the install (see `ServicesStore.
	// applyRegistered`'s doc comment for why `reload()` is still kept there
	// too, as a synchronous guarantee independent of event-delivery timing).
	// `onServiceUnregistered` is its mirror, for the same reason in reverse:
	// an uninstalled version's row must DISAPPEAR everywhere, not linger
	// offering Start for a binary that is gone.
	onMount(() => {
		let unlistens: Array<() => void> = [];
		let disposed = false;

		void (async () => {
			// Accumulated as each subscription resolves, so a failure registering
			// the Nth listener can still tear down the N-1 that already registered
			// rather than leaking them. A list rather than one variable per
			// listener: there are three of them now (state, registered,
			// unregistered), and the per-variable shape multiplied a case into the
			// disposal path, the catch, and the cleanup for every one added.
			const stops: Array<() => void> = [];
			try {
				// Subscribe BEFORE the snapshot, keeping the ordering the Services page used:
				// a state change (or a registration) landing mid-fetch at least reaches the
				// listener.
				stops.push(await onServiceState((ev) => servicesStore.applyState(ev)));
				stops.push(await onServiceRegistered((ev) => servicesStore.applyRegistered(ev.status)));
				// The removal half (package-uninstall design D4): a service the user
				// uninstalled must leave the Services page and the titlebar count
				// without a relaunch, on every route.
				stops.push(await onServiceUnregistered((ev) => servicesStore.applyUnregistered(ev.id)));
				// `await` means teardown may already have run (dev HMR disposes this layout).
				// Drop every listener immediately rather than leaking them past the cleanup.
				if (disposed) {
					for (const stop of stops) stop();
					return;
				}
				unlistens = stops;
				// Resolves rather than rejects — the store captures load failures on
				// `error`, which the Services page's banner renders.
				await servicesStore.loadServices();
			} catch (e) {
				// Only the `on*` subscriptions can land here, and the ipc barrel
				// normalizes it. Tear down whichever listeners DID register so a
				// partial failure cannot leak them past this closure.
				for (const stop of stops) stop();
				servicesStore.fail(e as IpcError);
			}
		})();

		return () => {
			disposed = true;
			for (const stop of unlistens) stop();
			unlistens = [];
		};
	});

	// How far this launch got — asked HERE, once, and it is the question that
	// decides whether there is an app to show at all (degraded-boot design D2).
	//
	// Its own `onMount`, separate from every other one in this file, for the
	// reason they are all separate: no failure may take another down. That
	// matters more here than anywhere else — this is the one ask that still
	// answers when almost nothing else does, so it must not be sequenced behind
	// a supervisor subscription that cannot succeed on the very boots this
	// exists to describe.
	//
	// Asked once and never re-asked: `BootState` is managed once in `setup` and
	// never replaced, so the answer cannot change while the app is running.
	// `load()` resolves rather than rejects — a failed ask becomes `askFailed`,
	// which renders the children plus a banner rather than a takeover.
	onMount(() => {
		void bootStatusStore.load();
	});

	// Whether `state.db` opened this run — asked HERE, once, for the same reason the
	// supervisor snapshot is: the answer is app-level rather than page-level (the
	// store is down everywhere, not on one route), and the layout is the one
	// component that outlives navigation, so the banner AppShell renders from this
	// cannot flash back in on every move between Sites and Logs.
	//
	// Asked once and never re-asked: `Db::open` runs at startup and the handle is
	// managed exactly once, so the answer cannot change while the app is running.
	// Its own `onMount` rather than a line inside the supervisor block above,
	// matching the quit listener's separation: neither failure may take the other
	// down. `load()` resolves rather than rejects — the store keeps a failed ask as
	// silence, never as a fabricated reason.
	onMount(() => {
		void storeStatusStore.load();
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
					pendingInstallInfo = null;
					quitOpen = true;
					// Fire-and-forget: a failure here must not block the dialog from
					// opening — it only means the install sentence is missing, not
					// that the whole confirmation is broken. `pendingInstallInfo` stays
					// `null`, the same as "nothing is installing".
					void pendingInstall()
						.then((info) => {
							pendingInstallInfo = info;
						})
						.catch(() => {});
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
<!-- The gate (design D2). All four `BootRendering` kinds are named here rather
     than three plus a fallthrough: an unnamed branch is how a fifth state comes
     to inherit a fourth's screen, which is the failure `bootRendering`'s own
     `never` arm exists to stop at compile time. -->
{#if rendering.kind === 'pending'}
	<!-- Nothing yet, and that is a THIRD answer rather than a shortcut to one of
	     the other two. Rendering the children here would mount the real pages on
	     a degraded launch, fire the commands that cannot answer, and leave spec
	     §9.1 — no page shows Tauri's `.manage()` string — depending on
	     `boot_status` winning a race against them. Rendering the takeover here
	     would put a failure screen in front of every healthy launch for a frame.
	     A local IPC round trip is what this waits on, not a network. -->
{:else if rendering.kind === 'takeover'}
	<BootTakeover boot={rendering.boot} {quitting} {quitError} onQuit={onConfirmQuit} />
{:else if rendering.kind === 'app' || rendering.kind === 'appDespiteFailedAsk'}
	<!-- Both kinds render the children, and only `appDespiteFailedAsk` also owes
	     a banner — which AppShell renders, because `.window` is a `height: 100%`
	     grid and a banner beside it would push the titlebar out of the window.
	     They stay two kinds rather than one so that debt cannot be dropped
	     silently. -->
	{@render children()}
{/if}
{#if quitOpen}
	<QuitDialog
		{pending}
		pendingInstall={pendingInstallInfo}
		{quitting}
		error={quitError}
		onCancel={onCancelQuit}
		onConfirm={onConfirmQuit}
	/>
{/if}
