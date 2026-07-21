// SPDX-License-Identifier: GPL-3.0-or-later
// Services panel state: snapshot seeds it, events drive it (UI contract).
import type { LogLine, ServiceLogEvent, ServiceStateEvent, ServiceStatus } from './ipc';

export interface ServicesApi {
	listServices(): Promise<ServiceStatus[]>;
	serviceLogTail(id: string, n: number): Promise<LogLine[]>;
}

export interface UiLog extends LogLine {
	id: string;
}

const LOG_CAP = 50;

export class ServicesStore {
	services = $state<ServiceStatus[]>([]);
	logs = $state<UiLog[]>([]);

	constructor(private api: ServicesApi) {}

	async init(): Promise<void> {
		this.services = await this.api.listServices();
		const first = this.services[0];
		if (first) {
			const tail = await this.api.serviceLogTail(first.id, LOG_CAP);
			this.logs = tail.map((l) => ({ ...l, id: first.id }));
		}
	}

	applyState(ev: ServiceStateEvent): void {
		this.services = this.services.map((s) =>
			s.id === ev.id ? { ...s, state: ev.state, pid: ev.state.kind === 'running' ? s.pid : null } : s
		);
	}

	applyLog(ev: ServiceLogEvent): void {
		const next = [...this.logs, { id: ev.id, tsMs: ev.tsMs, level: ev.level, line: ev.line }];
		this.logs = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}
}
