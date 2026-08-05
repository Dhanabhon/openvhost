// SPDX-License-Identifier: GPL-3.0-or-later
//
// Render-level proof that `DatabasesEmpty.svelte` is genuinely engine-generic
// (P1 MariaDB UI design D1, task 3: "give it what it needs to speak for
// either engine"), added as a NEW file — the existing
// `DatabasesEmpty.svelte.test.ts` (7 tests) is this task's own
// behaviour-preserving gate and stays green UNMODIFIED.
//
// Vacuity: the "no `{#if engine === …}` in a template" fix
// (`installInviteBody` moved onto `EngineDescriptor`) was proved able to
// matter by temporarily hardcoding `MARIADB_DESCRIPTOR.installInviteBody` to
// `MYSQL_DESCRIPTOR`'s own literal and confirming `'names MariaDB's own
// GitHub release, never MySQL or Oracle'` below went red. Reverted
// immediately after.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import DatabasesEmpty from './DatabasesEmpty.svelte';

function renderEmpty(engine: 'mysql' | 'mariadb', anyInstalled: boolean): string {
	return render(DatabasesEmpty, { props: { engine, anyInstalled } }).body;
}

describe('DatabasesEmpty — engine prop defaults to mysql', () => {
	it('renders exactly the old mysql-prefixed markup when engine is omitted', () => {
		const body = render(DatabasesEmpty, { props: { anyInstalled: false } }).body;
		expect(body).toContain('data-testid="databases-no-mysql"');
		expect(body).not.toContain('mariadb');
	});
});

describe('DatabasesEmpty — engine="mariadb" paints the descriptor, not hardcoded MySQL copy', () => {
	it('uses the mariadb id prefix, not mysql’s', () => {
		const body = renderEmpty('mariadb', false);
		expect(body).toContain('data-testid="databases-no-mariadb"');
		expect(body).not.toContain('data-testid="databases-no-mysql"');
	});

	it('invites the user to install MariaDB, by name', () => {
		const body = renderEmpty('mariadb', false);
		expect(body).toMatch(/Install MariaDB to get started/);
	});

	it('names MariaDB’s own GitHub release, never MySQL or Oracle', () => {
		const body = renderEmpty('mariadb', false);
		expect(body).toMatch(/downloads MariaDB from its own GitHub release/i);
		expect(body).not.toMatch(/MySQL|Oracle/);
	});

	// Design D2: no Homebrew fallback for MariaDB anywhere in this app — the
	// invite says plainly that Homebrew was never involved, unlike MySQL's
	// "no Homebrew required" (which would wrongly imply an optional path).
	it('says Homebrew was never involved, not merely "not required"', () => {
		const body = renderEmpty('mariadb', false);
		expect(body).toMatch(/never gone through homebrew/i);
		expect(body).not.toMatch(/no Homebrew required/i);
	});

	it('shows nothing once MariaDB is installed, same gate as MySQL', () => {
		expect(renderEmpty('mariadb', true)).not.toContain('data-testid="databases-no-mariadb"');
	});

	// Mirrors the MySQL invite's own scope: whether a PARTICULAR host can
	// install right now is a per-row fact (the row's own `awaitingRelease`/
	// `unavailable` states), never this component's concern.
	it('renders no rescan control of its own, same as MySQL', () => {
		expect(renderEmpty('mariadb', false)).not.toContain('data-testid="databases-check-again"');
	});
});
