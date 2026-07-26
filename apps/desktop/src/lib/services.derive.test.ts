// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { runningCount, pillClass, pendingServiceNames, formatNameList } from './services.derive';
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

describe('pendingServiceNames', () => {
	// `starting` counts: a service coming up has a live child, and reporting
	// "nothing is running" while nginx starts would be a lie the user acts on.
	it('counts running and starting, not stopped or failed', () => {
		expect(
			pendingServiceNames([
				svc('nginx', 'running'),
				svc('PHP-FPM', 'starting'),
				svc('idle', 'stopped'),
				svc('broken', 'failed')
			])
		).toEqual(['nginx', 'PHP-FPM']);
	});

	it('returns nothing when everything is stopped', () => {
		expect(pendingServiceNames([svc('a', 'stopped'), svc('b', 'stopped')])).toEqual([]);
	});
});

describe('formatNameList', () => {
	it('joins one, two and three names the way a sentence would', () => {
		expect(formatNameList(['nginx'])).toBe('nginx');
		expect(formatNameList(['nginx', 'PHP-FPM'])).toBe('nginx and PHP-FPM');
		expect(formatNameList(['a', 'b', 'c'])).toBe('a, b and c');
	});

	// The caller decides whether to render a sentence at all; an empty list must
	// not produce a stray " and " or the word "undefined".
	it('is empty for an empty list', () => {
		expect(formatNameList([])).toBe('');
	});
});
