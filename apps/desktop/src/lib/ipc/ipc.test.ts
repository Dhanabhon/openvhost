// SPDX-License-Identifier: GPL-3.0-or-later
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => invokeMock(...args)
}));

import { coreInfo } from './index';

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
