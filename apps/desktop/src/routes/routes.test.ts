// SPDX-License-Identifier: GPL-3.0-or-later
//
// Route-level guards for two things a page can silently get wrong:
//
//   1. WHICH page `/` is. Sites is the landing route and Services lives at
//      `/services`; the rail links and `aria-current` have to agree with that, and
//      the landing page relies on AppShell's/Rail's DEFAULT `active` rather than
//      passing one — so a default that drifts out of step with `/` would highlight
//      the wrong destination with nothing else failing.
//   2. That the titlebar's "N running" comes from the shared supervisor state.
//      It used to be `runningCount={0}` on the Sites page — a literal that no
//      type or lint rule can object to, and that read as "0 running" at launch
//      even with services up.
//
// Rendered through `svelte/server`, which needs no DOM and so runs in the existing
// `node` vitest project (the pattern established by
// `lib/components/titlebar.drag.test.ts`). `onMount` does not run under SSR, so
// nothing here reaches Tauri IPC — the shared store is seeded directly instead.

import { beforeEach, describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import SitesPage from './+page.svelte';
import ServicesPage from './services/+page.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import type { ServiceStatus } from '$lib/ipc';

const svc = (id: string, kind: 'running' | 'stopped'): ServiceStatus => ({
	id,
	displayName: id,
	endpoint: null,
	pid: kind === 'running' ? 1 : null,
	state: { kind }
});

/** Whatever the titlebar pill claims is running. */
function titlebarCount(body: string): string {
	const shown = body.match(/([0-9]+) running/);
	if (shown === null) throw new Error('the titlebar rendered no running count');
	return shown[1];
}

/**
 * The rail's nav link for `label`, as `{ href, current }`. Parsed from the
 * anchor's own attributes rather than matched against one literal tag string, so
 * these assertions survive a change in how Svelte orders emitted attributes.
 */
function railLink(body: string, label: 'Sites' | 'Services'): { href: string; current: boolean } {
	const anchor = [...body.matchAll(/<a\b([^>]*)>([\s\S]*?)<\/a>/g)].find(([, , inner]) =>
		inner.includes(label)
	);
	if (anchor === undefined) throw new Error(`the rail rendered no ${label} link`);
	return {
		href: anchor[1].match(/href="([^"]*)"/)?.[1] ?? '',
		current: /aria-current="page"/.test(anchor[1])
	};
}

// The shared store is a module singleton, so every test states the supervisor
// state it expects instead of inheriting the previous one's.
beforeEach(() => {
	servicesStore.services = [];
	servicesStore.error = null;
});

describe('a supervisor failure', () => {
	// The startup `listServices` runs in the LAYOUT, so its failure is not tied to any
	// one page — but the banner used to live only on the Services page. With Sites as
	// the landing route that meant a failed startup load showed as an unexplained
	// "0 running" in the titlebar and nothing else: a false claim about the user's
	// system, made silently, on the first screen. AppShell renders the banner now, so
	// assert it on BOTH routes rather than only where it happens to have worked before.
	const failure = { kind: 'core' as const, message: 'supervisor unreachable' };

	it('renders on the landing route, not just on Services', () => {
		servicesStore.error = failure;
		const { body } = render(SitesPage);
		expect(body).toContain('data-testid="error-banner"');
		expect(body).toContain('supervisor unreachable');
	});

	it('still renders on Services', () => {
		servicesStore.error = failure;
		const { body } = render(ServicesPage);
		expect(body).toContain('data-testid="error-banner"');
		expect(body).toContain('supervisor unreachable');
	});

	it('renders exactly once per page, so moving it did not double it up', () => {
		servicesStore.error = failure;
		for (const page of [SitesPage, ServicesPage]) {
			const { body } = render(page);
			expect(body.match(/data-testid="error-banner"/g)).toHaveLength(1);
		}
	});

	it('shows no banner when nothing has failed', () => {
		const { body } = render(SitesPage);
		expect(body).not.toContain('data-testid="error-banner"');
	});
});

describe('the landing route (/)', () => {
	it('is Sites', () => {
		const { body } = render(SitesPage);
		expect(body).toContain('data-testid="sites"');
		expect(body).toContain('>Sites</h1>');
	});

	it('marks Sites as the current rail destination without passing `active`', () => {
		const { body } = render(SitesPage);
		expect(railLink(body, 'Sites').current).toBe(true);
		expect(railLink(body, 'Services').current).toBe(false);
	});

	it('links Sites at / and Services at /services', () => {
		const { body } = render(SitesPage);
		expect(railLink(body, 'Sites').href).toBe('/');
		expect(railLink(body, 'Services').href).toBe('/services');
	});
});

describe('the /services route', () => {
	it('still renders the services panel and the log pane', () => {
		const { body } = render(ServicesPage);
		expect(body).toContain('data-testid="services"');
		expect(body).toContain('data-testid="log"');
	});

	it('renders a row per service off the shared list, with the state-appropriate action', () => {
		servicesStore.services = [svc('nginx', 'running'), svc('php-fpm', 'stopped')];
		const { body } = render(ServicesPage);
		expect(body).toContain('data-testid="pill-nginx"');
		expect(body).toContain('aria-label="Stop nginx"');
		expect(body).toContain('data-testid="pill-php-fpm"');
		expect(body).toContain('aria-label="Start php-fpm"');
	});

	it('marks Services as the current rail destination', () => {
		const { body } = render(ServicesPage);
		expect(railLink(body, 'Services').current).toBe(true);
		expect(railLink(body, 'Sites').current).toBe(false);
	});
});

describe('the titlebar running count', () => {
	it('reports the shared supervisor state on the landing page, not a hardcoded 0', () => {
		servicesStore.services = [
			svc('nginx', 'running'),
			svc('php-fpm', 'running'),
			svc('mariadb', 'stopped')
		];
		expect(titlebarCount(render(SitesPage).body)).toBe('2');
	});

	it('is the same count on both routes, from the one shared store', () => {
		servicesStore.services = [svc('nginx', 'running'), svc('php-fpm', 'stopped')];
		expect(titlebarCount(render(SitesPage).body)).toBe('1');
		expect(titlebarCount(render(ServicesPage).body)).toBe('1');
	});

	// Two different values through the same code path: a literal that happened to
	// match one of them cannot pass both.
	it('tracks the shared store rather than any constant', () => {
		servicesStore.services = [svc('nginx', 'stopped')];
		expect(titlebarCount(render(SitesPage).body)).toBe('0');
		servicesStore.services = [svc('nginx', 'running'), svc('php-fpm', 'running')];
		expect(titlebarCount(render(SitesPage).body)).toBe('2');
	});
});
