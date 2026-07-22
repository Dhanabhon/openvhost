// SPDX-License-Identifier: GPL-3.0-or-later
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => invokeMock(...args)
}));

import { coreInfo, listServices } from './index';
import type { ServiceStatus } from './index';
import { ServicesStore } from '../services.svelte';

const sample = {
	appVersion: '0.1.0',
	os: 'macos',
	arch: 'aarch64',
	openvhostHome: '/Users/x/.openvhost'
};

describe('coreInfo', () => {
	beforeEach(() => invokeMock.mockReset());

	it('maps success to CoreInfo', async () => {
		invokeMock.mockResolvedValueOnce(sample);
		await expect(coreInfo()).resolves.toEqual(sample);
		expect(invokeMock).toHaveBeenCalledWith('core_info', { simulateError: null });
	});

	it('maps failure to a thrown IpcError', async () => {
		invokeMock.mockRejectedValueOnce({ kind: 'simulated' });
		await expect(coreInfo(true)).rejects.toEqual({ kind: 'simulated' });
		expect(invokeMock).toHaveBeenCalledWith('core_info', { simulateError: true });
	});

	it('passes a core-variant IpcError through unchanged', async () => {
		invokeMock.mockRejectedValueOnce({ kind: 'core', message: 'home dir unresolvable' });
		await expect(coreInfo()).rejects.toEqual({ kind: 'core', message: 'home dir unresolvable' });
	});

	it('normalizes a non-IpcError throw into a core-variant IpcError', async () => {
		invokeMock.mockRejectedValueOnce(new Error('ipc transport down'));
		await expect(coreInfo()).rejects.toEqual({
			kind: 'core',
			message: 'Error: ipc transport down'
		});
	});

	it('normalizes a plain-string rejection into a core-variant IpcError', async () => {
		invokeMock.mockRejectedValueOnce('transport down');
		await expect(coreInfo()).rejects.toEqual({ kind: 'core', message: 'transport down' });
	});
});

describe('listServices (non-coreInfo wrapper)', () => {
	beforeEach(() => invokeMock.mockReset());

	it('normalizes a plain-string rejection into a banner-safe IpcError', async () => {
		invokeMock.mockRejectedValueOnce('list transport down');
		const caught: unknown = await listServices().catch((e: unknown) => e);
		// Banner-safe: the error banner does `'message' in error`, which throws
		// if `error` is a primitive (the `in` operator requires an object on
		// the right-hand side) — so this only passes once the raw string has
		// actually been normalized into an object.
		expect(() => 'message' in (caught as object)).not.toThrow();
		expect(caught).toEqual({ kind: 'core', message: 'list transport down' });
	});
});

const svc = (id: string, kind: 'stopped' | 'running'): ServiceStatus =>
	({ id, displayName: id, endpoint: null, pid: null, state: { kind } }) as ServiceStatus;

describe('ServicesStore', () => {
	const api = {
		listServices: async () => [svc('demo-ticker', 'stopped')],
		serviceLogTail: async () => [{ tsMs: 1, level: 'info', line: 'seed' }] as never[]
	};

	it('init seeds services and log tail', async () => {
		const store = new ServicesStore(api as never);
		await store.init();
		expect(store.services).toHaveLength(1);
		expect(store.logs[0]?.line).toBe('seed');
	});

	it('applyState replaces the matching service state', async () => {
		const store = new ServicesStore(api as never);
		await store.init();
		store.applyState({ id: 'demo-ticker', state: { kind: 'running' }, detail: null } as never);
		expect(store.services[0]?.state.kind).toBe('running');
	});

	it('applyLog caps the feed at 50', async () => {
		const store = new ServicesStore(api as never);
		for (let i = 0; i < 60; i++) {
			store.applyLog({ id: 'x', tsMs: i, level: 'info', line: `l${i}` } as never);
		}
		expect(store.logs).toHaveLength(50);
		expect(store.logs[0]?.line).toBe('l10');
	});
});
