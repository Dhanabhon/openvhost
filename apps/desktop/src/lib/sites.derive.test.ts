// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import {
	composeDomain,
	defaultPhpVersion,
	enabledPill,
	findMissingRuntimeSite,
	phpVersionMissing,
	phpVersionOptions,
	scaffoldPreview,
	splitDomain
} from './sites.derive';
import type { SiteDto } from './ipc';

const site = (overrides: Partial<SiteDto> = {}): SiteDto => ({
	id: 'a1',
	name: 'hello',
	domain: 'hello.localhost',
	docroot: '/srv/www/hello',
	webServer: 'nginx',
	phpVersion: '8.4',
	enabled: true,
	createdAt: 1,
	updatedAt: 1,
	...overrides
});

describe('composeDomain / splitDomain', () => {
	it('composes a subdomain onto .localhost', () => {
		expect(composeDomain('myshop')).toBe('myshop.localhost');
	});
	it('round-trips a composed domain', () => {
		expect(splitDomain(composeDomain('blog'))).toBe('blog');
	});
	it('strips exactly one trailing .localhost', () => {
		expect(splitDomain('a.localhost.localhost')).toBe('a.localhost');
	});
	it('returns a non-suffixed domain unchanged', () => {
		expect(splitDomain('example.test')).toBe('example.test');
	});
});

describe('enabledPill', () => {
	it('maps enabled/disabled to label + pill class', () => {
		expect(enabledPill(true)).toEqual({ label: 'enabled', cls: 'pill-running' });
		expect(enabledPill(false)).toEqual({ label: 'disabled', cls: 'pill-stopped' });
	});
});

describe('phpVersionOptions', () => {
	it('offers the versions actually installed', () => {
		const opts = phpVersionOptions(undefined, ['8.1', '8.3']);
		expect(opts.map((o) => o.value)).toEqual(['8.1', '8.3']);
	});

	it('keeps the stored version selectable when it is not installed', () => {
		// Dropping it would make the <select> render blank and silently rewrite
		// the site's PHP version to something the user never chose.
		const opts = phpVersionOptions('7.4', ['8.3']);
		expect(opts[0].value).toBe('7.4');
		expect(opts[0].label).toMatch(/not available|not installed/i);
	});

	it('does not duplicate the stored version when it is installed', () => {
		const opts = phpVersionOptions('8.3', ['8.1', '8.3']);
		expect(opts.filter((o) => o.value === '8.3')).toHaveLength(1);
	});

	it('still offers something when nothing is installed', () => {
		// An empty <select> would leave the user unable to save at all.
		const opts = phpVersionOptions('8.3', []);
		expect(opts.length).toBeGreaterThan(0);
		expect(opts[0].value).toBe('8.3');
	});

	it('adds nothing when there is no stored version and nothing is installed (a doomed Add form)', () => {
		expect(phpVersionOptions(undefined, [])).toEqual([]);
		expect(phpVersionOptions('', [])).toEqual([]);
	});
});

describe('defaultPhpVersion', () => {
	it('defaults a new site to the newest installed version', () => {
		// A site that is broken before the user has touched anything is the
		// second of the three mistakes in spec §5.0.
		expect(defaultPhpVersion(['8.1', '8.3', '8.5'])).toBe('8.5');
	});

	it('has no default to offer when nothing is installed', () => {
		expect(defaultPhpVersion([])).toBeUndefined();
	});

	it('compares major.minor numerically, not lexically', () => {
		// "8.9" > "8.10" as strings, but 8.10 is the newer release.
		expect(defaultPhpVersion(['8.9', '8.10'])).toBe('8.10');
	});
});

describe('phpVersionMissing', () => {
	// Task 8 stops a NEW site from choosing a version this machine lacks, but the
	// machine can change under an EXISTING one (`brew uninstall php@8.3`) at any
	// time, so this has to warn independent of whether Apply has ever run.
	it('is true when the stored version is not installed', () => {
		expect(phpVersionMissing(site({ phpVersion: '8.4' }), ['8.5'])).toBe(true);
	});

	it('is false when the stored version is installed', () => {
		expect(phpVersionMissing(site({ phpVersion: '8.5' }), ['8.5'])).toBe(false);
	});

	// I2 (branch-review-fix-report.md): `null` means the environment is UNKNOWN
	// (still loading, or the read failed) — a distinct fact from "known and
	// empty" (`[]`), which the caller used to collapse into the same `[]` via
	// `phpEnv?.runtimes ?? []`. This must return `false` (no badge) for `null`
	// even though the SAME site would be flagged against an empty-but-KNOWN
	// list — otherwise "unknown" would just be a slower way of saying "missing".
	it('is false (no badge) when the environment is unknown, unlike a known-empty one', () => {
		expect(phpVersionMissing(site({ phpVersion: '8.4' }), null)).toBe(false);
		expect(phpVersionMissing(site({ phpVersion: '8.4' }), [])).toBe(true);
	});
});

describe('findMissingRuntimeSite', () => {
	// Mirrors `render_set`'s `MissingRuntime` pre-check in
	// crates/openvhost-core/src/site/apply/mod.rs: enabled + nginx (`is_servable`),
	// first offender in list order. A disabled site's stale version is not a
	// reason Apply would fail, so it must not gate the banner's actions.
	it('finds the first enabled, nginx-served site missing its PHP version', () => {
		const found = findMissingRuntimeSite(
			[
				site({ id: 'a1', name: 'shop', phpVersion: '8.5' }),
				site({ id: 'a2', name: 'hello', phpVersion: '8.4' })
			],
			['8.5']
		);
		expect(found?.name).toBe('hello');
	});

	it('ignores a disabled site even if its version is missing', () => {
		const found = findMissingRuntimeSite([site({ enabled: false, phpVersion: '8.4' })], ['8.5']);
		expect(found).toBeNull();
	});

	it('is null when every servable site has an installed version', () => {
		expect(findMissingRuntimeSite([site({ phpVersion: '8.5' })], ['8.5'])).toBeNull();
	});
});

describe('scaffoldPreview', () => {
	it('joins parent and name', () =>
		expect(scaffoldPreview('/Users/x/Downloads', 'my-site')).toBe('/Users/x/Downloads/my-site'));
	it('normalizes trailing slashes', () =>
		expect(scaffoldPreview('/Users/x/Downloads//', 'my-site')).toBe('/Users/x/Downloads/my-site'));
	it('handles the root parent', () => expect(scaffoldPreview('/', 'a')).toBe('/a'));
	it('returns null while name is empty', () => expect(scaffoldPreview('/x', '')).toBeNull());
	it('returns null while parent is blank', () => expect(scaffoldPreview('  ', 'a')).toBeNull());
});
