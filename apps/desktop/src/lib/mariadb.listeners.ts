// SPDX-License-Identifier: GPL-3.0-or-later
// The Databases page's MariaDB live-event wiring — a PARALLEL subscriber to
// `databases.listeners.ts`'s `subscribeDatabaseEvents` (P1 MariaDB UI design
// D6), not a wider signature on it: `MariadbStore` holds scalars where
// `DatabasesStore` holds per-major maps, so the two subscribers stay small
// and separately readable rather than merging into one function juggling two
// unrelated shapes. The page manages two disposers.
//
// WHY THIS FILE EXISTS, same reasoning as `databases.listeners.ts`'s own
// header: `onMount` never runs under `svelte/server`, so the seam between a
// typed IPC event and the store it feeds is untestable from a route test —
// this file is what makes it testable at all. A neuter experiment that
// severed `applyInstallProgress`'s callback body here would be exactly as
// invisible to the rest of the suite as the MySQL regression that file
// documents, which is the whole reason this wiring gets its own test file
// rather than living inline in `onMount`.
//
// What is still NOT covered here: that `+page.svelte` actually calls this on
// mount and calls the returned disposer on unmount — that needs a DOM the
// server renderer does not have.

import type {
	MariadbInitLogEvent,
	MariadbInstallLogEvent,
	MariadbInstallProgressDto,
	MariadbInstallProgressEvent
} from './ipc';

/** The three subscriptions this page needs for MariaDB, injected so a test
 *  never touches real IPC. Each resolves with its own unlisten function. */
export interface MariadbEventApi {
	onMariadbInstallLog(cb: (ev: MariadbInstallLogEvent) => void): Promise<() => void>;
	onMariadbInstallProgress(cb: (ev: MariadbInstallProgressEvent) => void): Promise<() => void>;
	onMariadbInitLog(cb: (ev: MariadbInitLogEvent) => void): Promise<() => void>;
}

/** The `MariadbStore` surface these events feed — structural, so a test can
 *  pass a recorder instead of a whole store. */
export interface MariadbEventSink {
	appendInstallLog(line: string): void;
	applyInstallProgress(progress: MariadbInstallProgressDto): void;
	appendInitLog(line: string): void;
}

/** The `UninstallStore` surface. `uninstalling` is read at DELIVERY time, not
 *  at subscription time — same reasoning `databases.listeners.ts`'s own
 *  identically-named interface states: which operation owns the shared
 *  MariaDB log channel can change while the listener is alive. */
export interface UninstallEventSink {
	readonly uninstalling: string;
	appendLog(major: string, line: string): void;
}

/** The identity value MariaDB's own uninstall log lines are attributed under
 *  in the SHARED `UninstallStore` — `uninstallStore.request('mariadb', major)`
 *  is always called with this same value (see `mariadb.svelte.ts`'s
 *  `MARIADB_SERIES`). Redeclared as a literal rather than imported, the same
 *  decoupling `mariadb.svelte.ts`'s own `UiLog` doc comment gives: this file
 *  stays independent of that module's exports. */
const MARIADB_SERIES = '11.4';

/**
 * Register every live subscription the Databases page needs for MariaDB, and
 * return the one function that releases all of them.
 *
 * Same idempotent-disposer and unmounted-mid-registration handling as
 * `subscribeDatabaseEvents` — see that function's own doc comment for the
 * full reasoning, unchanged here: `isDisposed` is polled after the
 * registrations settle, and the returned disposer is safe to call twice.
 *
 * `mariadb-install-log-event` is the SAME dual-purpose channel design D3
 * gives MySQL's own `mysql-install-log-event`: MariaDB's install progress
 * reports through {@link onMariadbInstallProgress} instead, so in practice
 * this channel's only producer is an uninstall's `Removal::PackageTree` step
 * failing to report through it. Routing on the CURRENT operation (checked at
 * delivery time, mirroring `subscribeDatabaseEvents`) is what keeps a
 * MariaDB uninstall's output in the uninstall dialog rather than in the row.
 */
export async function subscribeMariadbEvents(
	api: MariadbEventApi,
	store: MariadbEventSink,
	uninstall: UninstallEventSink,
	isDisposed: () => boolean
): Promise<() => void> {
	const stopInstallLog = await api.onMariadbInstallLog((ev) => {
		if (uninstall.uninstalling !== '') uninstall.appendLog(MARIADB_SERIES, ev.line);
		else store.appendInstallLog(ev.line);
	});
	const stopProgress = await api.onMariadbInstallProgress((ev) =>
		store.applyInstallProgress(ev.progress)
	);
	// POST-HOC ONLY (`mariadb.svelte.ts`'s own `initLog` doc comment): this
	// fires once, after a failed init ends, never while it is still running.
	const stopInitLog = await api.onMariadbInitLog((ev) => store.appendInitLog(ev.line));

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
