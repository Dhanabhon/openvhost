// SPDX-License-Identifier: GPL-3.0-or-later
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => invokeMock(...args)
}));

const listenMock = vi.fn();
vi.mock('@tauri-apps/api/event', () => ({
	listen: (...args: unknown[]) => listenMock(...args),
	once: vi.fn(),
	emit: vi.fn()
}));

import { applySites, coreInfo, listServices, onServiceState, planSiteApply } from './index';

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

describe('planSiteApply', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns the plan data on success', async () => {
		const plan = { changes: [{ path: '/tmp/ovh/nginx.conf', kind: 'modified', diff: '- a\n+ b' }] };
		invokeMock.mockResolvedValueOnce(plan);
		await expect(planSiteApply()).resolves.toEqual(plan);
		expect(invokeMock).toHaveBeenCalledWith('plan_site_apply');
	});

	it('throws the normalized IpcError on failure', async () => {
		invokeMock.mockRejectedValueOnce({
			kind: 'core',
			message: 'site legacy needs PHP 7.4, which is not installed (installed: 8.4)'
		});
		await expect(planSiteApply()).rejects.toEqual({
			kind: 'core',
			message: 'site legacy needs PHP 7.4, which is not installed (installed: 8.4)'
		});
	});

	it('normalizes a non-IpcError throw into a core-variant IpcError', async () => {
		invokeMock.mockRejectedValueOnce(new Error('ipc transport down'));
		await expect(planSiteApply()).rejects.toEqual({
			kind: 'core',
			message: 'Error: ipc transport down'
		});
	});
});

describe('applySites', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns the outcome data on success', async () => {
		const outcome = { applied: 3, restarted: ['php-fpm-8.4', 'nginx'], notStarted: [] };
		invokeMock.mockResolvedValueOnce(outcome);
		await expect(applySites()).resolves.toEqual(outcome);
		expect(invokeMock).toHaveBeenCalledWith('apply_sites');
	});

	it('throws the normalized IpcError on failure', async () => {
		invokeMock.mockRejectedValueOnce({ kind: 'core', message: 'apply failed' });
		await expect(applySites()).rejects.toEqual({ kind: 'core', message: 'apply failed' });
	});
});

// `ServicesStore`'s own tests live in `../services.svelte.test.ts` (alongside
// `sites.svelte.test.ts`) — this file is about the IPC barrel.

describe('onServiceState', () => {
	beforeEach(() => listenMock.mockReset());

	it('resolves to the unlisten function the transport returns', async () => {
		const unlisten = vi.fn();
		listenMock.mockResolvedValueOnce(unlisten);
		await expect(onServiceState(() => {})).resolves.toBe(unlisten);
		expect(listenMock).toHaveBeenCalledWith('service-state-event', expect.any(Function));
	});

	it('delivers the event payload to the callback', async () => {
		const seen: unknown[] = [];
		listenMock.mockImplementationOnce(async (_name: string, cb: (e: unknown) => void) => {
			cb({ event: 'service-state-event', id: 1, payload: { id: 'nginx' } });
			return vi.fn();
		});
		await onServiceState((ev) => seen.push(ev));
		expect(seen).toEqual([{ id: 'nginx' }]);
	});

	// `events.*.listen` reaches the transport directly rather than through the
	// `unwrap` helper the commands use, so without explicit normalization a failed
	// subscription rejects with a raw Error and the banner's `'message' in error`
	// read (and the `IpcError` type) no longer hold.
	it('normalizes a raw transport failure into a core-variant IpcError', async () => {
		listenMock.mockRejectedValueOnce(new Error('event transport down'));
		await expect(onServiceState(() => {})).rejects.toEqual({
			kind: 'core',
			message: 'Error: event transport down'
		});
	});

	it('passes an IpcError-shaped rejection through unchanged', async () => {
		listenMock.mockRejectedValueOnce({ kind: 'proc', message: 'supervisor gone' });
		await expect(onServiceState(() => {})).rejects.toEqual({
			kind: 'proc',
			message: 'supervisor gone'
		});
	});
});
