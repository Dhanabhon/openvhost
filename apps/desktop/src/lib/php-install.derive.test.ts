// SPDX-License-Identifier: GPL-3.0-or-later
//
// The decisions behind the Languages page after it stopped requiring Homebrew
// (off-Homebrew slice 5C). Every one of them is a pure function here rather
// than an `{#if}` in a component, so the table can be stated exhaustively
// instead of sampled through SSR markup.
//
// WHAT THIS FILE CANNOT COVER: whether the components actually CALL these.
// `LanguageRow.svelte.test.ts`, `LanguagesEmpty.svelte.test.ts` and
// `routes/languages/languages-page.test.ts` cover that seam, at three levels.

import { describe, expect, it } from 'vitest';
import type { PhpInstallProgressDto, PhpInstallResultDto, PhpPackageOfferDto } from './ipc';
import {
	noRouteToAnyPhp,
	phpInstallDeclaredTotal,
	phpInstallInvite,
	phpInstallOffered,
	phpInstallProgressLabel,
	phpInstallProgressPercent,
	phpNoRouteNote,
	phpOutcomeRender,
	phpSourceBadge
} from './php-install.derive';

/** The three offers, named once so no test spells a `kind` string by hand and
 *  quietly tests a variant that does not exist. */
const AVAILABLE: PhpPackageOfferDto = { kind: 'available', version: '8.4.24' };
const AWAITING: PhpPackageOfferDto = { kind: 'awaitingRelease', tag: 'php-8.4.24' };
const UNAVAILABLE: PhpPackageOfferDto = { kind: 'unavailable', target: 'macos-arm64' };

/** Every state the packaged pipeline can report, in the order it reports them. */
const EVERY_PROGRESS: PhpInstallProgressDto[] = [
	{ kind: 'started', total: 4096 },
	{ kind: 'downloaded', bytes: 1024 },
	{ kind: 'verified' },
	{ kind: 'extracted' },
	{ kind: 'linked' }
];

/** Every unordered pair of indices into `xs`, so a "these all differ" claim is
 *  actually checked rather than asserted one item at a time. */
function pairs<T>(xs: readonly T[]): [T, T][] {
	const out: [T, T][] = [];
	for (let i = 0; i < xs.length; i += 1) {
		for (let j = i + 1; j < xs.length; j += 1) out.push([xs[i], xs[j]]);
	}
	return out;
}

describe('phpSourceBadge', () => {
	it('names the exact patch level for a runtime OpenVHost installed', () => {
		// The whole point of the field slice 5B made real: the version is a
		// directory name, so reporting it executes nothing.
		const badge = phpSourceBadge({ kind: 'packaged', version: '8.4.24' });
		expect(badge?.label).toBe('OpenVHost 8.4.24');
		expect(badge?.title).toContain('8.4.24');
	});

	// Deliberately unlike MysqlRow's and WebServerRow's identical chips, which do
	// label their Homebrew runtimes. Spec §5 says "Homebrew rows carry none", and
	// §8.6 is what makes it binding: a chip on every brewed row would be a
	// visible change to every real machine today.
	it('gives a Homebrew keg no badge at all, so a brew-only machine gains nothing', () => {
		expect(phpSourceBadge({ kind: 'homebrew' })).toBeNull();
	});

	it('gives a major with nothing installed no badge', () => {
		expect(phpSourceBadge(null)).toBeNull();
	});
});

// Design D2's core. `brewFound` is an INPUT to a per-major answer, never the
// answer itself — the table below is the whole of it, and every cell matters
// because four of the six are states real machines are in today.
describe('phpInstallOffered', () => {
	it('offers our own package with or without Homebrew', () => {
		// The row this entire off-Homebrew programme exists for.
		expect(phpInstallOffered(AVAILABLE, false)).toBe(true);
		expect(phpInstallOffered(AVAILABLE, true)).toBe(true);
	});

	// §8.5 as corrected: withholding the button here would have REMOVED A
	// WORKING CONTROL. On this Apple Silicon machine 8.4 is `AwaitingRelease`
	// today and its Homebrew Install button installs PHP.
	it('keeps the Homebrew button on an AwaitingRelease row that has Homebrew', () => {
		expect(phpInstallOffered(AWAITING, true)).toBe(true);
	});

	it('offers nothing for an AwaitingRelease row with no Homebrew', () => {
		// `route_for` sends this offer to Homebrew, and `install_php` then fails
		// at `find_brew()` before spawning anything. A button whose only outcome
		// is "Homebrew was not found" is worse than no button.
		expect(phpInstallOffered(AWAITING, false)).toBe(false);
	});

	// `Unavailable` is the ORDINARY path, not the failure path: four majors out
	// of five today, and every major on Intel.
	it('installs an Unavailable row through Homebrew exactly as before', () => {
		expect(phpInstallOffered(UNAVAILABLE, true)).toBe(true);
	});

	it('offers nothing for an Unavailable row with no Homebrew', () => {
		expect(phpInstallOffered(UNAVAILABLE, false)).toBe(false);
	});
});

describe('phpNoRouteNote', () => {
	// §8.6, at the level the answer is actually decided. If this ever returns a
	// string, every real machine today gains a line of text on every row.
	it('says nothing at all on any row of a machine that has Homebrew', () => {
		for (const offer of [AVAILABLE, AWAITING, UNAVAILABLE]) {
			expect(phpNoRouteNote(offer, '8.4', true), offer.kind).toBeNull();
		}
	});

	it('says nothing on an Available row even without Homebrew, because it installs', () => {
		expect(phpNoRouteNote(AVAILABLE, '8.4', false)).toBeNull();
	});

	// §8.2b's 8.1/8.3/8.5 rows: PER ROW, and naming Homebrew, because for these
	// majors Homebrew genuinely is required and permanently so.
	it('names Homebrew, the major and the target on an Unavailable row with no Homebrew', () => {
		const note = phpNoRouteNote(UNAVAILABLE, '8.1', false);
		expect(note).toContain('Homebrew');
		expect(note).toContain('PHP 8.1');
		expect(note).toContain('macos-arm64');
	});

	// §8.5's other half: what AwaitingRelease withholds is the PACKAGED route,
	// and what it adds is naming the tag a maintainer has to publish.
	it('names the unpublished release tag on an AwaitingRelease row with no Homebrew', () => {
		const note = phpNoRouteNote(AWAITING, '8.4', false);
		expect(note).toContain('php-8.4.24');
		expect(note).toContain('Homebrew');
		// The next action belongs to the maintainer, and the copy has to say so
		// or the user is left waiting on something they could act on.
		expect(note).toMatch(/maintainer/i);
	});

	// The two no-route notes must not collapse into one sentence: "no package
	// exists for your machine" and "the package exists but is unpublished" are
	// different facts with different resolutions.
	it('tells the two no-route reasons apart', () => {
		expect(phpNoRouteNote(UNAVAILABLE, '8.4', false)).not.toBe(
			phpNoRouteNote(AWAITING, '8.4', false)
		);
	});

	// A note that offers no button and names no version is not an explanation.
	it('is never rendered where an Install button is', () => {
		for (const offer of [AVAILABLE, AWAITING, UNAVAILABLE]) {
			for (const brewFound of [true, false]) {
				const offered = phpInstallOffered(offer, brewFound);
				const note = phpNoRouteNote(offer, '8.4', brewFound);
				expect(offered && note !== null, `${offer.kind}/${brewFound}`).toBe(false);
				expect(offered || note !== null, `${offer.kind}/${brewFound}`).toBe(true);
			}
		}
	});
});

// The page-level dead end. Its whole correction in D2 is that it now asks a
// question about the ROWS instead of about one machine-wide bool.
describe('noRouteToAnyPhp', () => {
	const notInstalled = (offer: PhpPackageOfferDto) => ({ installed: false, offer });
	const installed = (offer: PhpPackageOfferDto) => ({ installed: true, offer });

	// §8.2 — the case the screen was actually written for, and the only one it
	// keeps.
	it('is a dead end with no Homebrew, nothing installed and nothing installable', () => {
		expect(
			noRouteToAnyPhp({
				brewFound: false,
				runtimes: [notInstalled(UNAVAILABLE), notInstalled(AWAITING)]
			})
		).toBe(true);
	});

	// §8.1 — the headline. A machine with a packaged PHP was being told it could
	// not install PHP, on a page simultaneously not listing the PHP it had.
	it('is not a dead end when a packaged PHP is already installed, Homebrew or not', () => {
		expect(
			noRouteToAnyPhp({
				brewFound: false,
				runtimes: [installed(UNAVAILABLE), notInstalled(UNAVAILABLE)]
			})
		).toBe(false);
	});

	// §8.2b — the case D2 exists for, and the one the first draft got wrong.
	it('is not a dead end when one major is installable from our own tree', () => {
		expect(
			noRouteToAnyPhp({
				brewFound: false,
				runtimes: [
					notInstalled(UNAVAILABLE), // 8.1
					notInstalled(AVAILABLE), // 8.4
					notInstalled(UNAVAILABLE) // 8.5
				]
			})
		).toBe(false);
	});

	// An AwaitingRelease offer is NOT a route: `route_for` sends it to Homebrew,
	// which is not here. Folding it in with `Available` would suppress the dead
	// end on a machine that genuinely has nowhere to go — which is today's
	// catalogue on every Apple Silicon Mac.
	it('does not count an unpublished package as a route', () => {
		expect(noRouteToAnyPhp({ brewFound: false, runtimes: [notInstalled(AWAITING)] })).toBe(true);
	});

	it('is never a dead end where Homebrew is present', () => {
		for (const offer of [AVAILABLE, AWAITING, UNAVAILABLE]) {
			expect(
				noRouteToAnyPhp({ brewFound: true, runtimes: [notInstalled(offer)] }),
				offer.kind
			).toBe(false);
		}
	});

	// The trap in defining this purely as "no row offers Install": an empty list
	// satisfies that vacuously. A page whose catalogue has not arrived yet must
	// not flash the bluntest screen in the app.
	it('is not a dead end with an empty runtime list on a machine that has Homebrew', () => {
		expect(noRouteToAnyPhp({ brewFound: true, runtimes: [] })).toBe(false);
	});

	it('is a dead end with an empty runtime list and no Homebrew, exactly as before', () => {
		expect(noRouteToAnyPhp({ brewFound: false, runtimes: [] })).toBe(true);
	});
});

describe('phpInstallInvite', () => {
	// Verbatim what this page has always shown. §8.6: a machine with Homebrew
	// must not see one character change.
	it('still names Homebrew wherever Homebrew is present', () => {
		expect(phpInstallInvite(true)).toBe(
			'Choose a version below — OpenVHost installs it through Homebrew and serves your sites with it.'
		);
	});

	// The same page-wide claim about a per-major fact that D2 removes one branch
	// up: on the machine with a packaged PHP and no brew, this sentence was a
	// lie about the route the Install button would take.
	it('stops naming Homebrew on a machine that does not have it', () => {
		expect(phpInstallInvite(false)).not.toMatch(/homebrew/i);
		expect(phpInstallInvite(false)).not.toBe(phpInstallInvite(true));
	});
});

// Design D4. `PhpInstallResultDto` gained eight arms beyond `Brew`, and the
// obvious `result.kind === 'brew'` test would leave every one of them rendering
// NOTHING — the C1 defect this page already fixed once, in a new costume.
describe('phpOutcomeRender', () => {
	const brew = (exitCode: number | null, detected: boolean): PhpInstallResultDto => ({
		kind: 'brew',
		exitCode,
		detected
	});

	// The Homebrew arm's three answers are unchanged from before this slice,
	// down to the wording, because it is the route every real machine takes.
	it('renders a non-zero brew exit as a failure naming the code', () => {
		const out = phpOutcomeRender(brew(1, false), '8.4');
		expect(out.alert).toContain('exited with code 1');
		expect(out.alert).toContain('PHP 8.4');
		expect(out.warning).toBeNull();
		expect(out.succeeded).toBe(false);
	});

	it('renders a signal-killed brew run as a failure too, not as silence', () => {
		// `null` is "no code at all", which is not a clean exit. Treating it as
		// one is what made a killed install render nothing.
		const out = phpOutcomeRender(brew(null, false), '8.4');
		expect(out.alert).toMatch(/killed/i);
		expect(out.succeeded).toBe(false);
	});

	it('warns when brew exits 0 and the version still is not there', () => {
		const out = phpOutcomeRender(brew(0, false), '8.4');
		expect(out.alert).toBeNull();
		expect(out.warning).toMatch(/was not found/i);
		expect(out.succeeded).toBe(false);
	});

	it('reports a clean brew install as a success with nothing to alarm anyone', () => {
		const out = phpOutcomeRender(brew(0, true), '8.4');
		expect(out).toEqual({ alert: null, warning: null, succeeded: true });
	});

	// A packaged install has NO exit code, because it spawns no child process.
	// `exitCode !== 0` against `undefined` is `true`, which is why a shared
	// brew-shaped result would have rendered this success as "brew was killed
	// before installing PHP 8.4 finished" — the render design D4 exists to
	// prevent.
	it('renders a successful packaged install as a success, never as a killed brew', () => {
		const out = phpOutcomeRender(
			{
				kind: 'installed',
				version: '8.4.24',
				detected: true,
				ledger: { kind: 'recorded' }
			},
			'8.4'
		);
		expect(out.alert).toBeNull();
		expect(out.succeeded).toBe(true);
		expect(out.alert ?? '').not.toMatch(/killed|brew/i);
	});

	it('warns when a packaged install lands but its php-fpm is not there', () => {
		const out = phpOutcomeRender(
			{ kind: 'installed', version: '8.4.24', detected: false, ledger: { kind: 'recorded' } },
			'8.4'
		);
		expect(out.warning).toMatch(/not found/i);
		expect(out.succeeded).toBe(false);
	});

	it('treats an already-installed package as a plain success', () => {
		const out = phpOutcomeRender({ kind: 'alreadyInstalled', version: '8.4.24' }, '8.4');
		expect(out).toEqual({ alert: null, warning: null, succeeded: true });
	});

	// The one deliberately silent arm: staging unwound with the dropped future,
	// so nothing happened and there is nothing to explain away.
	it('says nothing about a cancelled install', () => {
		expect(phpOutcomeRender({ kind: 'cancelled' }, '8.4')).toEqual({
			alert: null,
			warning: null,
			succeeded: false
		});
	});

	// THE property this describe block exists for. Every way an install can fail
	// must produce something a user can read. A `kind === 'brew'` test passes
	// every other assertion in this file and fails this one.
	it('renders every failure arm as a visible failure, never as silence', () => {
		const failures: PhpInstallResultDto[] = [
			{ kind: 'verificationFailed', expected: 'aa11', actual: 'bb22' },
			{ kind: 'stalled', detail: 'no bytes for 60s' },
			{ kind: 'awaitingRelease', tag: 'php-8.4.24' },
			{ kind: 'unavailable', target: 'macos-x86_64' },
			{ kind: 'failed', reason: 'the tarball had no bin directory' }
		];
		for (const result of failures) {
			const out = phpOutcomeRender(result, '8.4');
			expect(out.alert, result.kind).not.toBeNull();
			expect(out.alert, result.kind).toContain('8.4');
			expect(out.succeeded, result.kind).toBe(false);
		}
	});

	// A checksum mismatch is golden rule 6's whole point and must never read as
	// a transient glitch — it has to name both digests, or nobody can tell a
	// corrupted download from a substituted one.
	it('names both digests on a checksum mismatch', () => {
		const out = phpOutcomeRender(
			{ kind: 'verificationFailed', expected: 'aa11', actual: 'bb22' },
			'8.4'
		);
		expect(out.alert).toContain('aa11');
		expect(out.alert).toContain('bb22');
	});

	// Each failure arm carries a different fact; a shared "install failed"
	// sentence would throw away the part that resolves it.
	it('gives each failure arm its own words', () => {
		const texts = (
			[
				{ kind: 'verificationFailed', expected: 'aa11', actual: 'bb22' },
				{ kind: 'stalled', detail: 'no bytes for 60s' },
				{ kind: 'awaitingRelease', tag: 'php-8.4.24' },
				{ kind: 'unavailable', target: 'macos-x86_64' },
				{ kind: 'failed', reason: 'the tarball had no bin directory' }
			] satisfies PhpInstallResultDto[]
		).map((r) => phpOutcomeRender(r, '8.4').alert);
		expect(new Set(texts).size).toBe(texts.length);
	});

	// The detail the backend went to the trouble of carrying must survive to the
	// screen. Dropping it leaves the user with "it failed" and no next step.
	it('forwards the detail each failure arm carries', () => {
		expect(
			phpOutcomeRender({ kind: 'stalled', detail: 'no bytes for 60s' }, '8.4').alert
		).toContain('no bytes for 60s');
		expect(phpOutcomeRender({ kind: 'failed', reason: 'no bin directory' }, '8.4').alert).toContain(
			'no bin directory'
		);
		expect(
			phpOutcomeRender({ kind: 'unavailable', target: 'macos-x86_64' }, '8.4').alert
		).toContain('macos-x86_64');
		expect(phpOutcomeRender({ kind: 'awaitingRelease', tag: 'php-8.4.24' }, '8.4').alert).toContain(
			'php-8.4.24'
		);
	});

	// `alert` and `warning` occupy one slot in the markup (`{#if}` / `{:else if}`),
	// so a warning set alongside an alert would be invisible — which would make
	// it a fact the code believes it is showing and is not.
	it('never sets a warning it could not render, beside an alert', () => {
		const all: PhpInstallResultDto[] = [
			brew(1, false),
			brew(null, false),
			brew(0, false),
			brew(0, true),
			brew(1, true),
			{ kind: 'installed', version: '8.4.24', detected: false, ledger: { kind: 'recorded' } },
			{ kind: 'alreadyInstalled', version: '8.4.24' },
			{ kind: 'cancelled' },
			{ kind: 'verificationFailed', expected: 'a', actual: 'b' },
			{ kind: 'stalled', detail: 'x' },
			{ kind: 'awaitingRelease', tag: 't' },
			{ kind: 'unavailable', target: 'macos-x86_64' },
			{ kind: 'failed', reason: 'r' }
		];
		for (const result of all) {
			const out = phpOutcomeRender(result, '8.4');
			expect(out.alert !== null && out.warning !== null, result.kind).toBe(false);
		}
	});
});

// The packaged route's five pipeline states, whose consumer landed in the 5C
// fix wave.
//
// Vacuity: the distinctness claims are checked pairwise rather than one item at
// a time, and the rest assert exact strings or exact numbers rather than
// non-emptiness. Proven by mutation — making the `verified` arm return the
// `extracted` sentence reddened three tests here (pairwise, by-name, and the
// "says the checksum was checked" wording) plus the row test that renders it.
describe('phpInstallProgressLabel', () => {
	it('renders all five pipeline states pairwise-distinctly', () => {
		const labels = EVERY_PROGRESS.map((p) => phpInstallProgressLabel(p, 1024));
		for (const [a, b] of pairs(labels)) expect(a).not.toBe(b);
	});

	// Stated on its own as well as pairwise: this is the pair that carries the
	// verification guarantee golden rule 6 buys, and it should fail by name if it
	// ever collapses.
	it('never says the same thing for a verified download as for an extracted one', () => {
		expect(phpInstallProgressLabel({ kind: 'verified' }, null)).not.toBe(
			phpInstallProgressLabel({ kind: 'extracted' }, null)
		);
	});

	it('says the checksum was checked, in words a user can act on', () => {
		const label = phpInstallProgressLabel({ kind: 'verified' }, null);
		expect(label).toMatch(/checksum/i);
		expect(label).toMatch(/SHA-256/);
	});

	it('names the declared size when the server gave one', () => {
		expect(phpInstallProgressLabel({ kind: 'started', total: 1536 }, null)).toContain('1.50 KiB');
	});

	it('says so honestly when the server declared no size, and invents no number', () => {
		const label = phpInstallProgressLabel({ kind: 'started', total: null }, null);
		expect(label).toMatch(/did not say how large/i);
		expect(label).not.toMatch(/\d/);
	});

	it('shows progress against the total carried forward from the started event', () => {
		expect(phpInstallProgressLabel({ kind: 'downloaded', bytes: 512 }, 2048)).toBe(
			'Downloading — 512 B of 2.00 KiB'
		);
	});

	it('falls back to a "so far" reading rather than a fabricated denominator', () => {
		const label = phpInstallProgressLabel({ kind: 'downloaded', bytes: 512 }, null);
		expect(label).toContain('so far');
		expect(label).not.toContain(' of ');
	});

	// The one claim no packaged label may make: this route never runs a child
	// process, so nothing here may borrow the vocabulary of one — and it is not
	// MySQL's download either, so it must not name Oracle.
	it('mentions neither Homebrew nor Oracle on any step', () => {
		for (const p of EVERY_PROGRESS) {
			const label = phpInstallProgressLabel(p, 4096);
			expect(label, p.kind).not.toMatch(/brew|homebrew|oracle/i);
		}
	});
});

describe('phpInstallProgressPercent', () => {
	it('is a real percentage only while bytes are arriving against a known total', () => {
		expect(phpInstallProgressPercent({ kind: 'downloaded', bytes: 512 }, 2048)).toBe(25);
	});

	it('is null with no declared total, so no bar is drawn on a guess', () => {
		expect(phpInstallProgressPercent({ kind: 'downloaded', bytes: 512 }, null)).toBeNull();
	});

	it('is null for a zero or negative total rather than dividing by it', () => {
		expect(phpInstallProgressPercent({ kind: 'downloaded', bytes: 512 }, 0)).toBeNull();
		expect(phpInstallProgressPercent({ kind: 'downloaded', bytes: 512 }, -1)).toBeNull();
	});

	it('never exceeds 100 even if more bytes arrive than the server declared', () => {
		expect(phpInstallProgressPercent({ kind: 'downloaded', bytes: 4096 }, 2048)).toBe(100);
	});

	it('is null for the steps that are moments rather than durations', () => {
		expect(phpInstallProgressPercent({ kind: 'started', total: 10 }, 10)).toBeNull();
		expect(phpInstallProgressPercent({ kind: 'verified' }, 10)).toBeNull();
		expect(phpInstallProgressPercent({ kind: 'extracted' }, 10)).toBeNull();
		expect(phpInstallProgressPercent({ kind: 'linked' }, 10)).toBeNull();
	});
});

describe('phpInstallDeclaredTotal', () => {
	it('carries the declared length off the started event and nothing else', () => {
		expect(phpInstallDeclaredTotal({ kind: 'started', total: 4096 })).toBe(4096);
		expect(phpInstallDeclaredTotal({ kind: 'started', total: null })).toBeNull();
		expect(phpInstallDeclaredTotal({ kind: 'downloaded', bytes: 4096 })).toBeNull();
		expect(phpInstallDeclaredTotal({ kind: 'verified' })).toBeNull();
		expect(phpInstallDeclaredTotal({ kind: 'extracted' })).toBeNull();
		expect(phpInstallDeclaredTotal({ kind: 'linked' })).toBeNull();
	});
});
