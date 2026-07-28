// SPDX-License-Identifier: GPL-3.0-or-later
import type { ServiceStatus } from '$lib/ipc';

/**
 * The supervised state-kind a row shows, or `null` when the row has no
 * supervised service (Apache) or the snapshot has not arrived yet. Never falls
 * back to another row's state — a row showing a neighbour's status would be a
 * lie about what is running.
 *
 * Returns the KIND rather than the whole state object for two reasons:
 * `ServiceState` is not exported from `$lib/ipc`, and `StatusPill` takes
 * `kind: StateKind`. Indexing off the exported `ServiceStatus` keeps this in
 * step with the binding without widening the barrel.
 */
export function statusFor(
	// `readonly` so a component can keep its own `services` prop readonly (the shape
	// ServicesPanel.svelte already uses) and still call this — the lookup never
	// mutates. Accepts a mutable array too, so no caller changes.
	services: readonly ServiceStatus[],
	serviceId: string | null
): ServiceStatus['state']['kind'] | null {
	if (serviceId === null) return null;
	return services.find((s) => s.id === serviceId)?.state.kind ?? null;
}

export function hotReloadLabel(supportsHotReload: boolean): string {
	return supportsHotReload ? 'Supported' : 'Not supported';
}

/** The reason a Start button is disabled. Spec §4 fixes this string; the form
 *  and its test both read it from here so they cannot drift apart. It names the
 *  next step rather than only the problem — "no config" alone leaves the user
 *  to guess that Apply is what produces one. */
export const NO_CONFIG_REASON = 'No config generated yet — apply your changes first.';

/** What the row's service control should be right now.
 *
 *  A discriminated union rather than a pile of booleans on the component: the
 *  choice is a decision, it is testable as a table here, and the component is
 *  left with nothing to decide. */
export type StartStopControl =
	| { kind: 'none' }
	| { kind: 'start'; disabled: boolean; reason: string }
	| { kind: 'retry' }
	| { kind: 'stop' };

/**
 * `statusKind === null` means the supervisor snapshot has NOT ARRIVED, which is
 * not the same as "stopped" — see the test. It renders nothing, the same rule
 * the status pill already follows (`{#if statusKind}` in WebServerRow.svelte).
 *
 * `configExists` only ever gates `start`. A `failed` service has already been
 * started once, and a `running` one is a live process; neither decision has
 * anything to do with a file being on disk right now.
 */
export function startStopFor(
	statusKind: ServiceStatus['state']['kind'] | null,
	configExists: boolean
): StartStopControl {
	if (statusKind === null) return { kind: 'none' };
	if (statusKind === 'failed') return { kind: 'retry' };
	if (statusKind === 'stopped') {
		return configExists
			? { kind: 'start', disabled: false, reason: '' }
			: { kind: 'start', disabled: true, reason: NO_CONFIG_REASON };
	}
	return { kind: 'stop' };
}
