// SPDX-License-Identifier: GPL-3.0-or-later
//
// Route-level SSR tests for the Languages page, seeding the SHARED
// `languagesStore` directly the same way `routes/routes.test.ts` seeds
// `servicesStore` — `onMount` never runs under `svelte/server`, so this is the
// only way to put the page into a terminal state without a live IPC layer.
//
// Rendered through `svelte/server`, so it runs in the existing `node` vitest
// project — same pattern as `routes/routes.test.ts`.

import { beforeEach, describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LanguagesPage from './+page.svelte';
import { languagesStore } from '$lib/languages.shared.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import { uninstallStore } from '$lib/uninstall.shared.svelte';
import type { PhpEnvironmentDto, PhpRuntimeDto, ServiceStatus } from '$lib/ipc';

function row(
	major: string,
	installed: boolean,
	overrides: Partial<PhpRuntimeDto> = {}
): PhpRuntimeDto {
	return {
		major,
		installed,
		// Catalogue rows by default — the ordinary case. The out-of-catalogue
		// test below overrides it, which is the only way the page can know: the
		// flag comes off the DTO, and the row has no way to infer it.
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

function env(brewFound: boolean, runtimes: PhpRuntimeDto[]): PhpEnvironmentDto {
	return { brewFound, brewSearched: ['/opt/homebrew/bin/brew'], runtimes };
}

function svc(id: string, state: ServiceStatus['state']): ServiceStatus {
	return { id, displayName: id, endpoint: null, pid: null, state };
}

/** Pulls out just the header Check again button's own opening tag, so a
 *  `disabled` assertion can fail for the reason it names rather than
 *  matching some unrelated part of the page. */
function checkAgainHeaderButtonTag(body: string): string {
	const match = body.match(/<button[^>]*data-testid="languages-check-again-header"[^>]*>/);
	if (!match) {
		throw new Error('expected the header Check again button to render');
	}
	return match[0];
}

/** Just the Uninstall button's own opening tag for `major`. */
function uninstallTag(body: string, major: string): string {
	const match = body.match(new RegExp(`<button[^>]*data-testid="uninstall-${major}"[^>]*>`));
	if (!match) throw new Error(`expected an Uninstall button for ${major}`);
	return match[0];
}

// `languagesStore`/`servicesStore` are module singletons — reset every field
// this page reads so no test inherits state a previous one left behind.
beforeEach(() => {
	languagesStore.env = null;
	languagesStore.installing = '';
	languagesStore.log = [];
	languagesStore.error = '';
	languagesStore.outcome = null;
	languagesStore.installProgress = null;
	languagesStore.installTotal = null;
	servicesStore.services = [];
	servicesStore.error = null;
	uninstallStore.target = null;
	uninstallStore.plan = null;
	uninstallStore.planning = false;
	uninstallStore.uninstalling = '';
	uninstallStore.error = '';
	uninstallStore.log = [];
});

describe('the /languages route', () => {
	it('renders the panel with no rows and no error on a fresh, unsettled load', () => {
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages"');
		expect(body).not.toContain('data-testid="languages-page-error"');
		expect(body).not.toMatch(/data-testid="lang-row-/);
	});

	// C3 regression test: before the fix, this exact state — `env` non-null
	// (an earlier load succeeded) and `error` non-empty (a later rescan
	// failed), with NEITHER brew nor any installed version — rendered NOTHING
	// for the error. `store.error !== '' && store.env === null` was false (env
	// is not null) and the rowlist never mounts (`brewFound || anyInstalled`
	// is false), so the failure was invisible on the one page whose "Check
	// again" exists specifically to recover from it.
	it('shows a failed rescan even though no rows are rendered', () => {
		languagesStore.env = env(false, []);
		languagesStore.error = 'runtime list is poisoned';
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages-page-error"');
		expect(body).toContain('runtime list is poisoned');
		expect(body).not.toMatch(/data-testid="lang-row-/);
	});

	// Same C3 case, but with brew found and a version already installed — the
	// rowlist DOES render here, so this pins that the panel-level banner is
	// independent of that, not an accidental side effect of the rowlist being
	// absent.
	it('shows a failed rescan alongside an already-populated rowlist too', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		languagesStore.error = 'the PHP discovery task failed to run';
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages-page-error"');
		expect(body).toContain('data-testid="lang-row-8.3"');
	});

	it('shows no error banner once it has been cleared', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		languagesStore.error = '';
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="languages-page-error"');
	});

	// C2 regression test: `LanguagesEmpty` renders its own "Check again" button
	// ONLY in the `!brewFound` branch, so once brew is found — whether nothing
	// is installed yet, or a version already is — that control used to be
	// unreachable from this page. Both `brewFound` states must offer it.
	it('offers Check again once brew is found, with nothing installed yet', () => {
		languagesStore.env = env(true, [row('8.4', false)]);
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages-check-again-header"');
	});

	it('offers Check again once brew is found, with a version already installed', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages-check-again-header"');
	});

	it('does not duplicate Check again on the no-brew page, which renders its own', () => {
		languagesStore.env = env(false, []);
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages-check-again"');
		expect(body).not.toContain('data-testid="languages-check-again-header"');
	});

	// A1 audit finding: `rescan_php_runtimes` now takes `InstallLock` with
	// `.lock().await` (H1's fix, so a rescan can never overwrite a
	// just-completed install with a stale set) — but that means pressing
	// Check again during an install now blocks for the whole build (twenty
	// minutes for a source formula) with no UI feedback, and repeated presses
	// queue unbounded waiters on that mutex. Both directions are asserted —
	// a one-directional check here would pass with `disabled` hardcoded
	// either way.
	it('disables the header Check again while an install is running', () => {
		languagesStore.env = env(true, [row('8.4', false)]);
		languagesStore.installing = '8.4';
		const { body } = render(LanguagesPage);
		expect(checkAgainHeaderButtonTag(body)).toContain('disabled');
	});

	it('leaves the header Check again enabled when no install is running', () => {
		languagesStore.env = env(true, [row('8.4', false)]);
		languagesStore.installing = '';
		const { body } = render(LanguagesPage);
		expect(checkAgainHeaderButtonTag(body)).not.toContain('disabled');
	});

	// Same property on the no-brew page's OWN "Check again" button
	// (`LanguagesEmpty`'s `languages-check-again`), which this page also feeds
	// `store.installing` into.
	it('disables the no-brew page Check again while an install is running', () => {
		languagesStore.env = env(false, []);
		languagesStore.installing = '8.4';
		const { body } = render(LanguagesPage);
		const match = body.match(/<button[^>]*data-testid="languages-check-again"[^>]*>/);
		expect(match?.[0]).toContain('disabled');
	});

	it('leaves the no-brew page Check again enabled when no install is running', () => {
		languagesStore.env = env(false, []);
		languagesStore.installing = '';
		const { body } = render(LanguagesPage);
		const match = body.match(/<button[^>]*data-testid="languages-check-again"[^>]*>/);
		expect(match?.[0]).not.toContain('disabled');
	});

	// Pins the page's own `serviceState` lookup (the `runtime.serviceId === null
	// ? null : servicesStore.services.find(...)?.state ?? null` glue in
	// +page.svelte) — nothing else exercises it, since LanguageRow's own tests
	// hand it `serviceState` directly and every other test here leaves
	// `servicesStore.services` empty. A runtime whose `serviceId` MATCHES an
	// entry in the shared snapshot must get that entry's state: seeding a
	// `failed` pool and asserting the row renders the failed surface is only
	// reachable through a correct `find`, not a hardcoded fallback.
	it('renders a matching runtime with its pool state from the shared services snapshot', () => {
		languagesStore.env = env(true, [row('8.4', true)]);
		servicesStore.services = [
			svc('php-fpm-8.4', { kind: 'failed', exit: 78, stderrTail: ['pool is broken'] })
		];
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="pool-failed-php-fpm-8.4"');
		expect(body).toContain('data-testid="retry-php-fpm-8.4"');
	});

	// Same lookup, the "no match" side: a runtime's `serviceId` that is not in
	// the snapshot at all (e.g. the supervisor hasn't reported it yet) must
	// render as `null` — no pill, no Start/Stop/Retry control — rather than
	// falling back to some default state.
	it('renders no pill and no control for a runtime whose serviceId matches nothing in the snapshot', () => {
		languagesStore.env = env(true, [row('8.4', true)]);
		servicesStore.services = [svc('some-other-service', { kind: 'running' })];
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="lang-pill-8.4"');
		expect(body).not.toContain('data-testid="start-php-fpm-8.4"');
		expect(body).not.toContain('data-testid="stop-php-fpm-8.4"');
		expect(body).not.toContain('data-testid="retry-php-fpm-8.4"');
	});

	// The guard's other branch: a not-installed runtime has `serviceId: null`,
	// so the lookup must short-circuit to `null` without ever touching
	// `servicesStore.services` — and the row still renders Install.
	it('renders Install for a not-installed runtime regardless of the services snapshot', () => {
		languagesStore.env = env(true, [row('8.4', false)]);
		servicesStore.services = [svc('unrelated', { kind: 'running' })];
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).not.toContain('data-testid="lang-pill-8.4"');
	});
});

// Package-uninstall design D6, at the route layer. Everything here is the
// PAGE's own glue — which store field reaches which prop, and whether the
// dialog is mounted at all — because that glue is exactly what per-component
// tests cannot see (the "gate the assembled product" lesson).
describe('the /languages route — uninstall', () => {
	it('offers Uninstall on an installed row', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="uninstall-8.3"');
	});

	it('offers no Uninstall on a row that is not installed', () => {
		languagesStore.env = env(true, [row('8.4', false)]);
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="uninstall-8.4"');
	});

	// The branch review's MEDIUM, at the route layer — where the bug actually
	// lived: `LanguageRow` had no way to know, because the page never told it.
	// A row test alone cannot catch that, which is the "gate the assembled
	// product" lesson this file exists for.
	it('threads the catalogue flag through, so an unmanaged major gets no Uninstall', () => {
		languagesStore.env = env(true, [row('7.4', true, { cataloged: false })]);
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="uninstall-7.4"');
		expect(body).toContain('data-testid="php-out-of-catalogue-7.4"');
		expect(body).toContain('brew uninstall php@7.4');
	});

	it('still offers Uninstall for the managed majors alongside it', () => {
		languagesStore.env = env(true, [row('7.4', true, { cataloged: false }), row('8.3', true)]);
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="uninstall-7.4"');
		expect(body).toContain('data-testid="uninstall-8.3"');
	});

	// The page feeds `languagesStore.installing` into the row; without that
	// wiring the button would stay live during a build and queue on the
	// install lock with no feedback.
	it('disables Uninstall while an install is running', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		languagesStore.installing = '8.4';
		const { body } = render(LanguagesPage);
		expect(uninstallTag(body, '8.3')).toContain('disabled');
	});

	// …and the same for `uninstallStore.uninstalling`, which is the OTHER
	// store this page now reads. A page that forgot to thread it would leave
	// every other row's Uninstall live during an uninstall.
	it('disables Uninstall while another uninstall is running', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		uninstallStore.uninstalling = '8.4';
		const { body } = render(LanguagesPage);
		expect(uninstallTag(body, '8.3')).toContain('disabled');
	});

	it('leaves Uninstall enabled when nothing is in flight', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		const { body } = render(LanguagesPage);
		expect(uninstallTag(body, '8.3')).not.toContain('disabled');
	});

	it('renders no confirmation until one is requested', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="uninstall-dialog"');
	});

	it('renders the confirmation, with the kept paths, once a plan is open', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		uninstallStore.target = { kind: 'php', major: '8.3' };
		uninstallStore.plan = {
			kind: 'php',
			major: '8.3',
			removes: ['the Homebrew formula php@8.3'],
			keeps: [{ what: 'Logs', path: '/Users/x/.openvhost/logs/php-fpm-8.3', headline: true }],
			blockers: []
		};
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="uninstall-dialog"');
		expect(body).toContain('Uninstall PHP 8.3?');
		expect(body).toContain('/Users/x/.openvhost/logs/php-fpm-8.3');
		expect(body).toContain('data-testid="uninstall-confirm"');
	});

	// Design D3 at the route layer: a blocked plan offers no way forward
	// anywhere on the page, not merely a disabled button.
	it('offers no way to proceed when the plan is blocked', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		uninstallStore.target = { kind: 'php', major: '8.3' };
		uninstallStore.plan = {
			kind: 'php',
			major: '8.3',
			removes: ['the Homebrew formula php@8.3'],
			keeps: [],
			blockers: [{ kind: 'sitesPinned', domains: ['shop.test'] }]
		};
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="uninstall-refused"');
		expect(body).toContain('shop.test');
		expect(body).not.toContain('data-testid="uninstall-confirm"');
	});
});

// Off-Homebrew slice 5C design D2, at the layer where the bug actually lived:
// the page, not the components. `LanguagesEmpty` renders whatever it is told;
// what was wrong was that the page told it `!brewFound` and hid the rowlist on
// the same bool. Every case below is a whole assembled page.
describe('the /languages route — the dead end is no longer "no Homebrew"', () => {
	const packaged = (major: string): PhpRuntimeDto =>
		row(major, true, {
			source: { kind: 'packaged', version: `${major}.24` },
			fullVersion: `${major}.24`,
			path: `/Users/x/.openvhost/packages/php/${major}/current/sbin/php-fpm`
		});

	// §8.1 — the headline, and the single most user-visible thing the whole
	// off-Homebrew programme was for. A machine with a packaged PHP and no
	// Homebrew was being told it could not install PHP, on a page that was
	// simultaneously not listing the PHP it already had.
	it('lists a packaged PHP and shows no dead end, on a machine with no Homebrew', () => {
		languagesStore.env = env(false, [packaged('8.4'), row('8.1', false)]);
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="languages-no-brew"');
		expect(body).toContain('data-testid="lang-row-8.4"');
		expect(body).toContain('data-testid="php-source-8.4"');
		expect(body).toContain('OpenVHost 8.4.24');
	});

	// §8.2 — the case the screen was actually written for. Unchanged, searched
	// paths and all: the change is WHEN it appears, not WHAT it says.
	it('renders the dead end exactly as before with no Homebrew and no route at all', () => {
		languagesStore.env = env(false, [row('8.1', false), row('8.4', false)]);
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages-no-brew"');
		expect(body).toContain('Homebrew is required to install PHP');
		expect(body).toContain('/opt/homebrew/bin/brew');
		expect(body).toContain('/bin/bash -c');
		// …and the rowlist stays hidden, because there is nothing to offer.
		expect(body).not.toMatch(/data-testid="lang-row-/);
	});

	// §8.2b — the case D2 exists for, and the one the spec's first draft got
	// wrong. 8.4 is installable from our own tree; 8.1/8.3/8.5 need Homebrew and
	// say so PER ROW. One sentence at the top of the page cannot be true of both.
	it('offers the packaged major and tells the others what they need, per row', () => {
		languagesStore.env = env(false, [
			row('8.1', false),
			row('8.3', false),
			row('8.4', false, { offer: { kind: 'available', version: '8.4.24' } }),
			row('8.5', false)
		]);
		const { body } = render(LanguagesPage);

		expect(body).not.toContain('data-testid="languages-no-brew"');
		expect(body).toContain('data-testid="install-8.4"');
		expect(body).not.toContain('data-testid="php-no-route-8.4"');

		for (const major of ['8.1', '8.3', '8.5']) {
			expect(body, major).toContain(`data-testid="lang-row-${major}"`);
			expect(body, major).not.toContain(`data-testid="install-${major}"`);
			expect(body, major).toContain(`data-testid="php-no-route-${major}"`);
		}
		expect(body).toMatch(/needs Homebrew/);
	});

	// The rescan is the ONE control that unsticks a user who has gone off to
	// install Homebrew by hand. It used to be gated on `brewFound`, which was
	// the exact complement of the dead end only while `!brewFound` triggered it.
	// After D2 that page has no dead end and would have had no Check again
	// either — the C2 regression, reintroduced by the fix for something else.
	it('still offers Check again on a no-Homebrew page that renders no dead end', () => {
		languagesStore.env = env(false, [packaged('8.4')]);
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="languages-check-again-header"');
		expect(body).not.toContain('data-testid="languages-check-again"');
	});

	// The invariant behind both gates: exactly one "Check again" on screen, in
	// every state this page can be in. Two would be a duplicated control; zero
	// would be a user with no way back.
	it('shows exactly one Check again in every state', () => {
		const states: PhpEnvironmentDto[] = [
			env(false, []),
			env(false, [row('8.4', false)]),
			env(false, [packaged('8.4')]),
			env(false, [row('8.4', false, { offer: { kind: 'available', version: '8.4.24' } })]),
			env(true, []),
			env(true, [row('8.4', false)]),
			env(true, [row('8.3', true)])
		];
		for (const [i, state] of states.entries()) {
			languagesStore.env = state;
			const { body } = render(LanguagesPage);
			const header = body.includes('data-testid="languages-check-again-header"') ? 1 : 0;
			const own = body.includes('data-testid="languages-check-again"') ? 1 : 0;
			expect(header + own, `state ${i}`).toBe(1);
		}
	});

	// A brew that went missing AFTER setup — uninstalled, or a PATH change. The
	// installed pool keeps its row and its controls, because an already-running
	// php-fpm needs no brew to serve. This state existed before the slice; what
	// changes is that the not-installed rows beside it now explain themselves
	// instead of offering a button that could only report "Homebrew was not
	// found".
	it('keeps an installed row usable when Homebrew has gone missing', () => {
		languagesStore.env = env(false, [row('8.3', true), row('8.4', false)]);
		servicesStore.services = [svc('php-fpm-8.3', { kind: 'running' })];
		const { body } = render(LanguagesPage);
		expect(body).not.toContain('data-testid="languages-no-brew"');
		expect(body).toContain('data-testid="lang-row-8.3"');
		expect(body).toContain('data-testid="stop-php-fpm-8.3"');
		expect(body).toContain('data-testid="uninstall-8.3"');
		expect(body).not.toContain('data-testid="install-8.4"');
		expect(body).toContain('data-testid="php-no-route-8.4"');
	});
});

// §8.6 — "nothing changes on a machine with Homebrew and no package tree, which
// is every real machine today, including the developer's. Establish it, do not
// assert it." This is that establishment: the exact environment this machine
// reports, rendered whole, checked against the page as it was before the slice.
describe('the /languages route — a machine with Homebrew and no package tree', () => {
	/** What `php_environment` returns on this Apple Silicon machine today: brew
	 *  present, 8.3 installed as a keg, 8.4 pinned but `AwaitingRelease` (no
	 *  release published), and every other major `Unavailable` (no artifact
	 *  built). */
	const today = (): PhpEnvironmentDto =>
		env(true, [
			row('8.1', false),
			row('8.2', false),
			row('8.3', true),
			row('8.4', false, { offer: { kind: 'awaitingRelease', tag: 'php-8.4.24' } }),
			row('8.5', false, { recommended: true })
		]);

	it('offers Install on every uninstalled row, including the AwaitingRelease one', () => {
		// The first draft of spec §8.5 would have deleted 8.4's button — a
		// working control, on the only machine anyone is running.
		languagesStore.env = today();
		const { body } = render(LanguagesPage);
		for (const major of ['8.1', '8.2', '8.4', '8.5']) {
			expect(body, major).toContain(`data-testid="install-${major}"`);
		}
	});

	it('adds no note, no badge and no new copy anywhere on the page', () => {
		languagesStore.env = today();
		const { body } = render(LanguagesPage);
		expect(body).not.toMatch(/data-testid="php-no-route-/);
		expect(body).not.toMatch(/data-testid="php-source-/);
		expect(body).not.toMatch(/needs Homebrew/);
		expect(body).not.toMatch(/php-8\.4\.24/);
		expect(body).not.toMatch(/maintainer/i);
	});

	it('renders no dead end, and the invitation only while nothing is installed', () => {
		languagesStore.env = today();
		const withPhp = render(LanguagesPage).body;
		expect(withPhp).not.toContain('data-testid="languages-no-brew"');
		expect(withPhp).not.toContain('data-testid="languages-no-php"');

		languagesStore.env = env(true, [row('8.4', false)]);
		const withoutPhp = render(LanguagesPage).body;
		expect(withoutPhp).not.toContain('data-testid="languages-no-brew"');
		expect(withoutPhp).toContain('data-testid="languages-no-php"');
		expect(withoutPhp).toContain(
			'Choose a version below — OpenVHost installs it through Homebrew and serves your sites with it.'
		);
	});

	it('keeps every row and its lifecycle controls exactly where they were', () => {
		languagesStore.env = today();
		servicesStore.services = [svc('php-fpm-8.3', { kind: 'running' })];
		const { body } = render(LanguagesPage);
		for (const major of ['8.1', '8.2', '8.3', '8.4', '8.5']) {
			expect(body, major).toContain(`data-testid="lang-row-${major}"`);
		}
		expect(body).toContain('data-testid="lang-pill-8.3"');
		expect(body).toContain('data-testid="stop-php-fpm-8.3"');
		expect(body).toContain('data-testid="uninstall-8.3"');
		expect(body).toContain('data-testid="languages-check-again-header"');
	});
});

// Task 1's wiring, observed from this page rather than re-implemented: the
// layout's `onServiceUnregistered` subscription calls
// `ServicesStore.applyUnregistered`, and this page reads that same shared
// snapshot for its pill and its Start/Stop control. Asserting through the
// store proves the page needs no reload of its own for the row to go quiet.
describe('the /languages route — a service that disappears', () => {
	it('drops the pool pill and its control without a page reload', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		servicesStore.services = [svc('php-fpm-8.3', { kind: 'running' })];
		const before = render(LanguagesPage).body;
		expect(before).toContain('data-testid="lang-pill-8.3"');
		expect(before).toContain('data-testid="stop-php-fpm-8.3"');

		// Exactly what the layout does on `SupervisorEvent::Unregistered`.
		servicesStore.applyUnregistered('php-fpm-8.3');

		const after = render(LanguagesPage).body;
		expect(after).not.toContain('data-testid="lang-pill-8.3"');
		expect(after).not.toContain('data-testid="stop-php-fpm-8.3"');
	});

	it('leaves an unrelated service alone', () => {
		languagesStore.env = env(true, [row('8.3', true)]);
		servicesStore.services = [svc('php-fpm-8.3', { kind: 'running' })];
		servicesStore.applyUnregistered('mysql-8.4');
		const body = render(LanguagesPage).body;
		expect(body).toContain('data-testid="lang-pill-8.3"');
	});
});

// The page → row seam for the packaged install's progress. `onMount` never runs
// under `svelte/server`, so the subscription itself is covered by
// `lib/languages.listeners.test.ts`; what this block covers is the half that
// lives in the template — that the page hands each row the store's progress
// SCOPED TO THAT ROW, and hands a Homebrew machine nothing at all.
//
// Vacuity: every assertion is on a testid the page does not otherwise emit, and
// each is paired with a negative case. Proven by mutation — replacing
// `store.progressFor(runtime.major)` with `null` reddened 'hands the installing
// row its own live pipeline state', and replacing it with the UNSCOPED
// `store.installProgress?.progress` reddened 'ignores an event belonging to a
// major other than the one installing'. The second mutation is the one worth
// recording: it survived the first version of that test, because with
// `installing` set to 8.4 only the 8.4 row paints either way. See the test's own
// comment for what had to change to make it discriminate.
describe('the /languages route — a packaged install in flight', () => {
	it('paints no pipeline anywhere while a Homebrew install runs', () => {
		// Every real machine today. `php-install-progress` has one emitter,
		// `run_package_install`, so this store field stays null for the whole of a
		// brew install (spec §8.6).
		languagesStore.env = env(true, [row('8.4', false)]);
		languagesStore.installing = '8.4';
		const { body } = render(LanguagesPage);
		expect(body).toContain('data-testid="lang-row-8.4"');
		expect(body).not.toContain('php-install-progress-8.4');
		expect(body).not.toContain('progressbar');
	});

	it('hands the installing row its own live pipeline state', () => {
		languagesStore.env = env(true, [row('8.4', false)]);
		languagesStore.installing = '8.4';
		languagesStore.applyInstallProgress('8.4', { kind: 'downloaded', bytes: 512 });
		languagesStore.installTotal = 2048;
		const { body } = render(LanguagesPage);
		expect(body).toContain('php-install-progress-8.4');
		expect(body).toContain('aria-valuenow="25"');
	});

	// The ONE thing `progressFor` buys that the row's own `isInstalling` guard
	// does not, and the first version of this test could not tell the two apart:
	// with `installing` set to 8.4, only the 8.4 row paints either way, so a page
	// handing every row the unscoped `store.installProgress` passed happily.
	//
	// What discriminates is an event for a major that is NOT the one installing —
	// a late throttled flush from a previous 8.3 attempt landing after the 8.4
	// run started, which `install()`'s clear cannot prevent because it happens
	// first. Unscoped, that paints 8.3's pipeline under the 8.4 row, which is the
	// same attribution bug this page already had to fix once for the log.
	it('ignores an event belonging to a major other than the one installing', () => {
		languagesStore.env = env(true, [row('8.3', false), row('8.4', false)]);
		languagesStore.installing = '8.4';
		languagesStore.applyInstallProgress('8.3', { kind: 'verified' });
		const { body } = render(LanguagesPage);
		// The row that is installing must not borrow another major's state…
		expect(body).not.toContain('php-install-progress-8.4');
		// …and the row the state belongs to is not installing, so it paints
		// nothing either. The positive control is the test above, which proves
		// this page does paint when the two majors agree.
		expect(body).not.toContain('php-install-progress-8.3');
	});
});
