// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import SiteDrawer from './SiteDrawer.svelte';
import type { SiteDto } from '$lib/ipc';

// Rendered server-side (`svelte/server`), which needs no DOM and so runs in the
// existing `node` vitest project. Svelte's SSR output carries the same `selected`
// attribute the browser would apply to a `<select>`'s current value, which is
// exactly what this file is about — see `selectedValues` below.

const site = (phpVersion: string): SiteDto => ({
	id: 'a',
	name: 'legacy',
	domain: 'legacy.localhost',
	docroot: '/srv/www/legacy',
	webServer: 'nginx',
	phpVersion,
	enabled: true,
	createdAt: 1,
	updatedAt: 1
});

/** The drawer's PHP-version `<select>`, rendered for `site` (`null` = the Add form). */
function phpSelect(dto: SiteDto | null): string {
	const { body } = render(SiteDrawer, {
		props: {
			site: dto,
			fieldErrors: {},
			onSave: async () => true,
			onDelete: async () => true,
			onClose: () => {}
		}
	});
	const select = body.match(/<select\b[^>]*id="f-php"[\s\S]*?<\/select>/);
	if (select === null) throw new Error('the drawer rendered no PHP-version <select>');
	return select[0];
}

/**
 * `value` of every `<option>` marked selected. Parsed attribute-order-tolerantly
 * rather than by matching one literal tag string, so the assertions below survive
 * a change in how Svelte orders the attributes it emits.
 */
function selectedValues(selectHtml: string): string[] {
	return [...selectHtml.matchAll(/<option\b([^>]*)>/g)]
		.filter(([, attrs]) => /\sselected(?=[\s=>]|$)/.test(attrs))
		.map(([, attrs]) => attrs.match(/value="([^"]*)"/)?.[1] ?? '');
}

describe('SiteDrawer PHP version', () => {
	// Regression (whole-branch review of #11): `PHP_VERSIONS` is a closed list, but
	// state.db can hold any version an older build — or a later edit of that list —
	// allowed. With no matching `<option>` the select rendered blank and the bound
	// value silently became the browser's own pick, so Save rewrote the site's PHP
	// version to something the user never chose.
	it('keeps a stored version that is not in the offered list selected', () => {
		expect(selectedValues(phpSelect(site('8.0')))).toEqual(['8.0']);
	});

	it('marks that version as not available rather than passing it off as offered', () => {
		expect(phpSelect(site('8.0'))).toContain('>8.0 — not available</option>');
	});

	// Control for the two above: proves `selectedValues` can see a selection at all,
	// so the '8.0' assertion fails on a missing option rather than on SSR simply
	// never emitting `selected`.
	it('selects a stored version that is in the offered list', () => {
		expect(selectedValues(phpSelect(site('8.3')))).toEqual(['8.3']);
	});

	it('leaves the offered list alone for a stored version that is in it', () => {
		expect(phpSelect(site('8.3'))).not.toContain('not available');
	});

	it('selects the newest offered version on the Add form', () => {
		expect(selectedValues(phpSelect(null))).toEqual(['8.4']);
	});
});
