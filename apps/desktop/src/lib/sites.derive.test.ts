// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import {
	composeDomain,
	enabledPill,
	phpVersionOptions,
	splitDomain,
	PHP_VERSIONS
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

describe('PHP_VERSIONS', () => {
	it('offers major.minor values only', () => {
		for (const v of PHP_VERSIONS) expect(v).toMatch(/^\d+\.\d+$/);
	});
});

describe('phpVersionOptions', () => {
	it('offers the fixed list, labelled by bare version, for a listed version', () => {
		expect(phpVersionOptions('8.3')).toEqual(PHP_VERSIONS.map((v) => ({ value: v, label: v })));
	});

	it('prepends an unlisted stored version so it stays representable', () => {
		expect(phpVersionOptions('8.0').map((o) => o.value)).toEqual(['8.0', ...PHP_VERSIONS]);
	});

	it('annotates the unlisted version instead of passing it off as offered', () => {
		expect(phpVersionOptions('8.0')[0].label).toBe('8.0 — not available');
	});

	it('adds nothing when there is no stored version yet (the Add form)', () => {
		expect(phpVersionOptions(undefined).map((o) => o.value)).toEqual([...PHP_VERSIONS]);
		expect(phpVersionOptions('').map((o) => o.value)).toEqual([...PHP_VERSIONS]);
	});
});
