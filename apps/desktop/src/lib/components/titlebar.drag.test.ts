// SPDX-License-Identifier: GPL-3.0-or-later
//
// Regression guard for "the app window cannot be dragged".
//
// With `titleBarStyle: "Overlay"` + `hiddenTitle: true` (tauri.conf.json) macOS draws ONLY the
// traffic lights — there is no native draggable titlebar strip. 100% of window dragging comes
// from the webview, via Tauri's injected drag script. So a mistake in this one component makes
// the whole window immovable, and nothing else in the gate suite can catch it (this repo cannot
// drive the real app: see the `sandbox-cannot-verify-gui` note).
//
// The contract we must satisfy is `isDragRegion()` in tauri 2.11.5
// (`src/window/scripts/drag.js`), which walks the composed path from the click target upward:
//
//   line 58  clickable element (A/BUTTON/INPUT/SELECT/TEXTAREA/LABEL/SUMMARY, [contenteditable],
//            [tabindex] != "-1", or an interactive [role]) with NO drag attr  -> return false
//   line 62  attr === "false"                                                -> return false
//   line 64  attr === "deep"                                                 -> return true
//   line 66  attr === "" (bare) or "true"  -> return el === composedPath[0], i.e. DRAG ONLY WHEN
//            THE CLICK TARGET IS *THAT EXACT ELEMENT*
//
// The original bug: `.titlebar` carried the BARE attribute while its `.titlebar-name` child has
// `flex: 1` and therefore spans nearly the entire strip. Every click landed on a child, line 66
// compared `.titlebar !== .titlebar-name`, and returned false — leaving only the flex `gap` and
// the right padding draggable.
//
// These tests render the real component (SSR — no DOM needed) and assert the two properties that
// keep line 58 and line 64 satisfied.

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import TitleBar from './TitleBar.svelte';

/** Tag names tauri's `isClickableElement` treats as click targets that BLOCK dragging. */
const CLICKABLE_TAGS = ['a', 'button', 'input', 'select', 'textarea', 'label', 'summary'];

/** Interactive ARIA roles tauri's `isClickableElement` treats the same way. */
const INTERACTIVE_ROLES = [
	'button',
	'link',
	'menuitem',
	'tab',
	'checkbox',
	'radio',
	'switch',
	'option'
];

function titlebarHtml(): string {
	return render(TitleBar, { props: { runningCount: 3 } }).body;
}

/** Every opening tag in `html`, as `{ tag, attrs }` — adequate for this tiny, fully-owned markup. */
function openingTags(html: string): { tag: string; attrs: string }[] {
	return [...html.matchAll(/<([a-zA-Z][a-zA-Z0-9-]*)((?:"[^"]*"|'[^']*'|[^>"'])*)>/g)].map((m) => ({
		tag: m[1].toLowerCase(),
		attrs: m[2]
	}));
}

describe('TitleBar drag region', () => {
	it('marks the titlebar as a deep drag region so clicks on its children still drag the window', () => {
		// "deep" (drag.js:64) is required, NOT the bare attribute (drag.js:66): `.titlebar-name`
		// has `flex: 1` and covers the strip, so the click target is essentially never `.titlebar`.
		expect(titlebarHtml()).toContain('data-tauri-drag-region="deep"');
	});

	it('has no clickable descendant that would silently block dragging', () => {
		// drag.js:58 — a clickable element WITHOUT its own drag attribute short-circuits the walk
		// to `false` before it ever reaches the "deep" ancestor. So adding e.g. a settings
		// <button> straight into the titlebar would carve a dead zone out of the drag region.
		// That is legitimate when intended (a button should act as a button) — but it must be a
		// deliberate choice, so this test fails and makes whoever adds it decide explicitly:
		// either give that element its own `data-tauri-drag-region`, or accept the dead zone and
		// update this test.
		const offenders = openingTags(titlebarHtml())
			.filter(({ attrs }) => !/\bdata-tauri-drag-region\b/.test(attrs))
			.filter(
				({ tag, attrs }) =>
					CLICKABLE_TAGS.includes(tag) ||
					/\bcontenteditable(?!\s*=\s*["']false["'])/.test(attrs) ||
					/\btabindex\s*=\s*["'](?!-1["'])/.test(attrs) ||
					INTERACTIVE_ROLES.some((role) =>
						new RegExp(`\\brole\\s*=\\s*["']${role}["']`).test(attrs)
					)
			)
			.map(({ tag, attrs }) => `<${tag}${attrs}>`);

		expect(offenders).toEqual([]);
	});

	it('grants the start_dragging capability the drag region depends on', () => {
		// The SECOND half of the contract, and the half that is invisible in the markup.
		//
		// Satisfying isDragRegion() only gets as far as drag.js:104:
		//     window.__TAURI_INTERNALS__.invoke('plugin:window|' + cmd)
		// which is fired WITHOUT await and WITHOUT .catch(). So if the capability does not permit
		// `start_dragging`, the ACL rejects it and the rejection is swallowed: correct attribute,
		// no drag, no error anywhere. Indistinguishable from the markup bug above.
		//
		// `core:default` is NOT enough. It pulls in `core:window:default`, which is a read-only
		// GETTER set (allow-inner-size, allow-is-focused, allow-title, …). It does include
		// `allow-internal-toggle-maximize` — which is why double-click-to-zoom works off
		// `core:default` alone while dragging does not — but it does NOT include
		// `allow-start-dragging`. That has to be granted explicitly.
		//
		// This asserts against the COMMITTED capability file. It cannot resolve the real ACL
		// (src-tauri/gen/ is generated and gitignored, so it does not exist in a clean checkout),
		// hence the hardcoded knowledge above about what core:window:default contains.
		const cap = JSON.parse(
			readFileSync(new URL('../../../src-tauri/capabilities/default.json', import.meta.url), 'utf8')
		);
		expect(cap.permissions).toContain('core:window:allow-start-dragging');
	});
});
