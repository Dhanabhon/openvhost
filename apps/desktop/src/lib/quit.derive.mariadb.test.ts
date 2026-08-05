// SPDX-License-Identifier: GPL-3.0-or-later
//
// Coverage for `PendingOperationKind`'s new `'mariadb'` member (P1 MariaDB UI
// design D4), added as a NEW file — `quit.derive.test.ts` stays green
// unmodified. Like `uninstall.derive.ts`'s `PackageKind`, this type had
// already drifted out of sync with the generated `InstallKindDto` the moment
// task 1 landed; `pnpm check` reported `+layout.svelte`'s assignment of
// `pendingInstallInfo` to `QuitDialog`'s `pendingInstall` prop as a real,
// pre-existing compile error before this task's fix.
//
// `set_running_mariadb_init`/`install_mariadb` (Rust) label the slot
// `"MariaDB {MARIADB_SERIES}"` — already a complete phrase, exactly
// `set_running_mysql_init` labels MySQL's. `operationLead('mariadb')`
// mirrors `operationLead('mysql')` for the identical reason.

import { describe, expect, it } from 'vitest';
import { pendingOperationCopy, pendingOperationSentence } from './quit.derive';

describe('an in-flight MariaDB operation', () => {
	it('supplies nothing in front of the already-complete label, like mysql', () => {
		const copy = pendingOperationCopy({ kind: 'mariadb', operation: 'install', label: 'MariaDB 11.4' });
		expect(copy.lead).toBe('');
		expect(copy.label).toBe('MariaDB 11.4');
	});

	it('never doubles the engine name', () => {
		const sentence = pendingOperationSentence({
			kind: 'mariadb',
			operation: 'install',
			label: 'MariaDB 11.4'
		});
		expect(sentence).toContain('MariaDB 11.4');
		expect(sentence).not.toContain('MariaDB MariaDB');
	});

	it('renders the install/uninstall/initialize sentences as distinctly as mysql’s', () => {
		const sentences = (['install', 'uninstall', 'initialize'] as const).map((operation) =>
			pendingOperationSentence({ kind: 'mariadb', operation, label: 'MariaDB 11.4' })
		);
		expect(new Set(sentences).size).toBe(3);
	});
});
