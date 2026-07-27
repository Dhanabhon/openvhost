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
