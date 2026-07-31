// SPDX-License-Identifier: GPL-3.0-or-later
// Services state: snapshot seeds it, events drive it (UI contract).
//
// One instance is shared app-wide (see `services.shared.svelte.ts`) because the
// titlebar's "N running" count belongs to every route, not just the Services
// page. That shapes two things here:
//   * `loadServices()` (what the count needs) is split from `loadLogTail()`
//     (what the Services page's LogPane needs) so the layout can hoist only the
//     first, and is deduped so both callers share one round trip;
//   * failures land on `error` instead of being thrown at the caller — the
//     layout that now performs the snapshot has no banner of its own.
import type { IpcError, LogLine, ServiceLogEvent, ServiceStateEvent, ServiceStatus } from './ipc';

export interface ServicesApi {
	listServices(): Promise<ServiceStatus[]>;
	serviceLogTail(id: string, n: number): Promise<LogLine[]>;
	startService(id: string): Promise<void>;
	stopService(id: string): Promise<void>;
}

export interface UiLog extends LogLine {
	id: string;
}

const LOG_CAP = 50;

export class ServicesStore {
	services = $state<ServiceStatus[]>([]);
	logs = $state<UiLog[]>([]);
	error = $state<IpcError | null>(null);

	/** In-flight (or settled) `listServices()` request, so the two callers share
	 *  one round trip. They race by construction: children mount before their
	 *  parent layout, so the Services page's `loadLogTail()` runs first and would
	 *  otherwise fire its own `listServices()` alongside the layout's. Reset on
	 *  failure so a later caller retries instead of inheriting an empty list. */
	private snapshot: Promise<void> | null = null;

	constructor(private api: ServicesApi) {}

	/** Fetch the service list once per successful load; later callers await the
	 *  first request's result. Never rejects — see the file header. */
	loadServices(): Promise<void> {
		return (this.snapshot ??= this.fetchServices());
	}

	/**
	 * Force a fresh `listServices()` round trip, discarding any memoized
	 * result — the escape hatch for a caller that KNOWS the set of registered
	 * services changed underneath this store.
	 *
	 * I1 audit finding (branch-review-fix-report.md): `Supervisor::register`
	 * used to emit no event, and `applyState` below only maps over the
	 * services ALREADY in `this.services` — a `StateChanged` for an id it
	 * does not recognize is silently dropped (see that method's own doc
	 * comment). So installing a PHP version after launch registered a real
	 * supervisor row this store never learned about: the Languages page would
	 * offer Start, the click would genuinely start the pool, and the row
	 * would keep saying Start forever. `languages/+page.svelte` calls this
	 * after a successful `install()`/`rescan()` — the two moments it KNOWS
	 * the registered set may have grown.
	 *
	 * The durable fix has since shipped (tray slice, Task 1): `register` now
	 * emits `SupervisorEvent::Registered`, forwarded over IPC and applied by
	 * {@link applyRegistered}, which the layout subscribes to on every route —
	 * so a newly registered service now arrives here without either caller
	 * needing to ask. This method stays anyway: it is a synchronous
	 * request/response the instant an install settles, independent of
	 * however long the event takes to round-trip the broadcast channel and
	 * IPC — cheap insurance against a caller observing a stale list in the
	 * gap, not a sign the event-based fix is incomplete.
	 */
	reload(): Promise<void> {
		this.snapshot = null;
		return this.loadServices();
	}

	private async fetchServices(): Promise<void> {
		try {
			this.services = await this.api.listServices();
		} catch (e) {
			this.fail(e as IpcError);
			this.snapshot = null;
		}
	}

	/**
	 * Seed the log feed from the first service's tail. Page-specific (only the
	 * Services page renders a LogPane), and it awaits the snapshot first so it
	 * cannot run before the service list it picks that service from exists.
	 *
	 * Re-seeding on every visit is deliberate: the tail is read from the
	 * supervisor's own ring buffer, so it already contains the lines that arrived
	 * while the user was on another route.
	 */
	async loadLogTail(): Promise<void> {
		await this.loadServices();
		const first = this.services[0];
		if (!first) return;
		try {
			const tail = await this.api.serviceLogTail(first.id, LOG_CAP);
			this.logs = tail.map((l) => ({ ...l, id: first.id }));
		} catch (e) {
			this.fail(e as IpcError);
		}
	}

	async start(id: string): Promise<void> {
		await this.act(() => this.api.startService(id));
	}

	async stop(id: string): Promise<void> {
		await this.act(() => this.api.stopService(id));
	}

	/** Clear the previous failure, then run an action, capturing its own. The
	 *  running state itself arrives on `service-state`, not from the return
	 *  value — these commands resolve as soon as the supervisor accepted them. */
	private async act(run: () => Promise<void>): Promise<void> {
		this.error = null;
		try {
			await run();
		} catch (e) {
			this.fail(e as IpcError);
		}
	}

	/** Record a failure raised outside this store's own api calls — the layout's
	 *  event subscription, the Services page's `coreInfo()` fetch — on the same
	 *  channel the banner already renders. Takes a normalized `IpcError`: the
	 *  `./ipc` barrel guarantees every function there rejects with one. */
	fail(error: IpcError): void {
		this.error = error;
	}

	/** `.map` over the EXISTING list — a `StateChanged` for an id not already in
	 *  `this.services` (a supervisor row registered after this store's one
	 *  `loadServices()` snapshot) matches nothing and is silently dropped, by
	 *  design: this method never grows the list. See `reload()`'s doc comment
	 *  (I1 audit finding) for the history, and {@link applyRegistered} — the
	 *  durable fix that DOES grow the list, for the one event
	 *  (`SupervisorEvent::Registered`) that actually means a new row exists. */
	applyState(ev: ServiceStateEvent): void {
		this.services = this.services.map((s) =>
			s.id === ev.id
				? { ...s, state: ev.state, pid: ev.state.kind === 'running' ? s.pid : null }
				: s
		);
	}

	/**
	 * React to a `Registered` event: unlike {@link applyState}, this is
	 * allowed to GROW the list — it is the durable fix for the exact gap
	 * `applyState`'s own drop test documents (see that method), landing
	 * instead of the `reload()` workaround. `register()`'s no-op guard for an
	 * already `starting`/`running` id (Rust side) means this never clobbers a
	 * live row: an id it already knows is simply refreshed with the freshly
	 * stored status (idempotent for a repeat registration of an
	 * already-stopped id), and an id it has never seen is appended.
	 *
	 * Kept sorted by id, matching `Supervisor::snapshot()` — the same order
	 * `listServices()` already returns — so a service arriving through this
	 * event renders in the same position a full reload would have put it.
	 */
	applyRegistered(status: ServiceStatus): void {
		const known = this.services.some((s) => s.id === status.id);
		const next = known
			? this.services.map((s) => (s.id === status.id ? status : s))
			: [...this.services, status];
		next.sort((a, b) => a.id.localeCompare(b.id));
		this.services = next;
	}

	/**
	 * React to an `Unregistered` event: the mirror of {@link applyRegistered},
	 * and the only thing that SHRINKS the list. Uninstalling a PHP or MySQL
	 * major removes its supervisor row (package-uninstall design D4); without
	 * this the row would sit on the Services page — and in the titlebar's
	 * "N running" count — until the next relaunch, offering Start for a binary
	 * that is no longer installed.
	 *
	 * Silent for an id this store does not hold: the event is broadcast to
	 * every subscriber, and a store whose first snapshot is still in flight
	 * (or that loaded before the id ever existed) has nothing to drop. Filter
	 * rather than index-and-splice, so a repeat event is idempotent for free.
	 *
	 * The log feed is deliberately NOT pruned. Spec D2 keeps a package's logs
	 * on disk precisely because a user often uninstalls BECAUSE something
	 * failed; erasing the lines already on screen at that moment would be the
	 * same mistake in miniature.
	 */
	applyUnregistered(id: string): void {
		this.services = this.services.filter((s) => s.id !== id);
	}

	applyLog(ev: ServiceLogEvent): void {
		const next = [...this.logs, { id: ev.id, tsMs: ev.tsMs, level: ev.level, line: ev.line }];
		this.logs = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}
}
