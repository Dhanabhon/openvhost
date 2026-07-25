// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import SiteDrawer, { filterDomainInput } from './SiteDrawer.svelte';
import type { SiteDto } from '$lib/ipc';

// Rendered server-side (`svelte/server`), which needs no DOM and so runs in the
// existing `node` vitest project. Svelte's SSR output carries the same `selected`
// attribute the browser would apply to a `<select>`'s current value, which is
// exactly what this file is about — see `selectedValues` below.
//
// The Domain field's caret handling cannot be driven here (no DOM, no key events), so
// its arithmetic is exported as a pure function and asserted directly instead.

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

/** The drawer's markup, rendered for `dto` (`null` = the Add form). */
function drawerHtml(dto: SiteDto | null, fieldErrors: Record<string, string> = {}): string {
	return render(SiteDrawer, {
		props: {
			site: dto,
			fieldErrors,
			onSave: async () => true,
			onDelete: async () => true,
			onClose: () => {}
		}
	}).body;
}

/** Collapse the whitespace Svelte preserves from source indentation. */
function text(html: string): string {
	return html
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

/** One element's opening tag, found by an attribute it carries. */
function tagWith(html: string, attribute: string): string {
	const match = html.match(new RegExp(`<[a-z]+\\b[^>]*${attribute}[^>]*>`));
	if (match === null) throw new Error(`no element carrying ${attribute}`);
	return match[0];
}

/** The drawer's PHP-version `<select>`. */
function phpSelect(dto: SiteDto | null): string {
	const select = drawerHtml(dto).match(/<select\b[^>]*id="f-php"[\s\S]*?<\/select>/);
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

describe('SiteDrawer domain field', () => {
	it('still edits the label only, with .localhost rendered beside it', () => {
		const html = drawerHtml(site('8.3'));
		expect(tagWith(html, 'id="f-domain"')).toContain('value="legacy"');
		expect(text(html)).toContain('.localhost');
	});

	// The filter is a typing affordance, not validation: `Domain::parse` in
	// crates/openvhost-core/src/site/model.rs is the authority. A `pattern`/`required`/
	// `maxlength` gate here could disagree with it and block a Save the backend would have
	// accepted (or accept one it rejects), so there deliberately is none.
	it('adds no client-side validity gate that could disagree with Domain::parse', () => {
		const input = tagWith(drawerHtml(site('8.3')), 'id="f-domain"');
		expect(input).not.toMatch(/\b(pattern|required|maxlength|minlength)\b/);
	});

	it('keeps hostname characters untouched', () => {
		expect(filterDomainInput('my-site.dev', 11)).toEqual({ value: 'my-site.dev', caret: 11 });
	});

	it('lowercases and drops everything a hostname label may not contain', () => {
		// A paste of `My_Site.COM` into an empty field: filtered, not truncated at the first
		// bad character and not rejected wholesale.
		expect(filterDomainInput('My_Site.COM', 11)).toEqual({ value: 'mysite.com', caret: 10 });
	});

	// THE regression this whole handler exists for. Typing a rejected character in the
	// middle of `abcd` must leave the caret where the user was (after `ab`), not shunt it to
	// the end of the string — which is what reassigning the value alone does.
	it('leaves the caret where the user is typing when a character is rejected', () => {
		expect(filterDomainInput('ab_cd', 3)).toEqual({ value: 'abcd', caret: 2 });
	});

	it('keeps the caret before an untouched prefix', () => {
		expect(filterDomainInput('ab_cd', 0)).toEqual({ value: 'abcd', caret: 0 });
		expect(filterDomainInput('ab_cd', 2)).toEqual({ value: 'abcd', caret: 2 });
	});

	it('counts every surviving character before the caret, not the raw offset', () => {
		// `abcd` with the caret at 2, then `My_Site.COM` pasted: raw caret 13, and the caret
		// must land after the 12 characters the paste actually contributed.
		expect(filterDomainInput('abMy_Site.COMcd', 13)).toEqual({
			value: 'abmysite.comcd',
			caret: 12
		});
	});

	it('drops non-ASCII letters instead of mangling them', () => {
		expect(filterDomainInput('café', 4)).toEqual({ value: 'caf', caret: 3 });
	});

	it('survives a caret beyond the end of the string', () => {
		expect(filterDomainInput('ab', 99)).toEqual({ value: 'ab', caret: 2 });
	});
});
