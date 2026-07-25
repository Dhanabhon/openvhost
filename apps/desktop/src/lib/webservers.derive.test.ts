// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { statusFor, hotReloadLabel } from './webservers.derive';
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
