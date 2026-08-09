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
		revealRunDir,
		type IpcError,
		type PendingInstallDto
	} from '$lib/ipc';
	import { errorMessage } from '$lib/errors';
	import { bootStatusStore } from '$lib/boot-status.shared.svelte';
	import { appIsOnScreen, bootRendering } from '$lib/boot-status.svelte';
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
	// The takeover's other button (degraded-boot D3). Its failure lives HERE rather
	// than in the screen for the same reason `quitError` does: the component takes
	// no IPC, so the layer that made the call is the layer that can say what went
	// wrong with it.
	let revealError = $state('');
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

	// No busy flag, unlike `onConfirmQuit`. A reveal has no half-finished state to
	// protect — pressing it twice opens the same Finder window twice — and a
	// `revealing` flag could only add a way for the button to get stuck disabled on
	// a screen whose whole point is that the app is already broken.
	async function onRevealRunDir(): Promise<void> {
		revealError = '';
		try {
			await revealRunDir();
		} catch (e) {
			// Whether this fails depends on WHICH route produced `runDirUnusable`,
			// and both were measured (see `reveal_run_dir`'s doc comment). A
			// read-only home with `run` absent, or a dangling symlink at the `run`
			// path, leaves nothing for `canonicalize` to resolve and comes back
			// "could not show <home>/run in Finder: No such file or directory (os
			// error 2)". A plain FILE at the `run` path resolves fine and the button
			// simply works. Rendering the failure rather than swallowing it is what
			// keeps the first case from looking like a dead button on an error
			// screen.
			revealError = errorMessage(e);
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
	//
	// GATED on the app being on screen, exactly like the status-bar poll at the
	// bottom of this file and for a sharper version of the same reason:
	// `list_services` extracts `State<Arc<Supervisor>>`, which is managed inside
	// the ONE boot arm that succeeded, so on a degraded boot it really does come
	// back as Tauri's *"You must call `.manage()`"* string and `normalizeError`
	// puts that verbatim into `IpcError.message`. Nothing renders it today —
	// `servicesStore.error` is drawn by the Services page, which the takeover
	// removes from the DOM — so §9.1 ("no page shows Tauri's `.manage()` string")
	// held only because of what happens to be rendered. Gating makes it a property
	// of the code.
	//
	// The whole block moves, not just the ask, so the *subscribe BEFORE the
	// snapshot* ordering below survives intact — and a boot with no supervisor
	// registers no supervisor listeners either, which is the honest outcome
	// rather than a bonus.
	//
	// An `$effect` and not an `onMount`, for the reason the poll's gate gives:
	// `onMount` runs while the boot ask is still in flight, so a gate evaluated
	// there would read `pending` and never wire up a healthy launch either. The
	// effect re-runs when `boot_status` answers.
	//
	// `servicesWired` makes it run at most once — the ask is asked once and never
	// re-asked, but a one-shot snapshot must not depend on that. Teardown stays in
	// its own `onMount` below rather than being returned from the effect: an
	// effect cleanup fires on every re-run, so the guard and a returned cleanup
	// together could drop the listeners and then decline to re-register them.
	let serviceListeners: Array<() => void> = [];
	let serviceWiringDisposed = false;
	let servicesWired = false;

	onMount(() => {
		return () => {
			serviceWiringDisposed = true;
			for (const stop of serviceListeners) stop();
			serviceListeners = [];
		};
	});

	$effect(() => {
		if (servicesWired || !appIsOnScreen(rendering)) return;
		servicesWired = true;

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
				if (serviceWiringDisposed) {
					for (const stop of stops) stop();
					return;
				}
				serviceListeners = stops;
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
	// Its own effect rather than a line inside the supervisor block above,
	// matching the quit listener's separation: neither failure may take the other
	// down. `load()` resolves rather than rejects — the store keeps a failed ask as
	// silence, never as a fabricated reason.
	//
	// GATED, and `state_store_status` is the sharpest case of all: PR #69 built it
	// precisely so a broken store could refuse in the user's words instead of
	// Tauri's — but it extracts a `DbHandle`, which is managed inside the ONE boot
	// arm that succeeded. So on a degraded boot Tauri refuses this command before
	// its body ever runs, with the very sentence PR #69 deleted. `db_state.rs`'s
	// own module header says so in as many words. Same `$effect` + once-guard
	// shape as the supervisor block above; no cleanup, because a one-shot read has
	// nothing to tear down.
	let storeStatusAsked = false;
	$effect(() => {
		if (storeStatusAsked || !appIsOnScreen(rendering)) return;
		storeStatusAsked = true;
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
	//
	// The listener now RECORDS visibility rather than acting on it, because it is
	// no longer the only input — see the `$effect` below, which is where the two
	// are combined.
	let windowIsVisible = $state(true);
	onMount(() => {
		const sync = () => {
			windowIsVisible = document.visibilityState === 'visible';
		};
		sync();
		document.addEventListener('visibilitychange', sync);
		return () => {
			document.removeEventListener('visibilitychange', sync);
			statsStore.stop();
		};
	});

	// …and on a boot that produced no app, sampling never starts at all.
	//
	// An `$effect` rather than a line inside the listener above, because there are
	// now TWO inputs and only one of them is an event: `onMount` runs while the
	// boot ask is still in flight, so a gate evaluated once there would read
	// `pending` and never start the poll on a healthy launch either. The effect
	// re-runs when `boot_status` answers.
	//
	// `stop()` on the false branch and not just "skip start": the ask resolves
	// AFTER mount, so on a degraded boot there is a window in which nothing has
	// started yet, and this must stay correct if that ordering ever changes.
	// `start()`/`stop()` are both idempotent — `start()` returns early when the
	// timer exists — so re-running this effect cannot double the sampling rate.
	//
	// No cleanup of its own: teardown stays in the `onMount` above, where it
	// already was and where it is paired with the listener removal, so unmounting
	// stops the timers by exactly the same line it always did.
	//
	// See `appIsOnScreen` for why the gate is "the children are rendered" rather
	// than "the boot was ready": they differ only on a FAILED ask, where the app
	// is on screen and a permanently blank status bar would be a second lie about
	// a machine that is probably fine.
	$effect(() => {
		if (windowIsVisible && appIsOnScreen(rendering)) statsStore.start();
		else statsStore.stop();
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
	<BootTakeover
		boot={rendering.boot}
		{quitting}
		{quitError}
		{revealError}
		onReveal={onRevealRunDir}
		onQuit={onConfirmQuit}
	/>
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
