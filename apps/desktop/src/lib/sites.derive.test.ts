// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import {
	composeDomain,
	defaultPhpVersion,
	enabledPill,
	phpVersionOptions,
	splitDomain
} from './sites.derive';

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
