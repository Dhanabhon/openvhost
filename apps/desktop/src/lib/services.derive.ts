// SPDX-License-Identifier: GPL-3.0-or-later
// `ServiceState` itself isn't re-exported from the frozen './ipc' barrel (only
// `ServiceStateEvent`/`ServiceStatus` are) — `StateKind` is derived from
// `ServiceStatus['state']['kind']` instead of importing `ServiceState`
// directly, so this stays byte-identical without touching lib/ipc/.
import type { ServiceStatus } from './ipc';

export function runningCount(services: readonly ServiceStatus[]): number {
	return services.filter((s) => s.state.kind === 'running').length;
}

export type StateKind = ServiceStatus['state']['kind'];
export function pillClass(kind: StateKind): string {
	return `pill-${kind}`;
}
