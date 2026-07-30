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

import {
	applyConfig,
	coreInfo,
	installPhp,
	listServices,
	onPhpInstallLog,
	onServiceState,
	pendingInstall,
	phpEnvironment,
	planConfigApply,
	rescanPhpRuntimes,
	saveWebServerSettings,
	webServerSettings
} from './index';
import type { WebServerSettingsDto } from './index';

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

describe('planConfigApply', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns the plan data on success', async () => {
		const plan = { changes: [{ path: '/tmp/ovh/nginx.conf', kind: 'modified', diff: '- a\n+ b' }] };
		invokeMock.mockResolvedValueOnce(plan);
		await expect(planConfigApply()).resolves.toEqual(plan);
		expect(invokeMock).toHaveBeenCalledWith('plan_config_apply');
	});

	it('throws the normalized IpcError on failure', async () => {
		invokeMock.mockRejectedValueOnce({
			kind: 'core',
			message: 'site legacy needs PHP 7.4, which is not installed (installed: 8.4)'
		});
		await expect(planConfigApply()).rejects.toEqual({
			kind: 'core',
			message: 'site legacy needs PHP 7.4, which is not installed (installed: 8.4)'
		});
	});

	it('normalizes a non-IpcError throw into a core-variant IpcError', async () => {
		invokeMock.mockRejectedValueOnce(new Error('ipc transport down'));
		await expect(planConfigApply()).rejects.toEqual({
			kind: 'core',
			message: 'Error: ipc transport down'
		});
	});
});

describe('applyConfig', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns the outcome data on success', async () => {
		const outcome = {
			applied: 3,
			restarted: ['php-fpm-8.4', 'nginx'],
			notStarted: [],
			needsAttention: []
		};
		invokeMock.mockResolvedValueOnce(outcome);
		await expect(applyConfig()).resolves.toEqual(outcome);
		expect(invokeMock).toHaveBeenCalledWith('apply_config');
	});

	it('throws the normalized IpcError on failure', async () => {
		invokeMock.mockRejectedValueOnce({ kind: 'core', message: 'apply failed' });
		await expect(applyConfig()).rejects.toEqual({ kind: 'core', message: 'apply failed' });
	});
});

const settings: WebServerSettingsDto = {
	workerConnections: 2048,
	clientMaxBodySize: '512m',
	keepaliveTimeout: 30,
	tcpNodelay: false,
	fastcgiConnectTimeout: 10,
	fastcgiSendTimeout: 120,
	fastcgiReadTimeout: 900,
	gzip: true,
	gzipCompLevel: 6,
	gzipTypes: 'text/css application/json'
};

describe('webServerSettings', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns the stored settings on success', async () => {
		invokeMock.mockResolvedValueOnce(settings);
		await expect(webServerSettings()).resolves.toEqual(settings);
		expect(invokeMock).toHaveBeenCalledWith('web_server_settings');
	});

	it('throws the normalized IpcError on failure', async () => {
		// A hand-edited state.db row is rejected on read, and the failure has
		// to reach the page rather than leaving the form blank and silent.
		invokeMock.mockRejectedValueOnce({
			kind: 'validation',
			field: 'gzip_comp_level',
			message: '"99" must be between 1 and 9'
		});
		await expect(webServerSettings()).rejects.toEqual({
			kind: 'validation',
			field: 'gzip_comp_level',
			message: '"99" must be between 1 and 9'
		});
	});
});

describe('saveWebServerSettings', () => {
	beforeEach(() => invokeMock.mockReset());

	it('sends the whole DTO under `input`', async () => {
		invokeMock.mockResolvedValueOnce(null);
		await expect(saveWebServerSettings(settings)).resolves.toBeUndefined();
		expect(invokeMock).toHaveBeenCalledWith('save_web_server_settings', { input: settings });
	});

	it('surfaces a validation error naming the field, so the form can mark it', async () => {
		// snake_case on purpose: `fieldErrors` is keyed by the BACKEND's field
		// names, the same convention the site editor already uses. A camelCase
		// key here would mark nothing.
		invokeMock.mockRejectedValueOnce({
			kind: 'validation',
			field: 'fastcgi_read_timeout',
			message: '"0" must be between 1 and 86400'
		});
		await expect(saveWebServerSettings({ ...settings, fastcgiReadTimeout: 0 })).rejects.toEqual({
			kind: 'validation',
			field: 'fastcgi_read_timeout',
			message: '"0" must be between 1 and 86400'
		});
	});

	it('normalizes a non-IpcError throw into a core-variant IpcError', async () => {
		invokeMock.mockRejectedValueOnce(new Error('ipc transport down'));
		await expect(saveWebServerSettings(settings)).rejects.toEqual({
			kind: 'core',
			message: 'Error: ipc transport down'
		});
	});
});

// `ServicesStore`'s own tests live in `../services.svelte.test.ts` (alongside
// `sites.svelte.test.ts`) — this file is about the IPC barrel.

describe('phpEnvironment', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns the environment data on success', async () => {
		const env = {
			brewFound: true,
			brewSearched: ['/opt/homebrew/bin/brew', '/usr/local/bin/brew'],
			runtimes: [
				{
					major: '8.3',
					installed: true,
					recommended: false,
					fullVersion: '8.3',
					path: '/opt/homebrew/opt/php@8.3/sbin/php-fpm',
					socketPath: 'run/php-fpm-8.3.sock',
					serviceId: 'php-fpm-8.3'
				}
			]
		};
		invokeMock.mockResolvedValueOnce(env);
		await expect(phpEnvironment()).resolves.toEqual(env);
		expect(invokeMock).toHaveBeenCalledWith('php_environment');
	});

	it('throws the normalized IpcError on failure', async () => {
		invokeMock.mockRejectedValueOnce({
			kind: 'core',
			message: 'no web server stack is configured for this platform'
		});
		await expect(phpEnvironment()).rejects.toEqual({
			kind: 'core',
			message: 'no web server stack is configured for this platform'
		});
	});

	it('normalizes a non-IpcError throw into a core-variant IpcError', async () => {
		invokeMock.mockRejectedValueOnce(new Error('ipc transport down'));
		await expect(phpEnvironment()).rejects.toEqual({
			kind: 'core',
			message: 'Error: ipc transport down'
		});
	});
});

describe('rescanPhpRuntimes', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns the environment data on success', async () => {
		const env = { brewFound: false, brewSearched: ['/opt/homebrew/bin/brew'], runtimes: [] };
		invokeMock.mockResolvedValueOnce(env);
		await expect(rescanPhpRuntimes()).resolves.toEqual(env);
		expect(invokeMock).toHaveBeenCalledWith('rescan_php_runtimes');
	});

	it('throws the normalized IpcError on failure', async () => {
		invokeMock.mockRejectedValueOnce({ kind: 'core', message: 'runtime list is poisoned' });
		await expect(rescanPhpRuntimes()).rejects.toEqual({
			kind: 'core',
			message: 'runtime list is poisoned'
		});
	});
});

// Review fix wave, Important 1: replaces the old PHP-only `pendingPhpInstall`
// (which had no test coverage of its own), so the quit dialog's kind-agnostic
// signal is proven at the IPC boundary too, not only in the Rust command
// tests and QuitDialog's own SSR tests.
describe('pendingInstall', () => {
	beforeEach(() => invokeMock.mockReset());

	it('returns null when nothing is installing', async () => {
		invokeMock.mockResolvedValueOnce(null);
		await expect(pendingInstall()).resolves.toBeNull();
		expect(invokeMock).toHaveBeenCalledWith('pending_install');
	});

	it('returns the kind and label of a PHP install in flight', async () => {
		const info = { kind: 'php', label: '8.4' };
		invokeMock.mockResolvedValueOnce(info);
		await expect(pendingInstall()).resolves.toEqual(info);
	});

	it('returns the kind and label of a MySQL install in flight', async () => {
		const info = { kind: 'mysql', label: 'MySQL 8.4' };
		invokeMock.mockResolvedValueOnce(info);
		await expect(pendingInstall()).resolves.toEqual(info);
	});

	it('throws the normalized IpcError on failure', async () => {
		invokeMock.mockRejectedValueOnce({ kind: 'core', message: 'poisoned' });
		await expect(pendingInstall()).rejects.toEqual({ kind: 'core', message: 'poisoned' });
	});
});

describe('installPhp', () => {
	beforeEach(() => invokeMock.mockReset());

	it('passes the major through and returns the outcome on success', async () => {
		const outcome = { major: '8.4', exitCode: 0, detected: true };
		invokeMock.mockResolvedValueOnce(outcome);
		await expect(installPhp('8.4')).resolves.toEqual(outcome);
		expect(invokeMock).toHaveBeenCalledWith('install_php', { major: '8.4' });
	});

	it('surfaces exitCode 0 with detected false rather than hiding it', async () => {
		// The silent-failure case this project keeps catching: brew reports
		// success but the version never appears. The wrapper must not paper
		// over that combination — it is exactly what the caller has to render.
		const outcome = { major: '8.4', exitCode: 0, detected: false };
		invokeMock.mockResolvedValueOnce(outcome);
		await expect(installPhp('8.4')).resolves.toEqual(outcome);
	});

	it('throws the normalized IpcError when an install is already running', async () => {
		invokeMock.mockRejectedValueOnce({
			kind: 'core',
			message: 'an install is already running'
		});
		await expect(installPhp('8.4')).rejects.toEqual({
			kind: 'core',
			message: 'an install is already running'
		});
	});

	it('throws a Validation IpcError naming php_version for a rejected version', async () => {
		invokeMock.mockRejectedValueOnce({
			kind: 'validation',
			field: 'php_version',
			message: '"--build-from-source" is not a major.minor version'
		});
		await expect(installPhp('--build-from-source')).rejects.toEqual({
			kind: 'validation',
			field: 'php_version',
			message: '"--build-from-source" is not a major.minor version'
		});
	});
});

describe('onPhpInstallLog', () => {
	beforeEach(() => listenMock.mockReset());

	it('resolves to the unlisten function the transport returns', async () => {
		const unlisten = vi.fn();
		listenMock.mockResolvedValueOnce(unlisten);
		await expect(onPhpInstallLog(() => {})).resolves.toBe(unlisten);
		expect(listenMock).toHaveBeenCalledWith('php-install-log-event', expect.any(Function));
	});

	it('delivers the event payload to the callback', async () => {
		const seen: unknown[] = [];
		listenMock.mockImplementationOnce(async (_name: string, cb: (e: unknown) => void) => {
			cb({
				event: 'php-install-log-event',
				id: 1,
				payload: { major: '8.4', tsMs: 1234, stream: 'stdout', line: '==> Installing php@8.4' }
			});
			return vi.fn();
		});
		await onPhpInstallLog((ev) => seen.push(ev));
		expect(seen).toEqual([
			{ major: '8.4', tsMs: 1234, stream: 'stdout', line: '==> Installing php@8.4' }
		]);
	});

	it('normalizes a raw transport failure into a core-variant IpcError', async () => {
		listenMock.mockRejectedValueOnce(new Error('event transport down'));
		await expect(onPhpInstallLog(() => {})).rejects.toEqual({
			kind: 'core',
			message: 'Error: event transport down'
		});
	});
});

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
