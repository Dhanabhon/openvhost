// SPDX-License-Identifier: GPL-3.0-or-later
//
// The rule behind the landing page's readiness banner, as a table.
//
// `siteReadiness` is a total function over 3 × 3 = 9 input pairs, and every one
// of them is stated below rather than sampled — the defect this slice removes
// was precisely a state nobody had written down (PHP present, nginx absent, and
// the page silent). The 3×3 block is therefore not padding: it is the shape of
// the bug.

import { describe, expect, it } from 'vitest';
import type { WebServerDto } from './ipc';
import {
	READINESS_MULTI_TITLE,
	nginxCheck,
	phpCheck,
	siteReadiness,
	type ReadinessCheck
} from './site-readiness.derive';

const CHECKS: readonly ReadinessCheck[] = ['unknown', 'present', 'absent'];

/** Every `ReadinessCheck` pair, so no combination can go unstated. */
function allPairs(): [ReadinessCheck, ReadinessCheck][] {
	return CHECKS.flatMap((php) =>
		CHECKS.map((nginx): [ReadinessCheck, ReadinessCheck] => [php, nginx])
	);
}

/** The web-server row `list_web_servers` builds for nginx. `binaryPath` is the
 *  only field this module reads; the rest are here so the fixture is a real
 *  `WebServerDto` and a backend field rename fails to compile. */
function nginxRow(binaryPath: string | null): WebServerDto {
	return {
		id: 'nginx',
		displayName: 'nginx',
		supported: true,
		serviceId: 'nginx',
		binaryPath,
		version: binaryPath === null ? null : '1.30.4',
		// Provenance, deliberately NOT the discriminator this module uses — see
		// `nginxCheck`'s doc comment. Present here so a fixture with a source but
		// no binary (and vice versa) can be written at all.
		source: binaryPath === null ? null : { kind: 'packaged', version: '1.30.4' },
		supportsHotReload: true,
		configPath: '/Users/x/.openvhost/etc/nginx/nginx.conf',
		configExists: true
	};
}

/** The Apache row, verbatim in shape from `WebServerDto::apache()` — unsupported,
 *  no binary, no source. It is in every real list, so no test may pass only
 *  because it was left out. */
function apacheRow(): WebServerDto {
	return {
		id: 'apache',
		displayName: 'Apache',
		supported: false,
		serviceId: null,
		binaryPath: null,
		version: null,
		source: null,
		supportsHotReload: false,
		configPath: null,
		configExists: false
	};
}

describe('siteReadiness over every check pair', () => {
	// The table, stated once. Each entry is the ids the banner must name, in
	// order — `[]` meaning "no banner at all".
	const expected = new Map<string, readonly ('php' | 'nginx')[]>([
		['unknown/unknown', []],
		['unknown/present', []],
		['unknown/absent', ['nginx']],
		['present/unknown', []],
		['present/present', []],
		['present/absent', ['nginx']],
		['absent/unknown', ['php']],
		['absent/present', ['php']],
		['absent/absent', ['php', 'nginx']]
	]);

	it('covers all nine pairs, so none can be quietly dropped', () => {
		expect(allPairs()).toHaveLength(9);
		expect([...expected.keys()].sort()).toEqual(
			allPairs()
				.map(([php, nginx]) => `${php}/${nginx}`)
				.sort()
		);
	});

	for (const [php, nginx] of allPairs()) {
		const key = `${php}/${nginx}`;
		const want = expected.get(key) ?? [];
		it(`names ${want.length === 0 ? 'nothing' : want.join(' and ')} when php is ${php} and nginx is ${nginx}`, () => {
			const notice = siteReadiness(php, nginx);
			if (want.length === 0) {
				expect(notice).toBeNull();
				return;
			}
			expect(notice?.lines.map((l) => l.id)).toEqual(want);
		});
	}
});

// Spec §7.1 — the state that renders NOTHING before this slice, and the one
// that justifies it. Asserted on its own, not only as a row of the table above,
// because it is the claim the whole change exists to make.
describe('no nginx, PHP installed (spec §7.1)', () => {
	it('shows a banner that names nginx', () => {
		const notice = siteReadiness('present', 'absent');
		expect(notice).not.toBeNull();
		expect(notice?.title).toContain('nginx');
	});

	it('points at the Web server page, and says nothing about PHP', () => {
		const notice = siteReadiness('present', 'absent');
		expect(notice?.lines).toEqual([
			{
				id: 'nginx',
				text: 'Sites are served by nginx.',
				route: '/web-server',
				linkText: 'Check the Web server page'
			}
		]);
		expect(JSON.stringify(notice)).not.toContain('PHP');
	});
});

// Spec §7.2 — the existing wording is not the bug. Pinned verbatim, so a future
// edit to the multi-missing case cannot quietly reword the case every
// PHP-less machine already sees.
describe('no PHP, nginx installed (spec §7.2)', () => {
	it('reads exactly as it did before this slice', () => {
		expect(siteReadiness('absent', 'present')).toEqual({
			title: 'No PHP version is installed yet',
			lines: [
				{
					id: 'php',
					text: 'Sites need one to run.',
					route: '/languages',
					linkText: 'Install a version on the Languages page'
				}
			]
		});
	});

	it('says nothing about nginx', () => {
		expect(JSON.stringify(siteReadiness('absent', 'present'))).not.toContain('nginx');
	});
});

// Spec §7.3 — ONE banner naming both, never two stacked. There is no shape in
// which this function can return two notices, which is the structural half of
// the guarantee; this pins the copy half.
describe('neither installed (spec §7.3)', () => {
	it('is one notice with two lines, in a fixed order', () => {
		expect(siteReadiness('absent', 'absent')).toEqual({
			title: READINESS_MULTI_TITLE,
			lines: [
				{
					id: 'php',
					text: 'No PHP version is installed.',
					route: '/languages',
					linkText: 'Install a version on the Languages page'
				},
				{
					id: 'nginx',
					text: 'nginx is not installed.',
					route: '/web-server',
					linkText: 'Check the Web server page'
				}
			]
		});
	});

	it('titles itself without claiming which one is missing', () => {
		const title = siteReadiness('absent', 'absent')?.title ?? '';
		expect(title).not.toContain('PHP');
		expect(title).not.toContain('nginx');
	});

	// Each line states its own fact here, because the title no longer can.
	// Single-missing keeps the "why" instead — the same requirement, two
	// sentences, chosen in one place so they cannot disagree.
	it('swaps each line from "why it matters" to "what is missing"', () => {
		expect(siteReadiness('absent', 'present')?.lines[0].text).toBe('Sites need one to run.');
		expect(siteReadiness('absent', 'absent')?.lines[0].text).toBe('No PHP version is installed.');
	});
});

// Spec §7.4 — every developed machine today, including the one this was written
// on. The banner must be invisible there or the slice is a regression for
// everyone.
describe('both installed (spec §7.4)', () => {
	it('shows no banner', () => {
		expect(siteReadiness('present', 'present')).toBeNull();
	});
});

// Spec §7.5 / §7.6 — "we have not looked" and "the look failed" both arrive here
// as `unknown`, and neither may become a claim. The page keeps them apart for
// its error banner; the readiness rule treats them the same on purpose.
describe('unknown is never a claim (spec §7.5, §7.6)', () => {
	it('says nothing before either read returns', () => {
		expect(siteReadiness('unknown', 'unknown')).toBeNull();
	});

	it('does not claim PHP is missing when only nginx has answered', () => {
		expect(siteReadiness('unknown', 'present')).toBeNull();
		expect(siteReadiness('unknown', 'absent')?.lines.map((l) => l.id)).toEqual(['nginx']);
	});

	it('does not claim nginx is missing when only PHP has answered', () => {
		expect(siteReadiness('present', 'unknown')).toBeNull();
		expect(siteReadiness('absent', 'unknown')?.lines.map((l) => l.id)).toEqual(['php']);
	});

	// A failed PHP read alongside a CONFIRMED missing nginx: the banner must
	// still name nginx. This is the case an `{:else if}` chain on the page would
	// silence, and the reason that chain was broken up.
	it('still names the side that answered when the other read failed', () => {
		const notice = siteReadiness('unknown', 'absent');
		expect(notice?.title).toBe('nginx is not installed');
		expect(notice?.lines).toHaveLength(1);
	});
});

describe('phpCheck', () => {
	it('reads null as unknown — not as an empty environment', () => {
		expect(phpCheck(null)).toBe('unknown');
	});

	it('reads an empty list as a confirmed absence', () => {
		expect(phpCheck([])).toBe('absent');
	});

	it('reads any installed major as present', () => {
		expect(phpCheck(['8.4'])).toBe('present');
		expect(phpCheck(['8.1', '8.4'])).toBe('present');
	});
});

describe('nginxCheck', () => {
	it('reads null as unknown — not as a missing nginx', () => {
		expect(nginxCheck(null)).toBe('unknown');
	});

	it('reads a row with a binary as present', () => {
		expect(
			nginxCheck([nginxRow('/Users/x/.openvhost/pkg/nginx/1.30.4/sbin/nginx'), apacheRow()])
		).toBe('present');
	});

	it('reads a row with no binary as a confirmed absence', () => {
		expect(nginxCheck([nginxRow(null), apacheRow()])).toBe('absent');
	});

	// Apache also reports `binaryPath: null`. Matching on the row id rather than
	// on "some row has no binary" is what keeps the unsupported row from
	// answering a question about nginx.
	it('is not answered by the Apache row, which also has no binary', () => {
		expect(nginxCheck([nginxRow('/opt/homebrew/bin/nginx'), apacheRow()])).toBe('present');
	});

	// A list without an nginx row is a shape this function does not recognise.
	// Claiming "nginx is not installed" from it would be a false statement about
	// the machine derived from a DTO change — the failure mode, not the warning.
	it('reads a list with no nginx row as unknown, never as absent', () => {
		expect(nginxCheck([apacheRow()])).toBe('unknown');
		expect(nginxCheck([])).toBe('unknown');
	});

	// A Homebrew nginx has a binary but no version (nginx has no --version flag,
	// so the row reports `null` when the probe fails). Present is decided by the
	// binary, never by the version.
	it('counts a nginx whose version could not be probed as present', () => {
		const unprobed: WebServerDto = {
			...nginxRow('/opt/homebrew/bin/nginx'),
			version: null,
			source: { kind: 'homebrew' }
		};
		expect(nginxCheck([unprobed, apacheRow()])).toBe('present');
	});
});
