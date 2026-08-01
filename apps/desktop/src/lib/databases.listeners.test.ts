// SPDX-License-Identifier: GPL-3.0-or-later
//
// The seam a route test structurally cannot reach. `onMount` never runs under
// `svelte/server`, so before this file the Databases page's progress
// subscription could be replaced with `() => {}` — an install would show
// "Preparing the download…" forever — and all 1000+ frontend tests stayed
// green. That mutation now fails here.

import { describe, expect, it, vi } from 'vitest';
import {
	subscribeDatabaseEvents,
	type DatabasesEventApi,
	type DatabasesEventSink,
	type UninstallEventSink
} from './databases.listeners';
import type {
	MysqlInitLogEvent,
	MysqlInstallLogEvent,
	MysqlInstallProgressDto,
	MysqlInstallProgressEvent
} from './ipc';

/** Captures the callbacks the page registers, so a test can fire a real event
 *  payload at them and see where it lands. */
function eventApi() {
	const stops = { installLog: vi.fn(), progress: vi.fn(), initLog: vi.fn() };
	const cbs: {
		installLog?: (ev: MysqlInstallLogEvent) => void;
		progress?: (ev: MysqlInstallProgressEvent) => void;
		initLog?: (ev: MysqlInitLogEvent) => void;
	} = {};
	const api: DatabasesEventApi = {
		onMysqlInstallLog: async (cb) => {
			cbs.installLog = cb;
			return stops.installLog;
		},
		onMysqlInstallProgress: async (cb) => {
			cbs.progress = cb;
			return stops.progress;
		},
		onMysqlInitLog: async (cb) => {
			cbs.initLog = cb;
			return stops.initLog;
		}
	};
	return { api, cbs, stops };
}

function sink() {
	const installLog: [string, string][] = [];
	const initLog: [string, string][] = [];
	const progress: MysqlInstallProgressDto[] = [];
	const store: DatabasesEventSink = {
		appendInstallLog: (major, line) => installLog.push([major, line]),
		applyInstallProgress: (p) => progress.push(p),
		appendInitLog: (major, line) => initLog.push([major, line])
	};
	return { store, installLog, initLog, progress };
}

function uninstallSink(uninstalling = ''): UninstallEventSink & { lines: [string, string][] } {
	const lines: [string, string][] = [];
	return { uninstalling, appendLog: (major, line) => lines.push([major, line]), lines };
}

const progressEvent = (progress: MysqlInstallProgressDto): MysqlInstallProgressEvent => ({
	major: '8.4',
	tsMs: 1,
	progress
});

describe('subscribeDatabaseEvents', () => {
	// THE regression. A severed callback here means the row shows "Preparing
	// the download…" for the entire install and the user never sees that the
	// checksum was checked.
	it('delivers every pipeline state to the store, unchanged', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		await subscribeDatabaseEvents(api, s.store, uninstallSink(), () => false);
		cbs.progress?.(progressEvent({ kind: 'started', total: 4096 }));
		cbs.progress?.(progressEvent({ kind: 'downloaded', bytes: 1024 }));
		cbs.progress?.(progressEvent({ kind: 'verified' }));
		cbs.progress?.(progressEvent({ kind: 'extracted' }));
		cbs.progress?.(progressEvent({ kind: 'linked' }));
		expect(s.progress).toEqual([
			{ kind: 'started', total: 4096 },
			{ kind: 'downloaded', bytes: 1024 },
			{ kind: 'verified' },
			{ kind: 'extracted' },
			{ kind: 'linked' }
		]);
	});

	it('routes an init log line to the init sink', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		await subscribeDatabaseEvents(api, s.store, uninstallSink(), () => false);
		cbs.initLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: 'rendering my.cnf' });
		expect(s.initLog).toEqual([['8.4', 'rendering my.cnf']]);
		expect(s.installLog).toEqual([]);
	});

	it('routes a MySQL log line to the uninstall dialog while an uninstall owns the channel', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('8.4');
		await subscribeDatabaseEvents(api, s.store, u, () => false);
		cbs.installLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: 'Uninstalling mysql@8.4' });
		expect(u.lines).toEqual([['8.4', 'Uninstalling mysql@8.4']]);
		expect(s.installLog).toEqual([]);
	});

	it('falls back to the page sink when no uninstall is in flight', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('');
		await subscribeDatabaseEvents(api, s.store, u, () => false);
		cbs.installLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: 'stray' });
		expect(s.installLog).toEqual([['8.4', 'stray']]);
		expect(u.lines).toEqual([]);
	});

	// Which operation owns the shared channel changes while the listener is
	// alive, so the check has to happen at DELIVERY time, not at subscription.
	it('re-reads which operation owns the channel on every line, not once at setup', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('');
		await subscribeDatabaseEvents(api, s.store, u, () => false);
		cbs.installLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: 'before' });
		(u as { uninstalling: string }).uninstalling = '8.4';
		cbs.installLog?.({ major: '8.4', tsMs: 2, stream: 'stdout', line: 'after' });
		expect(s.installLog).toEqual([['8.4', 'before']]);
		expect(u.lines).toEqual([['8.4', 'after']]);
	});

	it('releases every listener when the disposer runs', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeDatabaseEvents(api, s.store, uninstallSink(), () => false);
		release();
		expect(stops.installLog).toHaveBeenCalledTimes(1);
		expect(stops.progress).toHaveBeenCalledTimes(1);
		expect(stops.initLog).toHaveBeenCalledTimes(1);
	});

	it('is idempotent, so a double unmount cannot unlisten twice', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeDatabaseEvents(api, s.store, uninstallSink(), () => false);
		release();
		release();
		expect(stops.progress).toHaveBeenCalledTimes(1);
	});

	// The page can unmount while the three registrations are still in flight.
	// A listener registered after that would outlive the component with nothing
	// holding its disposer.
	it('releases everything immediately when the page unmounted mid-registration', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeDatabaseEvents(api, s.store, uninstallSink(), () => true);
		expect(stops.installLog).toHaveBeenCalledTimes(1);
		expect(stops.progress).toHaveBeenCalledTimes(1);
		expect(stops.initLog).toHaveBeenCalledTimes(1);
		// And the returned disposer must not fire them a second time.
		release();
		expect(stops.progress).toHaveBeenCalledTimes(1);
	});

	it('propagates a failed registration rather than silently listening to nothing', async () => {
		const s = sink();
		const api: DatabasesEventApi = {
			onMysqlInstallLog: async () => () => {},
			onMysqlInstallProgress: async () => {
				throw { kind: 'core', message: 'listener could not be registered' };
			},
			onMysqlInitLog: async () => () => {}
		};
		await expect(
			subscribeDatabaseEvents(api, s.store, uninstallSink(), () => false)
		).rejects.toMatchObject({ message: 'listener could not be registered' });
	});
});
