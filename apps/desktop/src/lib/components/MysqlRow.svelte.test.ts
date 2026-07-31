// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), same approach as
// `LanguageRow.svelte.test.ts`. WHAT THIS FILE CANNOT COVER: no DOM, so click
// handlers are exercised only through the `onclick` prop wiring, not by
// simulating a real click.
//
// One test per named `MysqlRowState` variant (spec D6's eight states), plus
// the out-of-catalogue "truly actionless" row, which never enters that state
// machine at all.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import MysqlRow from './MysqlRow.svelte';
import type { MysqlInstanceDto, ServiceStatus } from '$lib/ipc';
import type { MysqlInitFailure, UiLog } from '$lib/databases.derive';

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

function renderRow(
	props: Partial<{
		instance: MysqlInstanceDto;
		brewFound: boolean;
		installingMajor: string;
		installLog: UiLog[];
		installOutcome: { major: string; exitCode: number | null; detected: boolean } | null;
		installError: string;
		initializingMajor: string;
		initLog: UiLog[];
		initFailure: MysqlInitFailure | null;
		initError: string;
		uninstallingMajor: string;
		catalogedMajorsList: string[];
		serviceState: ServiceStatus['state'] | null;
		password?: string;
		revealed: boolean;
		revealing: boolean;
		passwordError: string;
		resetting: boolean;
		resetOutcome?: { kind: 'reset' } | { kind: 'authFailed'; detail: string };
		resetError: string;
		verifying: boolean;
		verifyResult?:
			| { kind: 'ok'; version: string; port: number }
			| { kind: 'authFailed'; detail: string }
			| { kind: 'failed'; detail: string };
		verifyError: string;
	}> = {}
): string {
	return render(MysqlRow, {
		props: {
			instance: props.instance ?? instance(),
			brewFound: props.brewFound ?? true,
			installingMajor: props.installingMajor ?? '',
			installLog: props.installLog ?? [],
			installOutcome: props.installOutcome ?? null,
			installError: props.installError ?? '',
			initializingMajor: props.initializingMajor ?? '',
			initLog: props.initLog ?? [],
			initFailure: props.initFailure ?? null,
			initError: props.initError ?? '',
			uninstallingMajor: props.uninstallingMajor ?? '',
			catalogedMajorsList: props.catalogedMajorsList ?? ['8.4'],
			serviceState: props.serviceState ?? null,
			password: props.password,
			revealed: props.revealed ?? false,
			revealing: props.revealing ?? false,
			passwordError: props.passwordError ?? '',
			resetting: props.resetting ?? false,
			resetOutcome: props.resetOutcome,
			resetError: props.resetError ?? '',
			verifying: props.verifying ?? false,
			verifyResult: props.verifyResult,
			verifyError: props.verifyError ?? '',
			onInstall: () => {},
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

describe('MysqlRow — noBrew', () => {
	it('offers no Install button when Homebrew is missing', () => {
		const body = renderRow({ brewFound: false });
		expect(body).toContain('data-testid="mysql-row-8.4"');
		expect(body).not.toContain('data-testid="install-8.4"');
	});
});

describe('MysqlRow — notInstalled', () => {
	it('offers Install and discloses the Homebrew-own-datadir fact', () => {
		const body = renderRow({ brewFound: true });
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).toMatch(/separate data directory/i);
	});

	it('disables Install while any install/init is running elsewhere', () => {
		const body = renderRow({ installingMajor: '9.9' });
		expect(body.match(/data-testid="install-8\.4"[^>]*>/)?.[0]).toContain('disabled');
	});

	it('renders a brew exit failure distinctly from a thrown error', () => {
		const body = renderRow({
			installOutcome: { major: '8.4', exitCode: 1, detected: false },
			installError: ''
		});
		expect(body).toMatch(/exited with code 1/i);
	});

	it('renders a thrown install error', () => {
		const body = renderRow({ installError: 'brew: no such formula' });
		expect(body).toContain('brew: no such formula');
	});
});

describe('MysqlRow — installing', () => {
	it('shows the live log and no Install button', () => {
		const log: UiLog[] = [{ id: '8.4', tsMs: 1, level: 'info', line: 'Pouring bottle...' }];
		const body = renderRow({ installingMajor: '8.4', installLog: log });
		expect(body).toContain('Pouring bottle...');
		expect(body).not.toContain('data-testid="install-8.4"');
	});
});

describe('MysqlRow — installedNotInitialized', () => {
	it('offers Initialize', () => {
		const body = renderRow({ instance: instance({ installed: true }) });
		expect(body).toContain('data-testid="initialize-8.4"');
	});

	it('disables Initialize while any install/init is running elsewhere', () => {
		const body = renderRow({
			instance: instance({ installed: true }),
			initializingMajor: '9.9'
		});
		expect(body.match(/data-testid="initialize-8\.4"[^>]*>/)?.[0]).toContain('disabled');
	});
});

describe('MysqlRow — initializing', () => {
	it('shows the live log and no Initialize button', () => {
		const log: UiLog[] = [{ id: '8.4', tsMs: 1, level: 'info', line: 'Rendering my.cnf...' }];
		const body = renderRow({
			instance: instance({ installed: true }),
			initializingMajor: '8.4',
			initLog: log
		});
		expect(body).toContain('Rendering my.cnf...');
		expect(body).not.toContain('data-testid="initialize-8.4"');
	});
});

describe('MysqlRow — a thrown initialize error (distinct from a settled Failed outcome)', () => {
	it('renders a thrown init error even though the row stays installedNotInitialized', () => {
		const body = renderRow({
			instance: instance({ installed: true }),
			initError: 'an install is already running'
		});
		expect(body).toContain('an install is already running');
		expect(body).toContain('data-testid="initialize-8.4"');
	});
});

describe('MysqlRow — initFailed', () => {
	it('names the step in plain language, shows the reason, and offers Retry', () => {
		const body = renderRow({
			instance: instance({ installed: true }),
			initFailure: { major: '8.4', step: 'setPassword', reason: 'unexpected EOF' }
		});
		expect(body).toContain('data-testid="init-failed-8.4"');
		expect(body).toMatch(/setting the root password/i);
		expect(body).toContain('unexpected EOF');
		expect(body).toContain('data-testid="retry-init-8.4"');
	});
});

describe('MysqlRow — datadirForeign', () => {
	it('reports the foreign content honestly and offers no destructive/initialize action', () => {
		const body = renderRow({
			instance: instance({
				installed: true,
				datadirState: { kind: 'foreign', detail: 'found stray.ibd' }
			})
		});
		expect(body).toContain('data-testid="datadir-foreign-8.4"');
		expect(body).toContain('found stray.ibd');
		expect(body).not.toContain('data-testid="initialize-8.4"');
		expect(body).not.toMatch(/delete|remove|overwrite/i);
	});
});

describe('MysqlRow — ready', () => {
	const ready = instance({
		installed: true,
		datadirState: { kind: 'initialized' },
		socketPath: '/Users/x/.openvhost/run/mysql-8.4.sock',
		serviceId: 'mysql-8.4'
	});

	it('renders the credentials block (connection, masked password, verify)', () => {
		const body = renderRow({ instance: ready, serviceState: { kind: 'stopped' } });
		expect(body).toContain('data-testid="mysql-credentials-8.4"');
		expect(body).toContain('/Users/x/.openvhost/run/mysql-8.4.sock');
		expect(body).toContain('data-testid="verify-connection-8.4"');
	});

	// Review fix, pinned at the wiring layer too (MysqlCredentials.svelte.test.ts
	// pins the component's own logic in isolation): a cached password with the
	// display gate off must stay masked even once threaded through the row.
	it('keeps the password masked when cached but not revealed, even through the row', () => {
		const body = renderRow({
			instance: ready,
			password: 'not-a-real-password',
			revealed: false
		});
		expect(body).not.toContain('not-a-real-password');
		expect(body).toContain('type="password"');
	});

	it('shows the password only once both cached and revealed, threaded through the row', () => {
		const body = renderRow({
			instance: ready,
			password: 'not-a-real-password',
			revealed: true
		});
		expect(body).toContain('not-a-real-password');
		expect(body).toContain('type="text"');
	});

	it('offers Start when stopped, Stop when running, Retry when failed', () => {
		expect(renderRow({ instance: ready, serviceState: { kind: 'stopped' } })).toContain(
			'data-testid="start-mysql-8.4"'
		);
		expect(renderRow({ instance: ready, serviceState: { kind: 'running' } })).toContain(
			'data-testid="stop-mysql-8.4"'
		);
		expect(
			renderRow({
				instance: ready,
				serviceState: { kind: 'failed', exit: 1, stderrTail: ['boom'] }
			})
		).toContain('data-testid="retry-mysql-8.4"');
	});

	it("shows the supervisor's own stderr tail on a failed pool", () => {
		const body = renderRow({
			instance: ready,
			serviceState: {
				kind: 'failed',
				exit: 1,
				stderrTail: ['[ERROR] unable to bind listening socket']
			}
		});
		expect(body).toContain('unable to bind listening socket');
	});

	it('points at brew services stop for a port-3306 conflict', () => {
		const body = renderRow({
			instance: ready,
			serviceState: {
				kind: 'failed',
				exit: 1,
				stderrTail: ['[ERROR] Address already in use']
			}
		});
		expect(body).toContain('brew services stop mysql@8.4');
	});

	it('does not show the port-conflict hint for an unrelated failure', () => {
		const body = renderRow({
			instance: ready,
			serviceState: { kind: 'failed', exit: 1, stderrTail: ['some other error'] }
		});
		expect(body).not.toContain('brew services stop');
	});

	it('renders no lifecycle control while the supervisor snapshot has not arrived', () => {
		const body = renderRow({ instance: ready, serviceState: null });
		expect(body).not.toMatch(/data-testid="(start|stop|retry)-mysql-8\.4"/);
	});
});

describe('MysqlRow — out-of-catalogue (truly actionless)', () => {
	const foreign = instance({
		major: '9.7',
		cataloged: false,
		installed: true,
		path: '/opt/homebrew/opt/mysql/bin/mysqld',
		datadirState: { kind: 'initialized' },
		socketPath: '/Users/x/.openvhost/run/mysql-9.7.sock',
		serviceId: 'mysql-9.7'
	});

	it('explains that this build does not manage it, naming the cataloged majors', () => {
		const body = renderRow({ instance: foreign, catalogedMajorsList: ['8.4'] });
		expect(body).toContain('data-testid="out-of-catalogue-9.7"');
		expect(body).toContain('8.4');
	});

	it('offers no install, initialize, start/stop, or credential actions whatsoever', () => {
		const body = renderRow({
			instance: foreign,
			serviceState: { kind: 'running' }
		});
		expect(body).not.toContain('data-testid="install-9.7"');
		expect(body).not.toContain('data-testid="initialize-9.7"');
		expect(body).not.toMatch(/data-testid="(start|stop|retry)-mysql-9\.7"/);
		expect(body).not.toContain('data-testid="mysql-credentials-9.7"');
		expect(body).not.toContain('data-testid="verify-connection-9.7"');
	});

	it('still names the major so it is not simply invisible', () => {
		const body = renderRow({ instance: foreign });
		expect(body).toContain('9.7');
	});
});

/** Just the Uninstall button's own opening tag, so a `disabled` assertion can
 *  fail for the reason it names rather than matching another control on the
 *  row. */
function uninstallTag(body: string, major: string): string {
	const match = body.match(new RegExp(`<button[^>]*data-testid="uninstall-${major}"[^>]*>`));
	if (!match) throw new Error(`expected an Uninstall button for ${major}`);
	return match[0];
}

// Package-uninstall design D6: an installed managed major gets an Uninstall
// action. What the confirmation SAYS is `UninstallDialog.svelte.test.ts` and
// `uninstall.derive.test.ts`; this file only pins when the action exists and
// when it is live.
describe('MysqlRow — the Uninstall action', () => {
	const ready = instance({
		installed: true,
		datadirState: { kind: 'initialized' },
		serviceId: 'mysql-8.4',
		socketPath: '/Users/x/.openvhost/run/mysql-8.4.sock'
	});

	it('offers Uninstall for a ready major', () => {
		const body = renderRow({ instance: ready, serviceState: { kind: 'stopped' } });
		expect(body).toContain('data-testid="uninstall-8.4"');
	});

	// Installed but never initialized: the engine's files are on disk, so there
	// is something to remove — and this is a likely moment to want it gone.
	it('offers Uninstall for an installed major that was never initialized', () => {
		const body = renderRow({ instance: instance({ installed: true }) });
		expect(body).toContain('data-testid="uninstall-8.4"');
		expect(body).toContain('data-testid="initialize-8.4"');
	});

	// Removing the engine never touches a datadir (design D2), so a datadir this
	// app refuses to adopt is no reason to trap the binaries with it.
	it('offers Uninstall for an installed major with a foreign datadir', () => {
		const body = renderRow({
			instance: instance({ installed: true, datadirState: { kind: 'foreign', detail: 'x' } })
		});
		expect(body).toContain('data-testid="uninstall-8.4"');
	});

	it('offers no Uninstall for a major that is not installed', () => {
		const body = renderRow({ instance: instance({ installed: false }), brewFound: true });
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).not.toContain('data-testid="uninstall-8.4"');
	});

	// The out-of-catalogue row renders NO action of any kind on purpose (spec
	// D1) — every command rejects an unmanaged major server-side, so an
	// Uninstall button here could only produce an error.
	it('offers no Uninstall for an installed major this build does not manage', () => {
		const body = renderRow({
			instance: instance({ major: '5.7', cataloged: false, installed: true }),
			catalogedMajorsList: ['8.4']
		});
		expect(body).toContain('data-testid="mysql-row-5.7"');
		expect(body).not.toContain('data-testid="uninstall-5.7"');
	});

	it('is enabled when nothing is in flight', () => {
		const body = renderRow({ instance: ready });
		expect(uninstallTag(body, '8.4')).not.toContain('disabled');
	});

	it('is disabled while an install is running', () => {
		const body = renderRow({ instance: ready, installingMajor: '8.4' });
		expect(uninstallTag(body, '8.4')).toContain('disabled');
	});

	it('is disabled while an initialize is running', () => {
		const body = renderRow({ instance: ready, initializingMajor: '8.4' });
		expect(uninstallTag(body, '8.4')).toContain('disabled');
	});

	it('is disabled while ANOTHER major is being uninstalled', () => {
		const body = renderRow({ instance: ready, uninstallingMajor: '8.0' });
		expect(uninstallTag(body, '8.4')).toContain('disabled');
		expect(uninstallTag(body, '8.4')).not.toContain('Uninstalling');
	});

	it('is disabled and says what it is doing while THIS major is uninstalled', () => {
		const body = renderRow({ instance: ready, uninstallingMajor: '8.4' });
		expect(uninstallTag(body, '8.4')).toContain('disabled');
		expect(body).toContain('Uninstalling…');
	});

	it('names the version in its accessible label', () => {
		const body = renderRow({ instance: ready });
		expect(uninstallTag(body, '8.4')).toContain('aria-label="Uninstall MySQL 8.4"');
	});
});
