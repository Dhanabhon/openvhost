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
});
