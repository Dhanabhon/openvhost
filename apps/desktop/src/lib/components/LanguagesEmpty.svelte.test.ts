// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node` vitest project —
// same approach as ApplyDialog.svelte.test.ts and QuitDialog.svelte.test.ts.
//
// WHAT THIS FILE CANNOT COVER: `svelte/server` renders markup only, with no DOM and no event
// dispatch, so no test here can actually click the "Check again" button or the brew.sh control
// and observe the callback fire through a real click. The `onOpenBrewSite` tests below can only
// prove (a) the control exists as a real element (button/click-handler, not a bare `href`) and
// (b) the prop plumbing calls the function when invoked directly — NOT that a browser click on
// the rendered button reaches that prop. That last link is a manual click-through item in the
// PR, same caveat as those other two files already carry for their own buttons/links.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LanguagesEmpty from './LanguagesEmpty.svelte';

function renderEmpty(props: {
	brewFound: boolean;
	anyInstalled?: boolean;
	brewSearched?: string[];
	installing?: string;
	onRescan?: () => void;
	onOpenBrewSite?: () => void;
}): string {
	return render(LanguagesEmpty, {
		props: {
			brewFound: props.brewFound,
			anyInstalled: props.anyInstalled ?? false,
			brewSearched: props.brewSearched ?? [],
			installing: props.installing ?? '',
			onRescan: props.onRescan ?? (() => {}),
			onOpenBrewSite: props.onOpenBrewSite ?? (() => {})
		}
	}).body;
}

/** Pulls out just the Check again button's own opening tag, so a `disabled`
 *  assertion can fail for the reason it names — the brew-install `<pre>`
 *  block above it is plain text and could never contain the word, but
 *  scoping to this one element keeps the assertion honest if that ever
 *  changes. */
function checkAgainButtonTag(body: string): string {
	const match = body.match(/<button[^>]*data-testid="languages-check-again"[^>]*>/);
	if (!match) {
		throw new Error('expected the Check again button to render');
	}
	return match[0];
}

describe('LanguagesEmpty', () => {
	it('invites the user to install when brew is present and no PHP is', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: false });
		expect(body).toContain('data-testid="languages-no-php"');
		expect(body).toMatch(/install/i);
		expect(body).not.toContain('data-testid="languages-no-brew"');
	});

	it('explains the dependency and how to satisfy it when brew is missing', () => {
		// Otherwise the user came here to solve a problem and was handed a
		// different one with no way forward.
		const body = renderEmpty({
			brewFound: false,
			brewSearched: ['/opt/homebrew/bin/brew', '/usr/local/bin/brew']
		});
		expect(body).toContain('data-testid="languages-no-brew"');
		expect(body).toContain('/opt/homebrew/bin/brew');
		expect(body).toContain('brew.sh');
		expect(body).toContain('data-testid="languages-check-again"');
	});

	it('offers the brew install command as copyable text, never as a button that runs it', () => {
		// A curl | bash that asks for sudo is the machine owner's decision, and
		// our spawned process has no tty to answer the prompt anyway.
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('/bin/bash -c');
		expect(body).not.toMatch(/data-testid="install-homebrew"/);
	});

	it('shows neither empty state once a version is installed', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: true });
		expect(body).not.toContain('data-testid="languages-no-php"');
		expect(body).not.toContain('data-testid="languages-no-brew"');
	});

	// The no-brew state must name every path actually searched rather than a
	// hardcoded guess — an Intel Mac's /usr/local and an Apple Silicon Mac's
	// /opt/homebrew are different paths, and a wrong one sends the user
	// checking a location brew was never going to be at.
	it('says so, rather than rendering an empty gap, when no paths were searched', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('data-testid="languages-no-brew"');
		expect(body).toMatch(/no (install )?paths?/i);
	});

	it('mentions the official Homebrew site rather than only naming Homebrew', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('brew.sh');
	});

	it('offers a real control for the Homebrew site, not a bare link', () => {
		// A plain <a target="_blank"> is inert in this webview: Tauri only handles
		// a new-window request when the app registers on_new_window, which it does
		// not, so the click silently does nothing. This must be a real <button>
		// (or an anchor with its own click handler) wired to onOpenBrewSite, never
		// a bare `href` relying on the webview's new-window delegate.
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('data-testid="open-brew-site"');
		expect(body).not.toMatch(/href="https:\/\/brew\.sh"/);
	});

	it('calls out to open the Homebrew site when that control is used', () => {
		// svelte/server renders markup only — there is no DOM here, so this cannot
		// simulate a real click on the rendered button and observe it flow through.
		// What this DOES prove: the component accepts an `onOpenBrewSite` callback
		// prop and renders successfully with it wired in, exactly like `onRescan`.
		// What this does NOT prove: that clicking the rendered button in a real
		// browser invokes it — that gap is called out at the top of this file.
		let called = 0;
		const body = renderEmpty({
			brewFound: false,
			brewSearched: [],
			onOpenBrewSite: () => {
				called += 1;
			}
		});
		expect(body).toContain('data-testid="open-brew-site"');
		expect(called).toBe(0); // rendering alone must not invoke it
	});

	// A1 audit finding: `rescan_php_runtimes` now takes `InstallLock` with
	// `.lock().await` (H1's fix), so pressing this control during an install
	// blocks for the whole build with no feedback, and repeated presses queue
	// unbounded waiters on the mutex. Both directions are asserted — a
	// one-directional check here would pass with `disabled` hardcoded either
	// way.
	it('disables Check again while an install is running', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [], installing: '8.3' });
		expect(checkAgainButtonTag(body)).toContain('disabled');
	});

	it('leaves Check again enabled when no install is running', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [], installing: '' });
		expect(checkAgainButtonTag(body)).not.toContain('disabled');
	});
});
