// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import {
	statusFor,
	hotReloadLabel,
	nginxSourceBadge,
	startStopFor,
	stoppedPoolsFor
} from './webservers.derive';
import type { NginxRuntimeSourceDto, ServiceStatus, SiteDto } from '$lib/ipc';
// NOTE: `ServiceState` is NOT exported from `$lib/ipc` (only `ServiceStateEvent`
// and `ServiceStatus`), and `StatusPill` takes `kind: StateKind` rather than a
// state object — so `statusFor` returns the kind STRING, indexed off the
// exported `ServiceStatus` type. That satisfies both without touching the barrel.

const svc = (id: string, kind: 'running' | 'stopped'): ServiceStatus => ({
	id,
	displayName: id,
	endpoint: null,
	pid: kind === 'running' ? 1 : null,
	state: { kind }
});

describe('statusFor', () => {
	it('finds the supervised service a row correlates with', () => {
		expect(statusFor([svc('nginx', 'running')], 'nginx')).toBe('running');
	});

	// Apache has no supervised service, so a row with no serviceId must render
	// "no status" rather than borrowing another row's state.
	it('is null for a row that is not a supervised service', () => {
		expect(statusFor([svc('nginx', 'running')], null)).toBeNull();
	});

	it('is null when the service is not in the snapshot yet', () => {
		expect(statusFor([], 'nginx')).toBeNull();
	});
});

describe('hotReloadLabel', () => {
	it('states support plainly in both directions', () => {
		expect(hotReloadLabel(true)).toBe('Supported');
		expect(hotReloadLabel(false)).toBe('Not supported');
	});
});

describe('startStopFor', () => {
	it('renders no control at all while the state is unknown', () => {
		// `statusFor` returns null for the first frame of EVERY visit — the route
		// fires load() and the shared subscription resolves after the first paint.
		// A Start button here would be the page asserting nginx is stopped before
		// it has asked, and the user would be one click from starting something
		// whose state they were never shown.
		expect(startStopFor(null, true)).toEqual({ kind: 'none' });
		expect(startStopFor(null, false)).toEqual({ kind: 'none' });
		// Same rule when config existence is ALSO unknown: two unknowns do not
		// combine into a control the user can act on.
		expect(startStopFor(null, null)).toEqual({ kind: 'none' });
	});

	it('offers Start when stopped with a config to start against', () => {
		expect(startStopFor('stopped', true)).toEqual({
			kind: 'start',
			disabled: false,
			reason: ''
		});
	});

	it('disables Start with a reason when there is no config yet', () => {
		// nginx spawns with `-c <config>`; without the file it exits immediately.
		expect(startStopFor('stopped', false)).toEqual({
			kind: 'start',
			disabled: true,
			reason: 'No config generated yet — apply your changes first.'
		});
	});

	it('offers Retry after a failure, and does not re-disable it', () => {
		// A failed service HAS been started, so a config existed at least once.
		// Disabling Retry on a stale `configExists: false` would strand the user
		// on a row whose own error text is telling them to try again.
		expect(startStopFor('failed', true)).toEqual({ kind: 'retry' });
		expect(startStopFor('failed', false)).toEqual({ kind: 'retry' });
	});

	it('enables Start with no reason when config existence could not be determined', () => {
		// `null` means the backend's stat itself failed (a permission error, a
		// dangling symlink, ...) — that is NOT the same fact as "confirmed
		// absent", and must not be treated as one. Disabling Start here would
		// repeat NO_CONFIG_REASON for a cause that has nothing to do with Apply,
		// and re-running Apply cannot fix it. The honest move is to let the user
		// try; a genuine failure surfaces as nginx's own stderr on the row.
		expect(startStopFor('stopped', null)).toEqual({
			kind: 'start',
			disabled: false,
			reason: ''
		});
	});

	it('offers Stop while running or still starting', () => {
		// `starting` gets Stop, not nothing: a start that hangs must be
		// interruptible, or the only way out is quitting the app.
		expect(startStopFor('running', true)).toEqual({ kind: 'stop' });
		expect(startStopFor('starting', true)).toEqual({ kind: 'stop' });
	});

	it('never disables Stop on a missing config', () => {
		// The process is running. Whether a file is on disk has no bearing on
		// whether the user may stop it.
		expect(startStopFor('running', false)).toEqual({ kind: 'stop' });
		expect(startStopFor('starting', false)).toEqual({ kind: 'stop' });
	});
});

describe('stoppedPoolsFor', () => {
	const site = (phpVersion: string, enabled = true): SiteDto => ({
		id: `s-${phpVersion}-${enabled}`,
		name: 'x',
		domain: 'x.localhost',
		docroot: '/x',
		webServer: 'nginx',
		phpVersion,
		enabled,
		createdAt: 0,
		updatedAt: 0
	});

	it('names a pool an enabled site needs that is not running', () => {
		// The 502 this exists to prevent: nginx up, pool down, site dead, and
		// nothing on screen connecting the three.
		expect(stoppedPoolsFor([site('8.4')], [svc('php-fpm-8.4', 'stopped')], true)).toEqual(['8.4']);
	});

	it('stays quiet when the pool is running', () => {
		expect(stoppedPoolsFor([site('8.4')], [svc('php-fpm-8.4', 'running')], true)).toEqual([]);
	});

	it('ignores disabled sites', () => {
		// Warning about a pool nothing is serving would train the user to
		// ignore this line, and then it fails when it matters.
		expect(stoppedPoolsFor([site('8.4', false)], [svc('php-fpm-8.4', 'stopped')], true)).toEqual(
			[]
		);
	});

	it('stays quiet while nginx itself is stopped', () => {
		// The user has not asked to serve anything yet. A pool warning here is
		// noise about a problem they do not have.
		expect(stoppedPoolsFor([site('8.4')], [svc('php-fpm-8.4', 'stopped')], false)).toEqual([]);
	});

	it('names a pool that is missing from the snapshot entirely', () => {
		// A PHP major with no registered service is not running by definition —
		// this is the never-installed case, and it is the one most likely to
		// bite a new user.
		expect(stoppedPoolsFor([site('8.4')], [], true)).toEqual(['8.4']);
	});

	it('names each version once, in order, however many sites share it', () => {
		expect(stoppedPoolsFor([site('8.4'), site('8.3'), site('8.4')], [], true)).toEqual([
			'8.3',
			'8.4'
		]);
	});
});

// Nginx source design D1/D2/D3. Mirrors `mysql-install.derive.test.ts`'s
// `mysqlSourceBadge` block exactly — same three claims, same shape — since
// the two functions answer the identical question for a differently-shaped
// row.
describe('nginxSourceBadge', () => {
	const packaged: NginxRuntimeSourceDto = { kind: 'packaged', version: '1.30.4' };
	const homebrew: NginxRuntimeSourceDto = { kind: 'homebrew' };

	it('shows nothing when no nginx was found', () => {
		expect(nginxSourceBadge(null)).toBeNull();
	});

	it('labels the two sources distinctly, so a migration is legible', () => {
		const a = nginxSourceBadge(packaged);
		const b = nginxSourceBadge(homebrew);
		expect(a).not.toBeNull();
		expect(b).not.toBeNull();
		expect(a?.label).not.toBe(b?.label);
		expect(a?.title).not.toBe(b?.title);
	});

	it('shows the exact version for a runtime OpenVHost installed', () => {
		expect(nginxSourceBadge(packaged)?.label).toBe('OpenVHost 1.30.4');
	});

	// The lie this rules out: printing a made-up patch release for a binary
	// nobody probed. nginx has no `--version` flag, only `-v` — finding out
	// means executing the binary, which design D2 exists to avoid on the
	// packaged path — so the badge carries no number at all.
	it('invents no version for a Homebrew runtime — its label carries no digits', () => {
		const badge = nginxSourceBadge(homebrew);
		expect(badge?.label).toBe('Homebrew');
		expect(badge?.label).not.toMatch(/\d/);
		expect(badge?.title).toMatch(/will not guess|which this badge/i);
	});
});
