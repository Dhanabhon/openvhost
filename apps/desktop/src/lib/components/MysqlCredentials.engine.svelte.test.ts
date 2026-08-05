// SPDX-License-Identifier: GPL-3.0-or-later
//
// Render-level proof that `MysqlCredentials.svelte` is genuinely
// engine-generic (P1 MariaDB UI design D1), added as a NEW file — the
// existing `MysqlCredentials.svelte.test.ts` (21 tests) is this task's own
// behaviour-preserving gate and stays green UNMODIFIED.
//
// Vacuity: the "3307, not 3306" assertion below was proved able to fail by
// hardcoding `resolvedPort` to always read `port ?? 3306` regardless of
// `descriptor.defaultPort`, and confirming it went red. Reverted immediately.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import MysqlCredentials from './MysqlCredentials.svelte';
import type { MysqlConnectionProofDto, MysqlResetOutcomeDto } from '$lib/ipc';

function renderCredentials(
	props: Partial<{
		engine: 'mysql' | 'mariadb';
		major: string;
		port: number;
		resetOutcome: MysqlResetOutcomeDto;
		verifyResult: MysqlConnectionProofDto;
	}> = {}
): string {
	return render(MysqlCredentials, {
		props: {
			engine: props.engine,
			major: props.major ?? '11.4',
			port: props.port,
			socketPath: '/Users/x/.openvhost/run/mariadb.sock',
			revealed: false,
			revealing: false,
			passwordError: '',
			confirmingReset: false,
			resetting: false,
			resetOutcome: props.resetOutcome,
			resetError: '',
			verifying: false,
			verifyResult: props.verifyResult,
			verifyError: '',
			onReveal: () => {},
			onHide: () => {},
			onCopyPassword: () => {},
			onRequestReset: () => {},
			onCancelReset: () => {},
			onConfirmReset: () => {},
			onVerify: () => {}
		}
	}).body;
}

describe('MysqlCredentials — port defaults from the engine descriptor', () => {
	it('still shows 3306 for mysql (default engine), unchanged', () => {
		const body = renderCredentials();
		expect(body).toContain('data-testid="conn-value-port-11.4">3306<');
	});

	it('shows 3307 for mariadb, not mysql’s 3306', () => {
		const body = renderCredentials({ engine: 'mariadb' });
		expect(body).toContain('data-testid="conn-value-port-11.4">3307<');
		expect(body).not.toContain('>3306<');
	});

	it('an explicit port prop still wins over either engine default', () => {
		const body = renderCredentials({ engine: 'mariadb', port: 9999 });
		expect(body).toContain('data-testid="conn-value-port-11.4">9999<');
	});
});

describe('MysqlCredentials — engine="mariadb" paints the descriptor, not hardcoded MySQL copy', () => {
	it('uses the mariadb id prefix for its container test id', () => {
		const body = renderCredentials({ engine: 'mariadb' });
		expect(body).toContain('data-testid="mariadb-credentials-11.4"');
		expect(body).not.toContain('data-testid="mysql-credentials-11.4"');
	});

	it('names MariaDB, not MySQL, in the password field’s accessible label', () => {
		const body = renderCredentials({ engine: 'mariadb' });
		expect(body).toContain('aria-label="MariaDB 11.4 root password"');
	});
});

// Fix wave item 1 (whole-branch review HIGH): a fourth instance of the
// "shared component says MySQL" bug — `STALE_CREDENTIAL_RECOVERY` was a bare
// module constant this component rendered UNCONDITIONALLY for both engines,
// unlike every other string in this file, which goes through `descriptor`.
// It survived the earlier engine-generic sweep precisely because that sweep
// looked for CALL SITES (functions like `mysqlPackageOfferNotice`); this is a
// STRING CONSTANT, not a function, so nothing flagged it. Neither
// `MariadbResetOutcomeDto::AuthFailed` nor
// `MariadbConnectionProofDto::AuthFailed` is hypothetical — both are real,
// tested backend outcomes a MariaDB user can actually hit.
//
// Vacuity: each assertion below was proved able to fail by temporarily making
// `MARIADB_DESCRIPTOR.staleCredentialRecovery` equal
// `STALE_CREDENTIAL_RECOVERY` (i.e. MySQL's own text) in
// `databases.derive.ts`, and confirming both tests below went red — the body
// then contained "reset MySQL's root password manually" under
// `engine="mariadb"`. The mutation was reverted immediately after.
describe('MysqlCredentials — engine="mariadb" never tells a stale-credential user to go fix MySQL', () => {
	it('names MariaDB’s own recovery procedure after a reset auth failure, never MySQL’s', () => {
		const body = renderCredentials({
			engine: 'mariadb',
			resetOutcome: { kind: 'authFailed', detail: 'Access denied for user root' }
		});
		expect(body).toContain('data-testid="reset-auth-failed-11.4"');
		expect(body).toMatch(/reset MariaDB's root password manually/i);
		expect(body).toMatch(/MariaDB's own --skip-grant-tables recovery procedure/i);
		expect(body).not.toMatch(/MySQL/);
	});

	it('names MariaDB’s own recovery procedure after a verify auth failure, never MySQL’s', () => {
		const body = renderCredentials({
			engine: 'mariadb',
			verifyResult: { kind: 'authFailed', detail: 'Access denied for user root' }
		});
		expect(body).toContain('data-testid="verify-auth-failed-11.4"');
		expect(body).toMatch(/reset MariaDB's root password manually/i);
		expect(body).not.toMatch(/MySQL/);
	});
});
