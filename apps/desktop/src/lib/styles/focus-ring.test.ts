// SPDX-License-Identifier: GPL-3.0-or-later
//
// Regression guard for the global `:focus-visible` ring.
//
// This one rule applies to every focusable control in the app, so a mistake in it
// is invisible in any single component's tests and shows up only as "the buttons
// look wrong" — which is how it was found (the quit dialog's Cancel button, whose
// focus ring sat directly against its own 1px border and read as a doubled frame).
//
// This repo cannot drive the real app to look at it (see the
// `sandbox-cannot-verify-gui` note), so the properties are asserted against the
// stylesheet itself.

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8');

/** The body of the global `:focus-visible` rule, without its braces. */
function focusVisibleBlock(): string {
	const m = css.match(/(?:^|\n):focus-visible\s*\{([^}]*)\}/);
	if (!m) throw new Error('no global :focus-visible rule found in tokens.css');
	return m[1];
}

describe(':focus-visible', () => {
	it('draws a ring, so keyboard users can see where they are', () => {
		// Deleting this is the one change that would make the app unusable by
		// keyboard while looking tidier in a screenshot.
		expect(focusVisibleBlock()).toMatch(/outline:\s*\d+px\s+solid/);
	});

	it('keeps the ring clear of a control that has its own 1px border', () => {
		// `.btn-quiet` and the inputs carry `border: 1px`. At `outline-offset: 1px`
		// the ring landed against that border and the pair read as two frames.
		const offset = focusVisibleBlock().match(/outline-offset:\s*(\d+)px/);
		expect(offset, 'outline-offset must be set explicitly').not.toBeNull();
		expect(Number(offset?.[1])).toBeGreaterThanOrEqual(2);
	});

	it('does not change the shape of whatever takes focus', () => {
		// An outline already follows the element's own border-radius. Setting a
		// radius in this rule instead mutates the ELEMENT — a pill or a card would
		// snap to the control radius the moment it was focused.
		expect(focusVisibleBlock()).not.toMatch(/border-radius/);
	});
});

describe('a focused quiet button', () => {
	// `.btn-quiet` is the only variant with a visible border of its own, so the
	// global ring's 2px offset stacked THREE concentric edges on it — border,
	// gap, ring — and read as a doubled frame. Reported twice on the quit
	// dialog's Cancel button. An earlier attempt widened the gap, which
	// separated the two frames rather than removing one.
	const button = readFileSync(new URL('../components/Button.svelte', import.meta.url), 'utf8');

	/** The body of `.btn-quiet:focus-visible`, without its braces. */
	function quietFocusBlock(): string {
		const m = button.match(/\.btn-quiet:focus-visible\s*\{([^}]*)\}/);
		if (!m) throw new Error('no `.btn-quiet:focus-visible` rule found in Button.svelte');
		return m[1];
	}

	it('closes the gap, so the ring and the border form one band', () => {
		// Any non-zero offset here puts the page background back between the
		// button's own border and the ring — which is the doubled frame.
		expect(quietFocusBlock()).toMatch(/outline-offset:\s*0/);
	});

	it("recolours its own border to the ring's colour", () => {
		// With the gap closed, a grey border against a green ring would still read
		// as two bands. Matching the colour is what makes them one.
		expect(quietFocusBlock()).toMatch(/border-color:\s*var\(--vh-focus-ring\)/);
	});

	it('leaves the primary button on the global ring', () => {
		// A green ring flush against a green fill would lose the contrast the 2px
		// gap gives it against the page. Primary has no border to double up with,
		// so it has nothing to fix.
		expect(button).not.toMatch(/\.btn-primary:focus-visible/);
	});
});

describe('focused, invalid form controls', () => {
	// The four controls with a visible border of their own: `.input` in
	// `WebServerSettingsForm.svelte`, `SiteDrawer.svelte` and
	// `MysqlCredentials.svelte` (its masked root-password field), and
	// `.trigger` in `Select.svelte` (its `role="combobox"` stand-in for a
	// native `<select>`). Same doubled-frame bug as `.btn-quiet`, fixed the
	// same way — plus a hard requirement `.btn-quiet` never had: an INVALID
	// control must read red even while focused, never the focus ring's
	// colour.
	//
	// Each file's own rules live beside the selector they modify (a global
	// `.input:focus-visible` in tokens.css would tie with the component's own
	// scoped `.input` rule at identical specificity and lose to load order —
	// see this task's report), which is exactly the setup that let the first
	// three copies drift from each other unnoticed. Comparing their rule
	// BODIES against each other, not just against a fixed expectation, is
	// what makes that drift a test failure instead of a future bug report.
	//
	// `MysqlCredentials` has no invalid state of its own (the password field
	// is never user-typed, so nothing there can be "invalid") — it is
	// included in the ordinary-focus comparison below but sits out the
	// invalid-focus one, same as any future bordered control that never
	// carries `aria-invalid` would.
	const files: Record<string, string> = {
		WebServerSettingsForm: readFileSync(
			new URL('../components/WebServerSettingsForm.svelte', import.meta.url),
			'utf8'
		),
		SiteDrawer: readFileSync(new URL('../components/SiteDrawer.svelte', import.meta.url), 'utf8'),
		Select: readFileSync(new URL('../components/Select.svelte', import.meta.url), 'utf8'),
		MysqlCredentials: readFileSync(
			new URL('../components/MysqlCredentials.svelte', import.meta.url),
			'utf8'
		)
	};

	/** Selector each file uses for its bordered control: `.input` for the
	 * three form-shaped ones, `.trigger` for `Select`'s combobox button. */
	const selectors: Record<string, string> = {
		WebServerSettingsForm: '.input',
		SiteDrawer: '.input',
		Select: '.trigger',
		MysqlCredentials: '.input'
	};

	/** The body (no braces) of `<selector>:focus-visible`, escaping the selector's
	 * leading dot for the regex. */
	function focusBlock(css: string, selector: string): string {
		const escaped = selector.replace('.', '\\.');
		const re = new RegExp(`${escaped}:focus-visible\\s*\\{([^}]*)\\}`);
		const m = css.match(re);
		if (!m) throw new Error(`no \`${selector}:focus-visible\` rule found`);
		return m[1];
	}

	/** The body of `<selector>[aria-invalid='true']:focus-visible` — the rule
	 * that must win over the plain one above when both apply, per "red wins". */
	function invalidFocusBlock(css: string, selector: string): string {
		const escaped = selector.replace('.', '\\.');
		const re = new RegExp(`${escaped}\\[aria-invalid='true'\\]:focus-visible\\s*\\{([^}]*)\\}`);
		const m = css.match(re);
		if (!m) throw new Error(`no \`${selector}[aria-invalid='true']:focus-visible\` rule found`);
		return m[1];
	}

	/** Normalises whitespace so the comparison is about declarations, not
	 * incidental formatting. */
	function normalise(block: string): string {
		return block
			.split(';')
			.map((decl) => decl.trim())
			.filter((decl) => decl !== '')
			.sort()
			.join(';');
	}

	const names = Object.keys(files);
	// `MysqlCredentials` sits out the invalid-focus checks: its password field
	// is never user-typed (read-only, generated), so it never carries
	// `aria-invalid` and has no `[aria-invalid='true']:focus-visible` rule to
	// compare — the same way a future bordered control with no invalid state
	// would.
	const invalidNames = names.filter((name) => name !== 'MysqlCredentials');

	it('closes the gap and recolours the border on every one of the four controls', () => {
		for (const name of names) {
			const block = focusBlock(files[name], selectors[name]);
			expect(block, name).toMatch(/outline-offset:\s*0/);
			expect(block, name).toMatch(/border-color:\s*var\(--vh-focus-ring\)/);
		}
	});

	it('turns the whole band red — border AND outline — when an invalid control is focused', () => {
		for (const name of invalidNames) {
			const block = invalidFocusBlock(files[name], selectors[name]);
			expect(block, name).toMatch(/border-color:\s*var\(--vh-fail\)/);
			expect(block, name).toMatch(/outline-color:\s*var\(--vh-fail\)/);
			// Red wins: the ring's colour token must not appear here at all, or the
			// two could be blended/overridden by a later rule reintroducing it.
			expect(block, name).not.toMatch(/--vh-focus-ring/);
		}
	});

	it('keeps the ordinary focus rule identical across all four controls', () => {
		const [first, ...rest] = names.map((name) =>
			normalise(focusBlock(files[name], selectors[name]))
		);
		for (const block of rest) expect(block).toBe(first);
	});

	it('keeps the invalid-focus rule identical across the controls that have one', () => {
		const [first, ...rest] = invalidNames.map((name) =>
			normalise(invalidFocusBlock(files[name], selectors[name]))
		);
		for (const block of rest) expect(block).toBe(first);
	});

	it('gives SiteDrawer the same base invalid marker WebServerSettingsForm has', () => {
		// The gap this task closes: SiteDrawer set `aria-invalid` on its fields
		// but had no CSS rule for it at all, so an invalid-but-idle field there
		// was indistinguishable from a valid one except for the message
		// underneath — worse than either "always grey" or "always red" once the
		// focus rule could turn it red.
		const re = /\.input\[aria-invalid='true'\]\s*\{([^}]*)\}/;
		const m = files.SiteDrawer.match(re);
		expect(m, 'SiteDrawer must have a base `.input[aria-invalid="true"]` rule').not.toBeNull();
		expect(m?.[1]).toMatch(/border-color:\s*var\(--vh-fail\)/);
	});

	it('gives Select the same base invalid marker the two forms have', () => {
		const re = /\.trigger\[aria-invalid='true'\]\s*\{([^}]*)\}/;
		const m = files.Select.match(re);
		expect(m, 'Select must have a base `.trigger[aria-invalid="true"]` rule').not.toBeNull();
		expect(m?.[1]).toMatch(/border-color:\s*var\(--vh-fail\)/);
	});
});
