// SPDX-License-Identifier: GPL-3.0-or-later
// Logs page state (P1 live-log-viewer design, spec D3/D4/D6:
// docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md). Page-local,
// not shared: unlike `ServicesStore`/`StatsStore` (visible on every route —
// the titlebar count, the status bar), nothing outside `/logs` needs this
// state, so `routes/logs/+page.svelte` constructs a fresh instance per
// mount, the same way `routes/+page.svelte` does for `SitesStore`.
//
// Interpretive note on the poll gate (spec D3: "'Follow' = a 500 ms poll,
// alive only while the Logs route is mounted and Follow is on"): this store
// deliberately does NOT gate the interval on the `follow` boolean itself.
// `start()`/`stop()` are keyed to (mounted, window visible) only — exactly
// `StatsStore`'s existing contract — and `follow` here controls two
// narrower things: whether the viewport auto-scrolls (the component's job,
// DOM-only) and whether `newRowsWhilePaused` gets set. Reasoning: spec D6
// separately requires "reveals 'Jump to latest' when new lines arrived"
// while the user is scrolled away reading history, which is only honest if
// rows keep arriving in the background — gating the poll on `follow` would
// make that badge either lie (always shown, never counts anything) or never
// fire at all. The "tested requirement" the spec calls out — "teardown on
// route change/blur is a tested requirement, not an assumption" — is
// rigorously implemented and tested here via `start()`/`stop()`; see
// task-6-report.md for this call flagged explicitly for the owner.
//
// Spec D7's two-mechanism seam, deliberately NOT unified (whole-branch
// review CRITICAL finding): a `"file"` source (nginx/php-fpm/site rows) is
// read through `readLogWindow` + this store's own poll. A `"ring"` source
// (`{kind:'serviceRing', id}`) is read through the EXISTING
// `service_log_tail` (one-shot tail) + `service-log` push event — `derive_path`
// in `commands.rs` REJECTS a ring source with a validation error, by design,
// so it must never reach `readLogWindow` at all. `selectSource` branches on
// `source.kind` right at the top for exactly this reason; `refresh()` (the
// poll's own entry point) carries a second, independent guard against the
// same mistake, so a caller cannot reach `readLogWindow` for a ring source
// through EITHER path. `ringLogs`/`startRingSubscription`/
// `stopRingSubscription` are the ring-only counterparts to `rows`/`start`/
// `stop` — kept as separate, clearly-named members rather than folded into
// the file-source ones, so the two mechanisms stay visibly distinct at the
// call site, matching spec D7's "two mechanisms, deliberately, documented
// at the seam."
import type {
	IpcError,
	LogLevel,
	LogLine,
	LogResetDto,
	LogRowDto,
	LogSourceDto,
	LogSourceRowDto,
	LogWindowDto,
	LogWindowQuery,
	ServiceLogEvent
} from './ipc';
import { isSourceListed, truncateToUtf8Bytes, LOG_NEEDLE_MAX_BYTES } from './logs.derive';

export interface LogsApi {
	listLogSources(): Promise<LogSourceRowDto[]>;
	readLogWindow(query: LogWindowQuery): Promise<LogWindowDto>;
	revealLogFolder(source: LogSourceDto): Promise<void>;
	/** Ring sources only (spec D7) — the one-shot tail half of the seam. */
	serviceLogTail(id: string, n: number): Promise<LogLine[]>;
	/** Ring sources only (spec D7) — the live push half of the seam. */
	onServiceLog(cb: (ev: ServiceLogEvent) => void): Promise<() => void>;
}

/** Spec D3's poll cadence. */
export const POLL_INTERVAL_MS = 500;
/** Client-side accumulation cap, mirroring the server's own `DEFAULT_ROWS`
 *  (spec D3: 500) — the "no virtualization" design (spec D6) relies on the
 *  rendered set staying bounded by construction; without this cap that is
 *  only true for a single call, not for a long Follow session where the
 *  cursor keeps advancing across many polls. Oldest rows are evicted first,
 *  the same discipline `ServicesStore.applyLog`'s `LOG_CAP` already uses. */
export const MAX_RENDERED_ROWS = 500;
/** Ring-log accumulation cap — reuses `ServicesStore.LOG_CAP`'s own value
 *  (50) rather than a new, untested figure: the same cap already proven
 *  against the real supervisor ring buffer via `serviceLogTail`. */
export const RING_LOG_CAP = 50;

export class LogsStore {
	sources = $state<LogSourceRowDto[]>([]);
	sourcesError = $state<IpcError | null>(null);

	selected = $state<LogSourceDto | null>(null);
	/** A `?source=` deep link that named something NOT in `sources` right
	 *  now. Deliberately never paired with a fallback selection — see
	 *  `selectFromDeepLink`'s doc comment. */
	requestedUnavailable = $state<LogSourceDto | null>(null);

	rows = $state<LogRowDto[]>([]);
	/** The ring-source counterpart to `rows` — populated only when `selected`
	 *  is a `serviceRing` source. Structurally identical to
	 *  `services.svelte.ts`'s `UiLog` (`ServiceLogEvent` already carries
	 *  `id`), so it feeds `LogPane.svelte`'s `logs` prop directly. */
	ringLogs = $state<ServiceLogEvent[]>([]);
	exists = $state(true);
	reset = $state<LogResetDto | null>(null);
	hasMore = $state(false);
	sizeBytes = $state(0);
	scannedBytes = $state(0);
	truncatedLines = $state(0);
	scanBoundReached = $state(false);
	readError = $state<IpcError | null>(null);
	/** Whether at least one read has settled (success or failure) for the
	 *  CURRENT selection — lets the UI tell "still loading" apart from a
	 *  genuinely empty result. */
	loaded = $state(false);

	follow = $state(true);
	/** Set when a poll delivers new rows while `follow` is off — drives the
	 *  "Jump to latest" affordance (spec D6). Cleared by `setFollow(true)`
	 *  and by `jumpToLatest()`. */
	newRowsWhilePaused = $state(false);

	needle = $state('');
	caseSensitive = $state(false);
	minLevel = $state<LogLevel | null>(null);

	private cursor: string | null = null;
	private timer: ReturnType<typeof setInterval> | null = null;
	/** Bumped on every state reset (`selectSource`/filter change). A
	 *  `refresh()` call captures it before awaiting the IPC round trip and
	 *  discards its own response if the generation moved on in the
	 *  meantime — the guard against a slow response for a superseded
	 *  selection contaminating the CURRENT one's rows. See
	 *  `inFlightGeneration` below for the DIFFERENT guard this alone does
	 *  not provide: against two overlapping calls that share this SAME
	 *  value. */
	private generation = 0;
	/**
	 * Re-entrancy guard, keyed on generation rather than a blanket flag:
	 * the generation a currently in-flight `readLogWindow` call belongs to,
	 * or `null` when none is in flight. Without this, the poll's next tick
	 * firing before a `>= POLL_INTERVAL_MS` read resolves (`read_window`
	 * bounds bytes scanned, not wall-clock time) produces two calls that
	 * share the same `generation` — `generation` alone does not catch
	 * this, since it exists to discard a response for a SUPERSEDED
	 * SELECTION, and two overlapping polls of the SAME selection are not
	 * that. Both would read the identical `this.cursor` and, since neither
	 * is "fresh", both would hit `applyWindow`'s append path — duplicating
	 * rows (review finding on this task; regression test: "does not
	 * duplicate rows when a second refresh starts before the first
	 * resolves").
	 *
	 * Keyed on generation, NOT a plain boolean, so a genuinely NEW
	 * selection is never held back by a stale call still finishing for the
	 * PREVIOUS one: `selectSource`/`restart` bump `generation` before
	 * calling `refresh()`, so their own call always has a generation that
	 * DIFFERS from whatever a stale in-flight call was captured with, and
	 * `refresh()` lets it proceed immediately — the stale one is left to
	 * resolve on its own and gets discarded by the existing generation
	 * check above, exactly as it already was before this guard existed.
	 * (An earlier, plain-boolean version of this guard blocked that case
	 * too and broke the pre-existing "stale-response guard" test — see
	 * that regression for the trace.) A call sharing the CURRENT
	 * generation with one already in flight (two poll ticks, or a poll
	 * tick racing `jumpToLatest`, neither of which bumps generation) is
	 * SKIPPED, not queued: `read_log_window` always resumes from `cursor`,
	 * so a skipped call loses no data — the next call (the following poll
	 * tick, at most `POLL_INTERVAL_MS` later, or the next explicit action)
	 * reads the same cursor the skipped call would have and simply catches
	 * up further.
	 */
	private inFlightGeneration: number | null = null;

	/** The live `service-log` subscription's unlisten function, or `null`
	 *  when not subscribed. Distinct from `ringSubscriptionEpoch` below —
	 *  the registration itself is async (`onServiceLog` returns a
	 *  `Promise<() => void>`), so there is a window where a subscription is
	 *  INTENDED but not yet actually registered. */
	private ringUnlisten: (() => void) | null = null;
	/**
	 * Re-entrancy guard for the ring subscription, keyed on an epoch —
	 * the SAME idiom `inFlightGeneration`/`generation` already use for
	 * `refresh()`'s guard, reused here rather than inventing a second one
	 * (per review instruction: "one idiom in this file, not two").
	 * Incremented on every `startRingSubscription()` ATTEMPT and on every
	 * `stopRingSubscription()` call; a registration commits `ringUnlisten`
	 * only if the epoch it captured before awaiting still matches when
	 * `onServiceLog` resolves — otherwise something else (a stop, or
	 * another start) has already superseded it, and it unregisters itself
	 * immediately instead of leaving a zombie listener behind or silently
	 * overwriting a newer, already-committed one.
	 *
	 * Without this (whole-branch review finding — a blur/focus flicker,
	 * the window is one Tauri `listen()` registration wide):
	 * `startRingSubscription` (A) begins awaiting registration; blur runs
	 * `stopRingSubscription()` (nothing to unlisten yet — A has not
	 * resolved); focus runs `startRingSubscription()` (B) again; A
	 * resolves and commits `ringUnlisten = unlistenA` (a plain "am I still
	 * meant to be active" boolean has no way to know A itself was
	 * superseded, since B already flipped it back to true); B resolves and
	 * OVERWRITES `ringUnlisten = unlistenB` WITHOUT ever calling
	 * `unlistenA()`. A's real registration survives forever — even past
	 * unmount's own `stopRingSubscription()`, which now only ever reaches
	 * B — and BOTH fire `applyRingLog` for every future event, duplicating
	 * every ring row. Keying on a monotonic epoch instead fixes this: A
	 * and B capture DIFFERENT epoch values, so whichever one resolves
	 * LAST is the only one that can still match `this.ringSubscriptionEpoch`
	 * — the other recognizes itself as stale and cleans up on the spot,
	 * regardless of resolution order. */
	private ringSubscriptionEpoch = 0;

	constructor(private api: LogsApi) {}

	async loadSources(): Promise<void> {
		try {
			this.sources = await this.api.listLogSources();
			this.sourcesError = null;
		} catch (e) {
			this.sourcesError = e as IpcError;
			this.sources = [];
		}
	}

	/**
	 * Resolve a `?source=` deep link against the already-loaded catalogue
	 * (call after `loadSources()`). A source that IS listed is selected
	 * normally. A source that is requested but NOT listed (a deleted site,
	 * an uninstalled PHP major) is recorded on `requestedUnavailable` and
	 * NOTHING is auto-selected in its place — a silent fallback to nginx
	 * behind an "unavailable" banner would flash real content the link
	 * never promised the instant the banner is dismissed, and would waste a
	 * read the user did not ask for. The picker stays visible; the user
	 * picks something themselves. `requested: null` (no deep link at all)
	 * falls back to the nginx error log, since nginx's globals are always
	 * listed (spec D7) and are a reasonable, always-available landing view.
	 */
	async selectFromDeepLink(requested: LogSourceDto | null): Promise<void> {
		if (requested === null) {
			this.requestedUnavailable = null;
			const fallback = this.sources.find((s) => s.source.kind === 'nginxError');
			if (fallback) await this.selectSource(fallback.source);
			return;
		}
		if (isSourceListed(this.sources, requested)) {
			this.requestedUnavailable = null;
			await this.selectSource(requested);
			return;
		}
		this.requestedUnavailable = requested;
	}

	/** Select a source and load its first window. Resets every piece of
	 *  per-source state (rows, ring logs, cursor, filter-independent facts,
	 *  follow) so nothing from the previous source can leak into the new
	 *  one — including across a FILE↔RING switch, which is why `ringLogs`
	 *  is cleared here unconditionally rather than only on the ring branch
	 *  below.
	 *
	 *  Branches on `source.kind` here, at the single entry point every
	 *  selection goes through, rather than only inside `refresh()`: spec
	 *  D7's two mechanisms are chosen ONCE, at selection time, not
	 *  re-decided on every read. A `serviceRing` source never calls
	 *  `refresh()`/`readLogWindow` at all (see `selectRingSource`); the
	 *  guard inside `refresh()` itself is a second, independent line of
	 *  defence, not the only one. */
	async selectSource(source: LogSourceDto): Promise<void> {
		this.generation += 1;
		this.selected = source;
		this.requestedUnavailable = null;
		this.rows = [];
		this.ringLogs = [];
		this.cursor = null;
		this.exists = true;
		this.reset = null;
		this.hasMore = false;
		this.readError = null;
		this.loaded = false;
		this.follow = true;
		this.newRowsWhilePaused = false;

		if (source.kind === 'serviceRing') {
			await this.selectRingSource(source.id);
			return;
		}
		await this.refresh();
	}

	/** The ring-source half of `selectSource` (spec D7): a one-shot
	 *  `service_log_tail` call, NEVER `readLogWindow` — `resolve_log_path`
	 *  rejects a ring source with a validation error by design, so this
	 *  must be the only read this store ever issues for one. Live updates
	 *  arrive separately, through `applyRingLog` via the subscription
	 *  `startRingSubscription()` owns. Guarded by `generation` exactly like
	 *  `refresh()` is, so a slow tail response for a source the user has
	 *  since switched away from cannot land on top of the new one. */
	private async selectRingSource(id: string): Promise<void> {
		const generation = this.generation;
		try {
			const tail = await this.api.serviceLogTail(id, RING_LOG_CAP);
			if (generation !== this.generation) return; // superseded
			this.ringLogs = tail.map((l) => ({ ...l, id }));
			this.readError = null;
		} catch (e) {
			if (generation !== this.generation) return;
			this.readError = e as IpcError;
		} finally {
			if (generation === this.generation) this.loaded = true;
		}
	}

	/** The live half of the ring seam: applies one `service-log` push event,
	 *  but ONLY when it belongs to the currently-selected ring source —
	 *  everything else (a different service, or no ring source selected at
	 *  all right now) is silently ignored. This check is by `id`, not
	 *  `generation`: the subscription is registered once for the whole
	 *  store lifetime (`startRingSubscription`), independent of whichever
	 *  source happens to be selected at any given moment, so there is no
	 *  single "current generation" this callback could compare itself
	 *  against the way `refresh()`'s response handler does. */
	private applyRingLog(ev: ServiceLogEvent): void {
		if (this.selected === null || this.selected.kind !== 'serviceRing') return;
		if (this.selected.id !== ev.id) return;
		const next = [...this.ringLogs, ev];
		this.ringLogs = next.length > RING_LOG_CAP ? next.slice(next.length - RING_LOG_CAP) : next;
	}

	/** A filter field changed: re-run from a fresh tail (spec D4 — an active
	 *  query seeks back the full scan bound, not just the loaded window, so
	 *  this is how a match older than the visible tail gets found), keeping
	 *  the current source and follow state. */
	private async restart(): Promise<void> {
		if (this.selected === null) return;
		this.generation += 1;
		this.rows = [];
		this.cursor = null;
		this.reset = null;
		this.loaded = false;
		await this.refresh();
	}

	async setNeedle(raw: string): Promise<void> {
		this.needle = truncateToUtf8Bytes(raw, LOG_NEEDLE_MAX_BYTES);
		await this.restart();
	}

	async setCaseSensitive(on: boolean): Promise<void> {
		this.caseSensitive = on;
		await this.restart();
	}

	async setMinLevel(level: LogLevel | null): Promise<void> {
		this.minLevel = level;
		await this.restart();
	}

	/** Auto-scroll on/off. Never touches `rows`/`cursor` — only
	 *  `selectSource`/`restart` (a genuinely new read target) do that. */
	setFollow(on: boolean): void {
		this.follow = on;
		if (on) this.newRowsWhilePaused = false;
	}

	/** "Jump to latest": resume following and catch up immediately, rather
	 *  than waiting for the next poll tick. The component's own auto-scroll
	 *  effect (keyed on `follow`) does the actual scrolling once `rows`
	 *  updates. */
	async jumpToLatest(): Promise<void> {
		this.setFollow(true);
		await this.refresh();
	}

	/** One bounded read for the current selection, resuming from `cursor`.
	 *  Never throws — a poll tick calling this must not crash on a
	 *  transient failure; the failure lands on `readError` instead, which a
	 *  later successful call clears. A no-op when nothing is selected, so a
	 *  poll timer left running for a moment after `selected` is cleared
	 *  costs nothing. Also a no-op (silently, resolving immediately) when a
	 *  call already in flight shares its generation — see
	 *  `inFlightGeneration`'s doc comment for why a DIFFERENT generation
	 *  (a genuinely new selection) is never held back by this.
	 *
	 *  A SECOND, independent no-op guard: a `serviceRing` source must NEVER
	 *  reach `readLogWindow` (spec D7 — `resolve_log_path` rejects one by
	 *  design). `selectSource` already never calls this method for a ring
	 *  selection, but the poll's own timer calls `refresh()` directly on
	 *  every tick without knowing what is currently selected — this is what
	 *  makes "the poll must not run for [ring sources]" true even if a
	 *  selection changes kind WHILE the poll is armed, rather than relying
	 *  on `selectSource`'s branch alone. */
	async refresh(): Promise<void> {
		if (this.selected === null || this.selected.kind === 'serviceRing') return;
		if (this.inFlightGeneration === this.generation) return;
		const generation = this.generation;
		this.inFlightGeneration = generation;
		const source = this.selected;
		try {
			const w = await this.api.readLogWindow({
				source,
				cursor: this.cursor,
				needle: this.needle === '' ? null : this.needle,
				caseSensitive: this.caseSensitive,
				minLevel: this.minLevel
			});
			if (generation !== this.generation) return; // superseded — see `generation`'s doc comment
			this.applyWindow(w);
			this.readError = null;
		} catch (e) {
			if (generation !== this.generation) return;
			this.readError = e as IpcError;
		} finally {
			if (generation === this.generation) this.loaded = true;
			if (this.inFlightGeneration === generation) this.inFlightGeneration = null;
		}
	}

	private applyWindow(w: LogWindowDto): void {
		this.cursor = w.cursor;
		this.exists = w.exists;
		this.reset = w.reset;
		this.hasMore = w.hasMore;
		this.sizeBytes = w.sizeBytes;
		this.scannedBytes = w.scannedBytes;
		this.truncatedLines = w.truncatedLines;
		this.scanBoundReached = w.scanBoundReached;

		// A reset or a missing file means `w.rows` is a FRESH tail, not a
		// continuation of what is already on screen — replacing (never
		// appending) is what makes a reset never double-print (spec D3).
		const fresh = w.reset !== null || !w.exists;
		const merged = fresh ? w.rows : [...this.rows, ...w.rows];
		this.rows =
			merged.length > MAX_RENDERED_ROWS ? merged.slice(merged.length - MAX_RENDERED_ROWS) : merged;

		// Review finding (minor a): a reset silently swaps the whole view
		// out from under a reader scrolled away from the bottom — MORE
		// jarring than ordinary new rows arriving, not less — so it earns
		// the same "Jump to latest" affordance those get, not only its own
		// separate reset notice (LogBody's unconditional banner covers the
		// "what happened"; this flag covers "there is something new to
		// look at"). Still gated on `w.rows.length > 0`: an empty fresh
		// tail (truncated to nothing) has nothing to jump to.
		if (!this.follow && w.rows.length > 0) this.newRowsWhilePaused = true;
	}

	/** Begin polling. Idempotent, mirroring `StatsStore.start()` — a second
	 *  call (a dev-HMR double mount) must not arm a second timer. Owned by
	 *  the page's `onMount`, gated there on mount + document visibility;
	 *  see the file header for why `follow` is not itself a gate here. */
	start(): void {
		if (this.timer !== null) return;
		this.timer = setInterval(() => void this.refresh(), POLL_INTERVAL_MS);
	}

	/** Stop polling. Safe to call when not started — the teardown path this
	 *  slice's "tested requirement" (spec D3) is about: called on route
	 *  unmount and on window blur alike, so an interval can never outlive
	 *  either. */
	stop(): void {
		if (this.timer !== null) clearInterval(this.timer);
		this.timer = null;
	}

	/** Begin the ring seam's live half (spec D7): subscribes to `service-log`
	 *  ONCE for the whole store lifetime, independent of which source
	 *  happens to be selected at any moment — `applyRingLog` filters
	 *  incoming events against the CURRENT selection itself, so there is no
	 *  need to re-subscribe on every ring-to-ring switch. Owned by the
	 *  page's `onMount`, called alongside `start()` (same mount +
	 *  visibility gate) — kept as a SEPARATE method rather than folded into
	 *  `start()` itself so the two mechanisms stay visibly distinct at
	 *  their one call site too, not only inside this class (spec D7: "two
	 *  mechanisms, deliberately, documented at the seam"). Idempotent while
	 *  already fully registered (`ringUnlisten !== null`); see
	 *  `ringSubscriptionEpoch`'s doc comment for how a start racing a stop
	 *  (or another start) is resolved. */
	async startRingSubscription(): Promise<void> {
		if (this.ringUnlisten !== null) return;
		const epoch = ++this.ringSubscriptionEpoch;
		const unlisten = await this.api.onServiceLog((ev) => this.applyRingLog(ev));
		if (epoch !== this.ringSubscriptionEpoch) {
			// Superseded by a stop, or by another start, while this
			// registration was in flight — unregister immediately rather
			// than leaving a zombie listener alive past teardown, or
			// silently overwriting a newer registration that already won.
			unlisten();
			return;
		}
		this.ringUnlisten = unlisten;
	}

	/** Stop the ring seam's live half. Safe to call when not started, and
	 *  safe to call while `startRingSubscription()` is still awaiting its
	 *  own registration — see `ringSubscriptionEpoch`'s doc comment for how
	 *  that in-flight registration discovers it was superseded and cleans
	 *  itself up rather than surviving as a zombie listener. */
	stopRingSubscription(): void {
		this.ringSubscriptionEpoch += 1;
		this.ringUnlisten?.();
		this.ringUnlisten = null;
	}

	/** Open the folder for the selected source in the OS file manager (spec
	 *  D8's recourse against unbounded on-disk growth). A no-op when
	 *  nothing is selected, or when the selection is a `serviceRing` source
	 *  (`reveal_log_folder_target` rejects one the same way `resolve_log_path`
	 *  does — a ring source has no on-disk log file for this app to derive
	 *  a folder from; unreachable from the UI today since `LogStatusLine`
	 *  is not rendered for a ring selection, but guarded here too rather
	 *  than relying on that alone). A failure lands on `readError` like any
	 *  other action here, never thrown at the caller. */
	async revealFolder(): Promise<void> {
		if (this.selected === null || this.selected.kind === 'serviceRing') return;
		try {
			await this.api.revealLogFolder(this.selected);
		} catch (e) {
			this.readError = e as IpcError;
		}
	}
}
