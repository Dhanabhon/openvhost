// SPDX-License-Identifier: GPL-3.0-or-later
//
// The store behind the app-level "data store unavailable" banner.
//
// Three states have to stay distinct here, and only one of them may speak:
// a healthy store, a store that reported a reason, and an ask that itself
// failed. Collapsing the third into the second would put a permanent banner on
// a perfectly healthy machine whose IPC hiccuped once — a false statement, made
// silently, which is the exact class of bug this whole slice removes.

import { describe, expect, it, vi } from 'vitest';
import { StoreStatusStore, type StoreStatusApi } from './store-status.svelte';

const REASON = 'unable to open database file (os error 14)';

function api(stateStoreStatus: StoreStatusApi['stateStoreStatus']): StoreStatusApi {
	return { stateStoreStatus };
}

describe('StoreStatusStore', () => {
	// Vacuity, measured: making `load()` discard the answer and assign
	// `reason = null` — a store that asks and then throws the reply away, which
	// is what an over-eager "reset before reading" refactor produces — reddened
	// `carries the real reason …` and `clears a stale reason …`, and left the
	// other six green. The healthy/down pair are each other's control: neither
	// mutation direction can satisfy both.

	it('says nothing at all when the store opened', async () => {
		const s = new StoreStatusStore(api(vi.fn(async () => null)));
		await s.load();
		expect(s.reason).toBeNull();
		expect(s.lastError).toBeNull();
	});

	it('carries the real reason when the store did not open', async () => {
		const s = new StoreStatusStore(api(vi.fn(async () => REASON)));
		await s.load();
		// Verbatim, not summarised: "permission denied" is the only actionable
		// thing the banner can offer, and a generic sentence would be no better
		// than the ".manage()" one this slice replaces.
		expect(s.reason).toBe(REASON);
	});

	it('is silent before the first answer arrives', () => {
		const s = new StoreStatusStore(api(vi.fn(async () => REASON)));
		// No banner on the first frame of a machine that is perfectly fine.
		expect(s.reason).toBeNull();
	});

	it('asks exactly once per load, and asks the command that answers it', async () => {
		const ask = vi.fn(async () => null);
		const s = new StoreStatusStore(api(ask));
		await s.load();
		expect(ask).toHaveBeenCalledTimes(1);
	});

	describe('an ask that itself failed', () => {
		const failure = { kind: 'core' as const, message: 'transport died' };

		it('resolves rather than rejecting, so the layout needs no catch', async () => {
			const s = new StoreStatusStore(
				api(
					vi.fn(async () => {
						throw failure;
					})
				)
			);
			await expect(s.load()).resolves.toBeUndefined();
		});

		it('claims nothing about the store — "could not tell" is not "it is down"', async () => {
			const s = new StoreStatusStore(
				api(
					vi.fn(async () => {
						throw failure;
					})
				)
			);
			await s.load();
			expect(s.reason).toBeNull();
			// …but the failure is not swallowed either: it is kept where a
			// diagnostic can find it, just never rendered as the reason.
			expect(s.lastError).toEqual(failure);
		});

		it('clears a stale reason rather than leaving the old one on screen', async () => {
			// The ordering that a naive `catch { /* keep going */ }` gets wrong:
			// a reason from an earlier load must not outlive an ask that could no
			// longer confirm it.
			let answer: () => Promise<string | null> = async () => REASON;
			const s = new StoreStatusStore(api(() => answer()));
			await s.load();
			expect(s.reason).toBe(REASON);

			answer = async () => {
				throw failure;
			};
			await s.load();
			expect(s.reason).toBeNull();
		});

		it('goes back to silence once a later ask succeeds', async () => {
			let answer: () => Promise<string | null> = async () => {
				throw failure;
			};
			const s = new StoreStatusStore(api(() => answer()));
			await s.load();
			expect(s.lastError).toEqual(failure);

			answer = async () => null;
			await s.load();
			expect(s.reason).toBeNull();
			expect(s.lastError).toBeNull();
		});
	});
});
