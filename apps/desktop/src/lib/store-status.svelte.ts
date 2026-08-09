// SPDX-License-Identifier: GPL-3.0-or-later
// Whether OpenVHost's data store (`state.db`) opened this run.
//
// The honesty half of the optional-state.db slice (design D5). With no store,
// several commands DEGRADE — they do their real work and quietly drop the part
// that came from `state.db` — so `list_log_sources` returns a shorter list and
// `php_environment` reports no chosen default. A shorter list is
// indistinguishable from "you have no sites"; that is a quiet wrong answer, and
// this store is what lets one app-level banner say so out loud.
//
// Three states, not two, and none of them is a boolean: `null` = the store is
// fine, a string = it is down and this is why, `undefined`… is deliberately not
// used — see `reason` below for how "we could not tell" is represented, and why
// it must NOT render as "the store is down".
//
// DOM-free and api-injected, the same shape as `stats.svelte.ts`: the layout
// calls `load()`, and this module's tests hand it a fake.
import type { IpcError } from './ipc';

export interface StoreStatusApi {
	stateStoreStatus: () => Promise<string | null>;
}

export class StoreStatusStore {
	/**
	 * Why the store is unavailable, or `null` for "no problem to report".
	 *
	 * `null` is also the state BEFORE the first answer arrives and after a
	 * failed ask, which is deliberate: "we could not tell" and "there is nothing
	 * wrong" must both render as silence. Claiming the store is down because the
	 * question itself failed would be the same class of false statement this
	 * whole slice exists to remove — the Sites page draws the same distinction
	 * for its readiness banner ("a failed read is not an absence").
	 */
	reason = $state<string | null>(null);
	/** The failed ask itself, for diagnostics. Never rendered as the reason. */
	lastError = $state<IpcError | null>(null);

	constructor(private api: StoreStatusApi) {}

	async load(): Promise<void> {
		try {
			this.reason = await this.api.stateStoreStatus();
			this.lastError = null;
		} catch (e) {
			// Back to silence, not to a fabricated reason. See `reason` above.
			this.reason = null;
			this.lastError = e as IpcError;
		}
	}
}
