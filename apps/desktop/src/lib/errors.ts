// SPDX-License-Identifier: GPL-3.0-or-later
// Turning a thrown value into something safe to render.

/**
 * A renderable message for any thrown value.
 *
 * Deliberately not `(e as IpcError).message ?? String(e)`, which is the obvious
 * version and wrong twice over:
 *
 * - `IpcError`'s `simulated` variant carries NO `message` field at all, so the
 *   union does not permit a bare `.message` access — and narrowing it away with a
 *   cast would trade a type error for a runtime `undefined` on screen.
 * - `String(e)` on an object renders the literal text "[object Object]". A fixed
 *   fallback is worse copy but never nonsense.
 */
export function errorMessage(e: unknown): string {
	if (typeof e === 'object' && e !== null && 'message' in e) {
		const m = (e as { message?: unknown }).message;
		if (typeof m === 'string' && m !== '') return m;
	}
	return 'The command failed.';
}
