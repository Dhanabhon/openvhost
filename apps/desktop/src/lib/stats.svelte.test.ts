// SPDX-License-Identifier: GPL-3.0-or-later
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { StatsStore, MEMORY_INTERVAL_MS, HOME_INTERVAL_MS } from './stats.svelte';
import type { StatsApi } from './stats.svelte';

function api(overrides: Partial<Record<string, unknown>> = {}): StatsApi {
	return {
		servicesMemory: vi.fn(async () => ({ bytes: 1000, processCount: 2 })),
		homeDiskUsage: vi.fn(async () => ({ bytes: 9999 })),
		...overrides
	} as unknown as StatsApi;
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('StatsStore', () => {
	it('starts with everything unknown, so nothing renders as a false zero', () => {
		const s = new StatsStore(api());
		expect(s.servicesBytes).toBeNull();
		expect(s.processCount).toBeNull();
		expect(s.homeBytes).toBeNull();
		// Distinguishes "not measured yet" from "measurement failed".
		expect(s.homePending).toBe(true);
	});

	it('takes both readings immediately on start', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		expect(a.servicesMemory).toHaveBeenCalledTimes(1);
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(1);
		expect(s.servicesBytes).toBe(1000);
		expect(s.processCount).toBe(2);
		expect(s.homeBytes).toBe(9999);
		expect(s.homePending).toBe(false);
		s.stop();
	});

	// The two cadences are the point of the design: memory is one syscall per pid,
	// the home figure is a directory walk. Sampling them together would either
	// throttle memory or hammer the disk.
	it('samples memory 30x more often than the home size', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(HOME_INTERVAL_MS);
		// 1 immediate + HOME_INTERVAL_MS / MEMORY_INTERVAL_MS ticks
		expect(a.servicesMemory).toHaveBeenCalledTimes(1 + HOME_INTERVAL_MS / MEMORY_INTERVAL_MS);
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(2); // immediate + one tick
		s.stop();
	});

	// The whole point of stop(): an app left open behind an IDE must cost nothing.
	it('issues no further calls after stop', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		const before = (a.servicesMemory as unknown as { mock: { calls: unknown[] } }).mock.calls
			.length;
		s.stop();
		await vi.advanceTimersByTimeAsync(MEMORY_INTERVAL_MS * 10);
		expect(a.servicesMemory).toHaveBeenCalledTimes(before);
	});

	// A failed sample must go back to unknown, NOT to zero: "0 MB · no processes"
	// is a specific, wrong claim, whereas "—" is the truth.
	it('returns a figure to unknown when its sample fails', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		expect(s.servicesBytes).toBe(1000);

		(a.servicesMemory as unknown as { mockRejectedValue: (e: unknown) => void }).mockRejectedValue({
			kind: 'proc',
			message: 'gone'
		});
		await vi.advanceTimersByTimeAsync(MEMORY_INTERVAL_MS);
		expect(s.servicesBytes).toBeNull();
		expect(s.processCount).toBeNull();
		s.stop();
	});

	// A failed FIRST home reading is a failure, not "still measuring" — otherwise
	// the strip says "measuring…" forever.
	it('clears homePending even when the first home reading fails', async () => {
		const s = new StatsStore(
			api({
				homeDiskUsage: vi.fn(async () => {
					throw { kind: 'core', message: 'nope' };
				})
			})
		);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		expect(s.homePending).toBe(false);
		expect(s.homeBytes).toBeNull();
		s.stop();
	});

	// start() twice (a dev-HMR double mount) must not double the polling rate.
	it('is idempotent across a second start', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		s.start();
		await vi.advanceTimersByTimeAsync(MEMORY_INTERVAL_MS * 3);
		expect(a.servicesMemory).toHaveBeenCalledTimes(4); // 1 immediate + 3 ticks
		s.stop();
	});
});
