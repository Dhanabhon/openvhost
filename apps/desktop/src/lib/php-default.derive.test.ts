// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import type { DefaultPhpDto } from './ipc';
import {
	DEFAULT_PHP_MISSING_TITLE,
	defaultPhpNotice,
	hasStoredDefault,
	isChosenDefault,
	offersDefaultChoice
} from './php-default.derive';

/** The four states, named once. `every()` is what keeps a test that iterates
 *  "all states" honest when a fifth is added: the `never` guards in the module
 *  break the build, and anything here that forgot to grow shows up as a state
 *  with no case rather than a silent pass. */
const nothingInstalled: DefaultPhpDto = { kind: 'nothingInstalled' };
const unset = (serving: string): DefaultPhpDto => ({ kind: 'unset', serving });
const preferred = (major: string): DefaultPhpDto => ({ kind: 'preferred', major });
const preferredMissing = (requested: string, serving: string | null): DefaultPhpDto => ({
	kind: 'preferredMissing',
	requested,
	serving
});

function every(): DefaultPhpDto[] {
	return [nothingInstalled, unset('8.1'), preferred('8.4'), preferredMissing('8.4', '8.1')];
}

describe('isChosenDefault', () => {
	it('is true only for the major the user actually chose', () => {
		expect(isChosenDefault(preferred('8.4'), '8.4')).toBe(true);
		expect(isChosenDefault(preferred('8.4'), '8.1')).toBe(false);
	});

	it('never marks a major that merely sorted first', () => {
		// `unset` HAS a serving major — the historical first-discovered rule — and
		// the whole point of this slice is that serving is not the same as chosen.
		// A badge here would relabel an accident as a decision.
		expect(isChosenDefault(unset('8.1'), '8.1')).toBe(false);
	});

	it('never marks the fallback a missing preference is being served by', () => {
		// Same reason, and the sharper case: 8.1 is what the user is GETTING, and
		// 8.4 is what they asked for. Badging 8.1 would say the failure is the
		// intent. `defaultPhpNotice` is what tells that story instead.
		const state = preferredMissing('8.4', '8.1');
		expect(isChosenDefault(state, '8.1')).toBe(false);
		expect(isChosenDefault(state, '8.4')).toBe(false);
	});

	it('marks nothing when nothing is installed', () => {
		expect(isChosenDefault(nothingInstalled, '8.4')).toBe(false);
	});
});

describe('hasStoredDefault', () => {
	it('separates "a choice was made" from "a major happens to be serving"', () => {
		expect(hasStoredDefault(preferred('8.4'))).toBe(true);
		// Stored but unhonourable is still stored — this is the arm that keeps the
		// control reachable in the state spec claim 4 exists for.
		expect(hasStoredDefault(preferredMissing('8.4', '8.1'))).toBe(true);

		expect(hasStoredDefault(unset('8.1'))).toBe(false);
		expect(hasStoredDefault(nothingInstalled)).toBe(false);
	});
});

describe('offersDefaultChoice', () => {
	it('offers the control when a real choice exists', () => {
		expect(offersDefaultChoice(unset('8.1'), 2)).toBe(true);
		expect(offersDefaultChoice(unset('8.1'), 5)).toBe(true);
	});

	it('withholds it when the answer is not in doubt', () => {
		// One PHP and nothing chosen: a button whose only effect is to store what
		// already happens. Withholding it is also what keeps a one-PHP machine —
		// the common case — pixel-identical to before this slice.
		expect(offersDefaultChoice(unset('8.1'), 1)).toBe(false);
		expect(offersDefaultChoice(nothingInstalled, 0)).toBe(false);
	});

	it('offers it on a single-major machine once a preference is stored', () => {
		// The clause that matters, per the module's own doc. Uninstall down to one
		// major while a preference names a different one and the answer IS in
		// doubt — gating purely on `count >= 2` would strand the user with a
		// choice they can neither see nor change.
		expect(offersDefaultChoice(preferredMissing('8.4', '8.1'), 1)).toBe(true);
		expect(offersDefaultChoice(preferred('8.1'), 1)).toBe(true);
	});
});

describe('defaultPhpNotice', () => {
	it('says nothing in every state that is working as intended', () => {
		// Including `preferred`: a default that is being honoured needs no banner,
		// and `unset` is every machine that predates the setting — which is what
		// keeps this slice invisible until someone chooses.
		expect(defaultPhpNotice(nothingInstalled)).toBeNull();
		expect(defaultPhpNotice(unset('8.1'))).toBeNull();
		expect(defaultPhpNotice(preferred('8.4'))).toBeNull();
	});

	it('names what was chosen, what is serving instead, and both ways out', () => {
		const note = defaultPhpNotice(preferredMissing('8.4', '8.1'));
		expect(note).not.toBeNull();
		// The chosen major, not just the fallback — the fact the user most needs
		// told, and the one a per-row note could not carry.
		expect(note).toContain('8.4');
		expect(note).toContain('8.1');
		// Legible is not enough on its own; the note has to be actionable.
		expect(note).toContain('Install PHP 8.4');
		expect(note).toMatch(/make another installed version the default/i);
	});

	it('tells a different truth when there is no fallback at all', () => {
		const note = defaultPhpNotice(preferredMissing('8.4', null));
		expect(note).not.toBeNull();
		expect(note).toContain('8.4');
		// The consequence is worse and the sentence has to say so, rather than
		// reusing the "served by X instead" wording with a blank where X goes.
		expect(note).toMatch(/no PHP at all/i);
		expect(note).not.toMatch(/served by PHP\s*\./i);
	});

	it('leads with a title that states the problem', () => {
		expect(DEFAULT_PHP_MISSING_TITLE).toMatch(/not installed/i);
	});
});

describe('the four states', () => {
	it('are each answered by every derivation, with no state left undecided', () => {
		// Not a tautology: each derivation is an exhaustive `switch` ending in a
		// `never` guard, so a fifth variant fails to COMPILE rather than falling
		// through to a default. What this asserts at runtime is the complement —
		// that none of the four currently throws or returns undefined, which a
		// hand-written `if/else` chain could do while still type-checking.
		for (const state of every()) {
			expect(typeof hasStoredDefault(state)).toBe('boolean');
			expect(typeof isChosenDefault(state, '8.4')).toBe('boolean');
			expect(typeof offersDefaultChoice(state, 2)).toBe('boolean');
			const note = defaultPhpNotice(state);
			expect(note === null || typeof note === 'string').toBe(true);
		}
	});

	it('put a badge on exactly one major, and only when one was chosen', () => {
		// The page-level invariant the row badge rests on: across every state and
		// every major, at most one row can claim to be the default.
		const majors = ['8.1', '8.3', '8.4'];
		for (const state of every()) {
			const badged = majors.filter((m) => isChosenDefault(state, m));
			expect(badged.length).toBeLessThanOrEqual(1);
			expect(badged.length === 1).toBe(state.kind === 'preferred');
		}
	});
});
