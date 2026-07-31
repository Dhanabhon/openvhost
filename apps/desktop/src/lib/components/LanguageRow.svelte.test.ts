// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), same approach as ApplyDialog.svelte.test.ts.
// WHAT THIS FILE CANNOT COVER: no DOM, so click handlers are exercised only through
// the `onclick` prop wiring Button.svelte already covers, not by simulating a click.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LanguageRow from './LanguageRow.svelte';
import type { InstallOutcomeDto, PhpRuntimeDto, ServiceStatus } from '$lib/ipc';
import type { UiLog } from '$lib/languages.svelte';

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
		recommended: false,
		fullVersion: null,
		path: installed ? `/opt/homebrew/opt/php@${major}/sbin/php-fpm` : null,
		socketPath: installed ? `/Users/x/.openvhost/run/php-fpm-${major}.sock` : null,
		serviceId: installed ? `php-fpm-${major}` : null,
		...overrides
	};
}

function renderRow(props: {
	row: PhpRuntimeDto;
	serviceState?: ServiceStatus['state'] | null;
	installing?: string;
	uninstalling?: string;
	log?: UiLog[];
	error?: string;
	outcome?: InstallOutcomeDto | null;
}): string {
	return render(LanguageRow, {
		props: {
			row: props.row,
			serviceState: props.serviceState ?? null,
			installing: props.installing ?? '',
			uninstalling: props.uninstalling ?? '',
			log: props.log ?? [],
			error: props.error ?? '',
			outcome: props.outcome ?? null,
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
			outcome: { major: '8.4', exitCode: 0, detected: false }
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
			outcome: { major: '8.4', exitCode: 0, detected: true }
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
			outcome: { major: '8.4', exitCode: 1, detected: false }
		});
		expect(body).toMatch(/exited with code 1/i);
		expect(body).toMatch(/php 8\.4/i);
	});

	it('renders a killed-by-signal brew run (no exit code at all) as failed too', () => {
		const body = renderRow({
			row: r('8.4', false),
			outcome: { major: '8.4', exitCode: null, detected: false }
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
			outcome: { major: '8.4', exitCode: 1, detected: false }
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
			outcome: { major: '8.4', exitCode: 1, detected: false }
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
			outcome: { major: '8.4', exitCode: 0, detected: true }
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
