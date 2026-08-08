// SPDX-License-Identifier: GPL-3.0-or-later
//
// The rendering half of the readiness banner. `site-readiness.derive.test.ts`
// pins WHAT gets said; this pins that all of it reaches the screen — the title,
// every line, and a working link per line.
//
// Rendered through `svelte/server`, which needs no DOM, so it runs in the
// existing `node` vitest project (the pattern `SiteDrawer.svelte.test.ts`
// established). This component has no interaction to drive.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import SiteReadinessBanner from './SiteReadinessBanner.svelte';
import { READINESS_MULTI_TITLE, siteReadiness } from '$lib/site-readiness.derive';

/** The notice for a given pair, or a thrown error — a `null` notice reaching
 *  this component is a caller bug, and a test that silently rendered nothing
 *  would look like a pass. */
function notice(php: 'unknown' | 'present' | 'absent', nginx: 'unknown' | 'present' | 'absent') {
	const n = siteReadiness(php, nginx);
	if (n === null) throw new Error(`siteReadiness(${php}, ${nginx}) has nothing to render`);
	return n;
}

/** Everything the user can actually read, tags stripped and whitespace
 *  collapsed — so a copy assertion cannot pass on markup the reader never
 *  sees, and cannot fail on how Svelte happened to wrap a line. */
function visible(body: string): string {
	return body
		.replace(/<[^>]*>/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('one missing requirement', () => {
	it('leads with the PHP headline and links the Languages page', () => {
		const { body } = render(SiteReadinessBanner, {
			props: { notice: notice('absent', 'present') }
		});
		expect(visible(body)).toBe(
			'No PHP version is installed yet Sites need one to run. Install a version on the Languages page .'
		);
		expect(body).toContain('href="/languages"');
	});

	it('leads with the nginx headline and links the Web server page', () => {
		const { body } = render(SiteReadinessBanner, {
			props: { notice: notice('present', 'absent') }
		});
		expect(visible(body)).toBe(
			'nginx is not installed Sites are served by nginx. Check the Web server page .'
		);
		expect(body).toContain('href="/web-server"');
	});

	// A single line is a sentence, not a one-item bullet list — that is what
	// keeps the PHP-only machine reading exactly as it did before this slice.
	it('renders no list', () => {
		const { body } = render(SiteReadinessBanner, {
			props: { notice: notice('absent', 'present') }
		});
		expect(body).not.toContain('<ul');
	});
});

describe('both missing', () => {
	it('is ONE banner, with both lines inside it', () => {
		const { body } = render(SiteReadinessBanner, { props: { notice: notice('absent', 'absent') } });
		expect(body.match(/data-testid="site-readiness-banner"/g)).toHaveLength(1);
		expect(body).toContain('data-testid="readiness-php"');
		expect(body).toContain('data-testid="readiness-nginx"');
	});

	it('says both facts and offers both remedies', () => {
		const { body } = render(SiteReadinessBanner, { props: { notice: notice('absent', 'absent') } });
		expect(visible(body)).toBe(
			`${READINESS_MULTI_TITLE} No PHP version is installed. ` +
				'Install a version on the Languages page . nginx is not installed. Check the Web server page .'
		);
		expect(body).toContain('href="/languages"');
		expect(body).toContain('href="/web-server"');
	});

	it('keeps the two remedies in list semantics rather than one run-on line', () => {
		const { body } = render(SiteReadinessBanner, { props: { notice: notice('absent', 'absent') } });
		expect(body.match(/<li\b/g)).toHaveLength(2);
	});
});

describe('the banner element', () => {
	// role="status" (polite), not "alert": nothing has failed. An assertive
	// region on every launch of a fresh install would interrupt a screen-reader
	// user for a fact they are about to reach anyway.
	it('is a polite live region, not an alert', () => {
		const { body } = render(SiteReadinessBanner, { props: { notice: notice('absent', 'absent') } });
		expect(body).toContain('role="status"');
		expect(body).not.toContain('role="alert"');
	});
});
