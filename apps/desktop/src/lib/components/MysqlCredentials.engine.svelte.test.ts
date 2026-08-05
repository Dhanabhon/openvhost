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

function renderCredentials(
	props: Partial<{ engine: 'mysql' | 'mariadb'; major: string; port: number }> = {}
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
			resetError: '',
			verifying: false,
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
