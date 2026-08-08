// SPDX-License-Identifier: GPL-3.0-or-later
//
// The ASSEMBLED landing page, mounted for real: `onMount` runs, both reads go
// out through the generated Tauri client, and the banner is asserted off the
// live DOM.
//
// This file exists because the parts passing is not the same as the product
// working — a lesson this project paid for once already (five UI-glue defects
// that every per-part test was blind to). `site-readiness.derive.test.ts` proves
// the rule and `SiteReadinessBanner.svelte.test.ts` proves the markup; only this
// one can fail when the page never asks about nginx at all, which is precisely
// the defect the slice removes.
//
// The seam is mocked at `@tauri-apps/api/core`'s `invoke`, NOT at `$lib/ipc`
// (the pattern `lib/ipc/ipc.test.ts` established). Everything above the wire is
// therefore the real thing: a page that called the wrong command, or read a
// field the DTO does not have, fails here.
//
// Runs under the `dom` (jsdom) vitest project — `svelte/server` never runs
// `onMount`, so no SSR test can reach any of these states.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/** Commands with no handler in the current test. Asserted empty, so a typo in a
 *  command name shows up as its own failure instead of silently arriving as a
 *  "the read failed" banner — which would look exactly like a passing §7.6. */
let unhandled: string[] = [];
let handlers: Record<string, () => unknown> = {};

const invokeMock = vi.fn(async (cmd: string) => {
	const handler = handlers[cmd];
	if (handler === undefined) {
		unhandled.push(cmd);
		// A plain object, not an Error: the generated `typedError` rethrows real
		// `Error`s and would escape the page's own catch.
		throw { kind: 'core', message: `no handler for ${cmd}` };
	}
	return handler();
});

vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => invokeMock(...(args as [string]))
}));
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
	once: vi.fn(async () => () => {}),
	emit: vi.fn(async () => {})
}));

import { mount, tick, unmount } from 'svelte';
import SitesPage from './+page.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import type { PhpEnvironmentDto, WebServerDto } from '$lib/ipc';

interface Deferred<T> {
	readonly promise: Promise<T>;
	resolve: (value: T) => void;
	reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
	let resolve!: (value: T) => void;
	let reject!: (reason: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

/** An IPC rejection as the wire delivers it: a plain `IpcError` object, never an
 *  `Error` instance (see `typedError` in the generated bindings). */
const IPC_FAILURE = { kind: 'core', message: 'command failed' };

function phpEnv(installedMajors: readonly string[]): PhpEnvironmentDto {
	return {
		brewFound: true,
		brewSearched: ['/opt/homebrew/bin/brew'],
		runtimes: ['8.1', '8.4'].map((major) => {
			const installed = installedMajors.includes(major);
			return {
				major,
				installed,
				cataloged: true,
				recommended: false,
				fullVersion: installed ? `${major}.0` : null,
				path: installed ? `/opt/homebrew/opt/php@${major}/sbin/php-fpm` : null,
				socketPath: installed ? `/tmp/php-fpm-${major}.sock` : null,
				serviceId: installed ? `php-fpm-${major}` : null,
				source: installed ? { kind: 'homebrew' as const } : null,
				offer: { kind: 'unavailable' as const, target: 'macos-arm64' }
			};
		}),
		defaultPhp:
			installedMajors.length === 0
				? { kind: 'nothingInstalled' }
				: { kind: 'unset', serving: installedMajors[0] }
	};
}

/** The list `list_web_servers` returns — nginx plus the always-present, always
 *  unsupported Apache row, whose `binaryPath` is `null` too. */
function webServers(nginxBinary: string | null): WebServerDto[] {
	return [
		{
			id: 'nginx',
			displayName: 'nginx',
			supported: true,
			serviceId: 'nginx',
			binaryPath: nginxBinary,
			version: nginxBinary === null ? null : '1.30.4',
			source: nginxBinary === null ? null : { kind: 'packaged', version: '1.30.4' },
			supportsHotReload: true,
			configPath: '/Users/x/.openvhost/etc/nginx/nginx.conf',
			configExists: true
		},
		{
			id: 'apache',
			displayName: 'Apache',
			supported: false,
			serviceId: null,
			binaryPath: null,
			version: null,
			source: null,
			supportsHotReload: false,
			configPath: null,
			configExists: false
		}
	];
}

const NGINX_PATH = '/Users/x/.openvhost/pkg/nginx/1.30.4/sbin/nginx';

/** The two reads this page makes that are nothing to do with readiness. Both
 *  succeed in every test here so neither can be the reason a banner is or is
 *  not on screen. */
function baseHandlers(): Record<string, () => unknown> {
	return {
		list_sites: () => [],
		plan_config_apply: () => ({ changes: [] })
	};
}

let host: HTMLElement;
let instance: object | null = null;

beforeEach(() => {
	unhandled = [];
	handlers = baseHandlers();
	servicesStore.services = [];
	servicesStore.error = null;
	host = document.createElement('div');
	document.body.appendChild(host);
});

afterEach(() => {
	if (instance !== null) unmount(instance);
	instance = null;
	host.remove();
	invokeMock.mockClear();
});

/** Mounts the page and lets every already-resolved read settle. Several turns,
 *  because the reads chain through `typedError` → `unwrap` → the page's own
 *  `finally` before Svelte re-renders. */
async function mountPage(): Promise<void> {
	instance = mount(SitesPage, { target: host });
	await settle();
}

async function settle(): Promise<void> {
	for (let i = 0; i < 6; i++) {
		await Promise.resolve();
		await tick();
	}
}

/** The readiness banner's visible text, or `null` when it is not rendered. */
function banner(): string | null {
	const el = host.querySelector('[data-testid="site-readiness-banner"]');
	return el === null ? null : (el.textContent ?? '').replace(/\s+/g, ' ').trim();
}

function bannerCount(): number {
	return host.querySelectorAll('[data-testid="site-readiness-banner"]').length;
}

function hasTestId(id: string): boolean {
	return host.querySelector(`[data-testid="${id}"]`) !== null;
}

/** Hrefs inside the readiness banner only — the page has other links. */
function bannerLinks(): string[] {
	return [...host.querySelectorAll('[data-testid="site-readiness-banner"] a')].map(
		(a) => a.getAttribute('href') ?? ''
	);
}

// SPEC §7.1 — no nginx, PHP installed. THE state that renders nothing at all
// before this slice: no banner, an invitation to add a site, and a site that
// does not serve. Delete the page's `list_web_servers` read, or its
// `nginxCheck` call, and this is the test that goes red.
describe('no nginx, PHP installed (spec §7.1)', () => {
	beforeEach(() => {
		handlers.php_environment = () => phpEnv(['8.4']);
		handlers.list_web_servers = () => webServers(null);
	});

	it('shows a banner that names nginx', async () => {
		await mountPage();
		expect(unhandled).toEqual([]);
		expect(banner()).toBe(
			'nginx is not installed Sites are served by nginx. Check the Web server page.'
		);
	});

	it('offers the Web server page as the way out', async () => {
		await mountPage();
		expect(bannerLinks()).toEqual(['/web-server']);
	});

	it('says nothing about PHP, which is installed', async () => {
		await mountPage();
		expect(banner()).not.toContain('PHP');
		expect(hasTestId('readiness-php')).toBe(false);
	});

	it('really did ask the backend for the web server list', async () => {
		await mountPage();
		expect(invokeMock.mock.calls.map(([cmd]) => cmd)).toContain('list_web_servers');
	});
});

// SPEC §7.2 — no PHP, nginx installed. The existing wording is not the bug.
describe('no PHP, nginx installed (spec §7.2)', () => {
	beforeEach(() => {
		handlers.php_environment = () => phpEnv([]);
		handlers.list_web_servers = () => webServers(NGINX_PATH);
	});

	it('reads exactly as it did before this slice', async () => {
		await mountPage();
		expect(unhandled).toEqual([]);
		expect(banner()).toBe(
			'No PHP version is installed yet Sites need one to run. Install a version on the Languages page.'
		);
	});

	it('still points at the Languages page and nowhere else', async () => {
		await mountPage();
		expect(bannerLinks()).toEqual(['/languages']);
	});

	it('says nothing about nginx, which is installed', async () => {
		await mountPage();
		expect(hasTestId('readiness-nginx')).toBe(false);
	});
});

// SPEC §7.3 — neither installed: ONE banner naming both, never two stacked.
describe('neither installed (spec §7.3)', () => {
	beforeEach(() => {
		handlers.php_environment = () => phpEnv([]);
		handlers.list_web_servers = () => webServers(null);
	});

	it('renders exactly one banner', async () => {
		await mountPage();
		expect(bannerCount()).toBe(1);
	});

	it('names both requirements inside it, with both remedies', async () => {
		await mountPage();
		expect(hasTestId('readiness-php')).toBe(true);
		expect(hasTestId('readiness-nginx')).toBe(true);
		expect(bannerLinks()).toEqual(['/languages', '/web-server']);
	});

	it('titles itself for the pair rather than for either one', async () => {
		await mountPage();
		expect(banner()).toContain("Sites can't run yet");
	});
});

// SPEC §7.4 — both installed. Every developed machine today, including this
// one: the slice must be invisible there.
describe('both installed (spec §7.4)', () => {
	beforeEach(() => {
		handlers.php_environment = () => phpEnv(['8.4']);
		handlers.list_web_servers = () => webServers(NGINX_PATH);
	});

	it('shows no banner at all', async () => {
		await mountPage();
		expect(unhandled).toEqual([]);
		expect(banner()).toBeNull();
	});

	it('shows no error banner either', async () => {
		await mountPage();
		expect(hasTestId('php-env-error-banner')).toBe(false);
		expect(hasTestId('web-servers-error-banner')).toBe(false);
	});
});

// SPEC §7.5 — before either read returns: nothing, and no flash of one. Each
// test here resolves its pending read at the END and asserts the banner then
// appears, so "no banner" cannot pass because the page was broken.
describe('before the reads return (spec §7.5)', () => {
	it('says nothing while both are in flight, then speaks when they land', async () => {
		const php = deferred<PhpEnvironmentDto>();
		const nginx = deferred<WebServerDto[]>();
		handlers.php_environment = () => php.promise;
		handlers.list_web_servers = () => nginx.promise;

		await mountPage();
		expect(banner()).toBeNull();

		php.resolve(phpEnv([]));
		nginx.resolve(webServers(null));
		await settle();
		expect(bannerCount()).toBe(1);
	});

	// The half a single-read page could never get wrong: PHP has answered
	// "installed", nginx has not answered at all. Claiming nginx is missing here
	// would be a false statement on the first screen of a perfectly fine machine.
	it('does not claim nginx is missing while only PHP has answered', async () => {
		const nginx = deferred<WebServerDto[]>();
		handlers.php_environment = () => phpEnv(['8.4']);
		handlers.list_web_servers = () => nginx.promise;

		await mountPage();
		expect(banner()).toBeNull();

		nginx.resolve(webServers(null));
		await settle();
		expect(banner()).toContain('nginx is not installed');
	});

	// The mirror image, which is the discipline this page already had for PHP.
	it('does not claim PHP is missing while only nginx has answered', async () => {
		const php = deferred<PhpEnvironmentDto>();
		handlers.php_environment = () => php.promise;
		handlers.list_web_servers = () => webServers(NGINX_PATH);

		await mountPage();
		expect(banner()).toBeNull();

		php.resolve(phpEnv([]));
		await settle();
		expect(banner()).toContain('No PHP version is installed');
	});
});

// SPEC §7.6 — a failed read is not an absence, on either side. The I2 finding,
// extended: "we could not look" and "there is nothing there" are different
// claims, and only the second may be stated.
describe('a failed read (spec §7.6)', () => {
	it('reports the PHP failure without claiming PHP is missing', async () => {
		handlers.php_environment = () => Promise.reject(IPC_FAILURE);
		handlers.list_web_servers = () => webServers(NGINX_PATH);

		await mountPage();
		expect(hasTestId('php-env-error-banner')).toBe(true);
		expect(banner()).toBeNull();
	});

	it('reports the web-server failure without claiming nginx is missing', async () => {
		handlers.php_environment = () => phpEnv(['8.4']);
		handlers.list_web_servers = () => Promise.reject(IPC_FAILURE);

		await mountPage();
		expect(hasTestId('web-servers-error-banner')).toBe(true);
		expect(banner()).toBeNull();
	});

	// The case the page's old `{#if phpEnvError}{:else if …}` chain would have
	// silenced: one read failed, the OTHER returned a confirmed absence. The
	// failure must not suppress the fact.
	it('still names nginx when the PHP read failed and nginx is genuinely absent', async () => {
		handlers.php_environment = () => Promise.reject(IPC_FAILURE);
		handlers.list_web_servers = () => webServers(null);

		await mountPage();
		expect(hasTestId('php-env-error-banner')).toBe(true);
		expect(banner()).toBe(
			'nginx is not installed Sites are served by nginx. Check the Web server page.'
		);
	});

	it('still names PHP when the web-server read failed and PHP is genuinely absent', async () => {
		handlers.php_environment = () => phpEnv([]);
		handlers.list_web_servers = () => Promise.reject(IPC_FAILURE);

		await mountPage();
		expect(hasTestId('web-servers-error-banner')).toBe(true);
		expect(banner()).toContain('No PHP version is installed yet');
		expect(banner()).not.toContain('nginx');
	});

	it('reports both failures and claims neither is missing', async () => {
		handlers.php_environment = () => Promise.reject(IPC_FAILURE);
		handlers.list_web_servers = () => Promise.reject(IPC_FAILURE);

		await mountPage();
		expect(hasTestId('php-env-error-banner')).toBe(true);
		expect(hasTestId('web-servers-error-banner')).toBe(true);
		expect(banner()).toBeNull();
	});
});
