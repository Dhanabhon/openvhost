// SPDX-License-Identifier: GPL-3.0-or-later
// The Databases page's live-event wiring, extracted from `+page.svelte`'s
// `onMount` so it can be tested at all.
//
// WHY THIS FILE EXISTS. A neuter experiment on this slice broke the page's
// progress subscription — replaced the callback body with `() => {}`, so an
// install would have shown "Preparing the download…" forever — and the whole
// suite stayed green. It had to: `onMount` never runs under `svelte/server`, so
// no route test can observe anything registered there, and the seam between a
// typed IPC event and the store it feeds was therefore untestable by
// construction. This is the same class of defect as the UI-glue bugs a
// whole-branch review caught after every per-part test passed.
//
// What is still NOT covered here: that `+page.svelte` actually calls this on
// mount and calls the returned disposer on unmount. That needs a DOM the server
// renderer does not have. Everything downstream of the call — which sink each
// event reaches, what happens when the page unmounts mid-registration, and
// whether every listener is really released — is covered by
// `databases.listeners.test.ts`.

import type {
	MysqlInitLogEvent,
	MysqlInstallLogEvent,
	MysqlInstallProgressDto,
	MysqlInstallProgressEvent
} from './ipc';

/** The three subscriptions this page needs, injected so a test never touches
 *  real IPC. Each resolves with its own unlisten function. */
export interface DatabasesEventApi {
	onMysqlInstallLog(cb: (ev: MysqlInstallLogEvent) => void): Promise<() => void>;
	onMysqlInstallProgress(cb: (ev: MysqlInstallProgressEvent) => void): Promise<() => void>;
	onMysqlInitLog(cb: (ev: MysqlInitLogEvent) => void): Promise<() => void>;
}

/** The `DatabasesStore` surface these events feed — structural, so the test
 *  can pass a recorder instead of a whole store. */
export interface DatabasesEventSink {
	appendInstallLog(major: string, line: string): void;
	applyInstallProgress(progress: MysqlInstallProgressDto): void;
	appendInitLog(major: string, line: string): void;
}

/** The `UninstallStore` surface. `uninstalling` is read at DELIVERY time, not
 *  at subscription time — which operation owns the shared MySQL log channel
 *  changes while the listener is alive. */
export interface UninstallEventSink {
	readonly uninstalling: string;
	appendLog(major: string, line: string): void;
}

/**
 * Register every live subscription the Databases page needs, and return the
 * one function that releases all of them.
 *
 * `isDisposed` is polled after the registrations settle: this page can unmount
 * while they are still in flight, and a listener registered after that would
 * otherwise outlive the component with nothing left holding its disposer. When
 * it reports `true`, everything registered so far is released and the returned
 * disposer is a no-op.
 *
 * The disposer is idempotent — calling it twice must not call an unlisten
 * function twice.
 */
export async function subscribeDatabaseEvents(
	api: DatabasesEventApi,
	store: DatabasesEventSink,
	uninstall: UninstallEventSink,
	isDisposed: () => boolean
): Promise<() => void> {
	const stopInstallLog = await api.onMysqlInstallLog((ev) => {
		// Since the move off the brew install path this channel has ONE producer
		// left — `uninstall_package`. Routing on the CURRENT operation is what
		// keeps a MySQL uninstall's output in the uninstall dialog rather than
		// in the row. `UninstallStore.appendLog` re-checks the same condition
		// itself, so this is a convenience, not the guard.
		if (uninstall.uninstalling !== '') uninstall.appendLog(ev.major, ev.line);
		else store.appendInstallLog(ev.major, ev.line);
	});
	// The install's own surface: five typed pipeline states, not stdout. This
	// is the line that a mutation silently severed while every test stayed
	// green — see this file's header.
	const stopProgress = await api.onMysqlInstallProgress((ev) =>
		store.applyInstallProgress(ev.progress)
	);
	const stopInitLog = await api.onMysqlInitLog((ev) => store.appendInitLog(ev.major, ev.line));

	const stops = [stopInstallLog, stopProgress, stopInitLog];
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
