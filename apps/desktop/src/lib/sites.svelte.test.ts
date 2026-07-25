// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { SitesStore } from './sites.svelte';
import type { SitesApi } from './sites.svelte';
import type { SiteDto, SiteInput } from './ipc';

const dto = (id: string, name: string): SiteDto => ({
	id,
	name,
	domain: `${name}.localhost`,
	docroot: `/srv/www/${name}`,
	webServer: 'nginx',
	phpVersion: '8.3',
	enabled: true,
	createdAt: 1,
	updatedAt: 1
});

const input: SiteInput = {
	name: 'shop',
	domain: 'shop.localhost',
	docroot: '/srv/www/shop',
	webServer: 'nginx',
	phpVersion: '8.3',
	enabled: true
};

// `as unknown as SitesApi` (not `as never`): callers keep a `const a = api()`
// reference to assert `a.createSite`/etc. were called, which needs a type
// with those members — `never` would satisfy the `SitesStore` constructor
// but make every later `a.<method>` access a compile error.
function api(overrides: Partial<Record<string, unknown>> = {}): SitesApi {
	return {
		listSites: vi.fn(async () => [dto('a', 'shop')]),
		createSite: vi.fn(async () => dto('a', 'shop')),
		updateSite: vi.fn(async () => dto('a', 'shop')),
		deleteSite: vi.fn(async () => true),
		...overrides
	} as unknown as SitesApi;
}

describe('SitesStore', () => {
	it('load() fills sites', async () => {
		const store = new SitesStore(api());
		await store.load();
		expect(store.sites.map((s) => s.name)).toEqual(['shop']);
	});

	it('save(null, input) creates then refetches', async () => {
		const a = api();
		const store = new SitesStore(a);
		expect(await store.save(null, input)).toBe(true);
		expect(a.createSite).toHaveBeenCalledWith(input);
		expect(a.listSites).toHaveBeenCalled();
	});

	it('save(id, input) updates then refetches', async () => {
		const a = api();
		const store = new SitesStore(a);
		expect(await store.save('a', input)).toBe(true);
		expect(a.updateSite).toHaveBeenCalledWith('a', input);
	});

	it('a validation error lands on fieldErrors and does not throw', async () => {
		const a = api({
			createSite: vi.fn(async () => {
				throw { kind: 'validation', field: 'domain', message: 'already taken' };
			})
		});
		const store = new SitesStore(a);
		expect(await store.save(null, input)).toBe(false);
		expect(store.fieldErrors.domain).toBe('already taken');
		expect(store.error).toBeNull();
	});

	it('a non-validation error lands on error', async () => {
		const a = api({
			listSites: vi.fn(async () => {
				throw { kind: 'core', message: 'state.db unavailable' };
			})
		});
		const store = new SitesStore(a);
		await store.load();
		expect(store.error?.kind).toBe('core');
	});

	it('remove() deletes then refetches, and a false result is still success', async () => {
		const a = api({ deleteSite: vi.fn(async () => false) });
		const store = new SitesStore(a);
		expect(await store.remove('a')).toBe(true);
		expect(a.listSites).toHaveBeenCalled();
	});
});
