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
