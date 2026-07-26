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
	//
	// I2: this is a plan Global Constraint, so it must pin the CONSTANTS
	// themselves, not just their ratio. Deriving the expected call counts from
	// the constants (as this test once did) makes it a tautology about the
	// ratio: MEMORY_INTERVAL_MS 2000 -> 60000 destroys the 30x ratio and still
	// passes, and MEMORY_INTERVAL_MS 2000 -> 100 with HOME_INTERVAL_MS 60000 ->
	// 2000 preserves the ratio while hammering the disk with a walk every 2s —
	// the exact idle-cost regression spec §5 exists to prevent — and also still
	// passes. Asserting the literals, and hard-coding the expected counts,
	// makes both mutations fail here.
	it('samples memory 30x more often than the home size', async () => {
		expect(MEMORY_INTERVAL_MS).toBe(2000);
		expect(HOME_INTERVAL_MS).toBe(60000);
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(HOME_INTERVAL_MS);
		expect(a.servicesMemory).toHaveBeenCalledTimes(31); // 1 immediate + 30 ticks @ 2000ms
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(2); // 1 immediate + 1 tick @ 60000ms
		s.stop();
	});

	// The whole point of stop(): an app left open behind an IDE must cost nothing.
	it('issues no further calls after stop', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		const memoryBefore = (a.servicesMemory as unknown as { mock: { calls: unknown[] } }).mock.calls
			.length;
		const homeBefore = (a.homeDiskUsage as unknown as { mock: { calls: unknown[] } }).mock.calls
			.length;
		s.stop();
		// Advance past HOME_INTERVAL_MS, not just a multiple of MEMORY_INTERVAL_MS: the
		// home timer only ticks once a minute, so a shorter advance can never observe a
		// leaked homeTimer even once we also assert on homeDiskUsage below — that
		// mismatch (asserting the fast call but advancing too little to exercise the
		// slow one) is exactly how a stop() that forgot to clear homeTimer once passed
		// this test undetected.
		await vi.advanceTimersByTimeAsync(HOME_INTERVAL_MS + MEMORY_INTERVAL_MS);
		expect(a.servicesMemory).toHaveBeenCalledTimes(memoryBefore);
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(homeBefore);
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

	// I3: the home-side mirror of the test above. A failed sample must return
	// the home figure to unknown too, NOT leave it at its last stale value —
	// same discipline as memory, so a permanently failing home read cannot show
	// a frozen number forever instead of spec §6's `—`. Succeeding FIRST (and
	// asserting the real value) before switching to a rejection is what proves
	// this: a fake that always rejects can never tell "never set" apart from
	// "reverted by the catch", because `homeBytes` starts at `null` anyway.
	it('returns the home figure to unknown when a later sample fails', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		expect(s.homeBytes).toBe(9999);

		(a.homeDiskUsage as unknown as { mockRejectedValue: (e: unknown) => void }).mockRejectedValue({
			kind: 'core',
			message: 'nope'
		});
		await vi.advanceTimersByTimeAsync(HOME_INTERVAL_MS);
		expect(s.homeBytes).toBeNull();
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
	//
	// I4: the guard (`if (this.memoryTimer !== null) return`) is checked once,
	// before EITHER timer is armed, so a second start() must leak neither a
	// memoryTimer NOR a homeTimer. The original version of this test asserted
	// only servicesMemory and advanced just MEMORY_INTERVAL_MS * 3 = 6000ms, so
	// it could never observe a leaked homeTimer (60000ms) — arming homeTimer
	// unconditionally ahead of the guard leaked one and still passed. Advancing
	// past HOME_INTERVAL_MS and asserting homeDiskUsage's count too (the same
	// two-part fix `stop()`'s test already uses above) closes that blind spot.
	it('is idempotent across a second start', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		s.start();
		await vi.advanceTimersByTimeAsync(HOME_INTERVAL_MS);
		expect(a.servicesMemory).toHaveBeenCalledTimes(31); // 1 immediate + 30 ticks @ 2000ms
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(2); // 1 immediate + 1 tick @ 60000ms
		s.stop();
	});

	// I1: the resume seam. `+layout.svelte` calls `start()` on EVERY
	// `visibilitychange` back to visible, not just once at app launch — Cmd+Tab
	// between an IDE and this app is the primary interaction. Without a guard,
	// each resume re-walks the disk as often as the cheap memory read, exactly
	// defeating the two-cadence split (see the file header). Ten hide/show
	// cycles inside one HOME_INTERVAL_MS must cost one walk, not ten, while
	// memory — cheap, and expected to feel live on every return — still samples
	// on every single resume.
	it('does not re-walk the home directory on a resume within one HOME_INTERVAL_MS', async () => {
		const a = api();
		const s = new StatsStore(a);
		for (let i = 0; i < 10; i++) {
			s.start();
			await vi.advanceTimersByTimeAsync(0);
			s.stop();
			// 10 cycles * 2000ms = 20000ms, well inside HOME_INTERVAL_MS (60000ms).
			await vi.advanceTimersByTimeAsync(2000);
		}
		expect(a.servicesMemory).toHaveBeenCalledTimes(10); // every resume
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(1); // only the first
	});
});
