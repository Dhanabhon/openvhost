// SPDX-License-Identifier: GPL-3.0-or-later
//
// The rendering half of the takeover: that each state gets its OWN screen, that
// the real path and the verbatim OS error reach it, and — the part nothing else
// in the gate can catch — that the window it draws is still movable.
//
// That last one is why this file also re-states the drag contract that
// `titlebar.drag.test.ts` pins for the ordinary shell. With `titleBarStyle:
// "Overlay"` + `hiddenTitle` macOS draws no draggable strip of its own, so a
// takeover built from a hand-rolled <div> would leave the user with a window
// they can neither move nor position to reach the close button — strictly worse
// than the broken window this screen replaces, and invisible to every other
// test here (this repo cannot drive the real app: see the
// `sandbox-cannot-verify-gui` note).
//
// Rendered through `svelte/server`, which needs no DOM, so it runs in the
// existing `node` vitest project — the pattern `StoreUnavailableBanner.svelte.
// test.ts` established. The one interaction (Quit) is proven at the layout
// seam, where the command it reaches actually exists.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import BootTakeover from './BootTakeover.svelte';
import type { DegradedBoot } from '$lib/boot-status.svelte';

const HOME = '/Users/tom/.openvhost';
const RUN_DIR = '/Users/tom/.openvhost/run';
const ERRNO_13 = 'Permission denied (os error 13)';

const ALREADY: DegradedBoot = { kind: 'alreadyRunning', home: HOME };
const UNUSABLE: DegradedBoot = { kind: 'runDirUnusable', path: RUN_DIR, reason: ERRNO_13 };
const NO_HOME: DegradedBoot = { kind: 'homeUnresolvable', reason: 'home directory unavailable' };

function html(boot: DegradedBoot, extra: Record<string, unknown> = {}): string {
	return render(BootTakeover, {
		props: { boot, onQuit: () => {}, onReveal: () => {}, ...extra }
	}).body;
}

/** Everything the user can actually read, tags stripped and whitespace
 *  collapsed — so a copy assertion cannot pass on markup the reader never sees,
 *  and cannot fail on how Svelte happened to wrap a line. */
function visible(body: string): string {
	return body
		.replace(/<[^>]*>/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('each state gets its own screen', () => {
	// Vacuity, measured: making one state's arm in `boot-takeover.derive.ts`
	// return another's copy — exactly what a wildcard branch produces — reddened
	// that state's case here and left the other two green, in all three
	// directions. Deleting the `{#each copy.details}` block below reddened every
	// test in `the facts the user has to act on` and left every title green.

	it('shows the already-running screen, and only that one', () => {
		const body = html(ALREADY);
		expect(body).toContain('data-testid="boot-already-running"');
		expect(body).not.toContain('data-testid="boot-run-dir-unusable"');
		expect(body).not.toContain('data-testid="boot-home-unresolvable"');
		expect(visible(body)).toContain('OpenVHost is already running');
	});

	it('shows the unusable-run-directory screen, and only that one', () => {
		const body = html(UNUSABLE);
		expect(body).toContain('data-testid="boot-run-dir-unusable"');
		expect(body).not.toContain('data-testid="boot-already-running"');
		expect(visible(body)).toContain('OpenVHost cannot use its working folder');
	});

	it('shows the unresolvable-home screen, and only that one', () => {
		const body = html(NO_HOME);
		expect(body).toContain('data-testid="boot-home-unresolvable"');
		expect(body).not.toContain('data-testid="boot-run-dir-unusable"');
		expect(visible(body)).toContain('OpenVHost cannot work out where to keep its files');
	});

	it('never renders the sentence this whole slice exists to delete', () => {
		for (const boot of [ALREADY, UNUSABLE, NO_HOME]) {
			expect(html(boot)).not.toContain('.manage()');
		}
	});
});

describe('the facts the user has to act on', () => {
	it('carries the contended home verbatim', () => {
		expect(visible(html(ALREADY))).toContain(HOME);
	});

	it('carries the run directory AND the OS error verbatim', () => {
		const body = visible(html(UNUSABLE));
		expect(body).toContain(RUN_DIR);
		expect(body).toContain(ERRNO_13);
	});

	it('passes a different path and errno through unchanged, so nothing is canned', () => {
		// The control the store slice established: `os error 13` present, `os error
		// 14` absent. A screen printing one fixed string would pass the two
		// assertions above and fail here.
		const body = visible(
			html({
				kind: 'runDirUnusable',
				path: '/Volumes/Data/openvhost/run',
				reason: 'No such file or directory (os error 2)'
			})
		);
		expect(body).toContain('/Volumes/Data/openvhost/run');
		expect(body).toContain('No such file or directory (os error 2)');
		expect(body).not.toContain('os error 13');
		expect(body).not.toContain(RUN_DIR);
	});
});

describe('the window it draws', () => {
	// The half that makes this a window rather than a page, and the half a
	// hand-rolled <div> silently loses.

	it('reserves the traffic-light strip by reusing the real titlebar', () => {
		// `TitleBar` is what carries `padding-left: env(titlebar-area-x, 72px)`, so
		// its presence is what keeps the card clear of the macOS traffic lights.
		// Asserted through the class it renders rather than by importing the
		// component twice.
		// Matched with a boundary rather than as a whole attribute: svelte appends
		// its scoping class, so the emitted value is `titlebar svelte-<hash>`.
		expect(html(ALREADY)).toMatch(/class="titlebar[\s"]/);
		expect(visible(html(ALREADY))).toContain('OpenVHost');
	});

	it('is draggable, so a degraded window is not a stuck window', () => {
		// `"deep"`, not the bare attribute (tauri 2.11.5 `drag.js:64` vs `:66`):
		// the bare form only drags when the click target IS that exact element,
		// and `.titlebar-name` covers the strip. Same contract as
		// `titlebar.drag.test.ts`, restated because this screen bypasses AppShell
		// entirely.
		//
		// Vacuity, measured: swapping `<TitleBar />` for a hand-rolled
		// `<div class="titlebar">` — the mistake D6 exists to prevent — reddened
		// this and the layout's `keeps the window movable on the takeover`, and
		// nothing else in the suite.
		expect(html(ALREADY)).toContain('data-tauri-drag-region="deep"');
	});

	it('claims no running count, because this window supervises nothing', () => {
		// `0 running` would be a plausible lie on the already-running screen: the
		// other instance IS serving the user's sites. Matched as "<digits> running"
		// and as the pill itself, NOT as the bare word — this screen's own title
		// contains "already running", and an assertion that could be satisfied by
		// deleting the title would be testing the wrong thing.
		//
		// Vacuity, measured: passing `runningCount={0}` here instead of `null` —
		// the obvious way to reuse a titlebar that used to require a number —
		// reddened this test alone, and left every `titlebarCount` assertion in
		// `routes.test.ts` green. That second half is the point: the ordinary
		// routes must keep their pill.
		expect(html(ALREADY)).not.toMatch(/\d+ running/);
		expect(html(ALREADY)).not.toContain('pill-running');
	});

	it('offers Quit, the one thing a user can do from here', () => {
		expect(html(ALREADY)).toContain('data-testid="boot-quit"');
		expect(visible(html(ALREADY))).toContain('Quit OpenVHost');
	});

	it('says so when a quit fails, rather than looking like a dead button', () => {
		const body = html(ALREADY, { quitError: 'the quit did not complete' });
		expect(body).toContain('data-testid="boot-quit-error"');
		expect(visible(body)).toContain('the quit did not complete');
	});

	it('shows no quit error when nothing has failed', () => {
		expect(html(ALREADY)).not.toContain('data-testid="boot-quit-error"');
	});

	it('disables Quit while one is already in flight', () => {
		expect(html(ALREADY, { quitting: true })).toContain('disabled');
		expect(visible(html(ALREADY, { quitting: true }))).toContain('Stopping…');
	});
});

describe('Reveal in Finder', () => {
	// The action D3 asks for, on the one screen that named a folder. Which screens
	// get it is `boot-takeover.derive.ts`'s call — asserted there — so what this
	// group owes is that the component honours that call and that a failed reveal
	// is visible.
	//
	// Vacuity, measured over the whole desktop suite:
	//
	//   * `{#if copy.revealsRunDir}` replaced with `{#if true}` — the component
	//     overriding the derive's call, which is how every screen would end up with
	//     a button pointed at a folder only one of them named: 2 red, `offers
	//     Reveal in Finder on no other screen` here and `is absent on the screens
	//     that named no folder` at the layout seam.
	//   * the `{#if revealError !== ''}` block deleted — a reveal that fails and
	//     says nothing, which on an error screen is the worst outcome available: 3
	//     red, `says so when the reveal fails …` and `keeps the two action failures
	//     apart …` here, and `says so when the reveal fails …` at the layout seam.
	//
	// `offers Reveal in Finder beside Quit …` survived both, and is instead the
	// test that reddens when the derive's `runDirUnusable` arm is flipped to
	// `false` (recorded in `boot-takeover.derive.test.ts`).

	it('offers Reveal in Finder beside Quit on the unusable-run-directory screen', () => {
		const body = html(UNUSABLE);
		// The premise first: without it, a screen that rendered no actions at all
		// would satisfy an "and Quit is still there" assertion by accident.
		expect(body).toContain('data-testid="boot-reveal"');
		expect(body).toContain('data-testid="boot-quit"');
		expect(visible(body)).toContain('Reveal in Finder');
	});

	it('offers Reveal in Finder on no other screen', () => {
		// `alreadyRunning` names a folder that is working fine for the other copy,
		// and `homeUnresolvable` resolved no path at all — a button on either could
		// only open some other state's directory. Quit asserted alongside, so this
		// cannot pass on a screen that lost its action row entirely.
		for (const boot of [ALREADY, NO_HOME]) {
			expect(html(boot)).toContain('data-testid="boot-quit"');
			expect(html(boot)).not.toContain('data-testid="boot-reveal"');
		}
	});

	it('keeps the folder readable as text, so the button is never the only way through', () => {
		// `reveal_item_in_dir` canonicalises, and the commonest `runDirUnusable` is
		// a run directory that could not be CREATED — so the button really can fail
		// and this text is what still works.
		expect(html(UNUSABLE)).toContain('data-testid="boot-run-dir"');
		expect(visible(html(UNUSABLE))).toContain(RUN_DIR);
	});

	it('says so when the reveal fails, rather than looking like a dead button', () => {
		// The worst available outcome on this screen: a user already stuck on an
		// error presses a button and nothing whatsoever happens.
		const body = html(UNUSABLE, { revealError: 'No such file or directory (os error 2)' });
		expect(body).toContain('data-testid="boot-reveal-error"');
		expect(visible(body)).toContain('No such file or directory (os error 2)');
	});

	it('shows no reveal error when nothing has failed', () => {
		expect(html(UNUSABLE)).not.toContain('data-testid="boot-reveal-error"');
	});

	it('keeps the two action failures apart, so the message names the right one', () => {
		// One shared slot would leave "the quit did not complete" on screen under a
		// Reveal button, or the reverse.
		const body = html(UNUSABLE, { quitError: 'quit failed', revealError: 'reveal failed' });
		expect(body).toContain('data-testid="boot-quit-error"');
		expect(body).toContain('data-testid="boot-reveal-error"');
		expect(visible(body)).toContain('quit failed');
		expect(visible(body)).toContain('reveal failed');
	});
});

describe('its live-region treatment', () => {
	it('is not an assertive live region, because it IS the page', () => {
		// Unlike `StoreUnavailableBanner`, which arrives over a running app and so
		// takes role="alert". Nothing renders before this screen, so there is no
		// "before" for it to interrupt — and `aria-live` on a whole page is a
		// well-known way to make a screen reader read it twice.
		expect(html(UNUSABLE)).not.toContain('role="alert"');
	});

	it('leads with a real page heading a screen reader can land on', () => {
		expect(html(UNUSABLE)).toMatch(/<h1[^>]*>/);
		expect(html(UNUSABLE)).toContain('<main');
	});
});
