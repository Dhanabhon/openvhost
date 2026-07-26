// SPDX-License-Identifier: GPL-3.0-or-later
// Status-bar figures and their polling.
//
// Two independent cadences, because the two readings cost wildly different
// amounts: services memory is one syscall per pid, while the home figure is a
// directory walk (measured at 40 ms over 6,470 files). Sampling them together
// would either throttle the cheap one or hammer the disk with the expensive one.
//
// `null` means UNKNOWN and is never coerced to 0 anywhere in this file. A zero
// is a specific claim — "nothing is running" — and rendering a failed sample as
// zero would state it falsely.
//
// DOM-free on purpose: `start()`/`stop()` are called by the layout, which owns
// the `visibilitychange` listener. That keeps this class unit-testable with fake
// timers and no jsdom.
import type { HomeUsageDto, IpcError, ServicesMemoryDto } from './ipc';

export interface StatsApi {
	servicesMemory(): Promise<ServicesMemoryDto>;
	homeDiskUsage(): Promise<HomeUsageDto>;
}

/** Services memory: one syscall per pid, so it can feel live. */
export const MEMORY_INTERVAL_MS = 2000;
/** Home size: a directory walk, so it is deliberately rare. */
export const HOME_INTERVAL_MS = 60000;

export class StatsStore {
	/** Bytes, or `null` for unknown. Never 0 as a stand-in for unknown. */
	servicesBytes = $state<number | null>(null);
	processCount = $state<number | null>(null);
	homeBytes = $state<number | null>(null);
	/**
	 * True until the first home reading SETTLES, either way. Lets the strip say
	 * "measuring…" for a walk in progress while still showing "—" for one that
	 * failed — two different things that would otherwise look identical.
	 */
	homePending = $state(true);
	/** Last sampling error, for diagnostics only. The strip renders "—", never this. */
	lastError = $state<IpcError | null>(null);

	private memoryTimer: ReturnType<typeof setInterval> | null = null;
	private homeTimer: ReturnType<typeof setInterval> | null = null;
	/**
	 * `Date.now()` of the last home reading that SUCCEEDED, or `null` if none
	 * has yet. Lets `start()` skip an immediate re-walk on a resume that lands
	 * within `HOME_INTERVAL_MS` of that success — see `start()`'s doc comment.
	 */
	private lastHomeSuccessAt: number | null = null;

	constructor(private api: StatsApi) {}

	async refreshMemory(): Promise<void> {
		try {
			const r = await this.api.servicesMemory();
			this.servicesBytes = r.bytes;
			this.processCount = r.processCount;
		} catch (e) {
			// Back to unknown, not to zero — see the file header.
			this.servicesBytes = null;
			this.processCount = null;
			this.lastError = e as IpcError;
		}
	}

	async refreshHome(): Promise<void> {
		try {
			this.homeBytes = (await this.api.homeDiskUsage()).bytes;
			this.lastHomeSuccessAt = Date.now();
		} catch (e) {
			this.homeBytes = null;
			this.lastError = e as IpcError;
		} finally {
			// `finally`, so a failed FIRST reading stops claiming "measuring…"
			// forever.
			this.homePending = false;
		}
	}

	/**
	 * Begin polling. Idempotent: a second call while already running is a no-op
	 * rather than a second set of timers, because a dev-HMR double mount would
	 * otherwise silently double the sampling rate.
	 *
	 * The home walk is additionally guarded at the RESUME seam: the layout
	 * calls `start()` on every `visibilitychange` back to visible (Cmd+Tab
	 * between an IDE and this app is the primary interaction), and without
	 * this guard that would re-walk the disk as often as the cheap memory
	 * read, defeating the entire reason the two cadences are split (file
	 * header; spec §4.3/§5). Gated on the last SUCCESS time, not on
	 * `homeBytes === null`: gating on the value would also suppress the retry
	 * after a FAILED read for a full interval, which spec §6 wants retried
	 * instead. The memory read stays unconditional here — it is cheap and the
	 * strip should feel live on every return.
	 */
	start(): void {
		if (this.memoryTimer !== null) return;
		void this.refreshMemory();
		const homeIsFresh =
			this.lastHomeSuccessAt !== null && Date.now() - this.lastHomeSuccessAt < HOME_INTERVAL_MS;
		if (!homeIsFresh) void this.refreshHome();
		this.memoryTimer = setInterval(() => void this.refreshMemory(), MEMORY_INTERVAL_MS);
		this.homeTimer = setInterval(() => void this.refreshHome(), HOME_INTERVAL_MS);
	}

	/** Stop polling. Safe to call when not started. */
	stop(): void {
		if (this.memoryTimer !== null) clearInterval(this.memoryTimer);
		if (this.homeTimer !== null) clearInterval(this.homeTimer);
		this.memoryTimer = null;
		this.homeTimer = null;
	}
}
