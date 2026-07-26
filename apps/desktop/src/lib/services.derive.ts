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

/**
 * Display names of everything a quit would interrupt.
 *
 * Not just `running`: a service mid-`starting` has a live child too, and the
 * quit dialog telling the user "nothing is running" while `nginx` is coming up
 * would be a lie. Mirrors the Rust side's `quit::pending_service_ids`, which
 * treats `stopped`/`failed` as the only terminal states.
 */
export function pendingServiceNames(services: readonly ServiceStatus[]): string[] {
	return services
		.filter((s) => s.state.kind !== 'stopped' && s.state.kind !== 'failed')
		.map((s) => s.displayName);
}

/** `["a"] → "a"`, `["a","b"] → "a and b"`, `["a","b","c"] → "a, b and c"`. */
export function formatNameList(names: readonly string[]): string {
	if (names.length === 0) return '';
	if (names.length === 1) return names[0];
	return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}
