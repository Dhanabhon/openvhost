// SPDX-License-Identifier: GPL-3.0-or-later
//
// Coverage for `PackageKind`'s new `'mariadb'` member (P1 MariaDB UI design
// D5), added as a NEW file — `uninstall.derive.test.ts` stays green
// unmodified. `PackageKind` here had drifted out of sync with the generated
// wire type the moment task 1 landed (`bindings.ts`'s own `PackageKind` was
// already `"php" | "mysql" | "mariadb"`); `pnpm check` reported it as two
// real, pre-existing compile errors (`ipc/index.ts` and `+layout.svelte`)
// before this task's fix.
//
// `brewFormula` is unexported, so its `null`-for-mariadb behaviour is proved
// here only through its one caller, `outOfCatalogueNote` — exactly what "every
// caller decides" (design D5) means in practice.

import { describe, expect, it } from 'vitest';
import { keptSentence, outOfCatalogueNote, packageLabel } from './uninstall.derive';

describe('packageLabel — mariadb', () => {
	it('names it MariaDB, distinct from php and mysql', () => {
		expect(packageLabel('mariadb')).toBe('MariaDB');
		expect(packageLabel('mariadb')).not.toBe(packageLabel('mysql'));
		expect(packageLabel('mariadb')).not.toBe(packageLabel('php'));
	});
});

describe('keptSentence — mariadb', () => {
	it('names the same guarantee mysql’s uninstall makes (spec §10 point 4)', () => {
		const sentence = keptSentence('mariadb', '11.4', []);
		expect(sentence).toMatch(/databases are not touched/i);
		expect(sentence).toMatch(/root password is kept/i);
	});
});

describe('outOfCatalogueNote — mariadb has no Homebrew formula (design D5)', () => {
	it('does not print `brew uninstall` with a null/blank formula', () => {
		const note = outOfCatalogueNote('mariadb', '11.4');
		expect(note).not.toContain('brew uninstall null');
		expect(note).not.toContain('brew uninstall undefined');
		expect(note).not.toContain('brew uninstall mariadb@11.4');
		expect(note).not.toContain('brew uninstall ');
	});

	it('still names the engine, still points at Check again', () => {
		const note = outOfCatalogueNote('mariadb', '11.4');
		expect(note).toContain('MariaDB 11.4');
		expect(note).toContain('Check again');
	});

	// Unchanged for php/mysql, which DO have a real formula — pinned here so a
	// future edit to the null-handling branch cannot silently swallow theirs.
	it('still prints the real brew command for php and mysql, unaffected by the mariadb branch', () => {
		expect(outOfCatalogueNote('php', '7.4')).toContain('brew uninstall php@7.4');
		expect(outOfCatalogueNote('mysql', '5.7')).toContain('brew uninstall mysql@5.7');
	});
});
