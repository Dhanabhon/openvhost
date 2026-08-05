// SPDX-License-Identifier: GPL-3.0-or-later
//
// Mirrors `databases.svelte.test.ts`'s structure and discipline, adapted for
// SCALARS rather than per-major dictionaries (P1 MariaDB UI design D1):
// every assertion below reads a bare field (`s.installing`, `s.password`, …)
// instead of indexing one by major, because this build ships exactly one
// series and there is no "which" left to key on.

import { describe, expect, it, vi } from 'vitest';
import { MARIADB_SERIES, MariadbStore, mariadbInstance, type MariadbApi } from './mariadb.svelte';
import type {
	MariadbConnectionProofDto,
	MariadbEnvironmentDto,
	MariadbInitOutcomeDto,
	MariadbInstallOutcomeDto,
	MariadbResetOutcomeDto
} from './ipc';

/** A promise plus its own resolver — mirrors `databases.svelte.test.ts`'s
 *  identical `deferred` helper. */
function deferred(): { promise: Promise<void>; release: () => void } {
	let release = (): void => {};
	const promise = new Promise<void>((r) => {
		release = r;
	});
	return { promise, release };
}

function env(overrides: Partial<MariadbEnvironmentDto> = {}): MariadbEnvironmentDto {
	return {
		installed: false,
		version: null,
		path: null,
		socketPath: null,
		serviceId: null,
		datadirState: { kind: 'notInitialized' },
		offer: { kind: 'available', version: '11.4.9' },
		...overrides
	};
}

/** A settled `cancelled` outcome, the shape `install()` resolves with once
 *  the backend's abort handle has fired. */
function cancelled(): MariadbInstallOutcomeDto {
	return { result: { kind: 'cancelled' } };
}

/** A settled install outcome. `installed` by default. */
function installed(overrides: Partial<MariadbInstallOutcomeDto> = {}): MariadbInstallOutcomeDto {
	return {
		result: { kind: 'installed', version: '11.4.9', detected: true, ledger: { kind: 'recorded' } },
		...overrides
	};
}

/** Never a real-looking password — same MANDATORY rule
 *  `databases.svelte.test.ts` states for itself (spec D3: no secret ever
 *  belongs in a test snapshot). */
const FAKE_REVEALED = 'not-a-real-password';

function api(overrides: Partial<MariadbApi> = {}): MariadbApi {
	return {
		mariadbEnvironment: vi.fn(async () => env()),
		rescanMariadb: vi.fn(async () => env()),
		installMariadb: vi.fn(async () => installed()),
		cancelMariadbInstall: vi.fn(async () => true),
		initializeMariadb: vi.fn(async () => ({ kind: 'initialized' }) as MariadbInitOutcomeDto),
		mariadbRootPassword: vi.fn(async () => FAKE_REVEALED),
		resetMariadbRootPassword: vi.fn(async () => ({ kind: 'reset' }) as MariadbResetOutcomeDto),
		verifyMariadbConnection: vi.fn(
			async () => ({ kind: 'ok', version: '11.4.9', port: 3307 }) as MariadbConnectionProofDto
		),
		...overrides
	};
}

describe('MARIADB_SERIES', () => {
	it('is the one series this build ships, matching openvhost_core::MARIADB_SERIES', () => {
		expect(MARIADB_SERIES).toBe('11.4');
	});
});

describe('mariadbInstance — the EngineInstanceDto adapter', () => {
	it('invents the identity value and cataloged flag the wire DTO carries neither of', () => {
		const instance = mariadbInstance(env({ installed: true }));
		expect(instance.major).toBe(MARIADB_SERIES);
		expect(instance.cataloged).toBe(true);
	});

	it('carries every other field straight through, unmodified', () => {
		const source = env({
			installed: true,
			path: '/x/mariadbd',
			socketPath: '/x/run/mariadb.sock',
			serviceId: 'mariadb-11.4',
			datadirState: { kind: 'initialized', version: '11.4.9' }
		});
		const instance = mariadbInstance(source);
		expect(instance.installed).toBe(true);
		expect(instance.path).toBe('/x/mariadbd');
		expect(instance.socketPath).toBe('/x/run/mariadb.sock');
		expect(instance.serviceId).toBe('mariadb-11.4');
		expect(instance.datadirState).toEqual({ kind: 'initialized', version: '11.4.9' });
		expect(instance.offer).toEqual({ kind: 'available', version: '11.4.9' });
	});

	// MariaDB has no provenance to disambiguate — one source, nothing to show.
	it('always reports no source, unconditionally', () => {
		expect(mariadbInstance(env({ installed: true })).source).toBeNull();
	});
});

describe('MariadbStore — environment load', () => {
	it('lists what the backend returns', async () => {
		const s = new MariadbStore(
			api({ mariadbEnvironment: vi.fn(async () => env({ installed: true })) })
		);
		await s.refresh();
		expect(s.env?.installed).toBe(true);
	});

	it('exposes anyInstalled from the loaded environment', async () => {
		const s = new MariadbStore(
			api({ mariadbEnvironment: vi.fn(async () => env({ installed: true })) })
		);
		await s.refresh();
		expect(s.anyInstalled).toBe(true);
	});

	it('is not anyInstalled before the first load settles', () => {
		const s = new MariadbStore(api());
		expect(s.anyInstalled).toBe(false);
	});

	it('keeps the last known environment when a refresh fails', async () => {
		let calls = 0;
		const s = new MariadbStore(
			api({
				mariadbEnvironment: vi.fn(async () => {
					calls += 1;
					if (calls === 1) return env({ installed: true });
					throw { kind: 'core', message: 'transient' };
				})
			})
		);
		await s.refresh();
		await s.refresh();
		expect(s.error).toContain('transient');
		expect(s.env?.installed).toBe(true);
	});

	it('keeps the last known environment when a rescan fails', async () => {
		let calls = 0;
		const s = new MariadbStore(
			api({
				mariadbEnvironment: vi.fn(async () => env({ installed: true })),
				rescanMariadb: vi.fn(async () => {
					calls += 1;
					if (calls === 1) return env({ installed: true });
					throw { kind: 'core', message: 'transient' };
				})
			})
		);
		await s.rescan();
		await s.rescan();
		expect(s.error).toContain('transient');
		expect(s.env?.installed).toBe(true);
	});

	it('records an outside failure on the same channel', () => {
		const s = new MariadbStore(api());
		s.fail({ kind: 'core', message: 'listener could not be registered' });
		expect(s.error).toContain('listener could not be registered');
	});
});

describe('MariadbStore — install', () => {
	it('marks installing and clears it when done, exposing the row-facing installingMajor', async () => {
		const s = new MariadbStore(api());
		const p = s.install();
		expect(s.installing).toBe(true);
		expect(s.installingMajor).toBe(MARIADB_SERIES);
		expect(await p).toBe(true);
		expect(s.installing).toBe(false);
		expect(s.installingMajor).toBe('');
	});

	it('refuses a second install while one is running', async () => {
		let calls = 0;
		const s = new MariadbStore(
			api({
				installMariadb: vi.fn(async () => {
					calls += 1;
					await new Promise((r) => setTimeout(r, 5));
					return installed();
				})
			})
		);
		await Promise.all([s.install(), s.install()]);
		expect(calls).toBe(1);
	});

	it('surfaces the error and clears installing when the install throws', async () => {
		const s = new MariadbStore(
			api({
				installMariadb: vi.fn(async () => {
					throw { kind: 'core', message: 'an install is already running' };
				})
			})
		);
		expect(await s.install()).toBe(false);
		expect(s.error).toContain('an install is already running');
		expect(s.installing).toBe(false);
	});

	it('remembers the pipeline state as each event arrives', () => {
		const s = new MariadbStore(api());
		s.applyInstallProgress({ kind: 'started', total: 4096 });
		expect(s.installProgress).toEqual({ kind: 'started', total: 4096 });
		s.applyInstallProgress({ kind: 'downloaded', bytes: 1024 });
		expect(s.installProgress).toEqual({ kind: 'downloaded', bytes: 1024 });
		s.applyInstallProgress({ kind: 'verified' });
		expect(s.installProgress).toEqual({ kind: 'verified' });
	});

	it('keeps the declared total across every later event', () => {
		const s = new MariadbStore(api());
		s.applyInstallProgress({ kind: 'started', total: 4096 });
		s.applyInstallProgress({ kind: 'downloaded', bytes: 1024 });
		s.applyInstallProgress({ kind: 'verified' });
		expect(s.installTotal).toBe(4096);
	});

	it('leaves the total null when the server declared none, rather than guessing', () => {
		const s = new MariadbStore(api());
		s.applyInstallProgress({ kind: 'started', total: null });
		s.applyInstallProgress({ kind: 'downloaded', bytes: 1024 });
		expect(s.installTotal).toBeNull();
	});

	it('clears the previous run’s progress as a new install starts, not when it ends', async () => {
		const s = new MariadbStore(
			api({
				installMariadb: vi.fn(async () => {
					expect(s.installProgress).toBeNull();
					expect(s.installTotal).toBeNull();
					return installed();
				})
			})
		);
		s.applyInstallProgress({ kind: 'started', total: 4096 });
		s.applyInstallProgress({ kind: 'verified' });
		await s.install();
	});

	it('caps the install log so a long install cannot grow without bound', () => {
		const s = new MariadbStore(api());
		for (let i = 0; i < 500; i += 1) s.appendInstallLog(`line ${i}`);
		expect(s.installLog.length).toBeLessThanOrEqual(200);
		expect(s.installLog.at(-1)?.line).toBe('line 499');
	});

	it('re-reads the environment after a successful install rather than assuming', async () => {
		let calls = 0;
		const s = new MariadbStore(
			api({
				mariadbEnvironment: vi.fn(async () => {
					calls += 1;
					return env({ installed: calls > 1 });
				})
			})
		);
		await s.refresh();
		expect(s.env?.installed).toBe(false);
		await s.install();
		expect(calls).toBe(2);
		expect(s.env?.installed).toBe(true);
	});
});

describe('MariadbStore — cancel an install', () => {
	it('reaches the backend while an install is in flight', async () => {
		const cancelMariadbInstall = vi.fn(async () => true);
		const held = deferred();
		const s = new MariadbStore(
			api({
				cancelMariadbInstall,
				installMariadb: vi.fn(async () => {
					await held.promise;
					return cancelled();
				})
			})
		);
		const running = s.install();
		await s.cancelInstall();
		expect(cancelMariadbInstall).toHaveBeenCalledTimes(1);
		expect(s.cancellingInstall).toBe(true);
		held.release();
		await running;
	});

	it('does nothing when no install is running — there is no permit to reclaim', async () => {
		const cancelMariadbInstall = vi.fn(async () => false);
		const s = new MariadbStore(api({ cancelMariadbInstall }));
		await s.cancelInstall();
		expect(cancelMariadbInstall).not.toHaveBeenCalled();
		expect(s.cancellingInstall).toBe(false);
	});

	it('is idempotent while the first cancel is still settling', async () => {
		const cancelMariadbInstall = vi.fn(async () => true);
		const held = deferred();
		const s = new MariadbStore(
			api({
				cancelMariadbInstall,
				installMariadb: vi.fn(async () => {
					await held.promise;
					return cancelled();
				})
			})
		);
		const running = s.install();
		await s.cancelInstall();
		await s.cancelInstall();
		expect(cancelMariadbInstall).toHaveBeenCalledTimes(1);
		held.release();
		await running;
	});

	it('settles the row with a cancelled outcome, and leaves it installable again', async () => {
		const s = new MariadbStore(api({ installMariadb: vi.fn(async () => cancelled()) }));
		await s.install();
		expect(s.installOutcome?.result).toEqual({ kind: 'cancelled' });
		expect(s.installing).toBe(false);
		expect(s.cancellingInstall).toBe(false);
	});

	it('puts the button back when nothing was actually stopped', async () => {
		const held = deferred();
		const s = new MariadbStore(
			api({
				cancelMariadbInstall: vi.fn(async () => false),
				installMariadb: vi.fn(async () => {
					await held.promise;
					return installed();
				})
			})
		);
		const running = s.install();
		await s.cancelInstall();
		expect(s.cancellingInstall).toBe(false);
		expect(s.error).toBe('');
		held.release();
		await running;
	});
});

describe('MariadbStore — initialize', () => {
	it('marks initializing and clears it when done, exposing initializingMajor', async () => {
		const s = new MariadbStore(api());
		const p = s.initialize();
		expect(s.initializing).toBe(true);
		expect(s.initializingMajor).toBe(MARIADB_SERIES);
		expect(await p).toBe(true);
		expect(s.initializing).toBe(false);
		expect(s.initializingMajor).toBe('');
	});

	it('refuses a second initialize while one is running', async () => {
		let calls = 0;
		const s = new MariadbStore(
			api({
				initializeMariadb: vi.fn(async (): Promise<MariadbInitOutcomeDto> => {
					calls += 1;
					await new Promise((r) => setTimeout(r, 5));
					return { kind: 'initialized' };
				})
			})
		);
		await Promise.all([s.initialize(), s.initialize()]);
		expect(calls).toBe(1);
	});

	it('remembers a failed outcome as MysqlInitFailure, with no cast needed for the narrower step union', async () => {
		const s = new MariadbStore(
			api({
				initializeMariadb: vi.fn(
					async () =>
						({
							kind: 'failed',
							step: 'setPassword',
							reason: 'auth denied'
						}) as MariadbInitOutcomeDto
				)
			})
		);
		await s.initialize();
		expect(s.initFailure).toEqual({
			major: MARIADB_SERIES,
			step: 'setPassword',
			reason: 'auth denied'
		});
	});

	it('is null for a non-failed outcome (initialized/alreadyInitialized/foreign)', async () => {
		const s = new MariadbStore(
			api({
				initializeMariadb: vi.fn(
					async () => ({ kind: 'alreadyInitialized' }) as MariadbInitOutcomeDto
				)
			})
		);
		await s.initialize();
		expect(s.initFailure).toBeNull();
	});

	it('supersedes a remembered failure once a later attempt succeeds', async () => {
		let calls = 0;
		const s = new MariadbStore(
			api({
				initializeMariadb: vi.fn(async (): Promise<MariadbInitOutcomeDto> => {
					calls += 1;
					if (calls === 1) return { kind: 'failed', step: 'render', reason: 'bad template' };
					return { kind: 'initialized' };
				})
			})
		);
		await s.initialize();
		expect(s.initFailure).not.toBeNull();
		await s.initialize();
		expect(s.initFailure).toBeNull();
	});

	it('re-reads the environment after a settled initialize, regardless of outcome', async () => {
		let calls = 0;
		const s = new MariadbStore(
			api({
				mariadbEnvironment: vi.fn(async () => {
					calls += 1;
					return env({ installed: true });
				}),
				initializeMariadb: vi.fn(
					async () => ({ kind: 'foreign', detail: 'unexpected.txt' }) as MariadbInitOutcomeDto
				)
			})
		);
		await s.refresh();
		await s.initialize();
		expect(calls).toBe(2);
	});

	// POST-HOC ONLY (task 3 brief): unlike MySQL's live init log, nothing
	// populates `initLog` DURING `initialize()` — only `appendInitLog`, fed
	// by the (separate) listener subscription once the run has ended, ever
	// does.
	it('never populates initLog as a side effect of initialize() itself', async () => {
		const s = new MariadbStore(
			api({
				initializeMariadb: vi.fn(
					async () =>
						({ kind: 'failed', step: 'render', reason: 'bad template' }) as MariadbInitOutcomeDto
				)
			})
		);
		await s.initialize();
		expect(s.initLog).toEqual([]);
	});
});

describe('MariadbStore — password reveal (never fetched eagerly)', () => {
	it('never calls mariadbRootPassword from refresh, rescan, install, or initialize', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new MariadbStore(api({ mariadbRootPassword: rootPassword }));
		await s.refresh();
		await s.rescan();
		await s.install();
		await s.initialize();
		expect(rootPassword).not.toHaveBeenCalled();
	});

	it('fetches and caches the password only once reveal() is called', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new MariadbStore(api({ mariadbRootPassword: rootPassword }));
		expect(s.password).toBeUndefined();
		await s.reveal();
		expect(s.password).toBe(FAKE_REVEALED);
		expect(rootPassword).toHaveBeenCalledTimes(1);
	});

	it('does not re-fetch an already-cached password (Reveal and Copy share the cache)', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new MariadbStore(api({ mariadbRootPassword: rootPassword }));
		await s.reveal();
		await s.reveal();
		expect(rootPassword).toHaveBeenCalledTimes(1);
	});

	it('surfaces a reveal failure without touching the page banner', async () => {
		const s = new MariadbStore(
			api({
				mariadbRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'no stored root password' };
				})
			})
		);
		await s.reveal();
		expect(s.passwordError).toContain('no stored root password');
		expect(s.password).toBeUndefined();
		expect(s.error).toBe('');
	});

	it('clears the revealing flag even when the fetch throws', async () => {
		const s = new MariadbStore(
			api({
				mariadbRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'x' };
				})
			})
		);
		await s.reveal();
		expect(s.revealing).toBe(false);
	});

	it('forgets a revealed password on demand (Hide), and re-fetches on the next reveal', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new MariadbStore(api({ mariadbRootPassword: rootPassword }));
		await s.reveal();
		s.forgetPassword();
		expect(s.password).toBeUndefined();
		await s.reveal();
		expect(rootPassword).toHaveBeenCalledTimes(2);
	});

	it('does nothing, without error, when asked to forget a password that was never revealed', () => {
		const s = new MariadbStore(api());
		expect(() => s.forgetPassword()).not.toThrow();
		expect(s.password).toBeUndefined();
	});

	// MANDATORY (review fix carried from MySQL's slice): Copy must never
	// un-mask the field on screen.
	it('copyPassword yields the value without ever turning on the display gate', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new MariadbStore(api({ mariadbRootPassword: rootPassword }));
		expect(s.revealed).toBe(false);
		const value = await s.copyPassword();
		expect(value).toBe(FAKE_REVEALED);
		expect(s.password).toBe(FAKE_REVEALED);
		expect(s.revealed).toBe(false);
		expect(rootPassword).toHaveBeenCalledTimes(1);
	});

	it('reveal() turns the display gate on; forgetPassword() (Hide) turns it back off', async () => {
		const s = new MariadbStore(api({ mariadbRootPassword: vi.fn(async () => FAKE_REVEALED) }));
		await s.reveal();
		expect(s.revealed).toBe(true);
		s.forgetPassword();
		expect(s.revealed).toBe(false);
	});

	it('reveal() after a prior copyPassword() reuses the cache and still turns the gate on', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new MariadbStore(api({ mariadbRootPassword: rootPassword }));
		await s.copyPassword();
		expect(s.revealed).toBe(false);
		await s.reveal();
		expect(s.revealed).toBe(true);
		expect(rootPassword).toHaveBeenCalledTimes(1);
	});

	it('does not turn the display gate on when reveal() itself fails to fetch', async () => {
		const s = new MariadbStore(
			api({
				mariadbRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'no stored root password' };
				})
			})
		);
		await s.reveal();
		expect(s.revealed).toBe(false);
	});
});

describe('MariadbStore — reset password', () => {
	it('regenerates and records the outcome', async () => {
		const s = new MariadbStore(api());
		await s.resetPassword();
		expect(s.resetOutcome).toEqual({ kind: 'reset' });
	});

	it('drops a cached password as the reset STARTS, even before it settles', async () => {
		const s = new MariadbStore(api());
		await s.reveal();
		expect(s.password).toBe(FAKE_REVEALED);
		await s.resetPassword();
		expect(s.password).toBeUndefined();
	});

	it('renders a stale-credential auth failure as its own distinct outcome, not a thrown error', async () => {
		const s = new MariadbStore(
			api({
				resetMariadbRootPassword: vi.fn(
					async () => ({ kind: 'authFailed', detail: 'Access denied' }) as MariadbResetOutcomeDto
				)
			})
		);
		await s.resetPassword();
		expect(s.resetOutcome).toEqual({ kind: 'authFailed', detail: 'Access denied' });
		expect(s.resetError).toBeFalsy();
	});

	it('surfaces a genuine spawn/IPC failure', async () => {
		const s = new MariadbStore(
			api({
				resetMariadbRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'could not write the ephemeral credential file' };
				})
			})
		);
		await s.resetPassword();
		expect(s.resetError).toContain('could not write the ephemeral credential file');
		expect(s.resetOutcome).toBeUndefined();
	});

	it('drops the previous verdict when the next reset starts, not only on the next success', async () => {
		let canSucceed = true;
		let verdictWhileRunning: MariadbResetOutcomeDto | undefined | 'never observed' =
			'never observed';
		const s = new MariadbStore(
			api({
				resetMariadbRootPassword: vi.fn(async (): Promise<MariadbResetOutcomeDto> => {
					if (canSucceed) return { kind: 'reset' };
					verdictWhileRunning = s.resetOutcome;
					throw { kind: 'core', message: 'boom' };
				})
			})
		);
		await s.resetPassword();
		expect(s.resetOutcome).toEqual({ kind: 'reset' });

		canSucceed = false;
		await s.resetPassword();
		expect(verdictWhileRunning).toBeUndefined();
	});

	it('clears the resetting flag even when the call throws', async () => {
		const s = new MariadbStore(
			api({
				resetMariadbRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'x' };
				})
			})
		);
		await s.resetPassword();
		expect(s.resetting).toBe(false);
	});
});

describe('MariadbStore — verify connection', () => {
	it('records the proof outcome', async () => {
		const s = new MariadbStore(api());
		await s.verifyConnection();
		expect(s.verifyResult).toEqual({ kind: 'ok', version: '11.4.9', port: 3307 });
	});

	it('renders authFailed/failed as distinct outcomes, not thrown errors', async () => {
		const s = new MariadbStore(
			api({
				verifyMariadbConnection: vi.fn(
					async () =>
						({ kind: 'failed', detail: 'connection refused' }) as MariadbConnectionProofDto
				)
			})
		);
		await s.verifyConnection();
		expect(s.verifyResult).toEqual({ kind: 'failed', detail: 'connection refused' });
		expect(s.verifyError).toBeFalsy();
	});

	it('drops the previous verdict when the next verify starts', async () => {
		let canSucceed = true;
		let verdictWhileRunning: MariadbConnectionProofDto | undefined | 'never observed' =
			'never observed';
		const s = new MariadbStore(
			api({
				verifyMariadbConnection: vi.fn(async (): Promise<MariadbConnectionProofDto> => {
					if (canSucceed) return { kind: 'ok', version: '11.4.9', port: 3307 };
					verdictWhileRunning = s.verifyResult;
					throw { kind: 'core', message: 'boom' };
				})
			})
		);
		await s.verifyConnection();
		canSucceed = false;
		await s.verifyConnection();
		expect(verdictWhileRunning).toBeUndefined();
	});

	it('clears the verifying flag even when the call throws', async () => {
		const s = new MariadbStore(
			api({
				verifyMariadbConnection: vi.fn(async () => {
					throw { kind: 'core', message: 'x' };
				})
			})
		);
		await s.verifyConnection();
		expect(s.verifying).toBe(false);
	});
});
