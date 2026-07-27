// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), same approach as ApplyDialog.svelte.test.ts.
// WHAT THIS FILE CANNOT COVER: no DOM, so click handlers are exercised only through
// the `onclick` prop wiring Button.svelte already covers, not by simulating a click.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LanguageRow from './LanguageRow.svelte';
import type { InstallOutcomeDto, PhpRuntimeDto } from '$lib/ipc';
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
	running?: boolean;
	installing?: string;
	log?: UiLog[];
	error?: string;
	outcome?: InstallOutcomeDto | null;
}): string {
	return render(LanguageRow, {
		props: {
			row: props.row,
			running: props.running ?? false,
			installing: props.installing ?? '',
			log: props.log ?? [],
			error: props.error ?? '',
			outcome: props.outcome ?? null,
			onInstall: () => {},
			onStart: () => {},
			onStop: () => {}
		}
	}).body;
}

describe('LanguageRow', () => {
	it('shows the version, path and socket when installed', () => {
		const body = renderRow({
			row: r('8.3', true, {
				fullVersion: '8.3.14',
				path: '/opt/homebrew/opt/php@8.3/sbin/php-fpm',
				socketPath: '/Users/x/.openvhost/run/php-fpm-8.3.sock',
				serviceId: 'php-fpm-8.3'
			})
		});
		expect(body).toContain('8.3.14');
		expect(body).toContain('/opt/homebrew/opt/php@8.3');
		expect(body).toContain('php-fpm-8.3.sock');
		expect(body).not.toContain('data-testid="install-8.3"');
	});

	// The "one line" branch-review-fix-report.md finding: `fullVersion` is
	// `null` for every row in production (no patch-level prober exists yet), so
	// falling back to `row.major` in the version column printed the major a
	// SECOND time right next to the "PHP 8.3" heading — implying a patch level
	// had been fetched when it had not. An em dash is the honest "unknown".
	it('shows an em dash rather than repeating the major when the patch level is unknown', () => {
		const body = renderRow({ row: r('8.3', true, { fullVersion: null }) });
		// The version column is the FIRST `meta mono` cell (path/socket follow with
		// their own extra classes) — matched by class rather than an exact literal
		// string so a scoped-style hash suffix on the class attribute cannot break
		// this assertion for reasons unrelated to what it is pinning.
		const versionCell = body.match(/<div class="meta mono[^"]*">([^<]*)<\/div>/);
		expect(versionCell?.[1]).toBe('—');
	});

	it('offers start and stop for an installed version', () => {
		// The install-to-running flow otherwise spans three pages.
		const body = renderRow({ row: r('8.3', true, { serviceId: 'php-fpm-8.3' }), running: false });
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

	it('renders nothing extra when there is no outcome at all', () => {
		const body = renderRow({ row: r('8.4', false) });
		expect(body).not.toMatch(/exited with code/i);
		expect(body).not.toMatch(/killed/i);
	});
});
