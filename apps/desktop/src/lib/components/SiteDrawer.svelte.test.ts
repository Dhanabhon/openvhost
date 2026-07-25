// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), which needs no DOM and so runs in the existing
// `node` vitest project.
//
// The PHP-version field is no longer a native `<select>`: it is `Select.svelte`, the APG
// select-only combobox. The first five cases below therefore assert the SAME INTENT the
// `<select>`-era ones did — a stored version that the offered list does not contain stays
// present and stays marked as the current selection — against `role="option"` +
// `aria-selected="true"` and the collapsed trigger's own visible value, instead of against
// `<option selected>`. The popup deliberately stays in the DOM while closed (`hidden`), so
// the whole option set is in server-rendered markup and remains assertable here.
//
// WHAT THIS FILE CANNOT COVER: there is no DOM in this project, so every interactive
// behaviour of the new controls — keyboard navigation and typeahead, caret position after a
// real keystroke, focus staying on the trigger while the popup is open, click-outside,
// Escape closing only the popup and not the whole drawer — is out of reach. Do not add a
// browser/jsdom project for it. The caret ARITHMETIC is the exception: it is exported as
// pure functions (`filterDomainInput`, `filterNameInput`) precisely so the part most likely
// to be wrong is testable here. The rest is listed as manual click-through in the task
// report.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import SiteDrawer, { filterDomainInput, filterNameInput } from './SiteDrawer.svelte';
import { PHP_VERSIONS, WEB_SERVERS } from '$lib/sites.derive';
import type { SiteDto } from '$lib/ipc';

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

/**
 * Every `role="option"` row in document order. Parsed attribute-order-tolerantly rather
 * than by matching one literal tag string, so these assertions survive a change in how
 * Svelte orders the attributes it emits.
 */
function phpOptions(html: string): { label: string; selected: boolean }[] {
	return [...html.matchAll(/<button\b([^>]*\brole="option"[^>]*)>([\s\S]*?)<\/button>/g)].map(
		([, attrs, inner]) => ({
			label: text(inner.match(/class="option-label[^"]*">([^<]*)</)?.[1] ?? ''),
			selected: /\baria-selected="true"/.test(attrs)
		})
	);
}

/** Labels of the rows marked as the current selection (expected: exactly one). */
function selectedLabels(html: string): string[] {
	return phpOptions(html)
		.filter((o) => o.selected)
		.map((o) => o.label);
}

/** What the COLLAPSED trigger shows — the selection a user actually sees before opening. */
function triggerValue(html: string): string {
	const match = html.match(/class="trigger-value[^"]*">([^<]*)</);
	if (match === null) throw new Error('the drawer rendered no PHP-version combobox trigger');
	return text(match[1]);
}

/** The web-server segmented control, and its two toggle buttons. */
function serverGroup(html: string): string {
	const match = html.match(/<div\b[^>]*role="group"[\s\S]*?<\/div>/);
	if (match === null) throw new Error('the drawer rendered no web-server group');
	return match[0];
}
function serverButtons(html: string): { label: string; attrs: string }[] {
	return [...serverGroup(html).matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].map(
		([, attrs, inner]) => ({ attrs, label: text(inner) })
	);
}

describe('SiteDrawer PHP version', () => {
	// Regression (whole-branch review of #11): `PHP_VERSIONS` is a closed list, but state.db
	// can hold any version an older build — or a later edit of that list — allowed. The
	// native `<select>` rendered blank for an unmatched value and the binding silently took
	// the browser's own pick, so Save rewrote the site's PHP version to something the user
	// never chose. The listbox has the same failure mode by another name: a value with no
	// row would leave nothing marked selected and nothing to navigate back to.
	it('keeps a stored version that is not in the offered list selected', () => {
		expect(selectedLabels(drawerHtml(site('8.0')))).toEqual(['8.0 — not available']);
	});

	it('shows that stored version on the collapsed trigger, so it is visible unopened', () => {
		expect(triggerValue(drawerHtml(site('8.0')))).toBe('8.0 — not available');
	});

	it('marks that version as not available rather than passing it off as offered', () => {
		expect(phpOptions(drawerHtml(site('8.0'))).map((o) => o.label)).toContain(
			'8.0 — not available'
		);
	});

	// Control for the three above: proves `selectedLabels` can see a selection at all, so
	// the '8.0' assertions fail on a missing row rather than on SSR simply never emitting
	// `aria-selected="true"`.
	it('selects a stored version that is in the offered list', () => {
		expect(selectedLabels(drawerHtml(site('8.3')))).toEqual(['8.3']);
	});

	it('leaves the offered list alone for a stored version that is in it', () => {
		expect(drawerHtml(site('8.3'))).not.toContain('not available');
	});

	it('selects the newest offered version on the Add form', () => {
		expect(selectedLabels(drawerHtml(null))).toEqual(['8.4']);
	});

	it('offers every version in PHP_VERSIONS', () => {
		const labels = phpOptions(drawerHtml(site('8.0'))).map((o) => o.label);
		expect(labels).toEqual(['8.0 — not available', ...PHP_VERSIONS]);
	});

	// The native `<select>` is gone; what replaces it has to be a real listbox, not a
	// div that merely looks like one.
	it('exposes the field as a collapsed listbox combobox instead of a native select', () => {
		const html = drawerHtml(site('8.3'));
		const trigger = tagWith(html, 'id="f-php"');
		expect(trigger).toContain('role="combobox"');
		expect(trigger).toContain('aria-haspopup="listbox"');
		expect(trigger).toContain('aria-expanded="false"');
		expect(trigger).toContain('aria-controls="f-php-listbox"');
		expect(tagWith(html, 'id="f-php-listbox"')).toContain('role="listbox"');
		expect(html).not.toContain('<select');
	});

	it('hides the popup until it is opened', () => {
		expect(tagWith(drawerHtml(site('8.3')), 'id="f-php-listbox"')).toMatch(/\shidden(?=[\s=>])/);
	});

	// The popup must stay inside the drawer's own subtree: SiteDrawer traps focus with a
	// window-scoped `focusin` handler that recaptures anything landing outside the dialog,
	// so a portalled popup would be yanked back the instant it opened. SSR cannot see a
	// runtime `document.body` portal, but it does pin the markup's shape.
	it('renders the popup inside the dialog element, not as a sibling of it', () => {
		const html = drawerHtml(site('8.3'));
		const dialog = html.indexOf('role="dialog"');
		const listbox = html.indexOf('id="f-php-listbox"');
		expect(dialog).toBeGreaterThan(-1);
		expect(listbox).toBeGreaterThan(dialog);
		expect(html.slice(listbox)).toContain('</aside>');
	});

	it('keeps the error wiring the native select carried', () => {
		const html = drawerHtml(site('8.3'), { php_version: 'must be major.minor digits, e.g. 8.3' });
		const trigger = tagWith(html, 'id="f-php"');
		expect(trigger).toContain('aria-invalid="true"');
		expect(trigger).toContain('aria-describedby="f-php-error"');
		expect(html).toContain('id="f-php-error"');
	});

	it('leaves aria-invalid off when the backend reported no problem', () => {
		expect(tagWith(drawerHtml(site('8.3')), 'id="f-php"')).not.toContain('aria-invalid');
	});

	it('keeps the per-site hint', () => {
		expect(text(drawerHtml(site('8.3')))).toContain(
			'Applies to this site only. Other sites keep their own version.'
		);
	});
});

describe('SiteDrawer web server', () => {
	it('offers exactly the web servers the frontend knows about', () => {
		expect(serverButtons(drawerHtml(null)).map((b) => b.label)).toEqual([...WEB_SERVERS]);
	});

	it('shows a brand mark beside each label', () => {
		expect(serverGroup(drawerHtml(null)).match(/class="brand/g)).toHaveLength(WEB_SERVERS.length);
	});

	// The owner's explicit decision: Apache keeps its logo and stays selectable, and the UI
	// says plainly that OpenVHost cannot serve it. A future "helpfully" disabled button
	// should fail here and be re-decided, not slipped in.
	it('leaves Apache selectable', () => {
		const apache = serverButtons(drawerHtml(null)).find((b) => b.label === 'apache');
		if (apache === undefined) throw new Error('the drawer rendered no Apache button');
		expect(apache.attrs).not.toMatch(/\bdisabled\b/);
	});

	it('states that OpenVHost cannot serve Apache sites yet', () => {
		expect(text(drawerHtml(null))).toContain(
			'OpenVHost cannot serve Apache sites yet — it only generates nginx config. ' +
				"An Apache site will save, but it won't be served."
		);
	});

	// Visually adjacent is not enough — a screen-reader user reaching the group has to hear
	// it. `aria-describedby` is read in the order given, so a backend error stays first.
	it('associates that notice with the group, after any backend error', () => {
		expect(serverGroup(drawerHtml(null))).toContain('aria-describedby="f-server-hint"');
		expect(serverGroup(drawerHtml(null, { web_server: 'nope' }))).toContain(
			'aria-describedby="f-server-error f-server-hint"'
		);
		expect(drawerHtml(null)).toContain('id="f-server-hint"');
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

describe('SiteDrawer name field', () => {
	// The owner typed Thai into this field and got the raw backend slug error back. Filtering
	// alone would have been worse in one way — a Thai keystroke produces NOTHING, so with no
	// stated rule the field looks broken — hence the permanent hint, asserted below.
	it('states the rule permanently, not only after a rejected save', () => {
		expect(drawerHtml(null)).toContain('id="f-name-hint"');
		expect(text(drawerHtml(null))).toContain('Lowercase letters, numbers and dashes only');
	});

	it('keeps the hint described even while a backend error is showing, error first', () => {
		expect(tagWith(drawerHtml(null, { name: 'taken' }), 'id="f-name"')).toContain(
			'aria-describedby="f-name-error f-name-hint"'
		);
	});

	// UNLIKE the domain field above, which deliberately has NO maxlength: there the input holds
	// only the label and `.localhost` is appended, so no client-side length can correspond to
	// `Domain::parse`'s 253-byte bound on the whole domain. Here the field holds exactly the
	// string `SiteName::parse` bounds at 1..=63 BYTES, and the filter guarantees ASCII, so 63
	// is the same number in both. Do not "harmonise" these two — they differ for a reason.
	it('caps length at the 63 SiteName::parse allows', () => {
		expect(tagWith(drawerHtml(site('8.3')), 'id="f-name"')).toContain('maxlength="63"');
	});

	it('keeps a slug untouched', () => {
		expect(filterNameInput('my-site-2', 9)).toEqual({ value: 'my-site-2', caret: 9 });
	});

	// THE difference from the domain filter. A dot is legal in a hostname and illegal in a
	// name; sharing one charset between the two fields would let `my.site` reach a backend
	// that rejects it.
	it('drops the dot a hostname would have kept', () => {
		expect(filterNameInput('my.site', 7)).toEqual({ value: 'mysite', caret: 6 });
	});

	it('drops Thai text rather than sending it to a parser that rejects it', () => {
		// The exact input from the owner's report.
		expect(filterNameInput('ทดสอบ', 5)).toEqual({ value: '', caret: 0 });
		// And mixed, so it is clear the Latin part survives rather than the whole entry dying.
		expect(filterNameInput('ทดสอบshop', 9)).toEqual({ value: 'shop', caret: 4 });
	});

	it('lowercases instead of rejecting', () => {
		expect(filterNameInput('MyShop', 6)).toEqual({ value: 'myshop', caret: 6 });
	});

	it('strips a leading dash, which SiteName::parse forbids', () => {
		expect(filterNameInput('-shop', 5)).toEqual({ value: 'shop', caret: 4 });
		expect(filterNameInput('---shop', 7)).toEqual({ value: 'shop', caret: 4 });
		// Caret inside the stripped run collapses to the start rather than going negative.
		expect(filterNameInput('---shop', 2)).toEqual({ value: 'shop', caret: 0 });
	});

	// A trailing dash is NOT stripped, deliberately: someone typing `my-` is mid-word, and
	// eating the dash would make `my-site` impossible to type. The backend error covers it.
	it('leaves a trailing dash alone so a dashed name stays typable', () => {
		expect(filterNameInput('my-', 3)).toEqual({ value: 'my-', caret: 3 });
	});

	it('keeps the caret where the user is typing when a character is rejected', () => {
		expect(filterNameInput('ab_cd', 3)).toEqual({ value: 'abcd', caret: 2 });
	});
});
