// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { WebServersStore, type WebServersApi } from './webservers.svelte';
import type { ValidationReportDto, WebServerDto } from '$lib/ipc';

const nginx: WebServerDto = {
	id: 'nginx',
	displayName: 'nginx',
	supported: true,
	serviceId: 'nginx',
	binaryPath: '/opt/homebrew/opt/nginx/bin/nginx',
	version: '1.27.3',
	source: null,
	supportsHotReload: true,
	configPath: '/home/.openvhost/conf/nginx.conf',
	configExists: true
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

	// `showConfig` clears this row's error as the read STARTS, and WebServerRow's
	// `toggleConfig` documents that clear as "the retry path after a failed read".
	// Losing it is permanent, not cosmetic: the row derives
	// `showConfig = configText !== undefined && configError === '' && !collapsed`, so a
	// row whose `configError[id]` is never cleared can NEVER display config text again,
	// however many times the user clicks. Deleting the clear left 129/129 green.
	it('clears the row error when the next read starts, so a failed read can be retried', async () => {
		let failing = true;
		const store = new WebServersStore(
			api({
				readWebServerConfig: vi.fn(async () => {
					if (failing) throw { kind: 'core', message: 'no such file' };
					return 'daemon off; worker_processes 1;';
				})
			})
		);
		await store.showConfig('nginx');
		expect(store.configError.nginx).toContain('no such file');

		failing = false;
		await store.showConfig('nginx');
		// BOTH halves are needed: text alone is not enough to reveal the block, because
		// a lingering error suppresses it.
		expect(store.configError.nginx).toBe('');
		expect(store.configText.nginx).toBe('daemon off; worker_processes 1;');
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

	// The failure this pins: validate once successfully (green "Config is valid"),
	// then have the binary move away so the next click cannot even LAUNCH the
	// validator. That is an `IpcError`, not a report with `ok: false`, so the store
	// writes the row error and — before this fix — never touched `reports[id]`. The
	// row then showed a fresh red "could not be launched" beside the earlier green
	// verdict: two statements about the SAME operation with nothing marking one
	// stale. Splitting the error channel would NOT fix that; dropping the verdict
	// when the run starts does. Both the mid-flight and the settled state are
	// asserted, because clearing in the `catch` instead would leave the stale
	// verdict on screen under "Validating…".
	it('drops the previous verdict when the next validate cannot be launched', async () => {
		let canLaunch = true;
		// Sentinel, not `undefined`: this must fail if the run never happened at all,
		// rather than pass because nothing was ever observed.
		let verdictWhileRunning: ValidationReportDto | undefined | 'the run never happened' =
			'the run never happened';
		const store = new WebServersStore(
			api({
				validateWebServerConfig: vi.fn(async () => {
					if (canLaunch) return { ok: true, stderr: 'syntax is ok' };
					// Read from INSIDE the in-flight run.
					verdictWhileRunning = store.reports.nginx;
					throw { kind: 'core', message: 'nginx binary not found at /opt/x/bin/nginx' };
				})
			})
		);

		await store.validate('nginx');
		expect(store.reports.nginx.ok).toBe(true);

		canLaunch = false;
		await store.validate('nginx');

		expect(verdictWhileRunning).toBeUndefined();
		expect(store.reports.nginx).toBeUndefined();
		expect(store.configError.nginx).toContain('nginx binary not found at /opt/x/bin/nginx');
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
