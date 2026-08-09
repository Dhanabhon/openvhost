// SPDX-License-Identifier: GPL-3.0-or-later
//
// The words themselves. This slice exists because the app once told a user
// *"You must call `.manage()` before using this command"*, so each screen owes
// three things and they are asserted as three things: what is wrong, what the
// user can do about it, and — for `alreadyRunning` alone — that this is normal
// and not a failure.
//
// The verbatim half is asserted with a control, the technique the store slice
// used: a second fixture whose values are different, so copy that printed one
// canned path or one canned errno cannot pass.

import { describe, expect, it } from 'vitest';
import { takeoverCopy } from './boot-takeover.derive';
import type { DegradedBoot } from './boot-status.svelte';

const HOME = '/Users/tom/.openvhost';
const RUN_DIR = '/Users/tom/.openvhost/run';
const ERRNO_13 = 'Permission denied (os error 13)';

/** Every sentence a screen says, as one string, so a phrase assertion cannot
 *  fail on which line happened to carry it. */
function prose(boot: DegradedBoot): string {
	return takeoverCopy(boot).lines.join(' ');
}

/** The verbatim block, as `label -> value`. */
function facts(boot: DegradedBoot): Record<string, string> {
	return Object.fromEntries(takeoverCopy(boot).details.map((d) => [d.label, d.value]));
}

describe('the already-running screen', () => {
	// Vacuity, measured: returning `runDirUnusable`'s copy from the
	// `alreadyRunning` arm — the exact shape of "a new state silently inherits
	// another's screen" — reddened all four tests here, plus the shared `has its
	// own test hook` guard, and left both groups below green. The mirror
	// mutations reddened those instead.
	const boot: DegradedBoot = { kind: 'alreadyRunning', home: HOME };

	it('says what is wrong in the user’s terms, not Tauri’s', () => {
		expect(takeoverCopy(boot).title).toBe('OpenVHost is already running');
		expect(prose(boot)).not.toContain('.manage()');
	});

	it('names the contended folder verbatim', () => {
		expect(facts(boot)).toEqual({ 'Working folder': HOME });
	});

	it('passes a different folder through unchanged, so nothing is hardcoded', () => {
		// The control for the assertion above: copy printing one canned path would
		// satisfy it just as well.
		const other = facts({ kind: 'alreadyRunning', home: '/opt/openvhost-home' });
		expect(other['Working folder']).toBe('/opt/openvhost-home');
		expect(other['Working folder']).not.toContain('.openvhost');
	});

	it('says this is normal and not a failure, then what to do', () => {
		// The half that stops this reading as a crash report. Without it a user
		// goes looking for damage that is not there.
		expect(prose(boot)).toContain('Nothing has gone wrong');
		expect(prose(boot)).toContain('still serving your sites');
		expect(prose(boot)).toContain('Switch to it from the Dock or the menu bar');
	});
});

describe('the unusable-run-directory screen', () => {
	// Vacuity, measured: returning `alreadyRunning`'s copy from this arm reddened
	// all five tests here, plus the shared `has its own test hook` guard, and
	// left the other two screens' groups green.
	const boot: DegradedBoot = { kind: 'runDirUnusable', path: RUN_DIR, reason: ERRNO_13 };

	it('says what is wrong without naming a Rust API at the user', () => {
		expect(takeoverCopy(boot).title).toBe('OpenVHost cannot use its working folder');
		expect(prose(boot)).not.toContain('.manage()');
	});

	it('carries the folder AND the OS error, both verbatim', () => {
		// On this screen the path and the errno ARE the payload — everything else
		// is context for them.
		expect(facts(boot)).toEqual({ Folder: RUN_DIR, Error: ERRNO_13 });
	});

	it('passes a different path and errno through unchanged', () => {
		const other = facts({
			kind: 'runDirUnusable',
			path: '/Volumes/Data/openvhost/run',
			reason: 'Read-only file system (os error 30)'
		});
		expect(other).toEqual({
			Folder: '/Volumes/Data/openvhost/run',
			Error: 'Read-only file system (os error 30)'
		});
		expect(JSON.stringify(other)).not.toContain('os error 13');
	});

	it('points at the error as the actionable line, and says how to fix it', () => {
		expect(prose(boot)).toContain('The error above is the part to act on');
		expect(prose(boot)).toContain('not writable by your user account');
		expect(prose(boot)).toContain('open OpenVHost again');
	});

	it('does not tell the user nothing is wrong', () => {
		// The reassurance belongs to `alreadyRunning` and nowhere else: here
		// something IS broken and the user has to fix it.
		expect(prose(boot)).not.toContain('Nothing has gone wrong');
	});
});

describe('the unresolvable-home screen', () => {
	// Vacuity, measured: returning `runDirUnusable`'s copy from this arm reddened
	// all three tests here, plus the shared `has its own test hook` guard, and
	// left the other two screens' groups green.
	//
	// Design D3 gives this state no bespoke UI — the SAME screen as
	// `runDirUnusable`, with a different sentence — because it needs `$HOME`
	// unset AND a failing passwd lookup, or a deleted working directory.
	const boot: DegradedBoot = { kind: 'homeUnresolvable', reason: 'home directory unavailable' };

	it('says what is wrong, in its own sentence rather than the run-dir one', () => {
		expect(takeoverCopy(boot).title).toBe('OpenVHost cannot work out where to keep its files');
		expect(prose(boot)).not.toContain('.manage()');
	});

	it('carries the reason verbatim, and names no path — because there is none', () => {
		expect(facts(boot)).toEqual({ Error: 'home directory unavailable' });
	});

	it('names the override that actually fixes it', () => {
		// `resolve_home` takes `OPENVHOST_HOME` first and absolutizes it, so this
		// is a remedy the user can really apply — and "full path" is not decoration:
		// a relative override is absolutized against a working directory that may
		// itself be what went missing.
		expect(prose(boot)).toContain('Set OPENVHOST_HOME to the full path of a folder you can write');
	});
});

describe('every screen', () => {
	const all: DegradedBoot[] = [
		{ kind: 'alreadyRunning', home: HOME },
		{ kind: 'runDirUnusable', path: RUN_DIR, reason: ERRNO_13 },
		{ kind: 'homeUnresolvable', reason: 'home directory unavailable' }
	];

	it('has its own test hook, so no two states can share a screen by accident', () => {
		const ids = all.map((b) => takeoverCopy(b).testId);
		expect(new Set(ids).size).toBe(all.length);
	});

	it('says something, and says it with at least one fact to act on', () => {
		for (const boot of all) {
			const copy = takeoverCopy(boot);
			expect(copy.title).not.toBe('');
			expect(copy.lines.length).toBeGreaterThan(0);
			expect(copy.details.length).toBeGreaterThan(0);
		}
	});

	describe('who gets Reveal in Finder (design D3)', () => {
		// Vacuity, measured, both directions, over the whole desktop suite.
		//
		// Flipping `revealsRunDir` to `true` on the `alreadyRunning` arm — a state
		// inheriting an action pointed at a folder it never named — reddened 4 of
		// 1599: `offers it on no other screen` and `gives every screen that offers
		// the button a folder …` here, `offers Reveal in Finder on no other screen`
		// in `BootTakeover.svelte.test.ts`, and `is absent on the screens that named
		// no folder` at the layout seam. `offers it on the one screen …` stayed
		// green.
		//
		// Flipping the `runDirUnusable` arm to `false` reddened 5, a disjoint set
		// except for one: `offers it on the one screen …` and `gives every screen
		// …` here, `offers Reveal in Finder beside Quit …` in the component file,
		// and BOTH layout-seam tests (`reaches the backend …`, `says so when the
		// reveal fails …`). Every `no other screen` case stayed green.
		//
		// `gives every screen …` reddens under both, which is what its explicit
		// premise assertion is for: without it, the second mutation would leave it
		// iterating an empty list and passing.

		it('offers it on the one screen that named a folder', () => {
			expect(
				takeoverCopy({ kind: 'runDirUnusable', path: RUN_DIR, reason: ERRNO_13 })
			).toMatchObject({ revealsRunDir: true });
		});

		it('offers it on no other screen, because there is nothing they could reveal', () => {
			// `alreadyRunning` names a folder that is FINE — the other copy is using
			// it — and `homeUnresolvable` resolved no path at all. A shared action
			// here could only open some other state's directory.
			expect(takeoverCopy({ kind: 'alreadyRunning', home: HOME }).revealsRunDir).toBe(false);
			expect(
				takeoverCopy({ kind: 'homeUnresolvable', reason: 'home directory unavailable' })
					.revealsRunDir
			).toBe(false);
		});

		it('gives every screen that offers the button a folder the user can also read', () => {
			// The tie between the two halves: a screen offering Reveal without a
			// `Folder` fact would be asking the user to trust a path they cannot see,
			// which is what the copyable text exists to prevent.
			const offering = all.map(takeoverCopy).filter((c) => c.revealsRunDir);
			// The premise, asserted first: with no screen offering the button this
			// test would otherwise claim to have proven something about all of them.
			expect(offering.length).toBeGreaterThan(0);
			for (const copy of offering) {
				expect(copy.details.map((d) => d.label)).toContain('Folder');
			}
		});
	});
});
