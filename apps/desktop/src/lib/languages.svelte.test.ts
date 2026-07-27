// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { LanguagesStore, type LanguagesApi } from './languages.svelte';
import type { InstallOutcomeDto, PhpEnvironmentDto, PhpRuntimeDto } from './ipc';

/** One catalogue/installed row, with sensible installed-shape defaults so most
 *  tests only need to state `major` and `installed`. */
function row(
	major: string,
	installed: boolean,
	overrides: Partial<PhpRuntimeDto> = {}
): PhpRuntimeDto {
	return {
		major,
		installed,
		recommended: false,
		fullVersion: null,
		path: installed ? `/opt/homebrew/opt/php@${major}/sbin/php-fpm` : null,
		socketPath: installed ? `/Users/x/.openvhost/run/php-fpm-${major}.sock` : null,
		serviceId: installed ? `php-fpm-${major}` : null,
		...overrides
	};
}

function env(runtimes: PhpRuntimeDto[]): PhpEnvironmentDto {
	return { brewFound: true, brewSearched: ['/opt/homebrew/bin/brew'], runtimes };
}

/** A `LanguagesApi` fake. `env`/`outcome` overrides are shared by every call
 *  that needs one — the tests that need per-call behaviour (a counter, a
 *  delay) construct the api object directly instead of going through this. */
function api(
	overrides: { env?: PhpEnvironmentDto; outcome?: InstallOutcomeDto } = {}
): LanguagesApi {
	return {
		phpEnvironment: async () => overrides.env ?? env([]),
		rescanPhpRuntimes: async () => overrides.env ?? env([]),
		installPhp: async () => overrides.outcome ?? { major: '', exitCode: 0, detected: true }
	};
}

describe('LanguagesStore', () => {
	it('lists what the backend returns', async () => {
		const s = new LanguagesStore(api({ env: env([row('8.3', true), row('8.4', false)]) }));
		await s.refresh();
		expect(s.env?.runtimes.map((r) => r.major)).toEqual(['8.3', '8.4']);
	});

	it('knows the difference between no PHP and no Homebrew', async () => {
		// Different states, different remedies — the page cannot infer the second
		// from an empty list.
		const noPhp = new LanguagesStore(
			api({ env: { brewFound: true, brewSearched: [], runtimes: [row('8.4', false)] } })
		);
		await noPhp.refresh();
		expect(noPhp.brewFound).toBe(true);
		expect(noPhp.anyInstalled).toBe(false);

		const noBrew = new LanguagesStore(
			api({ env: { brewFound: false, brewSearched: ['/opt/homebrew/bin/brew'], runtimes: [] } })
		);
		await noBrew.refresh();
		expect(noBrew.brewFound).toBe(false);
	});

	it('marks which version is installing and clears it when done', async () => {
		const s = new LanguagesStore(api({ outcome: { major: '8.4', exitCode: 0, detected: true } }));
		const p = s.install('8.4');
		expect(s.installing).toBe('8.4');
		expect(await p).toBe(true);
		expect(s.installing).toBe('');
	});

	it('refuses a second install while one is running', async () => {
		let calls = 0;
		const s = new LanguagesStore({
			phpEnvironment: async () => env([]),
			rescanPhpRuntimes: async () => env([]),
			installPhp: async () => {
				calls += 1;
				await new Promise((r) => setTimeout(r, 5));
				return { major: '8.4', exitCode: 0, detected: true };
			}
		});
		await Promise.all([s.install('8.4'), s.install('8.3')]);
		expect(calls).toBe(1);
	});

	it('keeps the log and surfaces the error when the install fails', async () => {
		const s = new LanguagesStore({
			phpEnvironment: async () => env([]),
			rescanPhpRuntimes: async () => env([]),
			installPhp: async () => {
				throw { kind: 'core', message: 'brew: no such formula' };
			}
		});
		s.appendLog('8.4', 'fetching');
		expect(await s.install('8.4')).toBe(false);
		expect(s.error).toContain('no such formula');
		expect(s.log.length).toBe(1);
		expect(s.installing).toBe('');
	});

	it("does not carry one version's output into the next attempt", async () => {
		// Install 8.4, watch it fail, then try 8.3: the 8.3 row must not show
		// 8.4's error output as if it were its own.
		const s = new LanguagesStore({
			phpEnvironment: async () => env([row('8.3', false), row('8.4', false)]),
			rescanPhpRuntimes: async () => env([]),
			installPhp: async () => {
				throw { kind: 'core', message: 'boom' };
			}
		});
		s.appendLog('8.4', 'fetching php@8.4');
		await s.install('8.4');
		expect(s.logFor('8.4').length).toBe(1);

		await s.install('8.3');
		expect(s.logFor('8.4')).toEqual([]);
		expect(s.logFor('8.3')).toEqual([]);
	});

	it('attributes output to the version it came from', () => {
		const s = new LanguagesStore(api());
		s.appendLog('8.3', 'fetching');
		expect(s.logFor('8.3').length).toBe(1);
		expect(s.logFor('8.4')).toEqual([]);
	});

	it('caps the log so a long install cannot grow without bound', () => {
		const s = new LanguagesStore(api());
		for (let i = 0; i < 500; i += 1) s.appendLog('8.3', `line ${i}`);
		expect(s.logFor('8.3').length).toBeLessThanOrEqual(200);
		// The tail is what matters when something fails.
		expect(s.logFor('8.3').at(-1)?.line).toBe('line 499');
	});

	it('keeps the last known environment when a refresh fails', async () => {
		// A failed re-read is not evidence that the machine lost its PHP.
		let calls = 0;
		const s = new LanguagesStore({
			phpEnvironment: async () => {
				calls += 1;
				if (calls === 1) return env([row('8.3', true)]);
				throw { kind: 'core', message: 'transient' };
			},
			rescanPhpRuntimes: async () => env([]),
			installPhp: async () => ({ major: '8.3', exitCode: 0, detected: true })
		});
		await s.refresh();
		await s.refresh();
		expect(s.error).toContain('transient');
		expect(s.env?.runtimes.length).toBe(1);
	});

	it('re-reads the environment after a successful install rather than assuming', async () => {
		// Assuming would show the version as installed even when the rescan did
		// not find it — the exact case `detected` exists to report.
		let calls = 0;
		const s = new LanguagesStore({
			phpEnvironment: async () => {
				calls += 1;
				return env([row('8.4', calls > 1)]);
			},
			rescanPhpRuntimes: async () => env([row('8.4', true)]),
			installPhp: async () => ({ major: '8.4', exitCode: 0, detected: true })
		});
		await s.refresh();
		expect(s.env?.runtimes[0].installed).toBe(false);
		await s.install('8.4');
		expect(calls).toBe(2);
		expect(s.env?.runtimes[0].installed).toBe(true);
	});
});
