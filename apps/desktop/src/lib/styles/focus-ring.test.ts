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
