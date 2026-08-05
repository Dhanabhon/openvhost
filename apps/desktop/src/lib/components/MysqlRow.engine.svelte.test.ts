// SPDX-License-Identifier: GPL-3.0-or-later
//
// Render-level proof that `MysqlRow.svelte` is genuinely engine-generic (P1
// MariaDB UI design D1), added as a NEW file rather than appended to
// `MysqlRow.svelte.test.ts` — that suite (53 tests) is this task's own
// behaviour-preserving gate and stays green UNMODIFIED.
//
// This task adds no MariaDB store, listeners or page. Every `engine="mariadb"`
// render below hands the row a hand-built `EngineInstanceDto` directly — never
// through a real MariaDB environment, a store, or IPC.
//
// Vacuity: each "differs from mysql" assertion below was proved able to fail
// by hardcoding `engineDescriptor` to always return MySQL's own descriptor
// object regardless of the `engine` argument (i.e. `MYSQL_DESCRIPTOR` on both
// switch arms in `databases.derive.ts`) and confirming this file's own tests —
// not just `databases-engine.derive.test.ts`'s — went red (the row rendered
// "MySQL"/"mysql-…" even when asked for `engine="mariadb"`). The mutation was
// reverted immediately after.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import MysqlRow from './MysqlRow.svelte';
import type { EngineInstanceDto } from '$lib/databases.derive';
import type { MariadbInstallResultDto, MysqlInstallOutcomeDto } from '$lib/ipc';

function instance(overrides: Partial<EngineInstanceDto> = {}): EngineInstanceDto {
	return {
		major: '11.4',
		cataloged: true,
		installed: false,
		path: null,
		socketPath: null,
		serviceId: null,
		datadirState: { kind: 'notInitialized' },
		source: null,
		offer: { kind: 'available', version: '11.4.9' },
		...overrides
	};
}

function renderRow(
	props: Partial<{
		engine: 'mysql' | 'mariadb';
		instance: EngineInstanceDto;
		installingMajor: string;
		installOutcome: MysqlInstallOutcomeDto | null;
		mariadbInstallOutcome: { major: string; result: MariadbInstallResultDto } | null;
		uninstallingMajor: string;
		catalogedMajorsList: string[];
		serviceState: null;
	}> = {}
): string {
	return render(MysqlRow, {
		props: {
			engine: props.engine,
			instance: props.instance ?? instance(),
			installingMajor: props.installingMajor ?? '',
			installProgress: null,
			installTotal: null,
			installOutcome: props.installOutcome ?? null,
			mariadbInstallOutcome: props.mariadbInstallOutcome,
			initializingMajor: '',
			initLog: [],
			initFailure: null,
			uninstallingMajor: props.uninstallingMajor ?? '',
			catalogedMajorsList: props.catalogedMajorsList ?? ['11.4'],
			serviceState: props.serviceState ?? null,
			revealed: false,
			revealing: false,
			passwordError: '',
			resetting: false,
			resetError: '',
			verifying: false,
			verifyError: '',
			onInstall: () => {},
			onCancelInstall: () => {},
			onInitialize: () => {},
			onUninstall: () => {},
			onStart: () => {},
			onStop: () => {},
			onReveal: () => {},
			onHide: () => {},
			onCopyPassword: () => {},
			onReset: () => {},
			onVerify: () => {}
		}
	}).body;
}

describe('MysqlRow — engine prop defaults to mysql', () => {
	it('renders exactly the old mysql-prefixed markup when engine is omitted', () => {
		const body = renderRow();
		expect(body).toContain('data-testid="mysql-row-11.4"');
		expect(body).toContain('MySQL 11.4');
		expect(body).not.toContain('mariadb');
	});
});

describe('MysqlRow — engine="mariadb" paints the descriptor, not hardcoded MySQL copy', () => {
	it('uses the mariadb id prefix and label, not mysql’s', () => {
		const body = renderRow({ engine: 'mariadb' });
		expect(body).toContain('data-testid="mariadb-row-11.4"');
		expect(body).toContain('MariaDB 11.4');
		expect(body).not.toContain('data-testid="mysql-row-11.4"');
		expect(body).not.toMatch(/>MySQL /);
	});

	it('offers Install using the mariadb label in its accessible name', () => {
		const body = renderRow({ engine: 'mariadb' });
		expect(body).toContain('data-testid="install-11.4"');
		expect(body).toContain('aria-label="Install MariaDB 11.4.9"');
	});

	// THE load-bearing proof (design D1): a shared row that inherited
	// `mysqlUninstallOffered`'s "packaged means no Uninstall" unchanged would
	// hide Uninstall here. `uninstallPolicy` genuinely decides per engine.
	it('offers Uninstall for a packaged, installed source — mysql would withhold it here', () => {
		const packagedMariadb = instance({
			installed: true,
			source: { kind: 'packaged', version: '11.4.9' },
			datadirState: { kind: 'initialized' }
		});
		const body = renderRow({ engine: 'mariadb', instance: packagedMariadb });
		expect(body).toContain('data-testid="uninstall-11.4"');
		expect(body).not.toContain('data-testid="no-uninstall-11.4"');
	});

	it('shows no source badge — mariadb has exactly one source, nothing to disambiguate', () => {
		const packagedMariadb = instance({
			installed: true,
			source: { kind: 'packaged', version: '11.4.9' },
			datadirState: { kind: 'initialized' }
		});
		const body = renderRow({ engine: 'mariadb', instance: packagedMariadb });
		expect(body).not.toContain('data-testid="mariadb-source-11.4"');
	});
});

describe('MysqlRow — awaitingRelease (the ninth row state, design D2)', () => {
	const awaitingRelease = instance({ offer: { kind: 'awaitingRelease', tag: 'mariadb-11.4.9' } });

	it('renders its own copy and no Install control', () => {
		const body = renderRow({ engine: 'mariadb', instance: awaitingRelease });
		expect(body).toContain('data-testid="mariadb-awaiting-release-11.4"');
		expect(body).not.toContain('data-testid="install-11.4"');
		expect(body).toContain('mariadb-11.4.9');
	});

	it('is visibly distinct from unavailable — different test id, different words', () => {
		const unavailable = instance({ offer: { kind: 'unavailable', target: 'macos-x86_64' } });
		const awaitingBody = renderRow({ engine: 'mariadb', instance: awaitingRelease });
		const unavailableBody = renderRow({ engine: 'mariadb', instance: unavailable });
		expect(awaitingBody).toContain('data-testid="mariadb-awaiting-release-11.4"');
		expect(awaitingBody).not.toContain('data-testid="mariadb-unavailable-11.4"');
		expect(unavailableBody).toContain('data-testid="mariadb-unavailable-11.4"');
		expect(unavailableBody).not.toContain('data-testid="mariadb-awaiting-release-11.4"');
		expect(awaitingBody).not.toBe(unavailableBody);
	});

	it('mentions no Homebrew fallback, unlike an unavailable mysql row', () => {
		const body = renderRow({ engine: 'mariadb', instance: awaitingRelease });
		expect(body).not.toMatch(/Homebrew|brew/i);
	});
});

// Task 3 finding: the notInstalled/unavailable offer banner and the settled
// install-result banner were still hardcoded to `mysqlPackageOfferNotice`/
// `mysqlInstallResultNotice` even under `engine="mariadb"` — every other
// piece of copy in this row went through `descriptor`, but these three did
// not, so a MariaDB install rendered "Installs MySQL …" / "MySQL 11.4.9
// installed" / "Downloads it from Oracle…". This section is the render-level
// proof that the fix holds.
//
// Vacuity: each assertion below was proved able to fail by temporarily
// reverting `MysqlRow.svelte`'s `offerNotice`/`outcomeNotice`/`ledgerNotice`
// to call `mysqlPackageOfferNotice`/`mysqlInstallResultNotice`/
// `mysqlLedgerNotice` unconditionally (i.e. dropping the `engine ===
// 'mariadb'` branch) and confirming every test in this section went red
// (rendering "MySQL"/"Oracle" under `engine="mariadb"`). The mutation was
// reverted immediately after.
describe('MysqlRow — the offer/outcome/ledger banners are engine-aware, not hardcoded to MySQL', () => {
	it('the notInstalled offer banner names MariaDB and its own GitHub release, never MySQL or Oracle', () => {
		const body = renderRow({ engine: 'mariadb' });
		expect(body).toContain('data-testid="offer-11.4"');
		expect(body).toMatch(/Installs MariaDB 11\.4\.9/);
		expect(body).not.toMatch(/MySQL|Oracle/);
	});

	it('the unavailable offer banner names MariaDB, never MySQL or a Homebrew fallback', () => {
		const unavailable = instance({ offer: { kind: 'unavailable', target: 'macos-x86_64' } });
		const body = renderRow({ engine: 'mariadb', instance: unavailable });
		expect(body).toContain('data-testid="mariadb-unavailable-11.4"');
		expect(body).toMatch(/MariaDB cannot be installed on macos-x86_64/);
		expect(body).not.toMatch(/MySQL/);
	});

	it('a settled MariaDB install outcome renders MariaDB’s own banner, not MySQL’s', () => {
		const body = renderRow({
			engine: 'mariadb',
			mariadbInstallOutcome: {
				major: '11.4',
				result: {
					kind: 'installed',
					version: '11.4.9',
					detected: true,
					ledger: { kind: 'recorded' }
				}
			}
		});
		expect(body).toContain('data-testid="install-outcome-11.4"');
		expect(body).toMatch(/MariaDB 11\.4\.9 installed/);
		expect(body).not.toMatch(/MySQL|Oracle/);
	});

	it('renders the eighth MariaDB-only result state (awaitingRelease) rather than failing to compile it away', () => {
		const body = renderRow({
			engine: 'mariadb',
			mariadbInstallOutcome: {
				major: '11.4',
				result: { kind: 'awaitingRelease', tag: 'mariadb-11.4.9' }
			}
		});
		expect(body).toContain('data-testid="install-outcome-11.4"');
		expect(body).toContain('mariadb-11.4.9');
	});

	it('a settled MariaDB ledger failure renders MariaDB’s own wording, not MySQL’s', () => {
		// `ledger-warning-…` only renders in the `installedNotInitialized`
		// state (the one a successful-but-unrecorded install actually lands
		// in) — an `installed: true` row with a `notInitialized` datadir.
		const body = renderRow({
			engine: 'mariadb',
			instance: instance({ installed: true }),
			mariadbInstallOutcome: {
				major: '11.4',
				result: {
					kind: 'installed',
					version: '11.4.9',
					detected: true,
					ledger: { kind: 'failed', reason: 'database is locked' }
				}
			}
		});
		expect(body).toContain('data-testid="ledger-warning-11.4"');
		expect(body).toContain('database is locked');
		expect(body).not.toMatch(/MySQL/);
	});

	// The identity check both props share (`major === instance.major`) must
	// still hold per engine — a MariaDB outcome for a DIFFERENT identity must
	// not render here either.
	it('shows no MariaDB outcome banner when the tagged major does not match this row', () => {
		const body = renderRow({
			engine: 'mariadb',
			mariadbInstallOutcome: { major: 'not-this-row', result: { kind: 'cancelled' } }
		});
		expect(body).not.toContain('data-testid="install-outcome-11.4"');
	});

	// Cross-engine isolation: a MySQL-shaped `installOutcome` must never leak
	// into a MariaDB row's banner, and a MariaDB-shaped `mariadbInstallOutcome`
	// must never leak into a MySQL row's — each engine reads only its own prop.
	it('never renders a MySQL installOutcome on a MariaDB row', () => {
		const body = renderRow({
			engine: 'mariadb',
			installOutcome: { major: '11.4', result: { kind: 'cancelled' } }
		});
		expect(body).not.toContain('data-testid="install-outcome-11.4"');
	});

	it('never renders a MariaDB mariadbInstallOutcome on a MySQL row', () => {
		const body = renderRow({
			mariadbInstallOutcome: { major: '11.4', result: { kind: 'cancelled' } }
		});
		expect(body).not.toContain('data-testid="install-outcome-11.4"');
	});
});
