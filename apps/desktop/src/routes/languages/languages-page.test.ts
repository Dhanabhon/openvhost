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
import type { PhpEnvironmentDto, PhpRuntimeDto } from '$lib/ipc';

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
});
