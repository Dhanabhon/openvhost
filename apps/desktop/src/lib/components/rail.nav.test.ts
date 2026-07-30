// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rail's nav contract, pinned at the component itself.
//
// `routes/routes.test.ts` already checks the rail as the two real pages render
// it — but every page reaches Rail through AppShell, which always forwards its
// own `active`. So Rail's OWN default is unreachable from a page test, and would
// drift out of step with `/` unnoticed. Since the default is the thing that
// decides which destination an `active`-less caller highlights, it gets pinned
// here directly.
//
// SSR (`svelte/server`) — no DOM needed, same as `titlebar.drag.test.ts`.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import Rail from './Rail.svelte';

/** The rail's nav link for `label`, as `{ href, current }`. */
function link(
	body: string,
	label: 'Sites' | 'Services' | 'Web server' | 'Languages' | 'Databases' | 'Logs'
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

const railHtml = (props: Record<string, unknown> = {}): string => render(Rail, { props }).body;

describe('Rail destinations', () => {
	it('sends Sites to /, Services to /services, Web server to /web-server, Languages to /languages, Databases to /databases and Logs to /logs', () => {
		const body = railHtml({ active: 'sites' });
		expect(link(body, 'Sites').href).toBe('/');
		expect(link(body, 'Services').href).toBe('/services');
		expect(link(body, 'Web server').href).toBe('/web-server');
		expect(link(body, 'Languages').href).toBe('/languages');
		expect(link(body, 'Databases').href).toBe('/databases');
		expect(link(body, 'Logs').href).toBe('/logs');
	});

	it('marks exactly the active destination with aria-current', () => {
		const current = (active: string): boolean[] => {
			const body = railHtml({ active });
			return [
				link(body, 'Sites').current,
				link(body, 'Services').current,
				link(body, 'Web server').current,
				link(body, 'Languages').current,
				link(body, 'Databases').current,
				link(body, 'Logs').current
			];
		};
		expect(current('sites')).toEqual([true, false, false, false, false, false]);
		expect(current('services')).toEqual([false, true, false, false, false, false]);
		expect(current('web-server')).toEqual([false, false, true, false, false, false]);
		expect(current('languages')).toEqual([false, false, false, true, false, false]);
		expect(current('databases')).toEqual([false, false, false, false, true, false]);
		expect(current('logs')).toEqual([false, false, false, false, false, true]);
	});

	// The brief puts Web server AFTER Services — it answers "what would run", which
	// only matters once you know what IS running — Languages after that, Databases
	// after Languages, and Logs after Databases (task 6 brief: Logs activates in its
	// EXISTING rail position, after Databases and before Settings — no reordering).
	// Every other case here looks each anchor up by its label, so the entries could be
	// reordered and all of them would still pass. Order is part of the contract, so it
	// is asserted directly.
	it('lists the destinations in order: Sites, Services, Web server, Languages, Databases, Logs', () => {
		const labels = [...railHtml().matchAll(/<a\b[^>]*>([\s\S]*?)<\/a>/g)].map(([, inner]) =>
			inner
				.replace(/<[^>]*>/g, '')
				.replace(/\s+/g, ' ')
				.trim()
		);
		expect(labels).toEqual(['Sites', 'Services', 'Web server', 'Languages', 'Databases', 'Logs']);
	});

	// Must match whatever `/` renders — Sites. AppShell.svelte defaults the same way.
	it('defaults to the landing destination when no `active` is passed', () => {
		const body = railHtml();
		expect(link(body, 'Sites').current).toBe(true);
		expect(link(body, 'Services').current).toBe(false);
		expect(link(body, 'Web server').current).toBe(false);
		expect(link(body, 'Languages').current).toBe(false);
		expect(link(body, 'Databases').current).toBe(false);
		expect(link(body, 'Logs').current).toBe(false);
	});

	// Settings has no destination yet, so it must not pretend to be a link (this
	// codebase never renders a fake `href="#"` control) — and Logs going live must not
	// have promoted it by accident.
	it('leaves Settings as a non-link', () => {
		const body = railHtml();
		const anchors = [...body.matchAll(/<a\b[^>]*>([\s\S]*?)<\/a>/g)].map(([, inner]) => inner);
		expect(anchors).toHaveLength(6);
		expect(body).toContain('Settings');
		expect(anchors.some((inner) => inner.includes('Settings'))).toBe(false);
	});
});
