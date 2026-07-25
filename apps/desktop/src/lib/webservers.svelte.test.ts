// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { WebServersStore, type WebServersApi } from './webservers.svelte';
import type { WebServerDto } from '$lib/ipc';

const nginx: WebServerDto = {
	id: 'nginx',
	displayName: 'nginx',
	supported: true,
	serviceId: 'nginx',
	binaryPath: '/opt/homebrew/opt/nginx/bin/nginx',
	version: '1.27.3',
	supportsHotReload: true,
	configPath: '/home/.openvhost/conf/nginx.conf'
};

function api(over: Partial<WebServersApi> = {}): WebServersApi {
	return {
		listWebServers: vi.fn(async () => [nginx]),
		readWebServerConfig: vi.fn(async () => 'daemon off;'),
		validateWebServerConfig: vi.fn(async () => ({ ok: true, stderr: 'syntax is ok' })),
		...over
	};
}

describe('WebServersStore', () => {
	it('loads rows', async () => {
		const store = new WebServersStore(api());
		await store.load();
		expect(store.servers).toEqual([nginx]);
		expect(store.error).toBeNull();
	});

	it('renders a load failure instead of showing an empty page', async () => {
		const store = new WebServersStore(
			api({
				listWebServers: vi.fn(async () => {
					throw { kind: 'core', message: 'boom' };
				})
			})
		);
		await store.load();
		expect(store.error).toEqual({ kind: 'core', message: 'boom' });
		expect(store.servers).toEqual([]);
	});

	it('keeps a config read failure on the row, not the page banner', async () => {
		const store = new WebServersStore(
			api({
				readWebServerConfig: vi.fn(async () => {
					throw { kind: 'core', message: 'no such file' };
				})
			})
		);
		await store.showConfig('nginx');
		expect(store.configError.nginx).toContain('no such file');
		// A per-row failure must not blank the whole page.
		expect(store.error).toBeNull();
	});

	it('exposes the validator stderr verbatim', async () => {
		const store = new WebServersStore(
			api({
				validateWebServerConfig: vi.fn(async () => ({
					ok: false,
					stderr: 'nginx: [emerg] unknown directive "bogus"'
				}))
			})
		);
		await store.validate('nginx');
		expect(store.reports.nginx.ok).toBe(false);
		expect(store.reports.nginx.stderr).toBe('nginx: [emerg] unknown directive "bogus"');
	});

	// A spawn failure is an IpcError, not a report. It must still surface — and it
	// must land on the ROW, so assert that channel specifically rather than
	// accepting either one (an assertion that can pass two ways pins neither).
	it('surfaces a validator that could not be launched, on the row', async () => {
		const store = new WebServersStore(
			api({
				validateWebServerConfig: vi.fn(async () => {
					throw { kind: 'core', message: 'could not be launched' };
				})
			})
		);
		await store.validate('nginx');
		expect(store.configError.nginx).toContain('could not be launched');
		expect(store.error).toBeNull();
		expect(store.reports.nginx).toBeUndefined();
	});

	it('clears the validating flag even when validation throws', async () => {
		const store = new WebServersStore(
			api({
				validateWebServerConfig: vi.fn(async () => {
					throw { kind: 'core', message: 'x' };
				})
			})
		);
		await store.validate('nginx');
		expect(store.validating.nginx).not.toBe(true);
	});
});
