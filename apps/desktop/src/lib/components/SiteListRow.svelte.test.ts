// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node` vitest project
// with no DOM — the same approach as SiteDrawer.svelte.test.ts.
//
// WHAT THIS FILE CANNOT COVER: the two-step delete confirm needs a click, and there is no
// DOM here, so only the row's INITIAL state is assertable — that the first Delete press
// cannot itself destroy anything (asserted below via the absence of `btn-danger`). That the
// second press calls `onDelete`, that Cancel returns to the normal actions, and that a
// mid-confirm list refetch keeps the confirm on the same row, are manual click-through
// items in the PR. The store-side guarantees (re-entrancy, error routing) are covered in
// sites.svelte.test.ts, where they are real functions rather than markup.

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import SiteListRow from './SiteListRow.svelte';
import type { SiteDto } from '$lib/ipc';

const site = (enabled: boolean, phpVersion = '8.3'): SiteDto => ({
	id: 'a1',
	name: 'shop',
	domain: 'shop.localhost',
	docroot: '/srv/www/shop',
	webServer: 'nginx',
	phpVersion,
	enabled,
	createdAt: 1,
	updatedAt: 1
});

function rowHtml(
	dto: SiteDto,
	extra: { busy?: boolean; rowError?: string; installed?: readonly string[] | null } = {}
): string {
	return render(SiteListRow, {
		props: {
			site: dto,
			installed: extra.installed ?? [dto.phpVersion],
			onEdit: () => {},
			onToggleEnabled: () => {},
			onOpen: () => {},
			onDelete: () => {},
			busy: extra.busy ?? false,
			rowError: extra.rowError ?? ''
		}
	}).body;
}

describe('SiteListRow actions', () => {
	// The label is the whole affordance: a toggle that says "Disable" on a disabled site
	// tells the user the opposite of the truth.
	it('offers Disable on an enabled site and Enable on a disabled one', () => {
		const on = rowHtml(site(true));
		expect(on).toContain('>Disable<');
		expect(on).not.toContain('>Enable<');

		const off = rowHtml(site(false));
		expect(off).toContain('>Enable<');
		expect(off).not.toContain('>Disable<');
	});

	// Every row renders the same three verbs, so an accessible name of just "Delete" is
	// ambiguous across a list. Each must name its site.
	it('names the site in every action label', () => {
		const html = rowHtml(site(true));
		expect(html).toContain('aria-label="Disable shop"');
		expect(html).toContain('aria-label="Edit shop"');
		expect(html).toContain('aria-label="Delete shop"');
	});

	// One press must only ASK. `btn-danger` is applied to exactly one control in this
	// component — the confirm step — so its absence here is the assertion that the
	// destructive control is not reachable in a single click.
	it('does not render the destructive control before confirming', () => {
		expect(rowHtml(site(true))).not.toContain('btn-danger');
	});

	// An icon has no text, so its accessible name is the ONLY thing a screen reader
	// gets — and every row's glyph is identical, so the name must carry the site.
	it('names the site on the icon-only open button', () => {
		const m = rowHtml(site(true));
		expect(m).toContain('aria-label="Open shop in a browser"');
		// The glyph itself is decorative once the button is named; announcing it twice
		// is worse than not announcing it.
		expect(m).toContain('aria-hidden="true"');
		// Sighted users get the same sentence on hover, naming the domain.
		expect(m).toContain('title="Open shop.localhost in a browser"');
	});

	// A disabled site is not being served, so the button would open a page that
	// cannot load. This is the whole reason the button is state-aware.
	it('disables opening for a disabled site but not for an enabled one', () => {
		expect(rowHtml(site(false))).toContain('aria-label="Open shop in a browser" disabled');
		expect(rowHtml(site(true))).not.toContain('aria-label="Open shop in a browser" disabled');
	});

	it('disables the row actions while an action is in flight', () => {
		expect(rowHtml(site(true), { busy: true })).toContain('disabled');
		expect(rowHtml(site(true), { busy: false })).not.toContain('disabled');
	});

	// `role="alert"` so the failure is announced, not just drawn — a row action gives no
	// other feedback that it did nothing.
	it('announces a row error and omits the element when there is none', () => {
		const failed = rowHtml(site(true), { rowError: 'docroot is gone' });
		expect(failed).toContain('role="alert"');
		expect(failed).toContain('docroot is gone');

		expect(rowHtml(site(true))).not.toContain('role="alert"');
	});

	// Visible the moment a site is out of sync with this machine, not only as a
	// surprise after a failed Apply — a machine changes under a site at any time
	// (`brew uninstall php@8.3`), and Task 8 only ever prevents a NEW site from
	// choosing a version that is missing.
	it('warns on the row when a site wants a version that is not installed', () => {
		const body = rowHtml(site(true, '8.4'), { installed: ['8.5'] });
		expect(body).toContain('data-testid="php-missing"');
		expect(body).toContain('8.4');
	});

	it('does not warn when the version is installed', () => {
		const body = rowHtml(site(true, '8.5'), { installed: ['8.5'] });
		expect(body).not.toContain('data-testid="php-missing"');
	});

	// I2 (branch-review-fix-report.md): `installed: null` means the environment
	// is UNKNOWN — still loading, or the read failed — not "definitely nothing
	// installed". Before the fix, `+page.svelte` collapsed both into `[]`, which
	// flagged EVERY row as "not installed" during the load flash and, worse, on
	// every failed read. The same site with a KNOWN-empty list (`[]`) still
	// warns — proving `null` suppresses the badge for its own reason, not
	// because an empty array happens to never match.
	it('suppresses the badge when the environment is unknown, even for a version nothing lists', () => {
		const unknown = rowHtml(site(true, '8.4'), { installed: null });
		expect(unknown).not.toContain('data-testid="php-missing"');

		const knownEmpty = rowHtml(site(true, '8.4'), { installed: [] });
		expect(knownEmpty).toContain('data-testid="php-missing"');
	});
	// Same defect as LanguageRow's Recommended badge: the version was a bare text
	// node in a fixed track, so adding a badge beside it took the width out of the
	// label and "PHP 8.4" wrapped — on exactly the rows the warning is for.
	it('gives the version label its own element so it can refuse to wrap', () => {
		const body = rowHtml(site(true, '8.4'), { installed: ['8.5'] });
		expect(body).toMatch(/<span[^>]*class="[^"]*\bversion\b[^"]*"[^>]*>\s*PHP 8\.4/);
	});

	it('renders the not-installed badge after the label, not inside it', () => {
		const body = rowHtml(site(true, '8.4'), { installed: ['8.5'] });
		const label = body.indexOf('PHP 8.4');
		const badge = body.indexOf('data-testid="php-missing"');
		expect(label).toBeGreaterThan(-1);
		expect(badge).toBeGreaterThan(label);
	});
});

// Spec D6: every site row (not only a "broken" one — SiteDto carries no
// state to be broken) gains a "View logs" deep link, defaulting to the
// site's ERROR log (the live-proof entry point).
describe('SiteListRow View logs deep link', () => {
	it('links to /logs with the site error log preselected', () => {
		const html = rowHtml(site(true));
		expect(html).toContain('href="/logs?source=site-error%3Ashop.localhost"');
	});

	it('names the site, not just "View logs" — the icon has no other text', () => {
		const html = rowHtml(site(true));
		expect(html).toContain('aria-label="View logs for shop"');
		expect(html).toContain('title="View logs for shop.localhost"');
	});

	it('stays reachable even when the site is disabled, unlike Open', () => {
		const html = rowHtml(site(false));
		const link = html.match(/<a[^>]*aria-label="View logs for shop"[^>]*>/)?.[0];
		expect(link).not.toContain('disabled');
	});

	it('is a real navigation link, not a button pretending to be one', () => {
		const html = rowHtml(site(true));
		expect(html).toMatch(/<a[^>]*aria-label="View logs for shop"/);
	});
});

// The wrapped narrow layout is a CSS container query, and there is no layout engine here —
// nothing below asserts that anything WRAPS. That was measured in a browser against the real
// stylesheet and the numbers are in the PR; a jsdom assertion about a class would only be a
// test that cannot fail, which this project has shipped enough of.
//
// What IS worth guarding is the seam, because it fails SILENTLY: the query lives in this
// component and names a container that only SitesPanel declares. Delete `container-name`
// there and every rule below `@container` simply stops matching — no error, no failing
// render, no visual difference until someone narrows the window and loses Delete again.
describe('the narrow-width layout stays wired to its container', () => {
	const read = (f: string) =>
		readFileSync(new URL(f, import.meta.url), 'utf8').match(/<style>([\s\S]*?)<\/style>/)?.[1] ??
		'';

	it('queries a container that SitesPanel actually declares', () => {
		const queried = read('./SiteListRow.svelte').match(/@container\s+([\w-]+)\s*\(/)?.[1];
		expect(queried, 'SiteListRow should query a NAMED container').toBeDefined();

		const panel = read('./SitesPanel.svelte');
		// Accept the longhand the source uses today or the `container:` shorthand a minifier
		// or a later refactor may produce — the point is the name, not how it is spelled.
		expect(panel).toMatch(
			new RegExp(`container-name:\\s*${queried}\\b|container:\\s*${queried}\\b`)
		);
		expect(panel).toMatch(/container-type:\s*inline-size|container:\s*[\w-]+\s*\/\s*inline-size/);
	});

	// The other half of the same silence. The wiring above can't drop, but the NUMBER can go
	// stale: widen a track, add a column, lengthen a button label, and the one-line row costs
	// more than the width at which it wraps — so it goes back to overflowing `.panel`'s
	// `overflow: hidden` and eating Delete, with no failing test and nothing to see until
	// someone narrows the window. This re-derives the cost from the stylesheet itself.
	it('keeps the one-line cost below the width at which the row wraps', () => {
		const css = read('./SiteListRow.svelte');
		const tokens = readFileSync(new URL('../styles/tokens.css', import.meta.url), 'utf8');
		const resolve = (decl: string) =>
			/^\d+px$/.test(decl)
				? Number(decl.slice(0, -2))
				: Number(
						tokens.match(
							new RegExp(`${decl.match(/var\((--[\w-]+)\)/)?.[1] ?? '\0'}:\\s*(\\d+)px`)
						)?.[1]
					);

		const tracks = (css.match(/grid-template-columns:\s*([^;]+);/)?.[1] ?? '')
			.replace(/minmax\(\s*(\d+px)[^)]*\)/g, '$1') // a track's floor is its minmax minimum
			.trim()
			.split(/\s+/);
		const floors = tracks.filter((t) => /^\d+px$/.test(t)).map((t) => Number(t.slice(0, -2)));
		// The one content-sized track is the actions; its floor lives on the element itself.
		const actionFloor = Number(css.match(/\.row-actions\s*\{[\s\S]*?min-width:\s*(\d+)px/)?.[1]);
		const rowRule = css.match(/\.row\s*\{[\s\S]*?\}/)?.[0] ?? '';
		const gap = resolve(rowRule.match(/\bgap:\s*([^;]+);/)?.[1]?.trim() ?? '');
		const padX = resolve(rowRule.match(/padding:\s*\S+\s+([^;]+);/)?.[1]?.trim() ?? '');
		const wrapsBelow = Number(css.match(/@container\s+[\w-]+\s*\(width\s*<\s*(\d+)px\)/)?.[1]);

		// Without this, a regex that silently stops matching yields 0 and the assertion below
		// passes for the wrong reason — a test that cannot fail, dressed as one that can.
		expect({ floors: floors.length, autos: tracks.filter((t) => t === 'auto').length }).toEqual({
			floors: tracks.length - 1,
			autos: 1
		});
		for (const [name, n] of Object.entries({ actionFloor, gap, padX, wrapsBelow })) {
			expect(n, `${name} should have been read out of the stylesheet`).toBeGreaterThan(0);
		}

		const oneLineCost =
			floors.reduce((a, b) => a + b, 0) + actionFloor + (tracks.length - 1) * gap + 2 * padX;
		expect(
			wrapsBelow,
			`the row costs ${oneLineCost}px on one line but only wraps below ${wrapsBelow}px — ` +
				`raise the @container threshold above ${oneLineCost}`
		).toBeGreaterThanOrEqual(oneLineCost);
	});
});
