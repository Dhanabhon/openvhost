// SPDX-License-Identifier: GPL-3.0-or-later
// Moved here from `ipc/ipc.test.ts` (that file is about the IPC barrel) and
// extended for the shared-instance shape: one deduped snapshot, a store-owned
// error channel, and start/stop as store methods.
import { describe, expect, it, vi } from 'vitest';
import { ServicesStore } from './services.svelte';
import type { ServicesApi } from './services.svelte';
import type { LogLine, ServiceStatus } from './ipc';

const svc = (id: string, kind: 'stopped' | 'running'): ServiceStatus =>
	({ id, displayName: id, endpoint: null, pid: null, state: { kind } }) as ServiceStatus;

const line = (l: string): LogLine => ({ tsMs: 1, level: 'info', line: l });

/** `as unknown as ServicesApi` so callers keep a typed reference to assert on —
 *  the same reason `sites.svelte.test.ts` does it. */
function api(overrides: Partial<Record<string, unknown>> = {}): ServicesApi {
	return {
		listServices: vi.fn(async () => [svc('demo-ticker', 'stopped')]),
		serviceLogTail: vi.fn(async () => [line('seed')]),
		startService: vi.fn(async () => {}),
		stopService: vi.fn(async () => {}),
		...overrides
	} as unknown as ServicesApi;
}

describe('ServicesStore snapshot', () => {
	it('loadServices() fills services', async () => {
		const store = new ServicesStore(api());
		await store.loadServices();
		expect(store.services.map((s) => s.id)).toEqual(['demo-ticker']);
	});

	// The layout hoists `loadServices()` for the titlebar count while the Services
	// page calls `loadLogTail()`, and children mount BEFORE their parent layout — so
	// without deduping these two would each fire their own `list_services`.
	it('shares one listServices() round trip between concurrent callers', async () => {
		const a = api();
		const store = new ServicesStore(a);
		await Promise.all([store.loadServices(), store.loadLogTail(), store.loadServices()]);
		expect(a.listServices).toHaveBeenCalledTimes(1);
	});

	it('captures a load failure on error instead of rejecting', async () => {
		const store = new ServicesStore(
			api({
				listServices: vi.fn(async () => {
					throw { kind: 'proc', message: 'supervisor unavailable' };
				})
			})
		);
		await expect(store.loadServices()).resolves.toBeUndefined();
		expect(store.error?.kind).toBe('proc');
		expect(store.services).toEqual([]);
	});

	// A failed snapshot must not be cached as "done", or the app would keep an empty
	// service list — and a permanently "0 running" titlebar — for the whole session.
	it('retries after a failed load rather than caching the failure', async () => {
		const listServices = vi
			.fn<() => Promise<ServiceStatus[]>>()
			.mockRejectedValueOnce({ kind: 'proc', message: 'not up yet' })
			.mockResolvedValueOnce([svc('demo-ticker', 'running')]);
		const store = new ServicesStore(api({ listServices }));
		await store.loadServices();
		await store.loadServices();
		expect(listServices).toHaveBeenCalledTimes(2);
		expect(store.services.map((s) => s.state.kind)).toEqual(['running']);
	});
});

describe('ServicesStore log feed', () => {
	it('loadLogTail() seeds from the first service, snapshotting it first', async () => {
		const a = api();
		const store = new ServicesStore(a);
		await store.loadLogTail();
		expect(a.serviceLogTail).toHaveBeenCalledWith('demo-ticker', 50);
		expect(store.logs.map((l) => ({ id: l.id, line: l.line }))).toEqual([
			{ id: 'demo-ticker', line: 'seed' }
		]);
	});

	it('loadLogTail() is a no-op when no service exists', async () => {
		const a = api({ listServices: vi.fn(async () => []) });
		const store = new ServicesStore(a);
		await store.loadLogTail();
		expect(a.serviceLogTail).not.toHaveBeenCalled();
		expect(store.logs).toEqual([]);
	});

	it('captures a tail failure on error', async () => {
		const store = new ServicesStore(
			api({
				serviceLogTail: vi.fn(async () => {
					throw { kind: 'proc', message: 'log file gone' };
				})
			})
		);
		await store.loadLogTail();
		expect(store.error?.kind).toBe('proc');
	});

	it('applyLog caps the feed at 50', () => {
		const store = new ServicesStore(api());
		for (let i = 0; i < 60; i++) {
			store.applyLog({ id: 'x', tsMs: i, level: 'info', line: `l${i}` });
		}
		expect(store.logs).toHaveLength(50);
		expect(store.logs[0]?.line).toBe('l10');
	});
});

describe('ServicesStore actions', () => {
	it('applyState replaces the matching service state', async () => {
		const store = new ServicesStore(api());
		await store.loadServices();
		store.applyState({ id: 'demo-ticker', state: { kind: 'running' }, detail: null });
		expect(store.services[0]?.state.kind).toBe('running');
	});

	// I1 audit finding: `Supervisor::register` emits no event, so a service
	// registered AFTER this store's one `loadServices()` snapshot arrives here
	// only as a `StateChanged` for an id `.map` has never seen — and `.map`
	// over the EXISTING array can only ever match or leave alone, never add.
	// This is the drop `reload()` exists to work around; pinned directly so a
	// future "helpful" rewrite of `applyState` cannot silently start growing
	// the list instead (which would be a real, and different, behaviour change)
	// without a test noticing either way.
	it('drops a StateChanged event for an id it has never registered', async () => {
		const store = new ServicesStore(api());
		await store.loadServices();
		store.applyState({ id: 'php-fpm-8.4', state: { kind: 'running' }, detail: null });
		expect(store.services.map((s) => s.id)).toEqual(['demo-ticker']);
		expect(store.services.some((s) => s.id === 'php-fpm-8.4')).toBe(false);
	});
});

// Task 1 of the tray slice (`SupervisorEvent::Registered`): unlike
// `applyState` above, a `Registered` event is allowed to GROW the list — it
// is the fix for the exact gap `applyState`'s drop test just pinned. This is
// the "the frontend store actually reacts" half of the fix; the Rust event
// alone would be silent without it.
describe('ServicesStore.applyRegistered', () => {
	it('appends a service this store has never seen, in id order', async () => {
		const store = new ServicesStore(api());
		await store.loadServices(); // seeds ['demo-ticker']
		store.applyRegistered(svc('php-fpm-8.4', 'stopped'));
		expect(store.services.map((s) => s.id)).toEqual(['demo-ticker', 'php-fpm-8.4']);
	});

	it('inserts in sorted-by-id position, matching Supervisor::snapshot() order', async () => {
		const store = new ServicesStore(api());
		await store.loadServices(); // seeds ['demo-ticker']
		store.applyRegistered(svc('a-earlier', 'stopped'));
		expect(store.services.map((s) => s.id)).toEqual(['a-earlier', 'demo-ticker']);
	});

	it('replaces rather than duplicates an already-known id', async () => {
		const store = new ServicesStore(api());
		await store.loadServices(); // seeds ['demo-ticker']
		store.applyRegistered(svc('demo-ticker', 'stopped'));
		expect(store.services.map((s) => s.id)).toEqual(['demo-ticker']);
	});

	it('a service registered before any snapshot still appears without a restart', () => {
		// No loadServices() at all — mirrors a Registered event arriving while
		// the very first snapshot is still in flight.
		const store = new ServicesStore(api());
		store.applyRegistered(svc('mysql-8.4', 'stopped'));
		expect(store.services.map((s) => s.id)).toEqual(['mysql-8.4']);
	});
});

// Task 1 of the package-uninstall slice (`SupervisorEvent::Unregistered`):
// the mirror of `applyRegistered`. Without this half, uninstalling a PHP
// major would leave its row on the Services page (and in the titlebar count)
// until the next relaunch — the exact "it simply fails honestly the next time
// it is started" behaviour the slice exists to end.
describe('ServicesStore.applyUnregistered', () => {
	it('drops the service that was removed', async () => {
		const store = new ServicesStore(
			api({
				listServices: vi.fn(async () => [svc('nginx', 'running'), svc('php-fpm-8.3', 'stopped')])
			})
		);
		await store.loadServices();
		store.applyUnregistered('php-fpm-8.3');
		expect(store.services.map((s) => s.id)).toEqual(['nginx']);
	});

	it('leaves every other service untouched', async () => {
		const store = new ServicesStore(
			api({
				listServices: vi.fn(async () => [
					svc('nginx', 'running'),
					svc('php-fpm-8.3', 'stopped'),
					svc('php-fpm-8.4', 'running')
				])
			})
		);
		await store.loadServices();
		store.applyUnregistered('php-fpm-8.3');
		expect(store.services.map((s) => [s.id, s.state.kind])).toEqual([
			['nginx', 'running'],
			['php-fpm-8.4', 'running']
		]);
	});

	// The event is broadcast to every subscriber, and this store may never
	// have loaded the id (a snapshot still in flight, or a service registered
	// and removed between two loads). Dropping nothing must be silent, not a
	// throw that lands on the error banner.
	it('is a no-op for an id it does not know, including a repeat', async () => {
		const store = new ServicesStore(api());
		await store.loadServices(); // seeds ['demo-ticker']
		store.applyUnregistered('never-seen');
		expect(store.services.map((s) => s.id)).toEqual(['demo-ticker']);
		store.applyUnregistered('demo-ticker');
		store.applyUnregistered('demo-ticker');
		expect(store.services).toEqual([]);
		expect(store.error).toBeNull();
	});

	// Spec D2: an uninstall keeps the logs — they are usually WHY the user
	// uninstalled. The store's feed is history, not a live view of the
	// registry, so removing a row must not retroactively erase what it said.
	it('does not touch the log feed', async () => {
		const store = new ServicesStore(api());
		await store.loadServices();
		store.applyLog({ id: 'demo-ticker', tsMs: 1, level: 'error', line: 'why it died' });
		store.applyUnregistered('demo-ticker');
		expect(store.logs.map((l) => l.line)).toEqual(['why it died']);
	});
});

describe('ServicesStore.reload', () => {
	// I1's cheap fix: `reload()` is the escape hatch a caller (the Languages
	// page, after a successful install/rescan) uses to see a newly-registered
	// service without a relaunch — proven here by having `listServices` return
	// a DIFFERENT set on its second call and asserting `reload()` actually
	// re-fetches rather than returning the memoized first snapshot the way
	// `loadServices()` would.
	it('forces a fresh fetch even after loadServices() has already memoized one', async () => {
		const listServices = vi
			.fn<() => Promise<ServiceStatus[]>>()
			.mockResolvedValueOnce([svc('demo-ticker', 'stopped')])
			.mockResolvedValueOnce([svc('demo-ticker', 'stopped'), svc('php-fpm-8.4', 'stopped')]);
		const store = new ServicesStore(api({ listServices }));

		await store.loadServices();
		expect(store.services.map((s) => s.id)).toEqual(['demo-ticker']);

		// A second `loadServices()` would return the SAME memoized promise and
		// NOT call the api again — this is exactly what made the newly
		// registered service invisible without `reload()`.
		await store.loadServices();
		expect(listServices).toHaveBeenCalledTimes(1);

		await store.reload();
		expect(listServices).toHaveBeenCalledTimes(2);
		expect(store.services.map((s) => s.id)).toEqual(['demo-ticker', 'php-fpm-8.4']);
	});

	it('start()/stop() call through to the api', async () => {
		const a = api();
		const store = new ServicesStore(a);
		await store.start('demo-ticker');
		await store.stop('demo-ticker');
		expect(a.startService).toHaveBeenCalledWith('demo-ticker');
		expect(a.stopService).toHaveBeenCalledWith('demo-ticker');
	});

	it('a failed action lands on error without rejecting', async () => {
		const store = new ServicesStore(
			api({
				startService: vi.fn(async () => {
					throw { kind: 'proc', message: 'port 80 busy' };
				})
			})
		);
		await expect(store.start('demo-ticker')).resolves.toBeUndefined();
		expect(store.error).toEqual({ kind: 'proc', message: 'port 80 busy' });
	});

	it("a new action clears the previous attempt's error", async () => {
		const store = new ServicesStore(api());
		store.fail({ kind: 'proc', message: 'port 80 busy' });
		await store.stop('demo-ticker');
		expect(store.error).toBeNull();
	});
});
