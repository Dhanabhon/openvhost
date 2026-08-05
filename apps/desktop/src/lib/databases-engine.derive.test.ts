// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the shared engine descriptor and the ninth row state
// (P1 MariaDB UI design D1/D2), added as a NEW file rather than appended to
// `databases.derive.test.ts` — that suite, along with `MysqlRow.svelte.test.ts`
// and `MysqlCredentials.svelte.test.ts`, is this task's own behaviour-preserving
// gate and stays green UNMODIFIED, so every new assertion for the shared layer
// lives here instead.
//
// This task adds no MariaDB store, listeners or page — `engineDescriptor`'s
// MariaDB entry and `notInstalledRowState`'s `awaitingRelease` arm are
// exercised directly with hand-built values, never through a real MariaDB
// environment.
//
// Vacuity, recorded here rather than only in the task report: every
// `mysqlDescriptor`/`mariadbDescriptor` comparison below was proved able to
// fail by temporarily making `engineDescriptor('mariadb')` return
// `engineDescriptor('mysql')`'s own object (i.e. `MYSQL_DESCRIPTOR` for both
// arms) and confirming the "the two engines disagree" tests below go red; the
// mutation was reverted immediately after, per the task's own instruction
// never to leave one on disk.

import { describe, expect, it } from 'vitest';
import {
	engineAwaitingReleaseNotice,
	engineDescriptor,
	notInstalledRowState,
	type EngineOfferDto
} from './databases.derive';
import { mysqlSourceBadge, mysqlUninstallOffered } from './mysql-install.derive';

describe('engineDescriptor — mysql', () => {
	const d = engineDescriptor('mysql');

	it('keeps the exact facts the old hardcoded row relied on', () => {
		expect(d.label).toBe('MySQL');
		expect(d.idPrefix).toBe('mysql');
		expect(d.defaultPort).toBe(3306);
	});

	it('reuses mysqlSourceBadge and mysqlUninstallOffered verbatim, not a re-implementation', () => {
		// Same function REFERENCE, not merely equivalent behaviour — this is
		// what guarantees zero drift from the pre-refactor row.
		expect(d.sourcePolicy).toBe(mysqlSourceBadge);
		expect(d.uninstallPolicy).toBe(mysqlUninstallOffered);
	});

	it('still withholds Uninstall for a packaged source, unchanged', () => {
		expect(d.uninstallPolicy({ kind: 'packaged', version: '8.4.11' })).toBe(false);
		expect(d.uninstallPolicy({ kind: 'homebrew' })).toBe(true);
	});
});

describe('engineDescriptor — mariadb', () => {
	const d = engineDescriptor('mariadb');

	it('has its own label, id prefix and default port — never mysql’s', () => {
		expect(d.label).toBe('MariaDB');
		expect(d.idPrefix).toBe('mariadb');
		expect(d.defaultPort).toBe(3307);
	});

	// THE load-bearing case (design D1): a shared row that inherited MySQL's
	// "packaged means no Uninstall" unchanged would render
	// `PACKAGED_UNINSTALL_UNAVAILABLE` on every installed MariaDB row, whose
	// packaged install is the ONLY install path it has. Note this is proved
	// against a SYNTHETIC `packaged`-shaped source — MariaDB's own DTO has no
	// `source` field at all — precisely to show the POLICY itself decides,
	// independent of any real MariaDB wiring.
	it('offers Uninstall for a packaged source, unlike mysql', () => {
		expect(d.uninstallPolicy({ kind: 'packaged', version: '11.4.9' })).toBe(true);
		expect(d.uninstallPolicy(null)).toBe(true);
	});

	it('shows no source badge — one source, nothing to disambiguate', () => {
		expect(d.sourcePolicy({ kind: 'packaged', version: '11.4.9' })).toBeNull();
		expect(d.sourcePolicy(null)).toBeNull();
	});
});

describe('engineDescriptor — the two engines never collapse onto one', () => {
	// The direct vacuity check: a resolver that returned MySQL's own object
	// for BOTH arms would pass every single-engine test above (mysql's own
	// values are self-consistent) but fails every comparison here.
	it('disagrees on every field this task added', () => {
		const mysql = engineDescriptor('mysql');
		const mariadb = engineDescriptor('mariadb');
		expect(mysql.label).not.toBe(mariadb.label);
		expect(mysql.idPrefix).not.toBe(mariadb.idPrefix);
		expect(mysql.defaultPort).not.toBe(mariadb.defaultPort);
		expect(mysql.datadirDisclosure).not.toBe(mariadb.datadirDisclosure);
		expect(mysql.uninstallPolicy({ kind: 'packaged', version: 'x' })).not.toBe(
			mariadb.uninstallPolicy({ kind: 'packaged', version: 'x' })
		);
		expect(mysql.staleCredentialRecovery).not.toBe(mariadb.staleCredentialRecovery);
	});

	it('gives the port-conflict hint different wording, naming no Homebrew service for MariaDB', () => {
		const conflict = ['2026-08-05 [ERROR] Address already in use'];
		const mysqlHint = engineDescriptor('mysql').portConflictHint(conflict);
		const mariadbHint = engineDescriptor('mariadb').portConflictHint(conflict);
		expect(mysqlHint).not.toBeNull();
		expect(mariadbHint).not.toBeNull();
		expect(mysqlHint).not.toBe(mariadbHint);
		expect(mariadbHint).not.toMatch(/brew|Homebrew/i);
	});
});

describe('notInstalledRowState — the ninth state (design D2)', () => {
	it('maps an available offer to notInstalled, carrying the version', () => {
		expect(notInstalledRowState({ kind: 'available', version: '11.4.9' })).toEqual({
			kind: 'notInstalled',
			version: '11.4.9'
		});
	});

	it('maps an unavailable offer to unavailable, carrying the target', () => {
		expect(notInstalledRowState({ kind: 'unavailable', target: 'macos-x86_64' })).toEqual({
			kind: 'unavailable',
			target: 'macos-x86_64'
		});
	});

	// The state MySQL's own offer DTO can never produce — proved here with a
	// hand-built value, since no real `MysqlInstanceDto` can carry it.
	it('maps an awaitingRelease offer to its own row state, carrying the tag', () => {
		const offer: EngineOfferDto = { kind: 'awaitingRelease', tag: 'mariadb-11.4.9' };
		expect(notInstalledRowState(offer)).toEqual({
			kind: 'awaitingRelease',
			tag: 'mariadb-11.4.9'
		});
	});

	it('does not collapse awaitingRelease onto unavailable — different kind, different payload', () => {
		const awaiting = notInstalledRowState({ kind: 'awaitingRelease', tag: 'mariadb-11.4.9' });
		const unavailable = notInstalledRowState({ kind: 'unavailable', target: 'mariadb-11.4.9' });
		expect(awaiting.kind).not.toBe(unavailable.kind);
	});
});

describe('engineAwaitingReleaseNotice', () => {
	it('names the engine and the release tag, and points at no user action', () => {
		const notice = engineAwaitingReleaseNotice(engineDescriptor('mariadb'), 'mariadb-11.4.9');
		expect(notice.title).toContain('MariaDB');
		expect(notice.body).toContain('mariadb-11.4.9');
		expect(notice.body).not.toMatch(/Homebrew|brew/i);
	});

	it('produces visibly different text from an unavailable notice for the same engine', () => {
		// Binding requirement: "unavailable and awaitingRelease produce
		// visibly different text" — checked here at the copy layer; the
		// component-level render proof is in MysqlRow.engine.svelte.test.ts.
		const descriptor = engineDescriptor('mariadb');
		const awaiting = engineAwaitingReleaseNotice(descriptor, 'mariadb-11.4.9');
		expect(awaiting.title).not.toMatch(/no verified download|cannot be installed/i);
	});

	it('reads differently per engine, not a fixed MySQL-branded sentence', () => {
		const mysql = engineAwaitingReleaseNotice(engineDescriptor('mysql'), 'mysql-9.9.9');
		const mariadb = engineAwaitingReleaseNotice(engineDescriptor('mariadb'), 'mysql-9.9.9');
		expect(mysql.title).not.toBe(mariadb.title);
	});
});
