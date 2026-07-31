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
import { WEB_SERVERS } from '$lib/sites.derive';
import type { SiteDto } from '$lib/ipc';

/** Stand-in for "what's actually installed on this machine" — same values the old
 * hardcoded `PHP_VERSIONS` list held, so every pre-existing assertion below keeps its
 * original expected output even though the source of the list changed. */
const INSTALLED = ['8.4', '8.3', '8.2', '8.1'];

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

/** The drawer's markup, rendered for `dto` (`null` = the Add form). `installed`
 * defaults to {@link INSTALLED} so every existing assertion below is unaffected;
 * the "nothing installed" describe block overrides it to `[]`. */
function drawerHtml(
	dto: SiteDto | null,
	fieldErrors: Record<string, string> = {},
	installed: readonly string[] = INSTALLED
): string {
	return render(SiteDrawer, {
		props: {
			site: dto,
			fieldErrors,
			installed,
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

	it('offers every installed version', () => {
		const labels = phpOptions(drawerHtml(site('8.0'))).map((o) => o.label);
		expect(labels).toEqual(['8.0 — not available', ...INSTALLED]);
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

describe('SiteDrawer PHP version — nothing installed', () => {
	// The trap this task closes, reproduced directly: a brand-new site with no PHP
	// version installed anywhere must not present a combobox with nothing real behind
	// it, above a Save button that would carry an empty/invalid version to the backend.
	it('does not render a combobox when adding a site with nothing installed', () => {
		expect(drawerHtml(null, {}, [])).not.toContain('role="combobox"');
	});

	it('says nothing is installed and points at the Languages page', () => {
		const html = drawerHtml(null, {}, []);
		expect(text(html)).toContain('No PHP version is installed yet');
		expect(html).toContain('href="/languages"');
	});

	it('disables Save so the doomed-to-fail site cannot be submitted', () => {
		const save = tagWith(drawerHtml(null, {}, []), 'data-testid="drawer-save"');
		expect(save).toContain('disabled');
	});

	// The one case `phpVersionOptions` can never actually leave empty: an existing
	// site's own stored version is always represented, even unannotated-available, so
	// editing must stay fully possible with nothing installed.
	it('still lets an existing site with an uninstalled stored version be edited', () => {
		const html = drawerHtml(site('8.3'), {}, []);
		expect(html).toContain('role="combobox"');
		expect(tagWith(html, 'data-testid="drawer-save"')).not.toContain('disabled');
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

describe('SiteDrawer create-folder checkbox', () => {
	// WHAT THIS FILE CANNOT COVER (see header): SSR renders only the checkbox's initial,
	// unchecked state — there is no DOM here to click it, so the live preview
	// appearing/updating, and its id joining `rootDescribedBy`, once checked are asserted
	// manually instead (task report's click-list), same convention as the header comment.
	it('create mode renders the create-folder checkbox unchecked', () => {
		const input = tagWith(drawerHtml(null), 'id="f-root-create"');
		expect(input).toContain('type="checkbox"');
		expect(input).not.toContain('checked');
	});

	it('edit mode renders no create-folder control at all', () => {
		expect(drawerHtml(site('8.3'))).not.toContain('f-root-create');
	});
});

describe('SiteDrawer docroot risk warning', () => {
	// docs/superpowers/specs/2026-07-31-p1-docroot-warning-design.md — the correction
	// for the exact incident: docroot silently saved as `~/Downloads` itself.
	//
	// WHAT THIS DESCRIBE BLOCK CANNOT COVER (see the file header, same limitation as
	// the create-folder checkbox above): `docroot` is `$state`, seeded ONCE at mount
	// from `site?.docroot ?? ''` — unconditionally `''` in create mode, since a
	// brand-new site has nothing to seed it from. There is therefore no way to SSR a
	// CREATE-mode render of this drawer with a non-blank, risky docroot without
	// simulating a user typing or browsing, which this DOM-less project cannot do.
	// `DocrootRiskWarning.svelte.test.ts` is what actually proves the warning renders
	// for a risky docroot with `mode: 'create'` — it takes `risk`/`mode` as plain
	// props, sidestepping the seeding problem entirely (see that file's header). What
	// IS provable here: (1) the wiring end-to-end in EDIT mode, where `site.docroot`
	// DOES seed the state directly; (2) that the warning is not gated by
	// `{#if site === null}` the way the checkbox is — edit mode renders the warning
	// while rendering NONE of the checkbox's markup, so the two are demonstrably on
	// independent gates; (3) create mode's default (blank) docroot is a NORMAL path,
	// so "renders nothing" here doubles as the create-mode half of "absent for a
	// normal path"; (4) Save stays enabled next to the warning (warn, never block).
	function siteAt(docroot: string): SiteDto {
		return { ...site('8.3'), docroot };
	}

	/** The risk-warning paragraph's OWN inner text, so mode-specific assertions
	 *  are precise about what the warning itself says rather than coupled to
	 *  whatever else does or does not appear elsewhere on the page. */
	function riskWarningText(html: string): string {
		const match = html.match(/<p\b[^>]*data-testid="docroot-risk-warning"[^>]*>([\s\S]*?)<\/p>/);
		if (match === null) throw new Error('the drawer rendered no docroot-risk-warning element');
		return text(match[1]);
	}

	it('edit mode: a risky docroot renders the warning, naming the folder and consequence', () => {
		const html = drawerHtml(siteAt('/Users/tom/Downloads'));
		expect(html).toContain('data-testid="docroot-risk-warning"');
		const warning = riskWarningText(html);
		expect(warning).toContain('Downloads');
		expect(warning).toContain("reachable at this site's domain");
		expect(warning).toContain('.php');
	});

	// Regression: the first six tests in this block used only mode-INDEPENDENT
	// substrings, so a `docrootMode` ternary silently wired backwards (create's
	// fix text shown in edit mode, or vice versa) would not have failed any of
	// them. Edit mode is the one case fully SSR-testable at all (create mode's
	// fix text is proven in DocrootRiskWarning.svelte.test.ts, see the header
	// above) — this is what actually pins SiteDrawer wires the CORRECT mode
	// through, not just A mode. Vacuity-proved by temporarily inverting
	// `docrootMode`'s ternary in SiteDrawer.svelte and confirming this goes red.
	it('edit mode: the warning offers the subfolder fix, not the create-only checkbox wording', () => {
		const warning = riskWarningText(drawerHtml(siteAt('/Users/tom/Downloads')));
		expect(warning).toContain('subfolder');
		expect(warning).not.toContain('Create a site folder inside this folder');
	});

	it('edit mode: the warning is NOT gated behind the create-only checkbox block', () => {
		const html = drawerHtml(siteAt('/Users/tom/Downloads'));
		expect(html).toContain('data-testid="docroot-risk-warning"');
		expect(html).not.toContain('f-root-create');
	});

	it('edit mode: the warning id joins aria-describedby on the input', () => {
		const input = tagWith(drawerHtml(siteAt('/Users/tom/Downloads')), 'id="f-root"');
		expect(input).toContain('aria-describedby="f-root-risk"');
	});

	it('edit mode: a normal docroot renders no warning', () => {
		const html = drawerHtml(siteAt('/srv/www/legacy'));
		expect(html).not.toContain('data-testid="docroot-risk-warning"');
		expect(tagWith(html, 'id="f-root"')).not.toContain('f-root-risk');
	});

	// Deliberately the ONLY assertion in this test (split out from a combined
	// "renders AND stays enabled" test per review): the render-gate vacuity
	// probe used elsewhere in this block makes the FIRST expect in a combined
	// test fail before the enablement expect ever runs, so it never actually
	// proved this claim. Vacuity-proved on its own by temporarily wiring
	// `disabled={submitting || phpUnavailable || !!docrootRiskValue}` on the
	// Save button in SiteDrawer.svelte and confirming ONLY this test goes red.
	it('edit mode: Save stays enabled next to the warning — warn, never block', () => {
		const html = drawerHtml(siteAt('/Users/tom/Downloads'));
		expect(tagWith(html, 'data-testid="drawer-save"')).not.toContain('disabled');
	});

	it('create mode: the default blank docroot (a normal path) renders no warning', () => {
		expect(drawerHtml(null)).not.toContain('data-testid="docroot-risk-warning"');
	});
});
