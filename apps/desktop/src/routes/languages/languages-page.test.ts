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
import type { PhpEnvironmentDto, PhpRuntimeDto, ServiceStatus } from '$lib/ipc';

function row(
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

// `languagesStore`/`servicesStore` are module singletons — reset every field
// this page reads so no test inherits state a previous one left behind.
beforeEach(() => {
	languagesStore.env = null;
	languagesStore.installing = '';
	languagesStore.log = [];
	languagesStore.error = '';
	languagesStore.outcome = null;
	servicesStore.services = [];
	servicesStore.error = null;
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
