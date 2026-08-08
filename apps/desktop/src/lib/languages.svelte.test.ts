// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { LanguagesStore, type LanguagesApi } from './languages.svelte';
import type { DefaultPhpDto, PhpEnvironmentDto, PhpInstallOutcomeDto, PhpRuntimeDto } from './ipc';

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
		// A row this build manages, which is what nearly every test here means.
		// Pass `cataloged: false` to exercise the hand-installed case.
		cataloged: true,
		recommended: false,
		fullVersion: null,
		path: installed ? `/opt/homebrew/opt/php@${major}/sbin/php-fpm` : null,
		socketPath: installed ? `/Users/x/.openvhost/run/php-fpm-${major}.sock` : null,
		serviceId: installed ? `php-fpm-${major}` : null,
		// A Homebrew keg, matching the `path` above — the shape every fixture
		// in this file already described (off-Homebrew slice 5C). Pass a
		// `packaged` source for a row installed from our own tree.
		source: installed ? { kind: 'homebrew' } : null,
		// What four of the five catalogue majors report on a real machine
		// today: this build has no artifact of its own for them. 8.4's
		// `awaitingRelease` is stated explicitly by the tests that mean it.
		offer: { kind: 'unavailable', target: 'macos-arm64' },
		...overrides
	};
}

/** What `defaultPhp` is on a machine that has never set a preference — which is
 *  every fixture in this file, all of which predate the setting. Derived rather
 *  than hardcoded so a fixture cannot claim a `serving` major it does not have:
 *  `unset` names the first installed runtime, exactly as the resolution does,
 *  and no runtimes means `nothingInstalled`. */
function noPreference(runtimes: PhpRuntimeDto[]): DefaultPhpDto {
	const first = runtimes.find((r) => r.installed);
	return first ? { kind: 'unset', serving: first.major } : { kind: 'nothingInstalled' };
}

function env(
	runtimes: PhpRuntimeDto[],
	brew: { found?: boolean; searched?: string[] } = {}
): PhpEnvironmentDto {
	return {
		brewFound: brew.found ?? true,
		brewSearched: brew.searched ?? ['/opt/homebrew/bin/brew'],
		runtimes,
		defaultPhp: noPreference(runtimes)
	};
}

/** A clean Homebrew install of `major` — the route every real machine takes
 *  today, and the only arm of `PhpInstallResultDto` that carries an exit code
 *  (off-Homebrew slice 5C design D4). The packaged arms are unreachable while
 *  every offer this build can make is `AwaitingRelease` or `Unavailable`. */
function brewOutcome(major: string): PhpInstallOutcomeDto {
	return { major, result: { kind: 'brew', exitCode: 0, detected: true } };
}

/** A `LanguagesApi` fake. `env`/`outcome` overrides are shared by every call
 *  that needs one — the tests that need per-call behaviour (a counter, a
 *  delay) construct the api object directly instead of going through this. */
function api(
	overrides: { env?: PhpEnvironmentDto; outcome?: PhpInstallOutcomeDto } = {}
): LanguagesApi {
	return {
		phpEnvironment: async () => overrides.env ?? env([]),
		rescanPhpRuntimes: async () => overrides.env ?? env([]),
		installPhp: async () => overrides.outcome ?? brewOutcome(''),
		// Accepts and forgets: every test in this file predates the preference
		// and asserts nothing about it. A test that means to exercise setting a
		// default builds its own api rather than leaning on this.
		setDefaultPhp: async () => {}
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
		const noPhp = new LanguagesStore(api({ env: env([row('8.4', false)], { searched: [] }) }));
		await noPhp.refresh();
		expect(noPhp.brewFound).toBe(true);
		expect(noPhp.anyInstalled).toBe(false);

		const noBrew = new LanguagesStore(api({ env: env([], { found: false }) }));
		await noBrew.refresh();
		expect(noBrew.brewFound).toBe(false);
	});

	// Off-Homebrew slice 5C design D2/D5. `brewFound` keeps its job and loses
	// the one it should not have had: it is no longer the page's first and
	// highest-priority state. The table itself is `php-install.derive.test.ts`'s;
	// what is only testable HERE is what the getter answers before the first
	// snapshot lands.
	it('claims no dead end before it has looked', async () => {
		// `env` is null on the very first frame of every visit, and a dead end is
		// a claim about what this machine CANNOT do. Answering "yes" here would
		// be one refactor away from flashing the bluntest screen in the app on
		// every page load.
		const s = new LanguagesStore(api());
		expect(s.env).toBeNull();
		expect(s.noRouteToAnyPhp).toBe(false);

		// …and it still answers honestly once a snapshot with nothing in it
		// arrives, so the guard above is not just suppressing the state.
		const dead = new LanguagesStore(api({ env: env([], { found: false, searched: [] }) }));
		await dead.refresh();
		expect(dead.noRouteToAnyPhp).toBe(true);
	});

	it('stops calling a machine with no Homebrew a dead end once it has a route', async () => {
		// §8.1: a packaged PHP already installed. §8.2b: one installable from our
		// own tree. Neither is a dead end, and both used to render one.
		const installed = new LanguagesStore(
			api({
				env: env([row('8.4', true, { source: { kind: 'packaged', version: '8.4.24' } })], {
					found: false
				})
			})
		);
		await installed.refresh();
		expect(installed.noRouteToAnyPhp).toBe(false);

		const offered = new LanguagesStore(
			api({
				env: env(
					[
						row('8.1', false),
						row('8.4', false, { offer: { kind: 'available', version: '8.4.24' } })
					],
					{ found: false }
				)
			})
		);
		await offered.refresh();
		expect(offered.noRouteToAnyPhp).toBe(false);
	});

	it('marks which version is installing and clears it when done', async () => {
		const s = new LanguagesStore(api({ outcome: brewOutcome('8.4') }));
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
			// Unused here; present because LanguagesApi requires it.
			setDefaultPhp: async () => {},
			installPhp: async () => {
				calls += 1;
				await new Promise((r) => setTimeout(r, 5));
				return brewOutcome('8.4');
			}
		});
		await Promise.all([s.install('8.4'), s.install('8.3')]);
		expect(calls).toBe(1);
	});

	it('keeps the log and surfaces the error when the install fails', async () => {
		const s = new LanguagesStore({
			phpEnvironment: async () => env([]),
			rescanPhpRuntimes: async () => env([]),
			// Unused here; present because LanguagesApi requires it.
			setDefaultPhp: async () => {},
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
			// Unused here; present because LanguagesApi requires it.
			setDefaultPhp: async () => {},
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
			// Unused here; present because LanguagesApi requires it.
			setDefaultPhp: async () => {},
			installPhp: async () => brewOutcome('8.3')
		});
		await s.refresh();
		await s.refresh();
		expect(s.error).toContain('transient');
		expect(s.env?.runtimes.length).toBe(1);
	});

	it('keeps the last known environment when a rescan fails', async () => {
		// "Check again" is the user's only recovery path once they've gone off to
		// install Homebrew themselves — a transient failure here must not blank
		// the page and erase the guidance they were just following.
		let calls = 0;
		const s = new LanguagesStore({
			phpEnvironment: async () => env([row('8.3', true)]),
			rescanPhpRuntimes: async () => {
				calls += 1;
				if (calls === 1) return env([row('8.3', true)]);
				throw { kind: 'core', message: 'transient' };
			},
			installPhp: async () => brewOutcome('8.3'),
			// Unused here; present because LanguagesApi requires it.
			setDefaultPhp: async () => {}
		});
		await s.rescan();
		await s.rescan();
		expect(s.error).toContain('transient');
		expect(s.env?.runtimes.length).toBe(1);
	});

	// ------------------------------------------------------------------
	// The packaged route's progress, whose subscriber landed in the 5C fix wave.
	//
	// Vacuity: every assertion below is against a field the constructor leaves
	// `null`, and each is paired with a negative case on the same field. Proven
	// by mutation — dropping the `installProgress = null` line from `install()`
	// reddened 'clears a previous run's progress…', and dropping the `major`
	// from `applyInstallProgress`'s stored value reddened 'attributes progress
	// to the major it arrived for'.
	// ------------------------------------------------------------------

	it('has no progress at all until an event arrives', () => {
		const s = new LanguagesStore(api());
		expect(s.installProgress).toBeNull();
		expect(s.installTotal).toBeNull();
		expect(s.progressFor('8.4')).toBeNull();
	});

	it('records each pipeline state as it arrives', () => {
		const s = new LanguagesStore(api());
		s.applyInstallProgress('8.4', { kind: 'started', total: 4096 });
		expect(s.progressFor('8.4')).toEqual({ kind: 'started', total: 4096 });
		s.applyInstallProgress('8.4', { kind: 'verified' });
		expect(s.progressFor('8.4')).toEqual({ kind: 'verified' });
	});

	// No later event repeats the declared length, so losing it here leaves every
	// `downloaded` reading with no denominator and the bar undrawable.
	it('captures the declared total off the started event and keeps it', () => {
		const s = new LanguagesStore(api());
		s.applyInstallProgress('8.4', { kind: 'started', total: 4096 });
		expect(s.installTotal).toBe(4096);
		s.applyInstallProgress('8.4', { kind: 'downloaded', bytes: 1024 });
		expect(s.installTotal).toBe(4096);
		s.applyInstallProgress('8.4', { kind: 'linked' });
		expect(s.installTotal).toBe(4096);
	});

	it('leaves the total null when the server declared none, rather than inventing one', () => {
		const s = new LanguagesStore(api());
		s.applyInstallProgress('8.4', { kind: 'started', total: null });
		s.applyInstallProgress('8.4', { kind: 'downloaded', bytes: 1024 });
		expect(s.installTotal).toBeNull();
	});

	// The progress twin of `logFor`, and there for the same reason: this store
	// has already shipped the untagged version of this bug once, rendering a
	// failed 8.4 install's output under the 8.3 row.
	it('attributes progress to the major it arrived for, and to no other row', () => {
		const s = new LanguagesStore(api());
		s.applyInstallProgress('8.4', { kind: 'verified' });
		expect(s.progressFor('8.4')).toEqual({ kind: 'verified' });
		expect(s.progressFor('8.3')).toBeNull();
		expect(s.progressFor('8.5')).toBeNull();
	});

	it("clears a previous run's progress as the next run starts, not when it ends", async () => {
		// A stale "Checksum verified" sitting above this attempt's first byte is
		// exactly the confusion this rule prevents.
		const s = new LanguagesStore(api({ outcome: brewOutcome('8.3') }));
		s.applyInstallProgress('8.4', { kind: 'verified' });
		s.applyInstallProgress('8.4', { kind: 'started', total: 4096 });
		expect(s.installTotal).toBe(4096);
		await s.install('8.3');
		expect(s.installProgress).toBeNull();
		expect(s.installTotal).toBeNull();
		expect(s.progressFor('8.4')).toBeNull();
	});

	// Spec §8.6, at the level the state actually lives. `php-install-progress` is
	// emitted only by `run_package_install`; `run_brew_install` streams
	// `php-install-log` instead. So on a machine with Homebrew and no package
	// tree a whole install runs and this field is still `null` — which is what
	// makes every consumer of it render nothing.
	it('stays null through a whole Homebrew install, which emits no progress at all', async () => {
		const s = new LanguagesStore(api({ outcome: brewOutcome('8.4') }));
		await s.install('8.4');
		expect(s.outcome).toEqual(brewOutcome('8.4'));
		expect(s.installProgress).toBeNull();
		expect(s.installTotal).toBeNull();
		expect(s.progressFor('8.4')).toBeNull();
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
			// Unused here; present because LanguagesApi requires it.
			setDefaultPhp: async () => {},
			installPhp: async () => brewOutcome('8.4')
		});
		await s.refresh();
		expect(s.env?.runtimes[0].installed).toBe(false);
		await s.install('8.4');
		expect(calls).toBe(2);
		expect(s.env?.runtimes[0].installed).toBe(true);
	});
});
