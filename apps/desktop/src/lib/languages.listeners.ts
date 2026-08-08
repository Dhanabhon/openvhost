// SPDX-License-Identifier: GPL-3.0-or-later
// The Languages page's live-event wiring, extracted from `+page.svelte`'s
// `onMount` — the PHP sibling of `databases.listeners.ts` and
// `mariadb.listeners.ts`, written the same way for the same reason.
//
// WHY THIS FILE EXISTS, copied from `databases.listeners.ts`'s header because
// it was earned there and applies here unchanged: `onMount` never runs under
// `svelte/server`, so nothing registered inside it can be observed by any route
// test. On the Databases page a neuter experiment severed the progress callback
// — an install would have shown "Preparing the download…" forever — and the
// whole suite stayed green. The PHP subscription lived inline in `onMount`
// until this file, so it had exactly that hole, and the progress event added by
// off-Homebrew slice 5C had no subscriber at all.
//
// What is still NOT covered here: that `+page.svelte` actually calls this on
// mount and calls the returned disposer on unmount. That needs a DOM the server
// renderer does not have. Everything downstream of the call — which sink each
// event reaches, what happens when the page unmounts mid-registration, and
// whether every listener is really released — is covered by
// `languages.listeners.test.ts`.

import type { PhpInstallLogEvent, PhpInstallProgressDto, PhpInstallProgressEvent } from './ipc';

/** The two subscriptions this page needs, injected so a test never touches real
 *  IPC. Each resolves with its own unlisten function. */
export interface LanguagesEventApi {
	onPhpInstallLog(cb: (ev: PhpInstallLogEvent) => void): Promise<() => void>;
	onPhpInstallProgress(cb: (ev: PhpInstallProgressEvent) => void): Promise<() => void>;
}

/** The `LanguagesStore` surface these events feed — structural, so a test can
 *  pass a recorder instead of a whole store. */
export interface LanguagesEventSink {
	appendLog(major: string, line: string): void;
	applyInstallProgress(major: string, progress: PhpInstallProgressDto): void;
}

/** The `UninstallStore` surface. `uninstalling` is read at DELIVERY time, not
 *  at subscription time — same reasoning `databases.listeners.ts`'s
 *  identically-named interface states: which operation owns the shared PHP log
 *  channel changes while the listener is alive. */
export interface UninstallEventSink {
	readonly uninstalling: string;
	appendLog(major: string, line: string): void;
}

/**
 * Register every live subscription the Languages page needs, and return the one
 * function that releases all of them.
 *
 * `isDisposed` is polled after the registrations settle: this page can unmount
 * while they are still in flight, and a listener registered after that would
 * otherwise outlive the component with nothing left holding its disposer. When
 * it reports `true`, everything registered so far is released and the returned
 * disposer is a no-op.
 *
 * The disposer is idempotent — calling it twice must not call an unlisten
 * function twice.
 *
 * **The two channels are different routes, not two views of one** (off-Homebrew
 * slice 5C design D4):
 *
 * * `php-install-log` is brew's own stdout/stderr, so the HOMEBREW route — and
 *   `uninstall_package`, which shares it (design D1: one lock, one output
 *   surface).
 * * `php-install-progress` is the PACKAGED route's five typed pipeline states.
 *   `run_package_install` is its only emitter; `run_brew_install` emits nothing
 *   on it. That is what makes subscribing here inert on a machine with Homebrew
 *   and no package tree (spec §8.6): the listener registers, and no event ever
 *   arrives to change a pixel.
 */
export async function subscribeLanguageEvents(
	api: LanguagesEventApi,
	store: LanguagesEventSink,
	uninstall: UninstallEventSink,
	isDisposed: () => boolean
): Promise<() => void> {
	const stopInstallLog = await api.onPhpInstallLog((ev) => {
		// One channel, two operations: `uninstall_package` streams on the SAME
		// `php-install-log` event `install_php` uses, so the line is routed by
		// whichever operation currently holds that lock. `UninstallStore.appendLog`
		// re-checks the same condition itself, so this routing is a convenience,
		// not the guard.
		if (uninstall.uninstalling !== '') uninstall.appendLog(ev.major, ev.line);
		else store.appendLog(ev.major, ev.line);
	});
	// The packaged install's own surface: five typed pipeline states, not stdout.
	// `ev.major` is forwarded rather than dropped — several PHP majors sit side
	// by side, so a progress bar has to know which row it belongs to, and this
	// store has already shipped the untagged version of that bug once for logs.
	const stopProgress = await api.onPhpInstallProgress((ev) =>
		store.applyInstallProgress(ev.major, ev.progress)
	);

	const stops = [stopInstallLog, stopProgress];
	let released = false;
	const release = (): void => {
		if (released) return;
		released = true;
		for (const stop of stops) stop();
	};

	if (isDisposed()) {
		release();
		return () => {};
	}
	return release;
}
