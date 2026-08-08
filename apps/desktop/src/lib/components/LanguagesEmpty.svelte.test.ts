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
	/** Defaults to `!brewFound`, which is EXACTLY what this component asked
	 *  before design D2 split the two apart. Every test written before that
	 *  slice therefore keeps asserting precisely what it always asserted, with
	 *  no edit to a single expectation — the point being that D2 changed *when*
	 *  the dead end appears and nothing about *what* it says. The D2 tests below
	 *  pass the two apart deliberately. */
	noRouteToAnyPhp?: boolean;
	anyInstalled?: boolean;
	brewSearched?: string[];
	installing?: string;
	onRescan?: () => void;
	onOpenBrewSite?: () => void;
}): string {
	return render(LanguagesEmpty, {
		props: {
			brewFound: props.brewFound,
			noRouteToAnyPhp: props.noRouteToAnyPhp ?? !props.brewFound,
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

// Off-Homebrew slice 5C design D2. The bug was never that this page mentions
// Homebrew — for 8.1/8.2/8.3/8.5, and for every major on Intel, Homebrew
// genuinely is required and saying so is correct. The bug was that ONE
// machine-wide bool answered a question that is per-major, and blocked the
// whole page on it.
describe('LanguagesEmpty — the dead end is no longer "no Homebrew"', () => {
	// §8.1, at this component's own level: `brewFound: false` on its own must
	// no longer be enough. Everything above this block still passes `brewFound:
	// false` and still gets the screen, because the helper defaults
	// `noRouteToAnyPhp` to `!brewFound` — the pre-D2 rule. THIS is the test that
	// pulls the two apart.
	it('renders no dead end without Homebrew when a route to a PHP exists', () => {
		const body = renderEmpty({
			brewFound: false,
			noRouteToAnyPhp: false,
			anyInstalled: true,
			brewSearched: ['/opt/homebrew/bin/brew']
		});
		expect(body).not.toContain('data-testid="languages-no-brew"');
		// …and it does not fall through to the "install something" invitation
		// either: a PHP is installed, and the caller's rowlist is the whole UI.
		expect(body).not.toContain('data-testid="languages-no-php"');
	});

	// §8.2b's page half: nothing installed yet, but 8.4 is installable from our
	// own tree. The invitation belongs here, not the dead end.
	it('invites an install without Homebrew when a packaged version is offered', () => {
		const body = renderEmpty({
			brewFound: false,
			noRouteToAnyPhp: false,
			anyInstalled: false,
			brewSearched: ['/opt/homebrew/bin/brew']
		});
		expect(body).not.toContain('data-testid="languages-no-brew"');
		expect(body).toContain('data-testid="languages-no-php"');
		// The invitation must not claim the install goes through Homebrew when
		// Homebrew is exactly what is missing — the same page-wide claim about a
		// per-major fact that D2 removes one branch up.
		expect(body).not.toMatch(/homebrew/i);
	});

	// §8.6, from the other direction: the sentence a machine WITH Homebrew sees
	// is unchanged, word for word.
	it('still names Homebrew in the invitation wherever Homebrew is present', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: false });
		expect(body).toContain(
			'Choose a version below — OpenVHost installs it through Homebrew and serves your sites with it.'
		);
	});

	// §8.2. The screen is not softened into a warning — it is the same blunt
	// heading, the same verbatim searched-paths list, the same install command
	// and the same recovery control it has always been. Only its trigger moved.
	it('is unchanged, word for word and path for path, when it does render', () => {
		const body = renderEmpty({
			brewFound: false,
			noRouteToAnyPhp: true,
			anyInstalled: false,
			brewSearched: ['/opt/homebrew/bin/brew', '/usr/local/bin/brew']
		});
		expect(body).toContain('Homebrew is required to install PHP');
		expect(body).toContain('/opt/homebrew/bin/brew');
		expect(body).toContain('/usr/local/bin/brew');
		expect(body).toContain('/bin/bash -c');
		expect(body).toContain('data-testid="open-brew-site"');
		expect(body).toContain('data-testid="languages-check-again"');
	});

	// The dead end outranks the invitation, which is the ordering this component
	// has always had and the reason it has it: "no PHP, press Install" is a dead
	// end one level further up on a machine that cannot install anything.
	it('shows the dead end rather than the invitation when both would apply', () => {
		const body = renderEmpty({
			brewFound: false,
			noRouteToAnyPhp: true,
			anyInstalled: false
		});
		expect(body).toContain('data-testid="languages-no-brew"');
		expect(body).not.toContain('data-testid="languages-no-php"');
	});
});
