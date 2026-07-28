// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { loadSitesOrFail } from './webserver-sites';
import type { SiteDto } from './ipc';

const site: SiteDto = {
	id: 'a',
	name: 'a',
	domain: 'a.test',
	docroot: '/srv/a',
	webServer: 'nginx',
	phpVersion: '8.4',
	enabled: true,
	createdAt: 0,
	updatedAt: 0
};

// This is the composition the Web server route wires into its `onMount` sites
// read. The route itself cannot be exercised at this level — `onMount` does
// not run under `svelte/server`'s static render (see `routes.test.ts`'s own
// header comment) — so this is the reachable seam: the exact function the
// page calls, tested directly against a fake `listSites`.
describe('loadSitesOrFail', () => {
	it('returns the sites on a successful read, without calling onFail', async () => {
		const onFail = vi.fn();

		const result = await loadSitesOrFail(async () => [site], onFail);

		expect(result).toEqual([site]);
		expect(onFail).not.toHaveBeenCalled();
	});

	// The empty-list fallback is still right (see the module's doc comment), but
	// silently swallowing the failure is what made a real 502 undiagnosable —
	// this pins that `onFail` now receives it.
	it('returns [] AND surfaces the failure via onFail on a rejected read', async () => {
		const error = { kind: 'core' as const, message: 'boom' };
		const onFail = vi.fn();

		const result = await loadSitesOrFail(async () => {
			throw error;
		}, onFail);

		expect(result).toEqual([]);
		expect(onFail).toHaveBeenCalledWith(error);
	});
});
