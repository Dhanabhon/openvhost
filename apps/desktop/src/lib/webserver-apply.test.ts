// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { reloadAfterApply } from './webserver-apply';

// This is the composition the Web server route wires into `ApplyDialog`'s
// `onApply` (see `+page.svelte`). The route itself cannot be exercised at
// this level — `svelte/server` renders it statically with no live DOM, so
// there is no way to open the dialog or click Apply in `routes.test.ts` (see
// that file's own header comment on why `onMount` and event handling do not
// run there). This is the reachable seam: the exact function the page wires
// in, tested directly against fakes for `run`/`reload`.
describe('reloadAfterApply', () => {
	it('reloads the web-server list after a successful apply', async () => {
		const run = vi.fn(async () => true);
		const reload = vi.fn(async () => {});

		const result = await reloadAfterApply(run, reload);

		expect(result).toBe(true);
		expect(reload).toHaveBeenCalledOnce();
	});

	// A failed apply must not look like it healed the stale `configExists` —
	// and must not spend a round trip re-reading a list that did not change.
	it('does not reload after a failed apply', async () => {
		const run = vi.fn(async () => false);
		const reload = vi.fn(async () => {});

		const result = await reloadAfterApply(run, reload);

		expect(result).toBe(false);
		expect(reload).not.toHaveBeenCalled();
	});
});
