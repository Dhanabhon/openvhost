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
import WebServerPage from './web-server/+page.svelte';
import LanguagesPage from './languages/+page.svelte';
import DatabasesPage from './databases/+page.svelte';
import LogsPage from './logs/+page.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import { storeStatusStore } from '$lib/store-status.shared.svelte';
import { webServersStore } from '$lib/webservers.svelte';
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
function railLink(
	body: string,
	label: 'Sites' | 'Services' | 'Web server'
): { href: string; current: boolean } {
	const anchor = [...body.matchAll(/<a\b([^>]*)>([\s\S]*?)<\/a>/g)].find(([, , inner]) =>
		inner.includes(label)
	);
	if (anchor === undefined) throw new Error(`the rail rendered no ${label} link`);
	return {
		href: anchor[1].match(/href="([^"]*)"/)?.[1] ?? '',
		current: /aria-current="page"/.test(anchor[1])
	};
}

// Both stores are module singletons, so every test states the state it expects
// instead of inheriting the previous one's. Nothing writes to `webServersStore`
// today — `onMount` does not run under SSR, so the /web-server page renders without
// its load — which makes this latent rather than load-bearing; it is here so the
// first test that DOES seed it cannot leak into its neighbours.
beforeEach(() => {
	servicesStore.services = [];
	servicesStore.error = null;
	storeStatusStore.reason = null;
	storeStatusStore.lastError = null;
	webServersStore.servers = [];
	webServersStore.error = null;
	webServersStore.configText = {};
	webServersStore.configError = {};
	webServersStore.reports = {};
	webServersStore.validating = {};
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

// A `state.db` that never opened is an APP-level condition, not a page-level
// one: `list_log_sources` comes back without its site rows, a stored default PHP
// reads as "no preference", and every write refuses. Design D5 answers it with
// ONE banner rather than a notice per page, so what has to be proven is that
// every route renders it — and, just as importantly, that no route renders it on
// a healthy machine. Both directions, on every route the design names.
//
// `onMount` does not run under SSR, so nothing here reaches Tauri IPC; the
// shared store is seeded directly, which is exactly why it is a shared module
// singleton (see `store-status.shared.svelte.ts`). The ASK itself — that the
// layout ever sets this — is `layout-store-status.dom.test.ts`, which cannot be
// done here.
//
// Vacuity, measured by mutation on the one line that wires them together:
// changing AppShell's `reason={storeStatusStore.reason}` to `reason={null}` —
// the banner mounted but reading nothing, which is exactly how UI glue goes
// missing here — reddened all six `is announced on …`, the count test and the
// coexistence test, and left all six `is silent on …` green. Making the banner
// render unconditionally reddened those six instead. Neither half detects the
// other's mutation, which is why both are here.
describe('an unopened state.db', () => {
	const REASON = 'unable to open database file (os error 14)';
	const routes: Array<[string, typeof SitesPage]> = [
		['/', SitesPage],
		['/services', ServicesPage],
		['/web-server', WebServerPage],
		['/languages', LanguagesPage],
		['/databases', DatabasesPage],
		['/logs', LogsPage]
	];

	it.each(routes)('is announced on %s', (_route, page) => {
		storeStatusStore.reason = REASON;
		const { body } = render(page);
		expect(body).toContain('data-testid="store-unavailable-banner"');
		// The REASON, on the page itself — not just the shared sentence. A banner
		// that could only say "unavailable" leaves the user nothing to act on.
		expect(body).toContain(REASON);
	});

	it.each(routes)('is silent on %s when the store opened fine', (_route, page) => {
		const { body } = render(page);
		expect(body).not.toContain('data-testid="store-unavailable-banner"');
	});

	it('renders exactly once per page, never once per panel', () => {
		storeStatusStore.reason = REASON;
		for (const [, page] of routes) {
			const { body } = render(page);
			expect(body.match(/data-testid="store-unavailable-banner"/g)).toHaveLength(1);
		}
	});

	// The two app-level banners are independent conditions and must be able to
	// coexist: a broken store does not imply a broken supervisor, and suppressing
	// either would hide a real failure behind an unrelated one.
	it('sits alongside the supervisor banner rather than replacing it', () => {
		storeStatusStore.reason = REASON;
		servicesStore.error = { kind: 'core', message: 'supervisor unreachable' };
		const { body } = render(SitesPage);
		expect(body).toContain('data-testid="error-banner"');
		expect(body).toContain('data-testid="store-unavailable-banner"');
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

	// The endpoint cell ellipsizes so a long value cannot wrap and inflate the row (the demo
	// ticker's `endpoint` is a whole sentence, not an address). Truncating without a `title`
	// would make the tail unreadable with no way to recover it, so the attribute is the half
	// of that fix worth pinning — the CSS itself is scoped and not visible to SSR.
	it('keeps a truncated endpoint readable via its title attribute', () => {
		const long = '__testchild · 1s interval · fails after 45 ticks';
		servicesStore.services = [{ ...svc('demo-ticker', 'stopped'), endpoint: long }];
		const { body } = render(ServicesPage);
		expect(body).toContain(`title="${long}"`);
	});

	// A service with no endpoint must not render `title=""`, which would show an empty
	// tooltip on hover. `undefined` omits the attribute; `null` would not.
	it('renders no title attribute when a service has no endpoint', () => {
		servicesStore.services = [svc('nginx', 'running')];
		const { body } = render(ServicesPage);
		expect(body).not.toContain('title=""');
	});

	it('marks Services as the current rail destination', () => {
		const { body } = render(ServicesPage);
		expect(railLink(body, 'Services').current).toBe(true);
		expect(railLink(body, 'Sites').current).toBe(false);
	});

	// LogPane's v0 feed is seeded from `services[0]` (unchanged by task 6 —
	// see logs.svelte.ts's file header for why the deeper "scoped to the
	// selected service" part of spec D6 is deferred), so "Open in Logs"
	// must link to THAT service's ring source, not an arbitrary one.
	it("gives LogPane an Open in Logs link to the first service's ring source", () => {
		servicesStore.services = [svc('nginx', 'running'), svc('php-fpm', 'stopped')];
		const { body } = render(ServicesPage);
		expect(body).toContain('href="/logs?source=service-ring%3Anginx"');
		expect(body).toContain('>Open in Logs<');
	});

	it('omits Open in Logs when there is no service to link to', () => {
		servicesStore.services = [];
		const { body } = render(ServicesPage);
		expect(body).not.toContain('>Open in Logs<');
	});

	// Spec D6: a FAILED service row gains "View logs" — deep-linking to its
	// ring source (read via the existing service_log_tail/service-log path,
	// spec D7, never read_log_window).
	//
	// `nginx` is listed FIRST and `mysql` (the failed one) SECOND on purpose:
	// LogPane's OWN "Open in Logs" link (a different feature, this same
	// task) points at `services[0]`, which would ALSO be `mysql` if it were
	// first — and would then produce the identical href this test asserts,
	// passing even if ServiceRow's own link were deleted entirely. An
	// earlier version of this test did exactly that by accident (single-
	// service fixture) and stayed green through a neuter-proof that removed
	// the real link; see task-6-report.md.
	it('gives a failed service row a View logs link to its ring source', () => {
		servicesStore.services = [
			svc('nginx', 'running'),
			{
				id: 'mysql',
				displayName: 'mysql',
				endpoint: null,
				pid: null,
				state: { kind: 'failed', exit: 1, stderrTail: ['bind: address already in use'] }
			}
		];
		const { body } = render(ServicesPage);
		const failDetail = body.match(/data-testid="failed-mysql"[\s\S]*?<\/div>\s*<\/div>/)?.[0];
		expect(failDetail).toBeDefined();
		expect(failDetail).toContain('href="/logs?source=service-ring%3Amysql"');
		expect(failDetail).toContain('>View logs<');
	});

	it('does not offer View logs on a healthy row (no fail-detail at all)', () => {
		servicesStore.services = [svc('nginx', 'running')];
		const { body } = render(ServicesPage);
		expect(body).not.toContain('>View logs<');
	});
});

// `onMount` does not run under SSR, so this renders the page WITHOUT its
// `list_web_servers` load — which is the point: the shell, the rail state and the
// panel's empty state must all be right before any IPC has answered.
describe('the /web-server route', () => {
	it('renders the web-server panel', () => {
		const { body } = render(WebServerPage);
		expect(body).toContain('data-testid="web-servers"');
	});

	it('marks Web server as the current rail destination', () => {
		const { body } = render(WebServerPage);
		expect(railLink(body, 'Web server').current).toBe(true);
		expect([railLink(body, 'Sites').current, railLink(body, 'Services').current]).toEqual([
			false,
			false
		]);
	});

	it('reports the shared supervisor state in the titlebar, like every other route', () => {
		servicesStore.services = [svc('nginx', 'running'), svc('php-fpm', 'stopped')];
		expect(titlebarCount(render(WebServerPage).body)).toBe('1');
	});

	// The page is a control now, not only an inspector. `onMount` does not run
	// under SSR, so the form renders in its not-yet-loaded state — which is
	// exactly what proves it is MOUNTED rather than waiting on a value that
	// never arrives in this harness.
	it('renders the settings form beneath the inspector', () => {
		const { body } = render(WebServerPage);
		expect(body).toContain('data-testid="web-server-settings"');
		expect(body).toContain('data-testid="settings-unloaded"');
	});

	it('no longer calls itself read-only', () => {
		const visible = render(WebServerPage)
			.body.replace(/<[^>]*>/g, ' ')
			.replace(/\s+/g, ' ');
		expect(visible).not.toMatch(/read-only/i);
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
