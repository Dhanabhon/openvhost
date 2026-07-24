// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { runningCount, pillClass } from './services.derive';
import type { ServiceStatus } from './ipc';

const svc = (id: string, kind: string): ServiceStatus =>
	({ id, displayName: id, endpoint: null, pid: null, state: { kind } }) as unknown as ServiceStatus;

describe('runningCount', () => {
	it('counts only running services', () => {
		expect(runningCount([svc('a', 'running'), svc('b', 'stopped'), svc('c', 'running')])).toBe(2);
	});
});
describe('pillClass', () => {
	it('maps each state kind to its pill modifier', () => {
		expect(pillClass('running')).toBe('pill-running');
		expect(pillClass('starting')).toBe('pill-starting');
		expect(pillClass('failed')).toBe('pill-failed');
		expect(pillClass('stopped')).toBe('pill-stopped');
	});
});
