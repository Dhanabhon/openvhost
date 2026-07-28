// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { SitesStore } from './sites.svelte';
import type { SitesApi } from './sites.svelte';
import type { CreateSiteResult, ScaffoldOutcomeDto, SiteDto, SiteInput } from './ipc';

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

/** `scaffold: null` (not requested/not applicable) unless a case says otherwise —
 *  same default the api() mock below uses for every test that isn't specifically
 *  about scaffolding. */
const createResult = (scaffold: ScaffoldOutcomeDto | null = null): CreateSiteResult => ({
	site: dto('a', 'shop'),
	scaffold
});

// `as unknown as SitesApi` (not `as never`): callers keep a `const a = api()`
// reference to assert `a.createSite`/etc. were called, which needs a type
// with those members — `never` would satisfy the `SitesStore` constructor
// but make every later `a.<method>` access a compile error.
function api(overrides: Partial<Record<string, unknown>> = {}): SitesApi {
	return {
		listSites: vi.fn(async () => [dto('a', 'shop')]),
		createSite: vi.fn(async () => createResult()),
		updateSite: vi.fn(async () => dto('a', 'shop')),
		deleteSite: vi.fn(async () => true),
		openSite: vi.fn(async () => undefined),
		...overrides
	} as unknown as SitesApi;
}

describe('SitesStore', () => {
	it('load() fills sites', async () => {
		const store = new SitesStore(api());
		await store.load();
		expect(store.sites.map((s) => s.name)).toEqual(['shop']);
	});

	it('save(null, input, false) creates then refetches', async () => {
		const a = api();
		const store = new SitesStore(a);
		expect(await store.save(null, input, false)).toBe(true);
		expect(a.createSite).toHaveBeenCalledWith(input, false);
		expect(a.listSites).toHaveBeenCalled();
	});

	it('save(id, input, false) updates then refetches', async () => {
		const a = api();
		const store = new SitesStore(a);
		expect(await store.save('a', input, false)).toBe(true);
		expect(a.updateSite).toHaveBeenCalledWith('a', input);
	});

	it('a validation error lands on fieldErrors and does not throw', async () => {
		const a = api({
			createSite: vi.fn(async () => {
				throw { kind: 'validation', field: 'domain', message: 'already taken' };
			})
		});
		const store = new SitesStore(a);
		expect(await store.save(null, input, false)).toBe(false);
		expect(store.fieldErrors.domain).toBe('already taken');
		expect(store.error).toBeNull();
	});

	it("clearErrors() drops a previous attempt's field errors", async () => {
		const a = api({
			createSite: vi.fn(async () => {
				throw { kind: 'validation', field: 'domain', message: 'already taken' };
			})
		});
		const store = new SitesStore(a);
		expect(await store.save(null, input, false)).toBe(false);
		expect(store.fieldErrors.domain).toBe('already taken');
		store.clearErrors();
		expect(store.fieldErrors).toEqual({});
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

describe('SitesStore.lastScaffold', () => {
	const outcome: ScaffoldOutcomeDto = { kind: 'created' };

	it('passes createFolder through to the api and stores the scaffold outcome', async () => {
		const a = api({ createSite: vi.fn(async () => createResult(outcome)) });
		const store = new SitesStore(a);
		expect(await store.save(null, input, true)).toBe(true);
		expect(a.createSite).toHaveBeenCalledWith(input, true);
		expect(store.lastScaffold).toEqual({
			siteName: input.name,
			docroot: '/srv/www/shop',
			outcome
		});
	});

	it('leaves lastScaffold null when scaffold was not requested', async () => {
		// The default api() mock already resolves `{ scaffold: null }` — the
		// honest shape `create_folder: false` produces on the Rust side.
		const a = api();
		const store = new SitesStore(a);
		expect(await store.save(null, input, false)).toBe(true);
		expect(a.createSite).toHaveBeenCalledWith(input, false);
		expect(store.lastScaffold).toBeNull();
	});

	it('dismissScaffold clears the notice', async () => {
		const a = api({ createSite: vi.fn(async () => createResult(outcome)) });
		const store = new SitesStore(a);
		await store.save(null, input, true);
		expect(store.lastScaffold).not.toBeNull();
		store.dismissScaffold();
		expect(store.lastScaffold).toBeNull();
	});

	it('reset clears a stale scaffold notice', async () => {
		const a = api({ createSite: vi.fn(async () => createResult(outcome)) });
		const store = new SitesStore(a);
		await store.save(null, input, true);
		expect(store.lastScaffold).not.toBeNull();
		// `clearErrors()` is the public entry point onto the store's shared
		// private `reset()` — the same one `load()`/`save()`/`remove()` call.
		store.clearErrors();
		expect(store.lastScaffold).toBeNull();
	});
});

describe('SitesStore row actions', () => {
	it('flips enabled through updateSite, preserving every other field', async () => {
		const a = api();
		const store = new SitesStore(a);
		await store.setEnabled(dto('a', 'shop'), false);
		// A whole-object write, so assert the WHOLE object: a toggle that dropped or
		// rewrote docroot/phpVersion/domain would still "work" and silently damage the row.
		expect(a.updateSite).toHaveBeenCalledWith('a', {
			name: 'shop',
			domain: 'shop.localhost',
			docroot: '/srv/www/shop',
			webServer: 'nginx',
			phpVersion: '8.3',
			enabled: false
		});
	});

	it('refetches after a successful row action', async () => {
		const a = api();
		const store = new SitesStore(a);
		await store.removeRow('a');
		expect(a.deleteSite).toHaveBeenCalledWith('a');
		expect(a.listSites).toHaveBeenCalled();
	});

	// A row failure must stay ON the row. Routing it to `error` would blank the list
	// behind a page banner over one row's problem.
	it('keeps a row failure on that row and off the page banner', async () => {
		const store = new SitesStore(
			api({
				deleteSite: vi.fn(async () => {
					throw { kind: 'core', message: 'row is gone' };
				})
			})
		);
		await store.removeRow('a');
		expect(store.rowError.a).toContain('row is gone');
		expect(store.error).toBeNull();
	});

	// `IpcError`'s `simulated` variant has no `message`, and `String(e)` on an object
	// renders "[object Object]" straight onto the row.
	it('never renders [object Object] for a message-less error', async () => {
		const store = new SitesStore(
			api({
				deleteSite: vi.fn(async () => {
					throw { kind: 'simulated' };
				})
			})
		);
		await store.removeRow('a');
		expect(store.rowError.a).not.toContain('[object Object]');
		expect(store.rowError.a).not.toBe('');
	});

	// The guard lives in the STORE, not only on a `disabled` attribute — deleting that
	// attribute must not be enough to fire two concurrent writes at one row.
	it('refuses a second concurrent action on the same row', async () => {
		let release: (() => void) | undefined;
		const gate = new Promise<void>((r) => (release = r));
		const a = api({
			deleteSite: vi.fn(async () => {
				await gate;
				return true;
			})
		});
		const store = new SitesStore(a);
		const first = store.removeRow('a');
		const second = await store.removeRow('a'); // while the first is still in flight
		expect(second).toBe(false);
		expect(a.deleteSite).toHaveBeenCalledTimes(1);
		release?.();
		await first;
	});

	it('clears busy even when the action throws, so the row is not stuck disabled', async () => {
		const store = new SitesStore(
			api({
				deleteSite: vi.fn(async () => {
					throw { kind: 'core', message: 'nope' };
				})
			})
		);
		await store.removeRow('a');
		expect(store.busy.a).not.toBe(true);
	});
});

describe('SitesStore.open', () => {
	// Opening a browser changes nothing in state.db, so a refetch afterwards could
	// only produce the list we already have. This is the difference from every other
	// row action, and it is the thing worth pinning.
	it('does not refetch the list after opening', async () => {
		const a = api();
		const s = new SitesStore(a);
		await s.open('a');
		expect(a.openSite).toHaveBeenCalledWith('a');
		expect(a.listSites).not.toHaveBeenCalled();
	});

	it('puts a failure on the row, not on the page banner', async () => {
		const s = new SitesStore(
			api({
				openSite: vi.fn(async () => {
					throw { kind: 'core', message: 'no browser' };
				})
			})
		);
		await s.open('a');
		expect(s.rowError.a).toContain('no browser');
		expect(s.error).toBeNull();
	});

	// A double-click must not open two tabs.
	it('refuses a second concurrent open on the same row', async () => {
		let release: (() => void) | undefined;
		const gate = new Promise<void>((r) => (release = r));
		const a = api({
			openSite: vi.fn(async () => {
				await gate;
			})
		});
		const s = new SitesStore(a);
		const first = s.open('a');
		expect(await s.open('a')).toBe(false);
		expect(a.openSite).toHaveBeenCalledTimes(1);
		release?.();
		await first;
	});

	it('clears busy even when opening throws, so the row is not stuck', async () => {
		const s = new SitesStore(
			api({
				openSite: vi.fn(async () => {
					throw { kind: 'core', message: 'nope' };
				})
			})
		);
		await s.open('a');
		expect(s.busy.a).not.toBe(true);
	});
});
