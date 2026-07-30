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
		...overrides
	};
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

	it('offers Check again once brew is found, with nothing installed yet', () => {
		databasesStore.env = env(true, [instance()]);
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-check-again-header"');
	});

	it('does not duplicate Check again on the no-brew page, which renders its own', () => {
		databasesStore.env = env(false, []);
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-check-again"');
		expect(body).not.toContain('data-testid="databases-check-again-header"');
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
			instance({
				installed: true,
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
			instance({
				installed: true,
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
