// SPDX-License-Identifier: GPL-3.0-or-later
//
// The quit confirmation's in-flight-operation sentence, asserted as words.
//
// The bug this file exists to keep dead (branch review, HIGH): the dialog wrote
// ONE sentence — "… is still installing. Quitting stops it immediately and
// discards the download/build in progress" — and rendered it for a `brew
// uninstall` too, where every clause is false. It typechecked because the
// component's prop simply did not declare `operation`, so the field Rust had
// been sending all along was dropped in silence.
//
// So the assertions below are not "is the string non-empty". They are: the two
// operations say structurally different things, and the removal wording does
// not contain the install wording's claims.

import { describe, expect, it } from 'vitest';
import {
	pendingOperationCopy,
	pendingOperationSentence,
	type PackageOperation,
	type PendingOperation,
	type PendingOperationKind
} from './quit.derive';

const KINDS: PendingOperationKind[] = ['php', 'mysql'];
const OPERATIONS: PackageOperation[] = ['install', 'uninstall', 'initialize'];

/** Every kind × operation combination, with the label shape that kind really
 *  carries (PHP bare, MySQL a complete phrase — see `PendingOperation`). */
const EVERY_COMBINATION: PendingOperation[] = KINDS.flatMap((kind) =>
	OPERATIONS.map((operation) => ({
		kind,
		operation,
		label: kind === 'php' ? '8.4' : 'MySQL 8.4'
	}))
);

describe('the label, and the word this UI supplies in front of it', () => {
	it("supplies PHP's missing word, because its label is a bare major", () => {
		const copy = pendingOperationCopy({ kind: 'php', operation: 'install', label: '8.4' });
		expect(copy.lead).toBe('PHP ');
		expect(copy.label).toBe('8.4');
		expect(pendingOperationSentence({ kind: 'php', operation: 'install', label: '8.4' })).toContain(
			'PHP 8.4'
		);
	});

	it("supplies nothing in front of MySQL's already-complete phrase", () => {
		const sentence = pendingOperationSentence({
			kind: 'mysql',
			operation: 'install',
			label: 'MySQL 8.4'
		});
		expect(sentence).toContain('MySQL 8.4');
		expect(sentence).not.toContain('MySQL MySQL');
	});

	it("renders a MySQL init's label verbatim, without the word it no longer carries", () => {
		const sentence = pendingOperationSentence({
			kind: 'mysql',
			operation: 'initialize',
			label: 'MySQL 8.4'
		});
		expect(sentence).toContain('MySQL 8.4');
		expect(sentence).not.toContain('MySQL MySQL');
	});

	it('never re-words the label', () => {
		for (const pending of EVERY_COMBINATION) {
			expect(pendingOperationCopy(pending).label).toBe(pending.label);
		}
	});
});

describe('an install in flight', () => {
	// Unchanged wording, pinned so the uninstall fix cannot drift it: this
	// sentence was correct, the failure was that it was the only one.
	it('says it is installing, and that the build in progress is lost', () => {
		const sentence = pendingOperationSentence({ kind: 'php', operation: 'install', label: '8.4' });
		expect(sentence).toContain('is still installing');
		expect(sentence).toContain('discards the download/build in progress');
		expect(sentence).toContain('only starting over');
	});
});

describe('an uninstall in flight — the HIGH the branch review found', () => {
	const removal: PendingOperation = { kind: 'php', operation: 'uninstall', label: '8.3' };

	it('does not claim anything is installing', () => {
		expect(pendingOperationSentence(removal)).not.toContain('is still installing');
	});

	it('does not claim a download or build is at risk, because none is', () => {
		const sentence = pendingOperationSentence(removal);
		expect(sentence).not.toContain('download/build');
		expect(sentence).not.toContain('only starting over');
	});

	// The real failure mode the live proof produced: brew's metadata and the
	// filesystem disagreeing — `brew list` still reporting `php@8.3 8.3.33`
	// while the keg had lost `bin/` and `INSTALL_RECEIPT.json`. A
	// half-uninstalled formula is possible, and this sentence is the only place
	// the user is told so.
	it('names the partial-removal risk instead', () => {
		const sentence = pendingOperationSentence(removal);
		expect(sentence).toContain('is still being removed');
		expect(sentence).toContain('half-removed');
		expect(sentence).toContain('still listed by brew');
	});

	// "Point forward" (brand guidelines §6.1.2): the recovery is a real check
	// the user can run, not "try again".
	it('points at Homebrew as the way to settle it after relaunch', () => {
		const sentence = pendingOperationSentence(removal);
		expect(sentence).toContain('brew');
		expect(sentence).toContain('after reopening OpenVHost');
	});

	it('names the version being removed, with the same leading word rule', () => {
		expect(pendingOperationSentence(removal)).toContain('PHP 8.3');
		expect(
			pendingOperationSentence({ kind: 'mysql', operation: 'uninstall', label: 'MySQL 8.4' })
		).toContain('MySQL 8.4');
	});
});

// The security audit's F1. In Rust an initialization used to be tagged
// `(Mysql, Install)` and told apart from a real install only by its label —
// which `InstallLock::abort_running_if` does not compare, so
// `cancel_mysql_install` aborted initializations. The Rust fix gave it its own
// `PackageOperation`, and this is the sentence that arrived with it: an
// initialization is neither an install nor a removal, and saying it "is still
// installing" was the visible half of the same collapse.
describe('an initialization in flight', () => {
	const init: PendingOperation = { kind: 'mysql', operation: 'initialize', label: 'MySQL 8.4' };

	it('does not borrow either of the other two sentences', () => {
		const sentence = pendingOperationSentence(init);
		expect(sentence).not.toContain('is still installing');
		expect(sentence).not.toContain('is still being removed');
		expect(sentence).not.toContain('download/build');
		expect(sentence).not.toContain('half-removed');
	});

	it('says what is actually happening, and names it', () => {
		const sentence = pendingOperationSentence(init);
		expect(sentence).toContain('MySQL 8.4');
		expect(sentence).toContain('is still being set up');
	});

	// Nothing is downloaded and no databases exist yet, so the honest cost is
	// the wait — and the leftover staging folder really is swept at next launch
	// (`sweep_stale_staging`), so promising that is not a guess.
	it('is honest that nothing of the user’s is at risk, and points forward', () => {
		const sentence = pendingOperationSentence(init);
		expect(sentence).toContain('nothing of yours is lost');
		expect(sentence).toContain('cleared the next time OpenVHost starts');
		expect(sentence).toContain('Set it up again');
	});
});

describe('the three operations never collapse onto one sentence', () => {
	// THE distinctness sweep, pairwise over every kind × operation pair rather
	// than "each is non-empty". A dialog that rendered one sentence for two
	// operations — the shipped bug — passes every non-emptiness check ever
	// written and fails only here.
	it('gives every pair of combinations a different sentence', () => {
		expect(EVERY_COMBINATION.length).toBe(6);
		for (let i = 0; i < EVERY_COMBINATION.length; i += 1) {
			for (let j = i + 1; j < EVERY_COMBINATION.length; j += 1) {
				expect(pendingOperationSentence(EVERY_COMBINATION[i])).not.toBe(
					pendingOperationSentence(EVERY_COMBINATION[j])
				);
			}
		}
	});

	// The subtler collapse: two sentences that differ only because the LABEL
	// differs, while the consequence they describe is identical. Comparing
	// `rest` — the part after the label — is what catches it, and it is the
	// exact shape of the audit F1 bug one layer down (two runs distinguished
	// only by prose).
	it('describes a different consequence, not merely a different subject', () => {
		for (const kind of KINDS) {
			const rests = OPERATIONS.map(
				(operation) => pendingOperationCopy({ kind, operation, label: 'x' }).rest
			);
			expect(new Set(rests).size).toBe(OPERATIONS.length);
		}
	});

	it('never emits an empty consequence for any combination', () => {
		for (const pending of EVERY_COMBINATION) {
			expect(pendingOperationCopy(pending).rest.length).toBeGreaterThan(0);
		}
	});

	// The three parts are one sentence; a `lead`/`rest` that did not join
	// cleanly would read as "PHP8.4is still…" on screen.
	it('joins into one readable sentence', () => {
		for (const pending of EVERY_COMBINATION) {
			const copy = pendingOperationCopy(pending);
			expect(pendingOperationSentence(pending)).toBe(`${copy.lead}${copy.label}${copy.rest}`);
			expect(copy.rest.startsWith(' ')).toBe(true);
		}
	});
});
