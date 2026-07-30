// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { DatabasesStore, type DatabasesApi } from './databases.svelte';
import type {
	MysqlConnectionProofDto,
	MysqlEnvironmentDto,
	MysqlInitOutcomeDto,
	MysqlInstanceDto,
	MysqlResetOutcomeDto
} from './ipc';

/** One catalogue/installed row, mirroring `languages.svelte.test.ts`'s `row()`. */
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

function env(instances: MysqlInstanceDto[], brewFound = true): MysqlEnvironmentDto {
	return { brewFound, brewSearched: ['/opt/homebrew/bin/brew'], instances };
}

/** Never a real-looking password: fixtures here must never contain anything
 *  that resembles the real 32-hex generated credential (spec D3 MANDATORY:
 *  no secret ever belongs in a test snapshot). */
const FAKE_REVEALED = 'not-a-real-password';

function api(overrides: Partial<DatabasesApi> = {}): DatabasesApi {
	return {
		mysqlEnvironment: vi.fn(async () => env([])),
		rescanMysql: vi.fn(async () => env([])),
		installMysql: vi.fn(async () => ({ major: '8.4', exitCode: 0, detected: true })),
		initializeMysql: vi.fn(async () => ({ kind: 'initialized' }) as MysqlInitOutcomeDto),
		mysqlRootPassword: vi.fn(async () => FAKE_REVEALED),
		resetMysqlRootPassword: vi.fn(async () => ({ kind: 'reset' }) as MysqlResetOutcomeDto),
		verifyMysqlConnection: vi.fn(
			async () => ({ kind: 'ok', version: '8.4.11', port: 3306 }) as MysqlConnectionProofDto
		),
		...overrides
	};
}

describe('DatabasesStore — environment load', () => {
	it('lists what the backend returns', async () => {
		const s = new DatabasesStore(api({ mysqlEnvironment: vi.fn(async () => env([instance()])) }));
		await s.refresh();
		expect(s.env?.instances.map((i) => i.major)).toEqual(['8.4']);
	});

	it('exposes brewFound and anyInstalled from the loaded environment', async () => {
		const s = new DatabasesStore(
			api({
				mysqlEnvironment: vi.fn(async () => env([instance({ installed: true })], false))
			})
		);
		await s.refresh();
		expect(s.brewFound).toBe(false);
		expect(s.anyInstalled).toBe(true);
	});

	it('is neither brewFound nor anyInstalled before the first load settles', () => {
		const s = new DatabasesStore(api());
		expect(s.brewFound).toBe(false);
		expect(s.anyInstalled).toBe(false);
	});

	it('keeps the last known environment when a refresh fails', async () => {
		let calls = 0;
		const s = new DatabasesStore(
			api({
				mysqlEnvironment: vi.fn(async () => {
					calls += 1;
					if (calls === 1) return env([instance({ installed: true })]);
					throw { kind: 'core', message: 'transient' };
				})
			})
		);
		await s.refresh();
		await s.refresh();
		expect(s.error).toContain('transient');
		expect(s.env?.instances.length).toBe(1);
	});

	it('keeps the last known environment when a rescan fails', async () => {
		let calls = 0;
		const s = new DatabasesStore(
			api({
				mysqlEnvironment: vi.fn(async () => env([instance({ installed: true })])),
				rescanMysql: vi.fn(async () => {
					calls += 1;
					if (calls === 1) return env([instance({ installed: true })]);
					throw { kind: 'core', message: 'transient' };
				})
			})
		);
		await s.rescan();
		await s.rescan();
		expect(s.error).toContain('transient');
		expect(s.env?.instances.length).toBe(1);
	});

	it('records an outside failure on the same channel', () => {
		const s = new DatabasesStore(api());
		s.fail({ kind: 'core', message: 'listener could not be registered' });
		expect(s.error).toContain('listener could not be registered');
	});
});

describe('DatabasesStore — install', () => {
	it('marks which major is installing and clears it when done', async () => {
		const s = new DatabasesStore(api());
		const p = s.install('8.4');
		expect(s.installing).toBe('8.4');
		expect(await p).toBe(true);
		expect(s.installing).toBe('');
	});

	it('refuses a second install while one is running', async () => {
		let calls = 0;
		const s = new DatabasesStore(
			api({
				installMysql: vi.fn(async () => {
					calls += 1;
					await new Promise((r) => setTimeout(r, 5));
					return { major: '8.4', exitCode: 0, detected: true };
				})
			})
		);
		await Promise.all([s.install('8.4'), s.install('8.5')]);
		expect(calls).toBe(1);
	});

	it('keeps the log and surfaces the error when the install throws', async () => {
		const s = new DatabasesStore(
			api({
				installMysql: vi.fn(async () => {
					throw { kind: 'core', message: 'brew: no such formula' };
				})
			})
		);
		s.appendInstallLog('8.4', 'fetching');
		expect(await s.install('8.4')).toBe(false);
		expect(s.error).toContain('no such formula');
		expect(s.installLog.length).toBe(1);
		expect(s.installing).toBe('');
	});

	it("does not carry one major's install output into the next attempt", async () => {
		const s = new DatabasesStore(
			api({
				installMysql: vi.fn(async () => {
					throw { kind: 'core', message: 'boom' };
				})
			})
		);
		s.appendInstallLog('8.4', 'fetching mysql@8.4');
		await s.install('8.4');
		expect(s.installLogFor('8.4').length).toBe(1);

		await s.install('8.5');
		expect(s.installLogFor('8.4')).toEqual([]);
		expect(s.installLogFor('8.5')).toEqual([]);
	});

	it('attributes install log output to the major it came from', () => {
		const s = new DatabasesStore(api());
		s.appendInstallLog('8.4', 'fetching');
		expect(s.installLogFor('8.4').length).toBe(1);
		expect(s.installLogFor('8.5')).toEqual([]);
	});

	it('caps the install log so a long install cannot grow without bound', () => {
		const s = new DatabasesStore(api());
		for (let i = 0; i < 500; i += 1) s.appendInstallLog('8.4', `line ${i}`);
		expect(s.installLogFor('8.4').length).toBeLessThanOrEqual(200);
		expect(s.installLogFor('8.4').at(-1)?.line).toBe('line 499');
	});

	it('re-reads the environment after a successful install rather than assuming', async () => {
		let calls = 0;
		const s = new DatabasesStore(
			api({
				mysqlEnvironment: vi.fn(async () => {
					calls += 1;
					return env([instance({ installed: calls > 1 })]);
				})
			})
		);
		await s.refresh();
		expect(s.env?.instances[0].installed).toBe(false);
		await s.install('8.4');
		expect(calls).toBe(2);
		expect(s.env?.instances[0].installed).toBe(true);
	});
});

describe('DatabasesStore — initialize', () => {
	it('marks which major is initializing and clears it when done', async () => {
		const s = new DatabasesStore(api());
		const p = s.initialize('8.4');
		expect(s.initializing).toBe('8.4');
		expect(await p).toBe(true);
		expect(s.initializing).toBe('');
	});

	it('refuses a second initialize while one is running', async () => {
		let calls = 0;
		const s = new DatabasesStore(
			api({
				initializeMysql: vi.fn(async (): Promise<MysqlInitOutcomeDto> => {
					calls += 1;
					await new Promise((r) => setTimeout(r, 5));
					return { kind: 'initialized' };
				})
			})
		);
		await Promise.all([s.initialize('8.4'), s.initialize('8.5')]);
		expect(calls).toBe(1);
	});

	it('attributes init log output to the major it came from, capped and scoped like install', () => {
		const s = new DatabasesStore(api());
		s.appendInitLog('8.4', 'rendering config');
		expect(s.initLogFor('8.4').length).toBe(1);
		expect(s.initLogFor('8.5')).toEqual([]);
	});

	it('remembers a failed outcome, attributed to the major it happened to', async () => {
		const s = new DatabasesStore(
			api({
				initializeMysql: vi.fn(
					async () =>
						({ kind: 'failed', step: 'setPassword', reason: 'auth denied' }) as MysqlInitOutcomeDto
				)
			})
		);
		await s.initialize('8.4');
		expect(s.initFailureFor('8.4')).toEqual({
			major: '8.4',
			step: 'setPassword',
			reason: 'auth denied'
		});
	});

	it('does not attribute a failure to a different major', async () => {
		const s = new DatabasesStore(
			api({
				initializeMysql: vi.fn(
					async () =>
						({ kind: 'failed', step: 'render', reason: 'bad template' }) as MysqlInitOutcomeDto
				)
			})
		);
		await s.initialize('8.4');
		expect(s.initFailureFor('8.5')).toBeNull();
	});

	it('is null for a non-failed outcome (initialized/alreadyInitialized/foreign)', async () => {
		const s = new DatabasesStore(
			api({
				initializeMysql: vi.fn(async () => ({ kind: 'alreadyInitialized' }) as MysqlInitOutcomeDto)
			})
		);
		await s.initialize('8.4');
		expect(s.initFailureFor('8.4')).toBeNull();
	});

	it('supersedes a remembered failure once a later attempt succeeds', async () => {
		let calls = 0;
		const s = new DatabasesStore(
			api({
				initializeMysql: vi.fn(async (): Promise<MysqlInitOutcomeDto> => {
					calls += 1;
					if (calls === 1) {
						return { kind: 'failed', step: 'render', reason: 'bad template' };
					}
					return { kind: 'initialized' };
				})
			})
		);
		await s.initialize('8.4');
		expect(s.initFailureFor('8.4')).not.toBeNull();
		await s.initialize('8.4');
		expect(s.initFailureFor('8.4')).toBeNull();
	});

	it('re-reads the environment after a settled initialize, regardless of outcome', async () => {
		let calls = 0;
		const s = new DatabasesStore(
			api({
				mysqlEnvironment: vi.fn(async () => {
					calls += 1;
					return env([instance({ installed: true })]);
				}),
				initializeMysql: vi.fn(
					async () => ({ kind: 'foreign', detail: 'unexpected.txt' }) as MysqlInitOutcomeDto
				)
			})
		);
		await s.refresh();
		await s.initialize('8.4');
		expect(calls).toBe(2);
	});
});

describe('DatabasesStore — password reveal (never fetched eagerly)', () => {
	it('never calls mysqlRootPassword from refresh, rescan, install, or initialize', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new DatabasesStore(api({ mysqlRootPassword: rootPassword }));
		await s.refresh();
		await s.rescan();
		await s.install('8.4');
		await s.initialize('8.4');
		expect(rootPassword).not.toHaveBeenCalled();
	});

	it('fetches and caches the password only once reveal() is called', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new DatabasesStore(api({ mysqlRootPassword: rootPassword }));
		expect(s.passwords['8.4']).toBeUndefined();
		await s.reveal('8.4');
		expect(s.passwords['8.4']).toBe(FAKE_REVEALED);
		expect(rootPassword).toHaveBeenCalledTimes(1);
	});

	it('does not re-fetch an already-cached password (Reveal and Copy share the cache)', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new DatabasesStore(api({ mysqlRootPassword: rootPassword }));
		await s.reveal('8.4');
		await s.reveal('8.4');
		expect(rootPassword).toHaveBeenCalledTimes(1);
	});

	it('keeps per-major passwords independent', async () => {
		const s = new DatabasesStore(
			api({ mysqlRootPassword: vi.fn(async (major: string) => `pw-for-${major}`) })
		);
		await s.reveal('8.4');
		await s.reveal('8.5');
		expect(s.passwords['8.4']).toBe('pw-for-8.4');
		expect(s.passwords['8.5']).toBe('pw-for-8.5');
	});

	it('surfaces a reveal failure on the row, not the page banner', async () => {
		const s = new DatabasesStore(
			api({
				mysqlRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'no stored root password' };
				})
			})
		);
		await s.reveal('8.4');
		expect(s.passwordError['8.4']).toContain('no stored root password');
		expect(s.passwords['8.4']).toBeUndefined();
		expect(s.error).toBe('');
	});

	it('clears the revealing flag even when the fetch throws', async () => {
		const s = new DatabasesStore(
			api({
				mysqlRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'x' };
				})
			})
		);
		await s.reveal('8.4');
		expect(s.revealing['8.4']).not.toBe(true);
	});

	it('forgets a revealed password on demand (Hide), and re-fetches on the next reveal', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new DatabasesStore(api({ mysqlRootPassword: rootPassword }));
		await s.reveal('8.4');
		s.forgetPassword('8.4');
		expect(s.passwords['8.4']).toBeUndefined();
		await s.reveal('8.4');
		expect(rootPassword).toHaveBeenCalledTimes(2);
	});

	it('does nothing, without error, when asked to forget a password that was never revealed', () => {
		const s = new DatabasesStore(api());
		expect(() => s.forgetPassword('8.4')).not.toThrow();
		expect(s.passwords['8.4']).toBeUndefined();
	});

	// Review fix: Copy must never un-mask the field on screen (a screen-share
	// scenario is exactly why) — it fetches/caches the SAME value Reveal does,
	// but must leave the separate display gate (`revealed`) untouched. Before
	// this fix there was no such gate at all: masking derived solely from
	// `passwords[major] !== undefined`, so calling the same cache-fill for
	// Copy silently flipped the field to plaintext.
	it('copyPassword yields the value without ever turning on the display gate', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new DatabasesStore(api({ mysqlRootPassword: rootPassword }));
		expect(s.revealed['8.4']).not.toBe(true);
		const value = await s.copyPassword('8.4');
		expect(value).toBe(FAKE_REVEALED);
		expect(s.passwords['8.4']).toBe(FAKE_REVEALED);
		expect(s.revealed['8.4']).not.toBe(true);
		expect(rootPassword).toHaveBeenCalledTimes(1);
	});

	it('reveal() turns the display gate on; forgetPassword() (Hide) turns it back off', async () => {
		const s = new DatabasesStore(api({ mysqlRootPassword: vi.fn(async () => FAKE_REVEALED) }));
		await s.reveal('8.4');
		expect(s.revealed['8.4']).toBe(true);
		s.forgetPassword('8.4');
		expect(s.revealed['8.4']).not.toBe(true);
	});

	it('reveal() after a prior copyPassword() reuses the cache and still turns the gate on', async () => {
		const rootPassword = vi.fn(async () => FAKE_REVEALED);
		const s = new DatabasesStore(api({ mysqlRootPassword: rootPassword }));
		await s.copyPassword('8.4');
		expect(s.revealed['8.4']).not.toBe(true);
		await s.reveal('8.4');
		expect(s.revealed['8.4']).toBe(true);
		expect(rootPassword).toHaveBeenCalledTimes(1); // shared cache, no re-fetch
	});

	it('does not turn the display gate on when reveal() itself fails to fetch', async () => {
		const s = new DatabasesStore(
			api({
				mysqlRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'no stored root password' };
				})
			})
		);
		await s.reveal('8.4');
		expect(s.revealed['8.4']).not.toBe(true);
	});
});

describe('DatabasesStore — reset password', () => {
	it('regenerates and records the outcome, per major', async () => {
		const s = new DatabasesStore(api());
		await s.resetPassword('8.4');
		expect(s.resetOutcome['8.4']).toEqual({ kind: 'reset' });
	});

	it('drops a cached password as the reset STARTS, even before it settles', async () => {
		const s = new DatabasesStore(api());
		await s.reveal('8.4');
		expect(s.passwords['8.4']).toBe(FAKE_REVEALED);
		await s.resetPassword('8.4');
		expect(s.passwords['8.4']).toBeUndefined();
	});

	it('renders a stale-credential auth failure as its own distinct outcome, not a thrown error', async () => {
		const s = new DatabasesStore(
			api({
				resetMysqlRootPassword: vi.fn(
					async () => ({ kind: 'authFailed', detail: 'Access denied' }) as MysqlResetOutcomeDto
				)
			})
		);
		await s.resetPassword('8.4');
		expect(s.resetOutcome['8.4']).toEqual({ kind: 'authFailed', detail: 'Access denied' });
		expect(s.resetError['8.4']).toBeFalsy();
	});

	it('surfaces a genuine spawn/IPC failure on the row', async () => {
		const s = new DatabasesStore(
			api({
				resetMysqlRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'could not write the ephemeral credential file' };
				})
			})
		);
		await s.resetPassword('8.4');
		expect(s.resetError['8.4']).toContain('could not write the ephemeral credential file');
		expect(s.resetOutcome['8.4']).toBeUndefined();
	});

	// Same drop-the-stale-verdict-as-the-run-starts discipline as
	// `webservers.svelte.ts`'s `validate()`: a second reset call that reads
	// `resetOutcome` from INSIDE its own in-flight run must see it already
	// cleared, not the previous call's settled verdict.
	it('drops the previous verdict when the next reset starts, not only on the next success', async () => {
		let canSucceed = true;
		let verdictWhileRunning: MysqlResetOutcomeDto | 'never observed' = 'never observed';
		const s = new DatabasesStore(
			api({
				resetMysqlRootPassword: vi.fn(async (): Promise<MysqlResetOutcomeDto> => {
					if (canSucceed) return { kind: 'reset' };
					verdictWhileRunning = s.resetOutcome['8.4'];
					throw { kind: 'core', message: 'boom' };
				})
			})
		);
		await s.resetPassword('8.4');
		expect(s.resetOutcome['8.4']).toEqual({ kind: 'reset' });

		canSucceed = false;
		await s.resetPassword('8.4');
		expect(verdictWhileRunning).toBeUndefined();
	});

	it('clears the resetting flag even when the call throws', async () => {
		const s = new DatabasesStore(
			api({
				resetMysqlRootPassword: vi.fn(async () => {
					throw { kind: 'core', message: 'x' };
				})
			})
		);
		await s.resetPassword('8.4');
		expect(s.resetting['8.4']).not.toBe(true);
	});
});

describe('DatabasesStore — verify connection', () => {
	it('records the proof outcome, per major', async () => {
		const s = new DatabasesStore(api());
		await s.verifyConnection('8.4');
		expect(s.verifyResult['8.4']).toEqual({ kind: 'ok', version: '8.4.11', port: 3306 });
	});

	it('renders authFailed/failed as distinct outcomes, not thrown errors', async () => {
		const s = new DatabasesStore(
			api({
				verifyMysqlConnection: vi.fn(
					async () => ({ kind: 'failed', detail: 'connection refused' }) as MysqlConnectionProofDto
				)
			})
		);
		await s.verifyConnection('8.4');
		expect(s.verifyResult['8.4']).toEqual({ kind: 'failed', detail: 'connection refused' });
		expect(s.verifyError['8.4']).toBeFalsy();
	});

	it('drops the previous verdict when the next verify starts', async () => {
		let canSucceed = true;
		let verdictWhileRunning: MysqlConnectionProofDto | 'never observed' = 'never observed';
		const s = new DatabasesStore(
			api({
				verifyMysqlConnection: vi.fn(async (): Promise<MysqlConnectionProofDto> => {
					if (canSucceed) return { kind: 'ok', version: '8.4.11', port: 3306 };
					verdictWhileRunning = s.verifyResult['8.4'];
					throw { kind: 'core', message: 'boom' };
				})
			})
		);
		await s.verifyConnection('8.4');
		canSucceed = false;
		await s.verifyConnection('8.4');
		expect(verdictWhileRunning).toBeUndefined();
	});

	it('clears the verifying flag even when the call throws', async () => {
		const s = new DatabasesStore(
			api({
				verifyMysqlConnection: vi.fn(async () => {
					throw { kind: 'core', message: 'x' };
				})
			})
		);
		await s.verifyConnection('8.4');
		expect(s.verifying['8.4']).not.toBe(true);
	});
});
