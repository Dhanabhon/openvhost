// SPDX-License-Identifier: GPL-3.0-or-later
//
// Route-level SSR tests for the MariaDB group on the Databases page (P1
// MariaDB UI design), seeding the SHARED `mariadbStore` directly the same
// way `databases-page.test.ts` seeds `databasesStore` — `onMount` never runs
// under `svelte/server`, so this is the only way to put the page into a
// terminal state without a live IPC layer. Added as a NEW file, mirroring
// how `databases-page.test.ts` itself is untouched: that suite is this
// task's own MySQL-unchanged gate.
//
// Rendered through `svelte/server`, same pattern as the MySQL route test.

import { beforeEach, describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import DatabasesPage from './+page.svelte';
import { databasesStore } from '$lib/databases.shared.svelte';
import { mariadbStore } from '$lib/mariadb.shared.svelte';
import { MARIADB_SERIES } from '$lib/mariadb.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import { uninstallStore } from '$lib/uninstall.shared.svelte';
import type { MariadbEnvironmentDto, MysqlEnvironmentDto, ServiceStatus } from '$lib/ipc';

function mariadbEnv(overrides: Partial<MariadbEnvironmentDto> = {}): MariadbEnvironmentDto {
	return {
		installed: false,
		version: null,
		path: null,
		socketPath: null,
		serviceId: null,
		datadirState: { kind: 'notInitialized' },
		// The state this build actually ships in TODAY (task brief): the
		// catalogue pin exists but the GitHub release is not published.
		offer: { kind: 'awaitingRelease', tag: 'mariadb-11.4.9' },
		...overrides
	};
}

function ready(overrides: Partial<MariadbEnvironmentDto> = {}): MariadbEnvironmentDto {
	return mariadbEnv({
		installed: true,
		version: '11.4.9',
		path: '/x/packages/mariadb/11.4/11.4.9/bin/mariadbd',
		socketPath: '/Users/x/.openvhost/run/mariadb.sock',
		serviceId: 'mariadb-11.4',
		datadirState: { kind: 'initialized', version: '11.4.9' },
		...overrides
	});
}

function mysqlEnv(): MysqlEnvironmentDto {
	return { brewFound: true, brewSearched: [], instances: [] };
}

function svc(id: string, state: ServiceStatus['state']): ServiceStatus {
	return { id, displayName: id, endpoint: null, pid: null, state };
}

// Every store this page's MariaDB group (and, for the coexistence tests, its
// MySQL group) reads — reset every field so no test inherits state a
// previous one left behind, mirroring `databases-page.test.ts`'s own
// `beforeEach` exactly, extended with `mariadbStore`'s scalars.
beforeEach(() => {
	databasesStore.env = null;
	databasesStore.error = '';
	databasesStore.installing = '';
	databasesStore.initializing = '';

	mariadbStore.env = null;
	mariadbStore.error = '';
	mariadbStore.installing = false;
	mariadbStore.installLog = [];
	mariadbStore.installProgress = null;
	mariadbStore.installTotal = null;
	mariadbStore.cancellingInstall = false;
	mariadbStore.installOutcome = null;
	mariadbStore.initializing = false;
	mariadbStore.initLog = [];
	mariadbStore.initOutcome = null;
	mariadbStore.password = undefined;
	mariadbStore.passwordError = '';
	mariadbStore.revealing = false;
	mariadbStore.revealed = false;
	mariadbStore.resetting = false;
	mariadbStore.resetOutcome = undefined;
	mariadbStore.resetError = '';
	mariadbStore.verifying = false;
	mariadbStore.verifyResult = undefined;
	mariadbStore.verifyError = '';

	servicesStore.services = [];
	servicesStore.error = null;
	uninstallStore.target = null;
	uninstallStore.plan = null;
	uninstallStore.planning = false;
	uninstallStore.uninstalling = '';
	uninstallStore.error = '';
	uninstallStore.log = [];
});

describe('the /databases route — the MariaDB group', () => {
	it('renders the panel with no row and no error on a fresh, unsettled load', () => {
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-mariadb"');
		expect(body).not.toContain('data-testid="databases-mariadb-page-error"');
		expect(body).not.toMatch(/data-testid="mariadb-row-/);
	});

	// `env === null` means the environment has never successfully loaded at
	// all (never even an empty one) — the generic "could not read" block, not
	// the per-row error banner, which requires an environment to hang the row
	// off of. Mirrors `databases-page.test.ts`'s own first-load-failure case.
	it('reports a total load failure with no row and no per-row error banner', () => {
		mariadbStore.env = null;
		mariadbStore.error = 'mariadb runtime list is poisoned';
		const { body } = render(DatabasesPage);
		expect(body).toContain('Could not read the MariaDB environment');
		expect(body).toContain('mariadb runtime list is poisoned');
		expect(body).not.toContain('data-testid="databases-mariadb-page-error"');
		expect(body).not.toMatch(/data-testid="mariadb-row-/);
	});

	it('shows a failed rescan alongside an empty, otherwise-loaded row', () => {
		mariadbStore.env = mariadbEnv();
		mariadbStore.error = 'mariadb runtime list is poisoned';
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-mariadb-page-error"');
		expect(body).toContain('mariadb runtime list is poisoned');
		expect(body).toContain(`data-testid="mariadb-row-${MARIADB_SERIES}"`);
	});

	it('shows a failed rescan alongside an already-populated row too', () => {
		mariadbStore.env = ready();
		mariadbStore.error = 'the MariaDB discovery task failed to run';
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="databases-mariadb-page-error"');
		expect(body).toContain(`data-testid="mariadb-row-${MARIADB_SERIES}"`);
	});

	it('marks the MariaDB heading distinctly from the MySQL one', () => {
		mariadbStore.env = mariadbEnv();
		const { body } = render(DatabasesPage);
		expect(body).toContain('>MariaDB<');
		expect(body).toContain('>MySQL<');
	});

	it('offers Check again with its own test id, distinct from MySQL’s', () => {
		mariadbStore.env = mariadbEnv();
		const { body } = render(DatabasesPage);
		expect(body).toContain('data-testid="mariadb-check-again-header"');
	});

	it('disables its own Check again while its own install is running', () => {
		mariadbStore.env = mariadbEnv();
		mariadbStore.installing = true;
		const { body } = render(DatabasesPage);
		const match = body.match(/<button[^>]*data-testid="mariadb-check-again-header"[^>]*>/);
		expect(match?.[0]).toContain('disabled');
	});

	// The state this build ships in TODAY (task brief): the release is
	// unpublished, so the row must render the awaitingRelease explanation and
	// offer no Install control — not a bug, the expected state.
	describe('the state this build actually ships in today: awaitingRelease', () => {
		it('renders its own copy and no Install control', () => {
			mariadbStore.env = mariadbEnv();
			const { body } = render(DatabasesPage);
			expect(body).toContain(`data-testid="mariadb-awaiting-release-${MARIADB_SERIES}"`);
			expect(body).not.toContain(`data-testid="install-${MARIADB_SERIES}"`);
			expect(body).toContain('mariadb-11.4.9');
		});

		it('shows the empty-state invite alongside it, since nothing is installed yet', () => {
			mariadbStore.env = mariadbEnv();
			const { body } = render(DatabasesPage);
			expect(body).toContain('data-testid="databases-no-mariadb"');
		});
	});

	// Hand-built, since no real environment can produce this state until the
	// release is published — proves the pipeline is wired correctly ahead of
	// that, per this task's own RED-test requirement.
	describe('once the release is published: notInstalled', () => {
		it('renders the rowlist and its own Install control', () => {
			mariadbStore.env = mariadbEnv({ offer: { kind: 'available', version: '11.4.9' } });
			const { body } = render(DatabasesPage);
			expect(body).toContain(`data-testid="mariadb-row-${MARIADB_SERIES}"`);
			expect(body).toContain(`data-testid="install-${MARIADB_SERIES}"`);
			expect(body).toContain('data-testid="offer-11.4"');
			expect(body).toMatch(/Installs MariaDB 11\.4\.9/);
		});

		it('shows the pipeline state the store received while installing', () => {
			mariadbStore.env = mariadbEnv({ offer: { kind: 'available', version: '11.4.9' } });
			mariadbStore.installing = true;
			mariadbStore.installProgress = { kind: 'verified' };
			const { body } = render(DatabasesPage);
			expect(body).toContain(`data-testid="install-progress-${MARIADB_SERIES}"`);
			expect(body).toMatch(/checksum verified/i);
		});

		it('offers Cancel while an install runs, and not otherwise', () => {
			mariadbStore.env = mariadbEnv({ offer: { kind: 'available', version: '11.4.9' } });
			expect(render(DatabasesPage).body).not.toContain(
				`data-testid="cancel-install-${MARIADB_SERIES}"`
			);
			mariadbStore.installing = true;
			expect(render(DatabasesPage).body).toContain(
				`data-testid="cancel-install-${MARIADB_SERIES}"`
			);
		});

		it('renders a settled checksum failure as a checksum failure', () => {
			mariadbStore.env = mariadbEnv({ offer: { kind: 'available', version: '11.4.9' } });
			mariadbStore.installOutcome = {
				result: { kind: 'verificationFailed', expected: 'a'.repeat(64), actual: 'b'.repeat(64) }
			};
			const { body } = render(DatabasesPage);
			expect(body).toMatch(/checksum did not match/i);
			expect(body).not.toMatch(/network error/i);
		});
	});

	describe('installed and ready', () => {
		it('renders the credentials block and no install control', () => {
			mariadbStore.env = ready();
			servicesStore.services = [svc('mariadb-11.4', { kind: 'running' })];
			const { body } = render(DatabasesPage);
			expect(body).toContain(`data-testid="mariadb-credentials-${MARIADB_SERIES}"`);
			expect(body).not.toContain(`data-testid="install-${MARIADB_SERIES}"`);
		});

		it('shows 3307 as the connection port, never MySQL’s 3306', () => {
			mariadbStore.env = ready();
			servicesStore.services = [svc('mariadb-11.4', { kind: 'running' })];
			const { body } = render(DatabasesPage);
			expect(body).toContain(`data-testid="conn-value-port-${MARIADB_SERIES}">3307<`);
		});

		it('renders a failed service state with the row’s own port-conflict hint, naming no MySQL/Homebrew occupant', () => {
			mariadbStore.env = ready();
			servicesStore.services = [
				svc('mariadb-11.4', { kind: 'failed', exit: 1, stderrTail: ['Address already in use'] })
			];
			const { body } = render(DatabasesPage);
			expect(body).toContain(`data-testid="pool-failed-mariadb-11.4"`);
			expect(body).toContain(`data-testid="port-conflict-hint-${MARIADB_SERIES}"`);
			expect(body).toMatch(/port 3307 conflict/);
			expect(body).not.toMatch(/mysql server|homebrew/i);
		});

		it('offers Uninstall for an installed MariaDB row — unlike a packaged MySQL runtime', () => {
			mariadbStore.env = ready();
			const { body } = render(DatabasesPage);
			expect(body).toContain(`data-testid="uninstall-${MARIADB_SERIES}"`);
			expect(body).not.toContain(`data-testid="no-uninstall-${MARIADB_SERIES}"`);
		});
	});

	describe('uninstall', () => {
		it('opens the SAME shared confirmation dialog MySQL uses, naming MariaDB and its datadir', () => {
			mariadbStore.env = ready();
			uninstallStore.target = { kind: 'mariadb', major: MARIADB_SERIES };
			uninstallStore.plan = {
				kind: 'mariadb',
				major: MARIADB_SERIES,
				removes: ['the packaged MariaDB 11.4.9 tree'],
				keeps: [
					{ what: 'Your databases', path: '/Users/x/.openvhost/data/mariadb/11.4', headline: true },
					{ what: 'The stored root password', path: null, headline: false }
				],
				blockers: []
			};
			const { body } = render(DatabasesPage);
			expect(body).toContain('data-testid="uninstall-dialog"');
			expect(body).toContain('Uninstall MariaDB 11.4?');
			expect(body).toContain('Your databases are not touched');
			expect(body).toContain('/Users/x/.openvhost/data/mariadb/11.4');
		});

		it('disables MariaDB’s own Uninstall while a DIFFERENT package uninstall is running', () => {
			mariadbStore.env = ready();
			uninstallStore.uninstalling = '8.3'; // a PHP uninstall on the other page
			const { body } = render(DatabasesPage);
			const match = body.match(
				new RegExp(`<button[^>]*data-testid="uninstall-${MARIADB_SERIES}"[^>]*>`)
			);
			expect(match?.[0]).toContain('disabled');
		});
	});

	// Spec §10 point 7: both engines installed, both visible, neither's
	// controls driving the other.
	describe('coexistence with MySQL', () => {
		it('renders both groups at once, each with its own row and neither leaking the other’s testid', () => {
			databasesStore.env = mysqlEnv();
			databasesStore.env.instances = [
				{
					major: '8.4',
					cataloged: true,
					installed: true,
					path: '/x/mysqld',
					socketPath: '/x/mysql-8.4.sock',
					serviceId: 'mysql-8.4',
					datadirState: { kind: 'initialized' },
					source: { kind: 'packaged', version: '8.4.11' },
					offer: { kind: 'available', version: '8.4.11' }
				}
			];
			mariadbStore.env = ready();
			const { body } = render(DatabasesPage);
			expect(body).toContain('data-testid="mysql-row-8.4"');
			expect(body).toContain(`data-testid="mariadb-row-${MARIADB_SERIES}"`);
		});

		// A MySQL install in flight must not disable the MariaDB row's own
		// controls — each row reads only its OWN engine's store.
		it('does not disable the MariaDB row while a MySQL install runs', () => {
			databasesStore.env = mysqlEnv();
			databasesStore.installing = '8.4';
			mariadbStore.env = mariadbEnv({ offer: { kind: 'available', version: '11.4.9' } });
			const { body } = render(DatabasesPage);
			const match = body.match(
				new RegExp(`<button[^>]*data-testid="install-${MARIADB_SERIES}"[^>]*>`)
			);
			expect(match?.[0]).not.toContain('disabled');
		});
	});
});
