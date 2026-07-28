// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { statusFor, hotReloadLabel, startStopFor } from './webservers.derive';
import type { ServiceStatus } from '$lib/ipc';
// NOTE: `ServiceState` is NOT exported from `$lib/ipc` (only `ServiceStateEvent`
// and `ServiceStatus`), and `StatusPill` takes `kind: StateKind` rather than a
// state object — so `statusFor` returns the kind STRING, indexed off the
// exported `ServiceStatus` type. That satisfies both without touching the barrel.

const svc = (id: string, kind: 'running' | 'stopped'): ServiceStatus => ({
	id,
	displayName: id,
	endpoint: null,
	pid: kind === 'running' ? 1 : null,
	state: { kind }
});

describe('statusFor', () => {
	it('finds the supervised service a row correlates with', () => {
		expect(statusFor([svc('nginx', 'running')], 'nginx')).toBe('running');
	});

	// Apache has no supervised service, so a row with no serviceId must render
	// "no status" rather than borrowing another row's state.
	it('is null for a row that is not a supervised service', () => {
		expect(statusFor([svc('nginx', 'running')], null)).toBeNull();
	});

	it('is null when the service is not in the snapshot yet', () => {
		expect(statusFor([], 'nginx')).toBeNull();
	});
});

describe('hotReloadLabel', () => {
	it('states support plainly in both directions', () => {
		expect(hotReloadLabel(true)).toBe('Supported');
		expect(hotReloadLabel(false)).toBe('Not supported');
	});
});

describe('startStopFor', () => {
	it('renders no control at all while the state is unknown', () => {
		// `statusFor` returns null for the first frame of EVERY visit — the route
		// fires load() and the shared subscription resolves after the first paint.
		// A Start button here would be the page asserting nginx is stopped before
		// it has asked, and the user would be one click from starting something
		// whose state they were never shown.
		expect(startStopFor(null, true)).toEqual({ kind: 'none' });
		expect(startStopFor(null, false)).toEqual({ kind: 'none' });
	});

	it('offers Start when stopped with a config to start against', () => {
		expect(startStopFor('stopped', true)).toEqual({
			kind: 'start',
			disabled: false,
			reason: ''
		});
	});

	it('disables Start with a reason when there is no config yet', () => {
		// nginx spawns with `-c <config>`; without the file it exits immediately.
		expect(startStopFor('stopped', false)).toEqual({
			kind: 'start',
			disabled: true,
			reason: 'No config generated yet — apply your changes first.'
		});
	});

	it('offers Retry after a failure, and does not re-disable it', () => {
		// A failed service HAS been started, so a config existed at least once.
		// Disabling Retry on a stale `configExists: false` would strand the user
		// on a row whose own error text is telling them to try again.
		expect(startStopFor('failed', true)).toEqual({ kind: 'retry' });
		expect(startStopFor('failed', false)).toEqual({ kind: 'retry' });
	});

	it('offers Stop while running or still starting', () => {
		// `starting` gets Stop, not nothing: a start that hangs must be
		// interruptible, or the only way out is quitting the app.
		expect(startStopFor('running', true)).toEqual({ kind: 'stop' });
		expect(startStopFor('starting', true)).toEqual({ kind: 'stop' });
	});

	it('never disables Stop on a missing config', () => {
		// The process is running. Whether a file is on disk has no bearing on
		// whether the user may stop it.
		expect(startStopFor('running', false)).toEqual({ kind: 'stop' });
		expect(startStopFor('starting', false)).toEqual({ kind: 'stop' });
	});
});
