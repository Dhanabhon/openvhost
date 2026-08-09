// SPDX-License-Identifier: GPL-3.0-or-later
//
// The wiring nothing else can see: that the LAYOUT actually asks whether
// `state.db` opened, and that the answer reaches the shared store the app-level
// banner renders from.
//
// This file exists because the parts passing is not the same as the product
// working — a lesson this project paid for once already (five UI-glue defects
// that every per-part test was blind to). `store-status.svelte.test.ts` proves
// the store, `StoreUnavailableBanner.svelte.test.ts` proves the markup and
// `routes.test.ts` proves every route renders it; only this one can fail when
// nobody ever asks the question, which is precisely how a whole feature's
// wiring goes missing.
//
// The seam is mocked at `@tauri-apps/api/core`'s `invoke`, NOT at `$lib/ipc`
// (the pattern `lib/ipc/ipc.test.ts` established), so everything above the wire
// is the real thing: a layout that called the wrong command name fails here.
//
// Runs under the `dom` (jsdom) vitest project — `svelte/server` never runs
// `onMount`, so no SSR test can reach this state at all.
//
// Vacuity, measured: emptying the layout's `onMount(() => { void
// storeStatusStore.load(); })` — the whole of the wiring — reddened four of the
// five tests below and left `leaves the store silent when the backend reports no
// problem` green. That one test passing on a layout that never asks anything is
// precisely why the other four are written the way they are.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let handlers: Record<string, () => unknown> = {};
/** Every command the layout invoked, in order. */
let asked: string[] = [];

const invokeMock = vi.fn(async (cmd: string) => {
	asked.push(cmd);
	const handler = handlers[cmd];
	// A plain object, not an Error: the generated `typedError` rethrows real
	// `Error`s and would escape the caller's own catch.
	if (handler === undefined) throw { kind: 'core', message: `no handler for ${cmd}` };
	return handler();
});

vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => invokeMock(...(args as [string]))
}));
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
	once: vi.fn(async () => () => {}),
	emit: vi.fn(async () => {})
}));

import { createRawSnippet, mount, tick, unmount } from 'svelte';
import Layout from './+layout.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import { storeStatusStore } from '$lib/store-status.shared.svelte';

const REASON = 'unable to open database file (os error 14)';

/** A stand-in for whatever page the layout happens to be wrapping. The layout's
 *  own behaviour is what is under test, so the child is deliberately inert. */
const children = createRawSnippet(() => ({ render: () => '<div data-testid="page"></div>' }));

let host: HTMLElement;
let instance: object | null = null;

beforeEach(() => {
	asked = [];
	// `list_services` succeeds so a supervisor failure can never be the reason
	// something below is or is not true. Everything else is left unhandled on
	// purpose: those stores capture their own failures, and this test is about
	// one command.
	handlers = { list_services: () => [] };
	servicesStore.services = [];
	servicesStore.error = null;
	storeStatusStore.reason = null;
	storeStatusStore.lastError = null;
	host = document.createElement('div');
	document.body.appendChild(host);
});

afterEach(() => {
	if (instance !== null) unmount(instance);
	instance = null;
	host.remove();
	invokeMock.mockClear();
});

/** Mounts the layout and lets every already-resolved read settle. Several
 *  turns, because the reads chain through `typedError` → `unwrap` → the store's
 *  own assignment before Svelte re-renders. */
async function mountLayout(): Promise<void> {
	instance = mount(Layout, { target: host, props: { children } });
	for (let i = 0; i < 6; i++) {
		await Promise.resolve();
		await tick();
	}
}

describe('the layout’s startup ask', () => {
	it('really does ask the backend whether the store opened', async () => {
		handlers.state_store_status = () => null;
		await mountLayout();
		expect(asked).toContain('state_store_status');
	});

	it('feeds a reported reason into the store the banner reads', async () => {
		handlers.state_store_status = () => REASON;
		await mountLayout();
		expect(storeStatusStore.reason).toBe(REASON);
	});

	// The other direction, and the one a "did it get wired up?" test most easily
	// fakes: a healthy machine must come out of this with nothing to say.
	it('leaves the store silent when the backend reports no problem', async () => {
		handlers.state_store_status = () => null;
		await mountLayout();
		expect(storeStatusStore.reason).toBeNull();
	});

	it('asks once, not once per settle turn', async () => {
		handlers.state_store_status = () => null;
		await mountLayout();
		expect(asked.filter((c) => c === 'state_store_status')).toHaveLength(1);
	});

	it('survives a failed ask without claiming the store is down', async () => {
		// No handler for `state_store_status` at all, so the invoke rejects —
		// "we could not tell", which must render as silence, not as a banner.
		await mountLayout();
		expect(asked).toContain('state_store_status');
		expect(storeStatusStore.reason).toBeNull();
		expect(storeStatusStore.lastError).not.toBeNull();
	});
});
