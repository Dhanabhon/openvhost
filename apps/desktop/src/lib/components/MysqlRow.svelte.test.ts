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
import type {
	MysqlInstallOutcomeDto,
	MysqlInstallProgressDto,
	MysqlInstanceDto,
	ServiceStatus
} from '$lib/ipc';
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
		source: null,
		offer: { kind: 'available', version: '8.4.11' },
		...overrides
	};
}

/** An installed row from OpenVHost's own package tree. */
function packaged(overrides: Partial<MysqlInstanceDto> = {}): MysqlInstanceDto {
	return instance({
		installed: true,
		source: { kind: 'packaged', version: '8.4.11' },
		...overrides
	});
}

/** An installed row from a Homebrew keg — still supported during the migration
 *  (design D3/D7), and still uninstallable through the brew path. */
function brewed(overrides: Partial<MysqlInstanceDto> = {}): MysqlInstanceDto {
	return instance({ installed: true, source: { kind: 'homebrew' }, ...overrides });
}

function renderRow(
	props: Partial<{
		instance: MysqlInstanceDto;
		installingMajor: string;
		installProgress: MysqlInstallProgressDto | null;
		installTotal: number | null;
		cancellingInstall: boolean;
		installOutcome: MysqlInstallOutcomeDto | null;
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
			installingMajor: props.installingMajor ?? '',
			installProgress: props.installProgress ?? null,
			installTotal: props.installTotal ?? null,
			cancellingInstall: props.cancellingInstall ?? false,
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

// The state that replaced `noBrew`. Installing MySQL no longer involves
// Homebrew at all, so "you have no brew" is not a reason to withhold anything;
// "this build publishes no checksum-verified download for your architecture" is.
describe('MysqlRow — unavailable (no verified download for this host)', () => {
	const intel = instance({ offer: { kind: 'unavailable', target: 'macos-x86_64' } });

	it('offers no Install button — an absence, not a button that would throw', () => {
		const body = renderRow({ instance: intel });
		expect(body).toContain('data-testid="mysql-row-8.4"');
		expect(body).not.toContain('data-testid="install-8.4"');
	});

	it('explains the absence, names the target, and points at the route that works', () => {
		const body = renderRow({ instance: intel });
		expect(body).toContain('data-testid="mysql-unavailable-8.4"');
		expect(body).toContain('macos-x86_64');
		expect(body).toContain('Homebrew');
	});

	// It must not read as a fault. The build exists upstream; OpenVHost has
	// simply not verified those bytes, and saying so is the honest version.
	it('reads as an absence rather than an error', () => {
		const body = renderRow({ instance: intel });
		expect(body).not.toMatch(/role="alert"/);
	});
});

describe('MysqlRow — notInstalled', () => {
	it('offers Install and says what pressing it will actually do', () => {
		const body = renderRow();
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).toContain('data-testid="offer-8.4"');
		expect(body).toContain('8.4.11');
		expect(body).toMatch(/SHA-256/);
	});

	it('no longer claims Homebrew creates a data directory as a side effect', () => {
		const body = renderRow();
		expect(body).not.toMatch(/separate data directory/i);
		expect(body).toMatch(/shared per version/i);
	});

	it('disables Install while any install/init is running elsewhere', () => {
		const body = renderRow({ installingMajor: '9.9' });
		expect(body.match(/data-testid="install-8\.4"[^>]*>/)?.[0]).toContain('disabled');
	});

	// The mandatory one, at the render layer: a checksum mismatch must not be
	// dressed up as a connection problem.
	it('renders a checksum failure as a checksum failure, not a network error', () => {
		const body = renderRow({
			installOutcome: {
				major: '8.4',
				result: { kind: 'verificationFailed', expected: 'a'.repeat(64), actual: 'b'.repeat(64) }
			}
		});
		expect(body).toContain('data-testid="install-outcome-8.4"');
		expect(body).toMatch(/checksum did not match/i);
		expect(body).toContain('b'.repeat(64));
		expect(body).not.toMatch(/network error/i);
	});

	it('renders a stalled transfer distinctly from a checksum failure', () => {
		const stalled = renderRow({
			installOutcome: { major: '8.4', result: { kind: 'stalled', detail: 'no data for 30.0s' } }
		});
		expect(stalled).toMatch(/stopped making progress/i);
		expect(stalled).not.toMatch(/checksum did not match/i);
	});

	it('renders a cancelled install as a clean stop, promising nothing was left behind', () => {
		const body = renderRow({ installOutcome: { major: '8.4', result: { kind: 'cancelled' } } });
		expect(body).toMatch(/install cancelled/i);
		expect(body).toMatch(/no half-downloaded files/i);
	});

	it('renders a thrown install error', () => {
		const body = renderRow({ installError: 'an install is already running' });
		expect(body).toContain('an install is already running');
	});

	it('shows another row\u2019s outcome nowhere near this one', () => {
		const body = renderRow({
			installOutcome: { major: '9.9', result: { kind: 'cancelled' } }
		});
		expect(body).not.toContain('data-testid="install-outcome-8.4"');
	});
});

describe('MysqlRow — installing', () => {
	it('shows the pipeline state and no Install button', () => {
		const body = renderRow({
			installingMajor: '8.4',
			installProgress: { kind: 'downloaded', bytes: 1024 },
			installTotal: 4096
		});
		expect(body).toContain('data-testid="install-progress-8.4"');
		expect(body).toMatch(/1\.00 KiB of 4\.00 KiB/);
		expect(body).not.toContain('data-testid="install-8.4"');
	});

	// The distinction golden rule 6 buys, at the render layer: a download that
	// was checked and one that merely arrived must not read the same.
	it('renders "verified" and "extracted" as different sentences', () => {
		const verified = renderRow({ installingMajor: '8.4', installProgress: { kind: 'verified' } });
		const extracted = renderRow({ installingMajor: '8.4', installProgress: { kind: 'extracted' } });
		const line = (body: string) =>
			body.match(/data-testid="install-progress-8\.4"[^>]*>([^<]*)</)?.[1] ?? '';
		expect(line(verified)).not.toBe('');
		expect(line(extracted)).not.toBe('');
		expect(line(verified)).not.toBe(line(extracted));
		expect(line(verified)).toMatch(/checksum/i);
	});

	it('says something honest before the first pipeline event arrives', () => {
		const body = renderRow({ installingMajor: '8.4', installProgress: null });
		expect(body).toContain('data-testid="install-progress-8.4"');
		expect(body).toMatch(/preparing the download/i);
		expect(body).not.toContain('data-testid="install-bar-8.4"');
	});

	it('draws a bar only when there is a real denominator to draw against', () => {
		const withTotal = renderRow({
			installingMajor: '8.4',
			installProgress: { kind: 'downloaded', bytes: 1024 },
			installTotal: 4096
		});
		const without = renderRow({
			installingMajor: '8.4',
			installProgress: { kind: 'downloaded', bytes: 1024 },
			installTotal: null
		});
		expect(withTotal).toContain('aria-valuenow="25"');
		expect(without).not.toContain('data-testid="install-bar-8.4"');
	});

	// MANDATORY. The download has no wall-clock bound and the package
	// pipeline's install permit is process-wide, so an install nobody can stop
	// starves every later one.
	it('offers Cancel while the install is running', () => {
		const body = renderRow({ installingMajor: '8.4', installProgress: { kind: 'verified' } });
		expect(body).toContain('data-testid="cancel-install-8.4"');
	});

	it('offers no Cancel when nothing is installing', () => {
		expect(renderRow()).not.toContain('data-testid="cancel-install-8.4"');
		expect(renderRow({ instance: packaged() })).not.toContain('data-testid="cancel-install-8.4"');
	});

	it('shows Cancel as already in flight rather than idle after it is pressed', () => {
		const body = renderRow({
			installingMajor: '8.4',
			installProgress: { kind: 'downloaded', bytes: 1 },
			cancellingInstall: true
		});
		expect(body).toMatch(/Cancelling…/);
		expect(body.match(/data-testid="cancel-install-8\.4"[^>]*>/)?.[0]).toContain('disabled');
	});
});

describe('MysqlRow — where a runtime came from', () => {
	// The whole reason `source` exists: the owner will be running a
	// brew-installed 8.4 and a packaged 8.4 at the same time.
	it('shows the exact version for a runtime OpenVHost installed', () => {
		const body = renderRow({ instance: packaged() });
		expect(body).toContain('data-testid="mysql-source-8.4"');
		expect(body).toContain('OpenVHost 8.4.11');
	});

	it('labels a Homebrew keg as Homebrew, inventing no patch version', () => {
		const body = renderRow({ instance: brewed() });
		const badge = body.match(/data-testid="mysql-source-8\.4"[^>]*>([^<]*)</)?.[1] ?? '';
		expect(badge).toBe('Homebrew');
		expect(badge).not.toMatch(/\d/);
	});

	it('shows no source badge for a major that is not installed', () => {
		expect(renderRow()).not.toContain('data-testid="mysql-source-8.4"');
	});

	// The out-of-catalogue row offers no ACTION (spec D1), but provenance is not
	// an action — "which mysqld am I actually running" is exactly the question
	// an unmanaged 9.x raises.
	it('still says where an out-of-catalogue runtime came from', () => {
		const body = renderRow({
			instance: brewed({ major: '9.7', cataloged: false }),
			catalogedMajorsList: ['8.4']
		});
		expect(body).toContain('data-testid="mysql-source-9.7"');
		expect(body).not.toContain('data-testid="install-9.7"');
		expect(body).not.toContain('data-testid="uninstall-9.7"');
	});
});

describe('MysqlRow — Uninstall is offered only where it can actually work', () => {
	// `openvhost-pkg` has no uninstall counterpart at all yet, and the existing
	// dialog drives `brew uninstall`. An affordance that is present and fails is
	// worse than one that is absent.
	it('offers no Uninstall for a runtime OpenVHost installed itself', () => {
		const body = renderRow({ instance: packaged({ datadirState: { kind: 'initialized' } }) });
		expect(body).not.toContain('data-testid="uninstall-8.4"');
		expect(body).toContain('data-testid="no-uninstall-8.4"');
		expect(body).toMatch(/not built yet/i);
	});

	it('still offers Uninstall for a Homebrew keg, which the brew path can remove', () => {
		const body = renderRow({ instance: brewed({ datadirState: { kind: 'initialized' } }) });
		expect(body).toContain('data-testid="uninstall-8.4"');
		expect(body).not.toContain('data-testid="no-uninstall-8.4"');
	});

	it('withholds Uninstall from a packaged runtime in every lifecycle state, not just Ready', () => {
		for (const datadirState of [
			{ kind: 'notInitialized' } as const,
			{ kind: 'initialized' } as const,
			{ kind: 'foreign', detail: 'stray.ibd' } as const
		]) {
			expect(renderRow({ instance: packaged({ datadirState }) })).not.toContain(
				'data-testid="uninstall-8.4"'
			);
		}
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
			instance: brewed({ datadirState: { kind: 'foreign', detail: 'found stray.ibd' } })
		});
		expect(body).toContain('data-testid="datadir-foreign-8.4"');
		expect(body).toContain('found stray.ibd');
		expect(body).not.toContain('data-testid="initialize-8.4"');
		// Nothing on this row offers to touch the foreign content itself —
		// scoped to the foreign note so the assertion fails for its own reason
		// rather than matching unrelated copy elsewhere on the row.
		const note = body.match(/data-testid="datadir-foreign-8\.4"[\s\S]*?<\/p>/)?.[0] ?? '';
		expect(note).not.toBe('');
		expect(note).not.toMatch(/delete|remove|overwrite/i);
	});
});

describe('MysqlRow — ready', () => {
	const ready = brewed({
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
	// A HOMEBREW keg throughout: since the move off Homebrew, the brew-driven
	// uninstall path only applies to runtimes brew installed. A packaged
	// runtime's absence of this control is pinned in its own describe above.
	const ready = brewed({
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
		const body = renderRow({ instance: brewed() });
		expect(body).toContain('data-testid="uninstall-8.4"');
		expect(body).toContain('data-testid="initialize-8.4"');
	});

	// Removing the engine never touches a datadir (design D2), so a datadir this
	// app refuses to adopt is no reason to trap the binaries with it.
	it('offers Uninstall for an installed major with a foreign datadir', () => {
		const body = renderRow({
			instance: brewed({ datadirState: { kind: 'foreign', detail: 'x' } })
		});
		expect(body).toContain('data-testid="uninstall-8.4"');
	});

	it('offers no Uninstall for a major that is not installed', () => {
		const body = renderRow({ instance: instance({ installed: false }) });
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).not.toContain('data-testid="uninstall-8.4"');
	});

	// The out-of-catalogue row renders NO action of any kind on purpose (spec
	// D1) — every command rejects an unmanaged major server-side, so an
	// Uninstall button here could only produce an error.
	it('offers no Uninstall for an installed major this build does not manage', () => {
		const body = renderRow({
			instance: brewed({ major: '5.7', cataloged: false }),
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
