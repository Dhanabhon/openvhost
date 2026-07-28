// SPDX-License-Identifier: GPL-3.0-or-later
//
// Regression guard for the app shell's one scrolling region.
//
// `.content` is a column flex container. Everything a route renders lands in it as
// a flex item, and a flex item shrinks by default. The automatic minimum size that
// normally stops an item shrinking below its own content does NOT apply to an item
// whose `overflow` is not `visible` — and every panel in this app sets
// `overflow: hidden` so its rounded corners clip the rows inside it. So the panels
// were shrinkable to zero, and a page taller than the window got its cards clipped
// mid-line instead of scrolling. It was found by looking at the running app: the
// nginx card's "Version 1.31.3" was sliced in half by the card's own bottom edge.
//
// This repo cannot drive the real app to look at it (see the
// `sandbox-cannot-verify-gui` note) and the SSR test harness has no layout engine,
// so the two properties that make the clipping impossible are asserted against the
// stylesheet itself — the same approach as focus-ring.test.ts.

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const shell = readFileSync(new URL('./AppShell.svelte', import.meta.url), 'utf8');

/** The body of a rule in AppShell's `<style>` block, without its braces. */
function block(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const m = shell.match(new RegExp(`(?:^|\\n)\\t*${escaped}\\s*\\{([^}]*)\\}`));
	if (!m) throw new Error(`no \`${selector}\` rule found in AppShell.svelte`);
	return m[1];
}

describe('the shell content region', () => {
	it('scrolls its overflow rather than letting the window grow', () => {
		// The premise the rest of this file depends on. If `.content` stops being the
		// scrolling region, the clipping question moves somewhere else entirely.
		expect(block('.content')).toMatch(/overflow:\s*auto/);
	});

	it('stops its children shrinking, so a tall page scrolls instead of being clipped', () => {
		// This is the whole fix. Without it, a panel with `overflow: hidden` has no
		// automatic minimum size and the browser shrinks it — clipping the card's
		// content at the card's edge while the page still reports itself as fine.
		expect(block('.content > :global(*)')).toMatch(/flex-shrink:\s*0/);
	});
});

describe('the panels this protects', () => {
	// Named individually rather than globbed: each of these is a flex child of
	// `.content` that opts out of its own automatic minimum size, which is exactly
	// the combination that caused the bug. A new panel added without `overflow`
	// simply is not vulnerable, so it does not need to be listed here — but if one
	// of THESE ever stops clipping its corners, the note above should be revisited
	// rather than the test quietly deleted.
	const panels = [
		'SitesPanel.svelte',
		'ServicesPanel.svelte',
		'WebServerPanel.svelte',
		'WebServerSettingsForm.svelte'
	];

	it.each(panels)('%s still clips its corners, so it still needs the guard', (file) => {
		const css = readFileSync(new URL(`./${file}`, import.meta.url), 'utf8');
		expect(css).toMatch(/overflow:\s*hidden/);
	});
});
