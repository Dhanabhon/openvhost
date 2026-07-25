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
function link(body: string, label: 'Sites' | 'Services'): { href: string; current: boolean } {
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
	it('sends Sites to / and Services to /services', () => {
		const body = railHtml({ active: 'sites' });
		expect(link(body, 'Sites').href).toBe('/');
		expect(link(body, 'Services').href).toBe('/services');
	});

	it('marks exactly the active destination with aria-current', () => {
		const sites = railHtml({ active: 'sites' });
		expect([link(sites, 'Sites').current, link(sites, 'Services').current]).toEqual([true, false]);
		const services = railHtml({ active: 'services' });
		expect([link(services, 'Sites').current, link(services, 'Services').current]).toEqual([
			false,
			true
		]);
	});

	// Must match whatever `/` renders — Sites. AppShell.svelte defaults the same way.
	it('defaults to the landing destination when no `active` is passed', () => {
		const body = railHtml();
		expect(link(body, 'Sites').current).toBe(true);
		expect(link(body, 'Services').current).toBe(false);
	});
});
