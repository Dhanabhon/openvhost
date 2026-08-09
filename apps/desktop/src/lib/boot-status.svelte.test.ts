// SPDX-License-Identifier: GPL-3.0-or-later
//
// The decision behind the takeover: four states, four renderings, and the one
// rule that must NOT be copied from the store slice.
//
// The pair that matters most here is `a failed ask` versus `a degraded answer`.
// Both are "not ready", and collapsing them either way ships a named failure:
// silence on a failed ask (the store slice's rule) leaves a possibly-broken app
// with nothing to say, and a takeover on a failed ask hides a perfectly healthy
// app behind a screen that knows nothing. Each group below is the other's
// control.

import { describe, expect, it, vi } from 'vitest';
import {
	bootRendering,
	BootStatusStore,
	type BootStatusApi,
	type DegradedBoot
} from './boot-status.svelte';
import type { BootStatusDto, IpcError } from './ipc';

const READY: BootStatusDto = { kind: 'ready' };
const ALREADY: DegradedBoot = { kind: 'alreadyRunning', home: '/Users/tom/.openvhost' };
const RUN_DIR: DegradedBoot = {
	kind: 'runDirUnusable',
	path: '/Users/tom/.openvhost/run',
	reason: 'Permission denied (os error 13)'
};
const NO_HOME: DegradedBoot = {
	kind: 'homeUnresolvable',
	reason: 'home directory unavailable'
};
const FAILURE: IpcError = { kind: 'core', message: 'transport died' };

function api(bootStatus: BootStatusApi['bootStatus']): BootStatusApi {
	return { bootStatus };
}

describe('bootRendering', () => {
	// Vacuity, measured, three mutations on the three branches:
	//
	//   * returning `{ kind: 'pending' }` for a failed ask — literally copying
	//     `StoreStatusStore`'s "a failed ask is silence" rule into a gate —
	//     reddened `renders the app anyway` and `wins over a stale answer`, plus
	//     the layout's own `renders the app anyway`, and left every other test in
	//     this file green.
	//   * returning `{ kind: 'app' }` for `status === null` reddened exactly one
	//     test, `renders nothing at all before the first answer arrives`.
	//   * returning `{ kind: 'app' }` for the three degraded arms reddened the
	//     three `renders its own takeover` cases (and seven layout tests) and
	//     left `renders the app when the boot was ready` green.
	//
	// No single mutation reddens more than its own branch, which is what says
	// these are four distinct decisions rather than one boolean wearing a union.

	it('renders nothing at all before the first answer arrives', () => {
		// NOT the app: mounting the real pages on a degraded launch would fire the
		// commands that cannot answer, and leave "no page shows Tauri's `.manage()`
		// string" depending on which promise resolved first.
		expect(bootRendering(null, null)).toEqual({ kind: 'pending' });
	});

	it('renders the app when the boot was ready', () => {
		expect(bootRendering(READY, null)).toEqual({ kind: 'app' });
	});

	it.each([
		['alreadyRunning', ALREADY],
		['runDirUnusable', RUN_DIR],
		['homeUnresolvable', NO_HOME]
	] as const)('renders its own takeover for %s', (_name, boot) => {
		expect(bootRendering(boot, null)).toEqual({ kind: 'takeover', boot });
	});

	describe('an ask that itself failed', () => {
		it('renders the app anyway, never a blank window', () => {
			// The opposite call from `store-status.svelte.ts`, deliberately: what is
			// gated here is the whole window, so silence would mean blanking a
			// working app over one unanswered question.
			expect(bootRendering(null, FAILURE)).toEqual({
				kind: 'appDespiteFailedAsk',
				error: FAILURE
			});
		});

		it('is a different kind from a healthy boot, so the banner cannot be dropped', () => {
			// Both render the children. If they were one kind, deleting the banner
			// would be invisible — and the banner is the entire difference between
			// "we could not tell" and "everything is fine".
			expect(bootRendering(null, FAILURE).kind).not.toBe(bootRendering(READY, null).kind);
		});

		it('wins over a stale answer rather than trusting one nobody could confirm', () => {
			// `load()` clears `status` on failure, so this pairing should not occur —
			// but the ordering is stated here rather than left to that one line,
			// because getting it backwards would show a takeover driven by an answer
			// the app already knows it cannot vouch for.
			expect(bootRendering(RUN_DIR, FAILURE).kind).toBe('appDespiteFailedAsk');
		});
	});
});

describe('BootStatusStore', () => {
	// Vacuity, measured: making `load()` discard the answer and assign
	// `status = null` — the shape an over-eager "reset before reading" refactor
	// produces — reddened `carries the answer through`, `clears a stale answer`
	// and `goes back to a clean answer`, and left `keeps the failure where the
	// banner can render it` green. Making `load()` swallow the rejection instead
	// reddened that one plus the same two ordering tests, and left `carries the
	// answer through` green. Neither mutation reddens the other's anchor test,
	// which is what says the success and failure paths are separately pinned.

	it('knows nothing before the first answer arrives', () => {
		const s = new BootStatusStore(api(vi.fn(async () => RUN_DIR)));
		// No takeover on the first frame of a machine that is perfectly fine.
		expect(s.status).toBeNull();
		expect(s.askFailed).toBeNull();
	});

	it('carries the answer through untouched, verbatim path and errno included', () => {
		const s = new BootStatusStore(api(vi.fn(async () => RUN_DIR)));
		return s.load().then(() => {
			expect(s.status).toEqual(RUN_DIR);
			expect(s.askFailed).toBeNull();
		});
	});

	it('asks exactly once per load', async () => {
		const ask = vi.fn(async () => READY);
		const s = new BootStatusStore(api(ask));
		await s.load();
		expect(ask).toHaveBeenCalledTimes(1);
	});

	describe('an ask that itself failed', () => {
		const reject = () =>
			vi.fn(async (): Promise<BootStatusDto> => {
				throw FAILURE;
			});

		it('resolves rather than rejecting, so the layout needs no catch', async () => {
			const s = new BootStatusStore(api(reject()));
			await expect(s.load()).resolves.toBeUndefined();
		});

		it('keeps the failure where the banner can render it', async () => {
			const s = new BootStatusStore(api(reject()));
			await s.load();
			expect(s.askFailed).toEqual(FAILURE);
			expect(s.status).toBeNull();
		});

		it('clears a stale answer rather than leaving an old takeover on screen', async () => {
			let answer: () => Promise<BootStatusDto> = async () => RUN_DIR;
			const s = new BootStatusStore(api(() => answer()));
			await s.load();
			expect(s.status).toEqual(RUN_DIR);

			answer = async () => {
				throw FAILURE;
			};
			await s.load();
			expect(s.status).toBeNull();
		});

		it('goes back to a clean answer once a later ask succeeds', async () => {
			let answer: () => Promise<BootStatusDto> = async () => {
				throw FAILURE;
			};
			const s = new BootStatusStore(api(() => answer()));
			await s.load();
			expect(s.askFailed).toEqual(FAILURE);

			answer = async () => READY;
			await s.load();
			expect(s.status).toEqual(READY);
			expect(s.askFailed).toBeNull();
		});
	});
});
