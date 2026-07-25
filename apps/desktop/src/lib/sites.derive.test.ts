// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { composeDomain, enabledPill, splitDomain, PHP_VERSIONS } from './sites.derive';

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

describe('PHP_VERSIONS', () => {
	it('offers major.minor values only', () => {
		for (const v of PHP_VERSIONS) expect(v).toMatch(/^\d+\.\d+$/);
	});
});
