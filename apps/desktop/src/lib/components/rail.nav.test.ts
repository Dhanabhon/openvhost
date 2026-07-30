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
	label: 'Sites' | 'Services' | 'Web server' | 'Languages' | 'Databases'
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
	it('sends Sites to /, Services to /services, Web server to /web-server, Languages to /languages and Databases to /databases', () => {
		const body = railHtml({ active: 'sites' });
		expect(link(body, 'Sites').href).toBe('/');
		expect(link(body, 'Services').href).toBe('/services');
		expect(link(body, 'Web server').href).toBe('/web-server');
		expect(link(body, 'Languages').href).toBe('/languages');
		expect(link(body, 'Databases').href).toBe('/databases');
	});

	it('marks exactly the active destination with aria-current', () => {
		const current = (active: string): boolean[] => {
			const body = railHtml({ active });
			return [
				link(body, 'Sites').current,
				link(body, 'Services').current,
				link(body, 'Web server').current,
				link(body, 'Languages').current,
				link(body, 'Databases').current
			];
		};
		expect(current('sites')).toEqual([true, false, false, false, false]);
		expect(current('services')).toEqual([false, true, false, false, false]);
		expect(current('web-server')).toEqual([false, false, true, false, false]);
		expect(current('languages')).toEqual([false, false, false, true, false]);
		expect(current('databases')).toEqual([false, false, false, false, true]);
	});

	// The brief puts Web server AFTER Services — it answers "what would run", which
	// only matters once you know what IS running — Languages after that, and Databases
	// after Languages (task 6 brief: "Rail: add Databases after Languages"). Every other
	// case here looks each anchor up by its label, so the entries could be reordered and
	// all of them would still pass. Order is part of the contract, so it is asserted
	// directly.
	it('lists the destinations in order: Sites, Services, Web server, Languages, Databases', () => {
		const labels = [...railHtml().matchAll(/<a\b[^>]*>([\s\S]*?)<\/a>/g)].map(([, inner]) =>
			inner
				.replace(/<[^>]*>/g, '')
				.replace(/\s+/g, ' ')
				.trim()
		);
		expect(labels).toEqual(['Sites', 'Services', 'Web server', 'Languages', 'Databases']);
	});

	// Must match whatever `/` renders — Sites. AppShell.svelte defaults the same way.
	it('defaults to the landing destination when no `active` is passed', () => {
		const body = railHtml();
		expect(link(body, 'Sites').current).toBe(true);
		expect(link(body, 'Services').current).toBe(false);
		expect(link(body, 'Web server').current).toBe(false);
		expect(link(body, 'Languages').current).toBe(false);
		expect(link(body, 'Databases').current).toBe(false);
	});

	// Logs and Settings have no destination yet, so they must not pretend to be links
	// (this codebase never renders a fake `href="#"` control) — and adding a fifth real
	// destination must not have promoted them by accident.
	it('leaves Logs and Settings as non-links', () => {
		const body = railHtml();
		const anchors = [...body.matchAll(/<a\b[^>]*>([\s\S]*?)<\/a>/g)].map(([, inner]) => inner);
		expect(anchors).toHaveLength(5);
		for (const label of ['Logs', 'Settings']) {
			expect(body).toContain(label);
			expect(anchors.some((inner) => inner.includes(label))).toBe(false);
		}
	});
});
