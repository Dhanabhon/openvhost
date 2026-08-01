// SPDX-License-Identifier: GPL-3.0-or-later
//
// Route-level SSR tests for the Databases page, seeding the SHARED
// `databasesStore` directly the same way `routes/languages/languages-page.test.ts`
// seeds `languagesStore` — `onMount` never runs under `svelte/server`, so this
// is the only way to put the page into a terminal state without a live IPC
// layer.
//
// Rendered through `svelte/server`, so it runs in the existing `node` vitest
// project — same pattern as `languages-page.test.ts`.

import { beforeEach, describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import DatabasesPage from './+page.svelte';
import { databasesStore } from '$lib/databases.shared.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import { uninstallStore } from '$lib/uninstall.shared.svelte';
import type { MysqlEnvironmentDto, MysqlInstanceDto, ServiceStatus } from '$lib/ipc';

function instance(overrides: Partial<MysqlInstanceDto> = {}): MysqlInstanceDto {
	return {
		major: '8.4',
		cataloged: true,
		installed: false,
		path: null,
		socketPath: null,
		serviceId: null,
		datadirState: { kind: 'notInitialized' },
		source: null,
		offer: { kind: 'available', version: '8.4.11' },
		...overrides
	};
}

/** An installed row from a Homebrew keg — still supported during the migration
 *  (design D3/D7), and the only source the brew-driven uninstall path applies
 *  to. */
function brewed(overrides: Partial<MysqlInstanceDto> = {}): MysqlInstanceDto {
	return instance({ installed: true, source: { kind: 'homebrew' }, ...overrides });
}

/** An installed row from OpenVHost's own package tree. */
function packaged(overrides: Partial<MysqlInstanceDto> = {}): MysqlInstanceDto {
	return instance({
		installed: true,
		source: { kind: 'packaged', version: '8.4.11' },
		...overrides
	});
}

function env(brewFound: boolean, instances: MysqlInstanceDto[]): MysqlEnvironmentDto {
	return { brewFound, brewSearched: ['/opt/homebrew/bin/brew'], instances };
}

function svc(id: string, state: ServiceStatus['state']): ServiceStatus {
	return { id, displayName: id, endpoint: null, pid: null, state };
}

// `databasesStore`/`servicesStore` are module singletons — reset every field
// this page reads so no test inherits state a previous one left behind.
beforeEach(() => {
	databasesStore.env = null;
	databasesStore.error = '';
	databasesStore.installing = '';
	databasesStore.installLog = [];
	databasesStore.installProgress = null;
	databasesStore.installTotal = null;
	databasesStore.cancellingInstall = false;
	databasesStore.installOutcome = null;
	databasesStore.initializing = '';
	databasesStore.initLog = [];
	databasesStore.initOutcome = null;
	databasesStore.passwords = {};
	databasesStore.passwordError = {};
	databasesStore.revealing = {};
	databasesStore.resetting = {};
	databasesStore.resetOutcome = {};
	databasesStore.resetError = {};
	databasesStore.verifying = {};
	databasesStore.verifyResult = {};
	databasesStore.verifyError = {};
	servicesStore.services = [];
	servicesStore.error = null;
	uninstallStore.target = null;
	uninstallStore.plan = null;
	uninstallStore.planning = false;
	uninstallStore.uninstalling = '';
	uninstallStore.error = '';
	uninstallStore.log = [];
});

describe('the /databases route', () => {
	it('renders the panel with no rows and no error on a fresh, unsettled load', () => {
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases"');
		expect(body).not.toContain('data-testid="databases-page-error"');
		expect(body).not.toMatch(/data-testid="mysql-row-/);
	});

	// Mirrors the Languages page's C3 regression test: `env` non-null (an
	// earlier load succeeded) and `error` non-empty (a later rescan failed),
	// with neither brew nor anything installed — the failure must still
	// render even though the rowlist itself is hidden.
	it('shows a failed rescan even though no rows are rendered', () => {
		databasesStore.env = env(false, []);
		databasesStore.error = 'mysql runtime list is poisoned';
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-page-error"');
		expect(body).toContain('mysql runtime list is poisoned');
		expect(body).not.toMatch(/data-testid="mysql-row-/);
	});

	it('shows a failed rescan alongside an already-populated rowlist too', () => {
		databasesStore.env = env(true, [
			instance({ installed: true, datadirState: { kind: 'initialized' } })
		]);
		databasesStore.error = 'the MySQL discovery task failed to run';
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-page-error"');
		expect(body).toContain('data-testid="mysql-row-8.4"');
	});

	it('shows no error banner once it has been cleared', () => {
		databasesStore.env = env(true, [
			instance({ installed: true, datadirState: { kind: 'initialized' } })
		]);
		databasesStore.error = '';
		const { body } = render(DatabasesPage);
		expect(body).not.toContain('data-testid="databases-page-error"');
	});

	it('offers Check again with nothing installed yet', () => {
		databasesStore.env = env(true, [instance()]);
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-check-again-header"');
	});

	// The gate this slice removes. Installing MySQL is download -> verify ->
	// extract, so a machine that has never had Homebrew must still see — and be
	// able to press — every control on this page. It used to see none of them.
	it('renders the rowlist and its Install control on a machine with no Homebrew', () => {
		databasesStore.env = env(false, [instance()]);
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="mysql-row-8.4"');
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).toContain('data-testid="databases-check-again-header"');
	});

	it('renders exactly one Check again control, never a second from the empty state', () => {
		databasesStore.env = env(false, []);
		const { body } = render(DatabasesPage);
		// The trailing quote disambiguates: `…-check-again-header` contains
		// `…-check-again` as a substring, so a bare `toContain` would pass
		// against the header and prove nothing.
		expect(body).not.toContain('data-testid="databases-check-again"');
		expect(body.match(/data-testid="databases-check-again-header"/g)?.length).toBe(1);
	});

	it('no longer tells a user without Homebrew to go install it', () => {
		databasesStore.env = env(false, [instance()]);
		const { body } = render(DatabasesPage);
		expect(body).not.toContain('data-testid="databases-no-brew"');
		expect(body).not.toMatch(/homebrew is required/i);
	});

	it('disables the header Check again while an install is running', () => {
		databasesStore.env = env(true, [instance()]);
		databasesStore.installing = '8.4';
		const { body } = render(DatabasesPage);
		const match = body.match(/<button[^>]*data-testid="databases-check-again-header"[^>]*>/);
		expect(match?.[0]).toContain('disabled');
	});

	it('leaves the header Check again enabled when nothing is running', () => {
		databasesStore.env = env(true, [instance()]);
		databasesStore.installing = '';
		databasesStore.initializing = '';
		const { body } = render(DatabasesPage);
		const match = body.match(/<button[^>]*data-testid="databases-check-again-header"[^>]*>/);
		expect(match?.[0]).not.toContain('disabled');
	});

	it('renders a Ready row with its supervisor state from the shared services snapshot', () => {
		databasesStore.env = env(true, [
			brewed({
				datadirState: { kind: 'initialized' },
				socketPath: '/Users/x/.openvhost/run/mysql-8.4.sock',
				serviceId: 'mysql-8.4'
			})
		]);
		servicesStore.services = [svc('mysql-8.4', { kind: 'failed', exit: 1, stderrTail: ['boom'] })];
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="pool-failed-mysql-8.4"');
		expect(body).toContain('data-testid="retry-mysql-8.4"');
		expect(body).toContain('data-testid="mysql-credentials-8.4"');
	});

	it('renders no pill or control for a row whose serviceId matches nothing in the snapshot', () => {
		databasesStore.env = env(true, [
			packaged({
				datadirState: { kind: 'initialized' },
				socketPath: '/x/mysql-8.4.sock',
				serviceId: 'mysql-8.4'
			})
		]);
		servicesStore.services = [svc('some-other-service', { kind: 'running' })];
		const { body } = render(DatabasesPage);
		expect(body).not.toContain('data-testid="mysql-pill-8.4"');
		expect(body).not.toContain('data-testid="start-mysql-8.4"');
		expect(body).not.toContain('data-testid="stop-mysql-8.4"');
	});

	it('renders Install for a not-installed major regardless of the services snapshot', () => {
		databasesStore.env = env(true, [instance({ installed: false })]);
		servicesStore.services = [svc('unrelated', { kind: 'running' })];
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).not.toContain('data-testid="mysql-pill-8.4"');
	});

	it('marks Databases as the current rail destination', () => {
		databasesStore.env = env(true, [instance()]);
		const { body } = render(DatabasesPage);
		const anchor = [...body.matchAll(/<a\b([^>]*)>([\s\S]*?)<\/a>/g)].find(([, , inner]) =>
			inner.includes('Databases')
		);
		expect(anchor).toBeDefined();
		expect(anchor?.[1]).toContain('aria-current="page"');
	});
});

// The three things this slice exists to make visible, checked where the page's
// own glue can break them — which per-component tests structurally cannot see.
describe('the /databases route — the tarball install, end to end on the page', () => {
	it('shows the pipeline state the store received, not a generic spinner', () => {
		databasesStore.env = env(true, [instance()]);
		databasesStore.installing = '8.4';
		databasesStore.installProgress = { kind: 'verified' };
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="install-progress-8.4"');
		expect(body).toMatch(/checksum verified/i);
	});

	// The one that carries golden rule 6: a checked download and an unchecked
	// one must not look the same on this page.
	it('renders verified and extracted as different sentences', () => {
		databasesStore.env = env(true, [instance()]);
		databasesStore.installing = '8.4';
		databasesStore.installProgress = { kind: 'verified' };
		const verified = render(DatabasesPage).body;
		databasesStore.installProgress = { kind: 'extracted' };
		const extracted = render(DatabasesPage).body;
		const line = (body: string) =>
			body.match(/data-testid="install-progress-8\.4"[^>]*>([^<]*)</)?.[1] ?? '';
		expect(line(verified)).not.toBe('');
		expect(line(verified)).not.toBe(line(extracted));
	});

	it('carries the declared total through to the byte reading', () => {
		databasesStore.env = env(true, [instance()]);
		databasesStore.installing = '8.4';
		databasesStore.installTotal = 4096;
		databasesStore.installProgress = { kind: 'downloaded', bytes: 1024 };
		const { body } = render(DatabasesPage);
		expect(body).toMatch(/1\.00 KiB of 4\.00 KiB/);
	});

	// MANDATORY: the install permit is process-wide and the download has no
	// wall-clock bound, so an install nobody can stop starves every later one.
	it('offers Cancel while an install runs, and not otherwise', () => {
		databasesStore.env = env(true, [instance()]);
		expect(render(DatabasesPage).body).not.toContain('data-testid="cancel-install-8.4"');
		databasesStore.installing = '8.4';
		expect(render(DatabasesPage).body).toContain('data-testid="cancel-install-8.4"');
	});

	it('reflects a cancel already in flight rather than an idle button', () => {
		databasesStore.env = env(true, [instance()]);
		databasesStore.installing = '8.4';
		databasesStore.cancellingInstall = true;
		const { body } = render(DatabasesPage);
		expect(body).toMatch(/Cancelling…/);
	});

	it('renders a settled checksum failure as a checksum failure', () => {
		databasesStore.env = env(true, [instance()]);
		databasesStore.installOutcome = {
			major: '8.4',
			result: { kind: 'verificationFailed', expected: 'a'.repeat(64), actual: 'b'.repeat(64) }
		};
		const { body } = render(DatabasesPage);
		expect(body).toMatch(/checksum did not match/i);
		expect(body).not.toMatch(/network error/i);
	});

	it('renders an unavailable target as an honest absence with no Install button', () => {
		databasesStore.env = env(true, [
			instance({ offer: { kind: 'unavailable', target: 'macos-x86_64' } })
		]);
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="mysql-unavailable-8.4"');
		expect(body).toContain('macos-x86_64');
		expect(body).not.toContain('data-testid="install-8.4"');
	});

	// D3's whole reason for existing: the owner will be running both at once.
	it('says which install each runtime came from when both sources are present', () => {
		databasesStore.env = env(true, [
			packaged({ major: '8.4', datadirState: { kind: 'initialized' } }),
			brewed({ major: '9.7', cataloged: false })
		]);
		const { body } = render(DatabasesPage);
		expect(body).toContain('OpenVHost 8.4.11');
		expect(body).toContain('data-testid="mysql-source-9.7"');
		const brewBadge = body.match(/data-testid="mysql-source-9\.7"[^>]*>([^<]*)</)?.[1] ?? '';
		expect(brewBadge).toBe('Homebrew');
	});

	it('offers no Uninstall for a packaged runtime, and says why', () => {
		databasesStore.env = env(true, [packaged({ datadirState: { kind: 'initialized' } })]);
		const { body } = render(DatabasesPage);
		expect(body).not.toContain('data-testid="uninstall-8.4"');
		expect(body).toContain('data-testid="no-uninstall-8.4"');
	});
});

/** Just the Uninstall button's own opening tag for `major`. */
function uninstallTag(body: string, major: string): string {
	const match = body.match(new RegExp(`<button[^>]*data-testid="uninstall-${major}"[^>]*>`));
	if (!match) throw new Error(`expected an Uninstall button for ${major}`);
	return match[0];
}

// Package-uninstall design D6, at the route layer — the page's own glue, which
// is what per-component tests structurally cannot see.
describe('the /databases route — uninstall', () => {
	// A HOMEBREW keg: the brew-driven uninstall path only applies to runtimes
	// brew installed. A packaged runtime's own (absent) affordance is pinned
	// below.
	const installed = brewed({
		datadirState: { kind: 'initialized' },
		serviceId: 'mysql-8.4',
		socketPath: '/Users/x/.openvhost/run/mysql-8.4.sock'
	});

	it('offers Uninstall on an installed row', () => {
		databasesStore.env = env(true, [installed]);
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="uninstall-8.4"');
	});

	it('offers no Uninstall on a row that is not installed', () => {
		databasesStore.env = env(true, [instance({ installed: false })]);
		const { body } = render(DatabasesPage);
		expect(body).not.toContain('data-testid="uninstall-8.4"');
	});

	it('disables Uninstall while an install is running', () => {
		databasesStore.env = env(true, [installed]);
		databasesStore.installing = '8.4';
		const { body } = render(DatabasesPage);
		expect(uninstallTag(body, '8.4')).toContain('disabled');
	});

	it('disables Uninstall while an initialize is running', () => {
		databasesStore.env = env(true, [installed]);
		databasesStore.initializing = '8.4';
		const { body } = render(DatabasesPage);
		expect(uninstallTag(body, '8.4')).toContain('disabled');
	});

	// The cross-page property the SHARED uninstall store exists for: one
	// `InstallLock` covers PHP and MySQL alike, so a PHP uninstall must disable
	// this page's buttons too.
	it('disables Uninstall while a PHP uninstall is running on the other page', () => {
		databasesStore.env = env(true, [installed]);
		uninstallStore.uninstalling = '8.3';
		const { body } = render(DatabasesPage);
		expect(uninstallTag(body, '8.4')).toContain('disabled');
	});

	it('leaves Uninstall enabled when nothing is in flight', () => {
		databasesStore.env = env(true, [installed]);
		const { body } = render(DatabasesPage);
		expect(uninstallTag(body, '8.4')).not.toContain('disabled');
	});

	it('renders no confirmation until one is requested', () => {
		databasesStore.env = env(true, [installed]);
		const { body } = render(DatabasesPage);
		expect(body).not.toContain('data-testid="uninstall-dialog"');
	});

	// The single most important assertion in this slice's UI: the datadir
	// sentence is the ONLY place a user learns their databases survive.
	it('renders the confirmation and says the databases are kept, naming the datadir', () => {
		databasesStore.env = env(true, [installed]);
		uninstallStore.target = { kind: 'mysql', major: '8.4' };
		uninstallStore.plan = {
			kind: 'mysql',
			major: '8.4',
			removes: ['the Homebrew formula mysql@8.4', 'the supervisor entry mysql-8.4'],
			keeps: [
				{ what: 'Your databases', path: '/Users/x/.openvhost/data/mysql/8.4', headline: true },
				{ what: 'The stored root password', path: null, headline: false }
			],
			blockers: []
		};
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="uninstall-dialog"');
		expect(body).toContain('Uninstall MySQL 8.4?');
		expect(body).toContain('Your databases are not touched');
		expect(body).toContain('/Users/x/.openvhost/data/mysql/8.4');
		expect(body).toContain('root password is kept');
	});

	it('offers no way to proceed while the server is still running', () => {
		databasesStore.env = env(true, [installed]);
		uninstallStore.target = { kind: 'mysql', major: '8.4' };
		uninstallStore.plan = {
			kind: 'mysql',
			major: '8.4',
			removes: [],
			keeps: [],
			blockers: [{ kind: 'serviceNotTerminal', id: 'mysql-8.4', state: 'running' }]
		};
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="uninstall-refused"');
		expect(body).toContain('mysql-8.4 is running');
		expect(body).not.toContain('data-testid="uninstall-confirm"');
	});
});

// Task 1's wiring, observed from this page rather than re-implemented.
describe('the /databases route — a service that disappears', () => {
	const installed = packaged({
		datadirState: { kind: 'initialized' },
		serviceId: 'mysql-8.4',
		socketPath: '/Users/x/.openvhost/run/mysql-8.4.sock'
	});

	it('drops the pill and its control without a page reload', () => {
		databasesStore.env = env(true, [installed]);
		servicesStore.services = [svc('mysql-8.4', { kind: 'running' })];
		const before = render(DatabasesPage).body;
		expect(before).toContain('data-testid="mysql-pill-8.4"');
		expect(before).toContain('data-testid="stop-mysql-8.4"');

		// Exactly what the layout does on `SupervisorEvent::Unregistered`.
		servicesStore.applyUnregistered('mysql-8.4');

		const after = render(DatabasesPage).body;
		expect(after).not.toContain('data-testid="mysql-pill-8.4"');
		expect(after).not.toContain('data-testid="stop-mysql-8.4"');
	});
});
