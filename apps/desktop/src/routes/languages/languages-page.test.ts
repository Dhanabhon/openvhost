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
		expect(body).not.toMatch(/data-testid="lang-row-/);
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
});
