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

	applyState(ev: ServiceStateEvent): void {
		this.services = this.services.map((s) =>
			s.id === ev.id
				? { ...s, state: ev.state, pid: ev.state.kind === 'running' ? s.pid : null }
				: s
		);
	}

	applyLog(ev: ServiceLogEvent): void {
		const next = [...this.logs, { id: ev.id, tsMs: ev.tsMs, level: ev.level, line: ev.line }];
		this.logs = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}
}
