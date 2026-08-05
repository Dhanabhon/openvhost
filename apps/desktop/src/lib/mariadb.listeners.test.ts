// SPDX-License-Identifier: GPL-3.0-or-later
//
// Mirrors `databases.listeners.test.ts`'s structure and reasoning exactly —
// the seam a route test structurally cannot reach, since `onMount` never
// runs under `svelte/server`. A severed `applyInstallProgress` callback here
// would be exactly as invisible to the rest of the suite as the MySQL
// regression that file documents.

import { describe, expect, it, vi } from 'vitest';
import {
	subscribeMariadbEvents,
	type MariadbEventApi,
	type MariadbEventSink,
	type UninstallEventSink
} from './mariadb.listeners';
import type { MariadbInitLogEvent, MariadbInstallLogEvent, MariadbInstallProgressDto } from './ipc';

const MARIADB_SERIES = '11.4';

/** Captures the callbacks the page registers, so a test can fire a real event
 *  payload at them and see where it lands. */
function eventApi() {
	const stops = { installLog: vi.fn(), progress: vi.fn(), initLog: vi.fn() };
	const cbs: {
		installLog?: (ev: MariadbInstallLogEvent) => void;
		progress?: (ev: { tsMs: number; progress: MariadbInstallProgressDto }) => void;
		initLog?: (ev: MariadbInitLogEvent) => void;
	} = {};
	const api: MariadbEventApi = {
		onMariadbInstallLog: async (cb) => {
			cbs.installLog = cb;
			return stops.installLog;
		},
		onMariadbInstallProgress: async (cb) => {
			cbs.progress = cb;
			return stops.progress;
		},
		onMariadbInitLog: async (cb) => {
			cbs.initLog = cb;
			return stops.initLog;
		}
	};
	return { api, cbs, stops };
}

function sink() {
	const installLog: string[] = [];
	const initLog: string[] = [];
	const progress: MariadbInstallProgressDto[] = [];
	const store: MariadbEventSink = {
		appendInstallLog: (line) => installLog.push(line),
		applyInstallProgress: (p) => progress.push(p),
		appendInitLog: (line) => initLog.push(line)
	};
	return { store, installLog, initLog, progress };
}

function uninstallSink(uninstalling = ''): UninstallEventSink & { lines: [string, string][] } {
	const lines: [string, string][] = [];
	return { uninstalling, appendLog: (major, line) => lines.push([major, line]), lines };
}

describe('subscribeMariadbEvents', () => {
	it('delivers every pipeline state to the store, unchanged', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		await subscribeMariadbEvents(api, s.store, uninstallSink(), () => false);
		cbs.progress?.({ tsMs: 1, progress: { kind: 'started', total: 4096 } });
		cbs.progress?.({ tsMs: 2, progress: { kind: 'downloaded', bytes: 1024 } });
		cbs.progress?.({ tsMs: 3, progress: { kind: 'verified' } });
		cbs.progress?.({ tsMs: 4, progress: { kind: 'extracted' } });
		cbs.progress?.({ tsMs: 5, progress: { kind: 'linked' } });
		expect(s.progress).toEqual([
			{ kind: 'started', total: 4096 },
			{ kind: 'downloaded', bytes: 1024 },
			{ kind: 'verified' },
			{ kind: 'extracted' },
			{ kind: 'linked' }
		]);
	});

	it('routes an init log line to the init sink, carrying no major', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		await subscribeMariadbEvents(api, s.store, uninstallSink(), () => false);
		cbs.initLog?.({ tsMs: 1, stream: 'stderr', line: 'temp server exited: auth denied' });
		expect(s.initLog).toEqual(['temp server exited: auth denied']);
		expect(s.installLog).toEqual([]);
	});

	// The dual-purpose channel (design D3): MariaDB's own install progress
	// reports through `onMariadbInstallProgress`, so `onMariadbInstallLog`'s
	// only real producer is an uninstall — routed under MARIADB_SERIES since
	// the wire event itself carries no major.
	it('routes a MariaDB log line to the uninstall dialog while an uninstall owns the channel', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink(MARIADB_SERIES);
		await subscribeMariadbEvents(api, s.store, u, () => false);
		cbs.installLog?.({ tsMs: 1, stream: 'stdout', line: 'Removing packages/mariadb/11.4' });
		expect(u.lines).toEqual([[MARIADB_SERIES, 'Removing packages/mariadb/11.4']]);
		expect(s.installLog).toEqual([]);
	});

	it('falls back to the page sink when no uninstall is in flight', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('');
		await subscribeMariadbEvents(api, s.store, u, () => false);
		cbs.installLog?.({ tsMs: 1, stream: 'stdout', line: 'stray' });
		expect(s.installLog).toEqual(['stray']);
		expect(u.lines).toEqual([]);
	});

	it('re-reads which operation owns the channel on every line, not once at setup', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('');
		await subscribeMariadbEvents(api, s.store, u, () => false);
		cbs.installLog?.({ tsMs: 1, stream: 'stdout', line: 'before' });
		(u as { uninstalling: string }).uninstalling = MARIADB_SERIES;
		cbs.installLog?.({ tsMs: 2, stream: 'stdout', line: 'after' });
		expect(s.installLog).toEqual(['before']);
		expect(u.lines).toEqual([[MARIADB_SERIES, 'after']]);
	});

	it('releases every listener when the disposer runs', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeMariadbEvents(api, s.store, uninstallSink(), () => false);
		release();
		expect(stops.installLog).toHaveBeenCalledTimes(1);
		expect(stops.progress).toHaveBeenCalledTimes(1);
		expect(stops.initLog).toHaveBeenCalledTimes(1);
	});

	it('is idempotent, so a double unmount cannot unlisten twice', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeMariadbEvents(api, s.store, uninstallSink(), () => false);
		release();
		release();
		expect(stops.progress).toHaveBeenCalledTimes(1);
	});

	it('releases everything immediately when the page unmounted mid-registration', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeMariadbEvents(api, s.store, uninstallSink(), () => true);
		expect(stops.installLog).toHaveBeenCalledTimes(1);
		expect(stops.progress).toHaveBeenCalledTimes(1);
		expect(stops.initLog).toHaveBeenCalledTimes(1);
		release();
		expect(stops.progress).toHaveBeenCalledTimes(1);
	});

	it('propagates a failed registration rather than silently listening to nothing', async () => {
		const s = sink();
		const api: MariadbEventApi = {
			onMariadbInstallLog: async () => () => {},
			onMariadbInstallProgress: async () => {
				throw { kind: 'core', message: 'listener could not be registered' };
			},
			onMariadbInitLog: async () => () => {}
		};
		await expect(
			subscribeMariadbEvents(api, s.store, uninstallSink(), () => false)
		).rejects.toMatchObject({ message: 'listener could not be registered' });
	});
});
