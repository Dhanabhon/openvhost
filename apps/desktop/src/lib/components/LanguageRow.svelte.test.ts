// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), same approach as ApplyDialog.svelte.test.ts.
// WHAT THIS FILE CANNOT COVER: no DOM, so click handlers are exercised only through
// the `onclick` prop wiring Button.svelte already covers, not by simulating a click.

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LanguageRow from './LanguageRow.svelte';
import type {
	PhpInstallOutcomeDto,
	PhpInstallProgressDto,
	PhpRuntimeDto,
	ServiceStatus
} from '$lib/ipc';
import type { UiLog } from '$lib/languages.svelte';

/** A settled Homebrew install — the route every real machine still takes, and
 *  the only arm of `PhpInstallResultDto` that carries an exit code
 *  (off-Homebrew slice 5C design D4). The literals these calls replace said
 *  `{ major, exitCode, detected }` back when the outcome type was brew-shaped;
 *  every assertion about them is unchanged. */
function brewOutcome(
	major: string,
	exitCode: number | null,
	detected: boolean
): PhpInstallOutcomeDto {
	return { major, result: { kind: 'brew', exitCode, detected } };
}

/** One row, with sensible installed-shape defaults so most tests only need to
 *  state `major` and `installed` — mirrors `row()` in languages.svelte.test.ts. */
function r(
	major: string,
	installed: boolean,
	overrides: Partial<PhpRuntimeDto> = {}
): PhpRuntimeDto {
	return {
		major,
		installed,
		// See `row()` in languages.svelte.test.ts — catalogued is the default;
		// `cataloged: false` is the hand-installed row that gets no Uninstall.
		cataloged: true,
		recommended: false,
		fullVersion: null,
		path: installed ? `/opt/homebrew/opt/php@${major}/sbin/php-fpm` : null,
		socketPath: installed ? `/Users/x/.openvhost/run/php-fpm-${major}.sock` : null,
		serviceId: installed ? `php-fpm-${major}` : null,
		// See `row()` in languages.svelte.test.ts — a Homebrew keg matching
		// `path` above, and the absence four of five majors report today.
		source: installed ? { kind: 'homebrew' } : null,
		offer: { kind: 'unavailable', target: 'macos-arm64' },
		...overrides
	};
}

function renderRow(props: {
	row: PhpRuntimeDto;
	/** Defaults to TRUE — the ordinary case, a version this build manages. The
	 *  out-of-catalogue tests below pass `false` explicitly, which is the point:
	 *  the row cannot decide this for itself, so it must be told. */
	cataloged?: boolean;
	/** Defaults to TRUE — a machine with Homebrew, which is every real machine
	 *  today and the state the rest of this file was written against. The D2
	 *  tests below pass `false` explicitly. Defaulting it this way is what makes
	 *  "nothing changes where Homebrew is present" (spec §8.6) something the
	 *  EXISTING assertions keep proving rather than something new ones claim. */
	brewFound?: boolean;
	serviceState?: ServiceStatus['state'] | null;
	installing?: string;
	uninstalling?: string;
	log?: UiLog[];
	error?: string;
	outcome?: PhpInstallOutcomeDto | null;
	/** Defaults to NULL — the only value a Homebrew machine can ever produce
	 *  (spec §8.6), since `php-install-progress` is emitted solely by
	 *  `run_package_install`. Defaulting it this way is what makes "a brew
	 *  install paints no pipeline" something every EXISTING assertion in this
	 *  file keeps proving rather than something one new test claims. */
	installProgress?: PhpInstallProgressDto | null;
	installTotal?: number | null;
}): string {
	return render(LanguageRow, {
		props: {
			row: props.row,
			cataloged: props.cataloged ?? true,
			brewFound: props.brewFound ?? true,
			serviceState: props.serviceState ?? null,
			installing: props.installing ?? '',
			uninstalling: props.uninstalling ?? '',
			log: props.log ?? [],
			error: props.error ?? '',
			outcome: props.outcome ?? null,
			installProgress: props.installProgress ?? null,
			installTotal: props.installTotal ?? null,
			onInstall: () => {},
			onUninstall: () => {},
			onStart: () => {},
			onStop: () => {}
		}
	}).body;
}

/** Just the Uninstall button's own opening tag, so a `disabled` assertion can
 *  fail for the reason it names rather than matching some other button on the
 *  row (`renderRow` output now holds two). */
function uninstallTag(body: string, major: string): string {
	const match = body.match(new RegExp(`<button[^>]*data-testid="uninstall-${major}"[^>]*>`));
	if (!match) throw new Error(`expected an Uninstall button for ${major}`);
	return match[0];
}

describe('LanguageRow', () => {
	// `fullVersion` is asserted separately, in 'the pool status pill' describe
	// block below — it no longer has a cell of its own, only the
	// install-success message.
	it('shows the path and socket when installed', () => {
		const body = renderRow({
			row: r('8.3', true, {
				path: '/opt/homebrew/opt/php@8.3/sbin/php-fpm',
				socketPath: '/Users/x/.openvhost/run/php-fpm-8.3.sock',
				serviceId: 'php-fpm-8.3'
			})
		});
		expect(body).toContain('/opt/homebrew/opt/php@8.3');
		expect(body).toContain('php-fpm-8.3.sock');
		expect(body).not.toContain('data-testid="install-8.3"');
	});

	it('offers start and stop for an installed version', () => {
		// The install-to-running flow otherwise spans three pages.
		const body = renderRow({
			row: r('8.3', true, { serviceId: 'php-fpm-8.3' }),
			serviceState: { kind: 'stopped' }
		});
		expect(body).toContain('data-testid="start-php-fpm-8.3"');
	});

	it('offers no lifecycle control for a version that is not installed', () => {
		const body = renderRow({ row: r('8.4', false) });
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).not.toMatch(/data-testid="(start|stop)-/);
	});

	it('marks the recommended version', () => {
		expect(renderRow({ row: r('8.5', false, { recommended: true }) })).toMatch(/recommended/i);
		expect(renderRow({ row: r('8.1', false, { recommended: false }) })).not.toMatch(/recommended/i);
	});

	it('disables the install button while any install is running', () => {
		expect(renderRow({ row: r('8.4', false), installing: '8.3' })).toContain('disabled');
		expect(renderRow({ row: r('8.4', false), installing: '' })).not.toContain('disabled');
	});

	it('says plainly when brew succeeded but the version was not found', () => {
		// exitCode 0 with detected false. Without this the user presses Install
		// again and again with nothing to explain the silence.
		const body = renderRow({
			row: r('8.4', false),
			outcome: brewOutcome('8.4', 0, false)
		});
		expect(body).toMatch(/could not find|was not found/i);
		expect(body).not.toContain('data-testid="install-success-8.4"');
	});

	it('keeps the failure output on screen with its line breaks', () => {
		const body = renderRow({ row: r('8.4', false), error: 'Error: line 1\nline 2' });
		expect(body).toContain('line 2');
		expect(body).toMatch(/white-space:\s*pre-wrap/);
	});

	it('tells the user a pool still has to be created after a successful install', () => {
		const body = renderRow({
			row: r('8.4', true, { fullVersion: '8.4.12' }),
			outcome: brewOutcome('8.4', 0, true)
		});
		expect(body).toMatch(/apply/i);
	});

	// C1 (branch-review-fix-report.md): `install_php` returns `Ok(..)` with a
	// non-zero `exitCode` for a brew run that genuinely failed — that is an
	// OUTCOME to render, not a thrown `error`. Before the fix nothing rendered
	// it at all: `error` stays '' (nothing threw), `notFound` requires
	// `exitCode === 0`, and the log itself was hidden by an `isInstalling` gate
	// that had already flipped false by the time this outcome exists.
	it('renders a failed brew exit instead of nothing', () => {
		const body = renderRow({
			row: r('8.4', false),
			outcome: brewOutcome('8.4', 1, false)
		});
		expect(body).toMatch(/exited with code 1/i);
		expect(body).toMatch(/php 8\.4/i);
	});

	it('renders a killed-by-signal brew run (no exit code at all) as failed too', () => {
		const body = renderRow({
			row: r('8.4', false),
			outcome: brewOutcome('8.4', null, false)
		});
		expect(body).toMatch(/killed/i);
	});

	// The log must survive past the moment `installing` resets to '' — that
	// reset happens in `install()`'s `finally`, BEFORE the row re-renders with
	// the settled (possibly failed) outcome, which is exactly when the log is
	// most needed. `installing: ''` here reproduces that post-settle render.
	it('keeps a non-empty log on screen once the install has settled, not only while installing', () => {
		const body = renderRow({
			row: r('8.4', false),
			installing: '',
			log: [{ id: '8.4', tsMs: 1, level: 'info', line: 'Error: dependency foo failed to build' }],
			outcome: brewOutcome('8.4', 1, false)
		});
		expect(body).toContain('Error: dependency foo failed to build');
	});

	// The recommended row was the only one with a badge, so it was the only one
	// whose label had to share a fixed track — and "PHP 8.5" broke across two
	// lines while every other row looked correct. SSR cannot measure layout, so
	// these pin the two structural properties that make the wrap impossible.
	it('gives the version label its own element so it can refuse to wrap', () => {
		// As a bare text node the label was the only flexible thing in the cell,
		// so the badge's width came straight out of it.
		const body = renderRow({ row: r('8.5', false, { recommended: true }) });
		expect(body).toMatch(/<span[^>]*class="[^"]*\bversion\b[^"]*"[^>]*>\s*PHP 8\.5/);
	});

	it('renders the badge beside the label rather than inside it', () => {
		const body = renderRow({ row: r('8.5', false, { recommended: true }) });
		const label = body.indexOf('PHP 8.5');
		const badge = body.indexOf('Recommended');
		expect(label).toBeGreaterThan(-1);
		expect(badge).toBeGreaterThan(label);
		// Nothing may reopen the label's span between the two.
		expect(body.slice(label, badge)).not.toMatch(/<span[^>]*class="[^"]*version/);
	});

	it('renders nothing extra when there is no outcome at all', () => {
		const body = renderRow({ row: r('8.4', false) });
		expect(body).not.toMatch(/exited with code/i);
		expect(body).not.toMatch(/killed/i);
	});
});

describe('the pool control', () => {
	const installed = r('8.4', true, { serviceId: 'php-fpm-8.4' });
	const notInstalled = r('8.4', false);

	it('offers Start when the pool is stopped', () => {
		const out = renderRow({ row: installed, serviceState: { kind: 'stopped' } });
		expect(out).toContain('data-testid="start-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="retry-php-fpm-8.4"');
	});

	it('offers Stop while running or still starting', () => {
		// `starting` gets Stop, not nothing: a start that hangs has to be
		// interruptible or the only way out is quitting the app.
		for (const kind of ['running', 'starting'] as const) {
			const out = renderRow({ row: installed, serviceState: { kind } });
			expect(out, kind).toContain('data-testid="stop-php-fpm-8.4"');
		}
	});

	it('offers Retry after a failure, not another Start', () => {
		// The whole point. `failed` used to collapse onto `stopped`, so the row
		// showed a Start button identical to the one the user had just pressed.
		const out = renderRow({
			row: installed,
			serviceState: { kind: 'failed', exit: 1, stderrTail: ['boom'] }
		});
		expect(out).toContain('data-testid="retry-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="start-php-fpm-8.4"');
	});

	it('renders no control at all while the state is unknown', () => {
		// `null` is the first frame of every visit. A Start button here asserts
		// the pool is stopped before the supervisor has answered.
		const out = renderRow({ row: installed, serviceState: null });
		expect(out).not.toContain('data-testid="start-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="stop-php-fpm-8.4"');
		expect(out).not.toContain('data-testid="retry-php-fpm-8.4"');
	});

	it('still offers Install first when PHP is not installed', () => {
		// Spec §5.1: the not-installed branch outranks every service-state
		// branch. Reversing them would replace Install with nothing on exactly
		// the rows a new user needs it.
		const out = renderRow({ row: notInstalled, serviceState: null });
		expect(out).toContain('data-testid="install-8.4"');
	});
});

describe('a failed pool', () => {
	const installed = r('8.4', true, { serviceId: 'php-fpm-8.4' });

	it("shows php-fpm's own words, not just a Retry button", () => {
		// Asserting on CONTENT, not on the presence of a block: an empty <pre>
		// would satisfy a weaker assertion and tell the user nothing about why
		// their pool did not start.
		const out = renderRow({
			row: installed,
			serviceState: {
				kind: 'failed',
				exit: 78,
				stderrTail: ['[08-Jul-2026 10:00:00] ERROR: unable to bind listening socket']
			}
		});
		expect(out).toContain('unable to bind listening socket');
		expect(out).toContain('data-testid="pool-failed-php-fpm-8.4"');
	});

	it('says a failure happened even when php-fpm said nothing', () => {
		// A pool killed by a signal has an empty tail. Rendering only the <pre>
		// would leave a failed row looking identical to a healthy one.
		const out = renderRow({
			row: installed,
			serviceState: { kind: 'failed', exit: null, stderrTail: [] }
		});
		expect(out).toContain('data-testid="pool-failed-php-fpm-8.4"');
		expect(out).toContain('PHP 8.4');
	});

	it('keeps a brew install failure and a pool failure apart', () => {
		// The two render in different places and mean different things. A row
		// showing both must show each in its own block, not one in place of the
		// other.
		const out = renderRow({
			row: installed,
			serviceState: { kind: 'failed', exit: 78, stderrTail: ['pool is broken'] },
			outcome: brewOutcome('8.4', 1, false)
		});
		expect(out).toContain('brew exited with code 1');
		expect(out).toContain('pool is broken');
	});
});

describe('the pool status pill', () => {
	const installed = r('8.4', true, { serviceId: 'php-fpm-8.4' });

	it('names the state for a pool the supervisor knows about', () => {
		const out = renderRow({ row: installed, serviceState: { kind: 'running' } });
		expect(out).toContain('data-testid="lang-pill-8.4"');
		expect(out).toContain('running');
	});

	it('renders nothing while the state is unknown', () => {
		// Same rule the control follows: an absent snapshot is not a state.
		const out = renderRow({ row: installed, serviceState: null });
		expect(out).not.toContain('data-testid="lang-pill-8.4"');
	});

	it('drops the full-version column that never had anything to show', () => {
		// It rendered an em dash on EVERY row, installed or not, because no
		// patch-level prober exists. To a reader that is not absent data, it is
		// data that failed to load.
		//
		// COUNTING the cells, not pattern-matching one of them. Two things make
		// the obvious assertions wrong here:
		//
		//  - `not.toContain('—')` would fail for reasons unrelated to this
		//    column, because the path and socket cells render an em dash too
		//    when their values are null.
		//  - the deleted test's `/<div class="meta mono[^"]*">/` regex is not
		//    specific to the version cell either. It looks like it is, because
		//    path and socket carry a `title` attribute after their class — but
		//    that attribute is `title={row.path ?? undefined}`, and Svelte OMITS
		//    an attribute whose value is `undefined`. With a null path the cell
		//    renders `<div class="meta mono path">` and the regex matches it.
		//
		// The count is unambiguous under every fixture: three `meta mono` cells
		// before (version, path, socket), two after. It is also the finding the
		// deleted test recorded: falling back to `row.major` in the version cell
		// printed the major a SECOND time right next to the "PHP 8.3" heading,
		// implying a patch level had been fetched when none had — which is why
		// this column had nothing worth showing in the first place.
		const out = renderRow({ row: { ...installed, fullVersion: null }, serviceState: null });
		const cells = out.match(/<div class="meta mono/g) ?? [];
		expect(cells).toHaveLength(2);
	});

	it('still names the full version in the install-success message', () => {
		// The FIELD stays; only the column goes. This message is where it is
		// genuinely useful and degrades honestly to the major.
		const out = renderRow({
			row: { ...installed, fullVersion: '8.4.13' },
			serviceState: null,
			outcome: brewOutcome('8.4', 0, true)
		});
		expect(out).toContain('8.4.13');
	});
});

// Package-uninstall design D6: an installed major gets an Uninstall action.
// Every assertion below is about the ACTION — the confirmation it opens is
// `UninstallDialog.svelte`'s own test file, and the copy is
// `uninstall.derive.test.ts`'s.
describe('LanguageRow — the Uninstall action', () => {
	it('offers Uninstall for an installed major', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: { kind: 'stopped' } });
		expect(body).toContain('data-testid="uninstall-8.3"');
	});

	// The Install and Uninstall branches are mutually exclusive by construction;
	// this pins that, because a row offering to uninstall something that was
	// never installed would call a command that can only fail.
	it('offers no Uninstall for a major that is not installed', () => {
		const body = renderRow({ row: r('8.3', false) });
		expect(body).toContain('data-testid="install-8.3"');
		expect(body).not.toContain('data-testid="uninstall-8.3"');
	});

	// An installed major whose pool has no supervisor row yet (or whose
	// snapshot has not arrived) still gets the action: that is exactly the
	// state a user most wants out of.
	it('offers Uninstall even with no service state for the row', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: null });
		expect(body).toContain('data-testid="uninstall-8.3"');
	});

	it('keeps the Start/Stop control alongside it', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: { kind: 'stopped' } });
		expect(body).toContain('data-testid="start-php-fpm-8.3"');
		expect(body).toContain('data-testid="uninstall-8.3"');
	});

	it('is enabled when nothing is in flight', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: null });
		expect(uninstallTag(body, '8.3')).not.toContain('disabled');
	});

	// One `InstallLock` serializes brew installs and brew uninstalls, so an
	// uninstall pressed during an install would only queue on a mutex with no
	// feedback — the same reasoning that already disables "Check again".
	it('is disabled while an install is running', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: null, installing: '8.4' });
		expect(uninstallTag(body, '8.3')).toContain('disabled');
	});

	it('is disabled while ANOTHER major is being uninstalled', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: null, uninstalling: '8.4' });
		expect(uninstallTag(body, '8.3')).toContain('disabled');
		// …and says nothing about itself: this row is not the one going away.
		expect(uninstallTag(body, '8.3')).not.toContain('Uninstalling');
	});

	it('is disabled and says what it is doing while THIS major is uninstalled', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: null, uninstalling: '8.3' });
		expect(uninstallTag(body, '8.3')).toContain('disabled');
		expect(body).toContain('Uninstalling…');
	});

	it('names the version in its accessible label', () => {
		const body = renderRow({ row: r('8.3', true), serviceState: null });
		expect(uninstallTag(body, '8.3')).toContain('aria-label="Uninstall PHP 8.3"');
	});
});

// The branch review's MEDIUM. `php_rows` lists an installed major from OUTSIDE
// the catalogue — a hand-installed `php@7.4`, or one a later catalogue drops —
// with `installed: true`, so it does not vanish from the page while it is still
// serving sites. The row then offered Uninstall, `Target::parse` refused the
// major it was never going to accept, and the dialog opened only to say "This
// could not be checked, so nothing has been changed."
describe('LanguageRow — an installed major this build does not manage', () => {
	it('offers no Uninstall, because the command could only refuse it', () => {
		const body = renderRow({ row: r('7.4', true), cataloged: false, serviceState: null });
		expect(body).not.toContain('data-testid="uninstall-7.4"');
	});

	// Absent affordance, present explanation — the one thing this page has got
	// wrong twice before (C2/C3). The note has to name the real command, or it
	// is not a next action.
	it('says why, and hands over the command that does work', () => {
		const body = renderRow({ row: r('7.4', true), cataloged: false, serviceState: null });
		expect(body).toContain('data-testid="php-out-of-catalogue-7.4"');
		expect(body).toContain('brew uninstall php@7.4');
		expect(body).toContain('Check again');
	});

	// It is still a real, supervised pool. Removing its lifecycle control would
	// trade a dead button for a dead row.
	it('keeps its Start/Stop control', () => {
		const body = renderRow({
			row: r('7.4', true, { serviceId: 'php-fpm-7.4' }),
			cataloged: false,
			serviceState: { kind: 'stopped' }
		});
		expect(body).toContain('data-testid="start-php-fpm-7.4"');
	});

	// The guard is `installed && cataloged`, and both halves have to matter. A
	// managed major keeps its button; an unmanaged one loses it; and the note
	// belongs to the unmanaged row only.
	it('leaves a managed major untouched: button present, note absent', () => {
		const body = renderRow({ row: r('8.3', true), cataloged: true, serviceState: null });
		expect(body).toContain('data-testid="uninstall-8.3"');
		expect(body).not.toContain('data-testid="php-out-of-catalogue-8.3"');
	});

	// A not-installed row is a catalogue row by construction (only installed
	// majors can fall outside it), so it must not sprout the note.
	it('says nothing about the catalogue for a version that is not installed', () => {
		const body = renderRow({ row: r('8.4', false), cataloged: true });
		expect(body).not.toContain('data-testid="php-out-of-catalogue-8.4"');
		expect(body).toContain('data-testid="install-8.4"');
	});
});

// Design D3. Provenance, not health — which install put these binaries here.
describe('LanguageRow — where a runtime came from', () => {
	it('names the exact patch level for a runtime OpenVHost installed', () => {
		// Slice 5B's whole asymmetry, finally spent: the version is a directory
		// name in our own tree, so nothing was executed to learn it.
		const body = renderRow({
			row: r('8.4', true, { source: { kind: 'packaged', version: '8.4.24' } })
		});
		expect(body).toContain('data-testid="php-source-8.4"');
		expect(body).toContain('OpenVHost 8.4.24');
	});

	// Deliberately unlike MysqlRow's and WebServerRow's otherwise identical
	// chips, which DO label their brewed runtimes. Spec §5 says Homebrew rows
	// carry none, and §8.6 is why it is binding: a chip on all five rows would
	// be a visible change to every real machine today.
	it('gives a Homebrew keg no badge at all', () => {
		const body = renderRow({ row: r('8.3', true, { source: { kind: 'homebrew' } }) });
		expect(body).not.toContain('data-testid="php-source-8.3"');
	});

	it('gives a major with nothing installed no badge', () => {
		expect(renderRow({ row: r('8.4', false) })).not.toContain('data-testid="php-source-8.4"');
	});

	// The badge is provenance, not a second status, so it must coexist with a
	// failed pill rather than reading as a contradiction beside one. Same
	// property `webserver.panel.test.ts` pins for the identical chip.
	it('coexists with a failed status pill rather than reading as a contradiction', () => {
		const body = renderRow({
			row: r('8.4', true, { source: { kind: 'packaged', version: '8.4.24' } }),
			serviceState: { kind: 'failed', exit: 1, stderrTail: ['boom'] }
		});
		expect(body).toContain('data-testid="php-source-8.4"');
		expect(body).toContain('data-testid="lang-pill-8.4"');
	});

	// A status pill is `class="pill pill-<kind>"` with a `.dot` child; this is a
	// `.badge`, with neither. Structural, not a colour assertion — SSR has no
	// stylesheet, so the palette is checked by reading the stylesheet below.
	//
	// The class list is matched apart from Svelte's own per-component scope
	// class (`svelte-<hash>`), which is appended to every styled element. That
	// hash is also why this badge is a third literal copy of MySQL's CSS rather
	// than a shared component: extracting one would move the hash, and so change
	// the rendered markup of two already-shipped rows.
	it('is not a status pill, structurally', () => {
		const body = renderRow({
			row: r('8.4', true, { source: { kind: 'packaged', version: '8.4.24' } }),
			serviceState: { kind: 'running' }
		});
		const badge = body.match(/<span[^>]*data-testid="php-source-8\.4"[^>]*>/)?.[0] ?? '';
		const classes = (badge.match(/class="([^"]*)"/)?.[1] ?? '')
			.split(/\s+/)
			.filter((c) => !c.startsWith('svelte-'));
		expect(classes).toEqual(['badge', 'source', 'source-packaged']);
		expect(badge).not.toMatch(/\bpill\b/);
	});
});

// Design D2, per row. `brewFound` is an INPUT to a per-major answer here; the
// page no longer answers it for everyone at once.
describe('LanguageRow — whether Install can actually work', () => {
	const available = { kind: 'available', version: '8.4.24' } as const;
	const awaiting = { kind: 'awaitingRelease', tag: 'php-8.4.24' } as const;
	const unavailable = { kind: 'unavailable', target: 'macos-arm64' } as const;

	// §8.6 — every real machine today. All three offer states, with Homebrew
	// present, must render exactly what they rendered before this slice: an
	// Install button and not one word more.
	it('changes nothing on a machine with Homebrew, in any offer state', () => {
		for (const offer of [available, awaiting, unavailable]) {
			const body = renderRow({ row: r('8.4', false, { offer }), brewFound: true });
			expect(body, offer.kind).toContain('data-testid="install-8.4"');
			expect(body, offer.kind).not.toContain('data-testid="php-no-route-8.4"');
			expect(body, offer.kind).not.toMatch(/needs Homebrew/i);
		}
	});

	// §8.5 as corrected. On this Apple Silicon machine 8.4 is `AwaitingRelease`
	// today AND has a working Homebrew Install button; the first draft of the
	// spec would have deleted it. What AwaitingRelease withholds is the PACKAGED
	// route, never the Homebrew one.
	it('keeps the Homebrew Install button on an AwaitingRelease row', () => {
		const body = renderRow({ row: r('8.4', false, { offer: awaiting }), brewFound: true });
		expect(body).toContain('data-testid="install-8.4"');
	});

	// §8.2b's installable row: our own bytes, so Homebrew is irrelevant to it.
	it('offers Install for an Available major even with no Homebrew', () => {
		const body = renderRow({ row: r('8.4', false, { offer: available }), brewFound: false });
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).not.toContain('data-testid="php-no-route-8.4"');
	});

	// §8.2b's other rows. Absent affordance, PRESENT EXPLANATION — this page has
	// shipped the other shape twice (C2/C3) and both times the user was left
	// pressing nothing and learning nothing. `install_php` here would fail at
	// `find_brew()` before spawning anything, so the button could only ever
	// produce "Homebrew was not found".
	it('replaces Install with a per-row explanation when nothing can install it', () => {
		const body = renderRow({ row: r('8.1', false, { offer: unavailable }), brewFound: false });
		expect(body).not.toContain('data-testid="install-8.1"');
		expect(body).toContain('data-testid="php-no-route-8.1"');
		expect(body).toContain('Homebrew');
		expect(body).toContain('macos-arm64');
	});

	// §8.5's other half: the row names the tag a maintainer has to publish,
	// because the next action genuinely is not the user's.
	it('names the unpublished release on an AwaitingRelease row with no Homebrew', () => {
		const body = renderRow({ row: r('8.4', false, { offer: awaiting }), brewFound: false });
		expect(body).not.toContain('data-testid="install-8.4"');
		expect(body).toContain('php-8.4.24');
		expect(body).toMatch(/maintainer/i);
	});

	// An installed row has nothing to install, so the note must not appear
	// beside its Start/Stop and Uninstall controls — that would read as though
	// the PHP it is running were somehow unavailable.
	it('says nothing about routes on a row that is already installed', () => {
		const body = renderRow({
			row: r('8.3', true, { offer: unavailable }),
			brewFound: false,
			serviceState: { kind: 'running' }
		});
		expect(body).not.toContain('data-testid="php-no-route-8.3"');
		expect(body).toContain('data-testid="stop-php-fpm-8.3"');
	});
});

// SSR renders markup with no stylesheet attached, so the two things that make a
// badge a badge — that it is MySQL's chip and not a lookalike, and that it
// cannot be mistaken for a status — are read straight off the stylesheets here.
// Both are cheap, and 4C's review checked the first one by hand; this makes the
// third copy unable to drift silently instead.
describe('the source badge, as CSS', () => {
	const styleOf = (rel: string) =>
		readFileSync(new URL(rel, import.meta.url), 'utf8').match(/<style>([\s\S]*?)<\/style>/)?.[1] ??
		'';

	/** One rule's declarations, comments stripped and whitespace flattened, so
	 *  two copies compare equal iff they SET THE SAME THINGS — a differing
	 *  comment is not a difference, a differing declaration is. */
	function declarations(css: string, selector: string): string {
		const escaped = selector.replace(/\./g, '\\.');
		const rule = css.match(new RegExp(`\\n\\t${escaped}\\s*\\{([^}]*)\\}`));
		if (rule === null) throw new Error(`no \`${selector}\` rule found`);
		return rule[1]
			.replace(/\/\*[\s\S]*?\*\//g, '')
			.split(';')
			.map((d) => d.trim().replace(/\s+/g, ' '))
			.filter((d) => d !== '')
			.join('; ');
	}

	// Design D3: reuse MySQL's existing chip rather than a lookalike. Svelte
	// scopes styles per component, so three components can only share one look
	// by holding three literal copies — and three literal copies can only be
	// kept honest by comparing them.
	it('sets exactly what MysqlRow.svelte’s identical chip sets', () => {
		const row = styleOf('./LanguageRow.svelte');
		const mysql = styleOf('./MysqlRow.svelte');
		for (const selector of ['.badge', '.badge.source', '.badge.source-packaged']) {
			expect(declarations(row, selector), selector).toBe(declarations(mysql, selector));
		}
	});

	// Without this the assertion above passes vacuously if a selector is renamed
	// on BOTH sides — and, worse, `declarations` returning '' for both would read
	// as agreement.
	it('actually found declarations to compare', () => {
		const row = styleOf('./LanguageRow.svelte');
		expect(declarations(row, '.badge.source-packaged')).toContain('--vh-link');
		expect(declarations(row, '.badge').split(';').length).toBeGreaterThan(5);
	});

	// "Cannot read as a status beside one" made mechanical: the two palettes are
	// disjoint. StatusPill paints run/start/fail/stop and nothing else; the
	// packaged chip paints `--vh-link` and nothing else. A future edit that
	// reached for a state colour here would put a green or red chip next to a
	// pill of a different colour, which is a contradiction the user has to
	// resolve.
	it('shares no colour token with StatusPill', () => {
		const badge = [
			declarations(styleOf('./LanguageRow.svelte'), '.badge'),
			declarations(styleOf('./LanguageRow.svelte'), '.badge.source'),
			declarations(styleOf('./LanguageRow.svelte'), '.badge.source-packaged')
		].join('; ');
		const pill = styleOf('./StatusPill.svelte');

		for (const token of ['--vh-run', '--vh-start', '--vh-fail', '--vh-stop']) {
			expect(badge, token).not.toContain(token);
			expect(pill, `StatusPill should still use ${token}`).toContain(token);
		}
		expect(badge).toContain('--vh-link');
		expect(pill).not.toContain('--vh-link');
	});
});

// The wrapped narrow layout is a CSS container query and there is no layout engine here, so
// nothing below asserts that anything WRAPS — that was measured in a browser and the numbers
// are in the PR. What is worth guarding is the two ways it can rot silently, both of which
// end with a control off-screen and every test still green.
describe('the narrow-width layout', () => {
	const styleOf = (rel: string) =>
		readFileSync(new URL(rel, import.meta.url), 'utf8').match(/<style>([\s\S]*?)<\/style>/)?.[1] ??
		'';
	const row = styleOf('./LanguageRow.svelte');

	it('queries a container the Languages page actually declares', () => {
		// The query names a container only the page declares. Drop `container-name` there and
		// every rule below `@container` stops matching — no error, no failing render, no
		// visual difference until someone narrows the window and loses Uninstall again.
		const queried = row.match(/@container\s+([\w-]+)\s*\(/)?.[1];
		expect(queried, 'LanguageRow should query a NAMED container').toBeDefined();

		const page = styleOf('../../routes/languages/+page.svelte');
		expect(page).toMatch(
			new RegExp(`container-name:\\s*${queried}\\b|container:\\s*${queried}\\b`)
		);
		expect(page).toMatch(/container-type:\s*inline-size|container:\s*[\w-]+\s*\/\s*inline-size/);
	});

	it('keeps the one-line cost below the width at which the row wraps', () => {
		// Widen a track and the row costs more than the width at which it wraps, so it goes
		// back to overflowing `.panel`'s `overflow: hidden`. Re-derived from the stylesheet.
		const tokens = readFileSync(new URL('../styles/tokens.css', import.meta.url), 'utf8');
		const resolve = (decl: string) =>
			/^\d+px$/.test(decl)
				? Number(decl.slice(0, -2))
				: Number(
						tokens.match(
							new RegExp(`${decl.match(/var\((--[\w-]+)\)/)?.[1] ?? '\0'}:\\s*(\\d+)px`)
						)?.[1]
					);

		// Measured in a browser at the widest the action column ever gets — `Installing…`
		// alone, which beats Stop + `Uninstalling…`. It cannot be read off the stylesheet:
		// unlike the Sites row, this `.row-actions` has no `min-width` floor, so the number
		// lives here with its provenance rather than pretending to be derived.
		const WIDEST_ACTION_COLUMN_PX = 174;

		const tracks = (row.match(/grid-template-columns:\s*([^;]+);/)?.[1] ?? '')
			.replace(/minmax\(\s*(\d+px)[^)]*\)/g, '$1')
			.trim()
			.split(/\s+/);
		const floors = tracks.filter((t) => /^\d+px$/.test(t)).map((t) => Number(t.slice(0, -2)));
		const rowRule = row.match(/\n\t\.row\s*\{[\s\S]*?\}/)?.[0] ?? '';
		const gap = resolve(rowRule.match(/\bgap:\s*([^;]+);/)?.[1]?.trim() ?? '');
		const padX = resolve(rowRule.match(/padding:\s*\S+\s+([^;]+);/)?.[1]?.trim() ?? '');
		const wrapsBelow = Number(row.match(/@container\s+[\w-]+\s*\(width\s*<\s*(\d+)px\)/)?.[1]);

		// Without this a regex that stops matching yields 0 and the assertion below passes for
		// the wrong reason — a test that cannot fail, dressed as one that can.
		expect({ floors: floors.length, autos: tracks.filter((t) => t === 'auto').length }).toEqual({
			floors: tracks.length - 1,
			autos: 1
		});
		for (const [name, n] of Object.entries({ gap, padX, wrapsBelow })) {
			expect(n, `${name} should have been read out of the stylesheet`).toBeGreaterThan(0);
		}

		const oneLineCost =
			floors.reduce((a, b) => a + b, 0) +
			WIDEST_ACTION_COLUMN_PX +
			(tracks.length - 1) * gap +
			2 * padX;
		expect(
			wrapsBelow,
			`the row costs ${oneLineCost}px on one line but only wraps below ${wrapsBelow}px — ` +
				`raise the @container threshold above ${oneLineCost}`
		).toBeGreaterThanOrEqual(oneLineCost);
	});

	// ------------------------------------------------------------------
	// The packaged route's progress, wired in the 5C fix wave.
	//
	// Vacuity: every "renders it" test asserts a testid the fixture does not
	// otherwise produce, and each is paired with a negative case on the same
	// testid. Proven by mutation against the row's one guard, once per half:
	// narrowing `{#if isInstalling && installProgress !== null}` to
	// `{#if isInstalling}` reddened 'paints nothing at all while a Homebrew
	// install runs' (and, at the route level, its page-wide twin); narrowing it
	// to `{#if installProgress !== null}` reddened 'stops painting the pipeline
	// once the run settles' and 'paints nothing while another major is the one
	// installing'. Neither half is redundant, which is why the condition is
	// written once inline rather than behind a named derived that could go
	// quietly half-dead.
	// ------------------------------------------------------------------
	describe('the packaged install pipeline', () => {
		const notInstalled = () => r('8.4', false, { offer: { kind: 'available', version: '8.4.24' } });

		// SPEC §8.6, and the single most important assertion in this block. A brew
		// install sets `installing` to this major and emits no progress event
		// EVER — `php-install-progress` has one emitter, `run_package_install` —
		// so a gate on `installing` alone would put a download line under every
		// Install press on every real machine today.
		it('paints nothing at all while a Homebrew install runs', () => {
			const body = renderRow({
				row: notInstalled(),
				installing: '8.4',
				installProgress: null
			});
			expect(body).not.toContain('php-install-progress-8.4');
			expect(body).not.toContain('php-install-bar-8.4');
			expect(body).not.toContain('progressbar');
			// And the button still says what it always said mid-install.
			expect(body).toContain('Installing…');
		});

		it('names the step the packaged pipeline is on', () => {
			const body = renderRow({
				row: notInstalled(),
				installing: '8.4',
				installProgress: { kind: 'verified' }
			});
			expect(body).toContain('php-install-progress-8.4');
			expect(body).toMatch(/checksum/i);
		});

		// The bar is drawn only where a percentage is honest: bytes arriving
		// against a length the server actually declared.
		it('draws a bar with the real percentage while bytes are arriving', () => {
			const body = renderRow({
				row: notInstalled(),
				installing: '8.4',
				installProgress: { kind: 'downloaded', bytes: 512 },
				installTotal: 2048
			});
			expect(body).toContain('php-install-bar-8.4');
			expect(body).toContain('aria-valuenow="25"');
			expect(body).toContain('width: 25%');
		});

		it('draws no bar when the server declared no length, rather than one on a guess', () => {
			const body = renderRow({
				row: notInstalled(),
				installing: '8.4',
				installProgress: { kind: 'downloaded', bytes: 512 },
				installTotal: null
			});
			// The line still renders — it just reads "so far" instead of a share.
			expect(body).toContain('php-install-progress-8.4');
			expect(body).toContain('so far');
			expect(body).not.toContain('php-install-bar-8.4');
		});

		it('draws no bar for the steps that are moments rather than durations', () => {
			for (const progress of [
				{ kind: 'started', total: 4096 },
				{ kind: 'verified' },
				{ kind: 'extracted' },
				{ kind: 'linked' }
			] satisfies PhpInstallProgressDto[]) {
				const body = renderRow({
					row: notInstalled(),
					installing: '8.4',
					installProgress: progress,
					installTotal: 4096
				});
				expect(body, progress.kind).toContain('php-install-progress-8.4');
				expect(body, progress.kind).not.toContain('php-install-bar-8.4');
			}
		});

		// `install()` resets `installing` to '' before the settled outcome renders,
		// so without this half a finished "Extracted…" would sit above the success
		// message for the rest of the page's life.
		it('stops painting the pipeline once the run settles', () => {
			const body = renderRow({
				row: notInstalled(),
				installing: '',
				installProgress: { kind: 'linked' }
			});
			expect(body).not.toContain('php-install-progress-8.4');
			expect(body).not.toContain('php-install-bar-8.4');
		});

		// Progress arriving for a DIFFERENT row must not paint this one. The
		// caller already scopes it (`LanguagesStore.progressFor`), and this is the
		// row's own half of that contract: it paints only while IT is the major
		// installing.
		it('paints nothing while another major is the one installing', () => {
			const body = renderRow({
				row: notInstalled(),
				installing: '8.3',
				installProgress: { kind: 'verified' }
			});
			expect(body).not.toContain('php-install-progress-8.4');
		});
	});
});
