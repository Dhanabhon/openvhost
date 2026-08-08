// SPDX-License-Identifier: GPL-3.0-or-later
//
// The seam a route test structurally cannot reach. `onMount` never runs under
// `svelte/server`, so before this file the Languages page's subscription could
// be replaced with `() => {}` and every frontend test stayed green — which is
// precisely how `php-install-progress` came to be fully built, throttled,
// DTO-mapped, unit-tested and subscribed to by nobody. That absence now fails
// here.
//
// Vacuity: proven by mutation, twice. Replacing the progress callback's body
// with `undefined` — the exact neuter that stayed green on the Databases page
// before its own listeners file existed — reddens 'delivers every pipeline
// state to the store, unchanged'. Passing a constant major instead of
// `ev.major` reddens 'forwards the event's own major rather than dropping it'.

import { describe, expect, it, vi } from 'vitest';
import {
	subscribeLanguageEvents,
	type LanguagesEventApi,
	type LanguagesEventSink,
	type UninstallEventSink
} from './languages.listeners';
import type { PhpInstallLogEvent, PhpInstallProgressDto, PhpInstallProgressEvent } from './ipc';

/** Captures the callbacks the page registers, so a test can fire a real event
 *  payload at them and see where it lands. */
function eventApi() {
	const stops = { installLog: vi.fn(), progress: vi.fn() };
	const cbs: {
		installLog?: (ev: PhpInstallLogEvent) => void;
		progress?: (ev: PhpInstallProgressEvent) => void;
	} = {};
	const api: LanguagesEventApi = {
		onPhpInstallLog: async (cb) => {
			cbs.installLog = cb;
			return stops.installLog;
		},
		onPhpInstallProgress: async (cb) => {
			cbs.progress = cb;
			return stops.progress;
		}
	};
	return { api, cbs, stops };
}

function sink() {
	const log: [string, string][] = [];
	const progress: [string, PhpInstallProgressDto][] = [];
	const store: LanguagesEventSink = {
		appendLog: (major, line) => log.push([major, line]),
		applyInstallProgress: (major, p) => progress.push([major, p])
	};
	return { store, log, progress };
}

function uninstallSink(uninstalling = ''): UninstallEventSink & { lines: [string, string][] } {
	const lines: [string, string][] = [];
	return { uninstalling, appendLog: (major, line) => lines.push([major, line]), lines };
}

const progressEvent = (
	progress: PhpInstallProgressDto,
	major = '8.4'
): PhpInstallProgressEvent => ({ major, tsMs: 1, progress });

describe('subscribeLanguageEvents', () => {
	// THE regression this file exists for. Nothing subscribed to
	// `php-install-progress` at all, so the day a release is published a user
	// pressing Install on a packaged major would have watched an empty log pane
	// for the length of a download — the packaged route spawns no child process,
	// so `php-install-log` carries nothing for it either.
	it('delivers every pipeline state to the store, unchanged', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		await subscribeLanguageEvents(api, s.store, uninstallSink(), () => false);
		cbs.progress?.(progressEvent({ kind: 'started', total: 4096 }));
		cbs.progress?.(progressEvent({ kind: 'downloaded', bytes: 1024 }));
		cbs.progress?.(progressEvent({ kind: 'verified' }));
		cbs.progress?.(progressEvent({ kind: 'extracted' }));
		cbs.progress?.(progressEvent({ kind: 'linked' }));
		expect(s.progress).toEqual([
			['8.4', { kind: 'started', total: 4096 }],
			['8.4', { kind: 'downloaded', bytes: 1024 }],
			['8.4', { kind: 'verified' }],
			['8.4', { kind: 'extracted' }],
			['8.4', { kind: 'linked' }]
		]);
	});

	// `PhpInstallProgressEvent` carries `major` where MariaDB's does not, because
	// several PHP majors sit side by side and a progress bar has to know which
	// row it belongs to. Dropping it here would hand the store an untagged state
	// and put the attribution back in the page — the exact shape that once
	// rendered a failed 8.4 install's log under the 8.3 row.
	it("forwards the event's own major rather than dropping it", async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		await subscribeLanguageEvents(api, s.store, uninstallSink(), () => false);
		cbs.progress?.(progressEvent({ kind: 'verified' }, '8.3'));
		cbs.progress?.(progressEvent({ kind: 'verified' }, '8.4'));
		expect(s.progress.map(([major]) => major)).toEqual(['8.3', '8.4']);
	});

	it('routes a PHP log line to the uninstall dialog while an uninstall owns the channel', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('8.4');
		await subscribeLanguageEvents(api, s.store, u, () => false);
		cbs.installLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: 'Uninstalling php@8.4' });
		expect(u.lines).toEqual([['8.4', 'Uninstalling php@8.4']]);
		expect(s.log).toEqual([]);
	});

	it('falls back to the page sink when no uninstall is in flight', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('');
		await subscribeLanguageEvents(api, s.store, u, () => false);
		cbs.installLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: '==> Fetching php@8.4' });
		expect(s.log).toEqual([['8.4', '==> Fetching php@8.4']]);
		expect(u.lines).toEqual([]);
	});

	// Which operation owns the shared channel changes while the listener is
	// alive, so the check has to happen at DELIVERY time, not at subscription.
	it('re-reads which operation owns the channel on every line, not once at setup', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		const u = uninstallSink('');
		await subscribeLanguageEvents(api, s.store, u, () => false);
		cbs.installLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: 'before' });
		(u as { uninstalling: string }).uninstalling = '8.4';
		cbs.installLog?.({ major: '8.4', tsMs: 2, stream: 'stdout', line: 'after' });
		expect(s.log).toEqual([['8.4', 'before']]);
		expect(u.lines).toEqual([['8.4', 'after']]);
	});

	// The two channels are different ROUTES, not two views of one: a brew line
	// must never be mistaken for pipeline progress, and vice versa.
	it('keeps the log channel and the progress channel apart', async () => {
		const { api, cbs } = eventApi();
		const s = sink();
		await subscribeLanguageEvents(api, s.store, uninstallSink(), () => false);
		cbs.installLog?.({ major: '8.4', tsMs: 1, stream: 'stdout', line: '==> Pouring php@8.4' });
		expect(s.progress).toEqual([]);
		cbs.progress?.(progressEvent({ kind: 'linked' }));
		expect(s.log).toEqual([['8.4', '==> Pouring php@8.4']]);
	});

	it('releases every listener when the disposer runs', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeLanguageEvents(api, s.store, uninstallSink(), () => false);
		release();
		expect(stops.installLog).toHaveBeenCalledTimes(1);
		expect(stops.progress).toHaveBeenCalledTimes(1);
	});

	it('is idempotent, so a double unmount cannot unlisten twice', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeLanguageEvents(api, s.store, uninstallSink(), () => false);
		release();
		release();
		expect(stops.progress).toHaveBeenCalledTimes(1);
	});

	// The page can unmount while the registrations are still in flight. A
	// listener registered after that would outlive the component with nothing
	// holding its disposer.
	it('releases everything immediately when the page unmounted mid-registration', async () => {
		const { api, stops } = eventApi();
		const s = sink();
		const release = await subscribeLanguageEvents(api, s.store, uninstallSink(), () => true);
		expect(stops.installLog).toHaveBeenCalledTimes(1);
		expect(stops.progress).toHaveBeenCalledTimes(1);
		// And the returned disposer must not fire them a second time.
		release();
		expect(stops.progress).toHaveBeenCalledTimes(1);
	});

	it('propagates a failed registration rather than silently listening to nothing', async () => {
		const s = sink();
		const api: LanguagesEventApi = {
			onPhpInstallLog: async () => () => {},
			onPhpInstallProgress: async () => {
				throw { kind: 'core', message: 'listener could not be registered' };
			}
		};
		await expect(
			subscribeLanguageEvents(api, s.store, uninstallSink(), () => false)
		).rejects.toMatchObject({ message: 'listener could not be registered' });
	});
});
