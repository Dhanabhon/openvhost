// SPDX-License-Identifier: GPL-3.0-or-later
//
// The wiring nothing else can see: that the LAYOUT actually asks how far this
// launch got, and that the answer decides whether there is an app on screen at
// all.
//
// This file exists because the parts passing is not the same as the product
// working — a lesson this project paid for once already (five UI-glue defects
// that every per-part test was blind to), and once more since (a store with no
// tests at all is how a whole feature's wiring went missing).
// `boot-status.svelte.test.ts` proves the decision, `BootTakeover.svelte.
// test.ts` proves the markup; only this one can fail when nobody ever asks the
// question, or when the answer reaches nothing.
//
// It is also the only place that can prove the children are GENUINELY GATED
// rather than visually covered: SSR markup would show a takeover drawn over a
// page that is still there, and "still there" means every command on it has
// already been fired at a backend that cannot answer.
//
// The seam is mocked at `@tauri-apps/api/core`'s `invoke`, NOT at `$lib/ipc`
// (the pattern `lib/ipc/ipc.test.ts` established), so everything above the wire
// is the real thing: a layout that called the wrong command name fails here.
//
// Runs under the `dom` (jsdom) vitest project — `svelte/server` never runs
// `onMount`, so no SSR test can reach this state at all.

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
import { bootStatusStore } from '$lib/boot-status.shared.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import { storeStatusStore } from '$lib/store-status.shared.svelte';
import type { BootStatusDto } from '$lib/ipc';

const HOME = '/Users/tom/.openvhost';
const RUN_DIR = '/Users/tom/.openvhost/run';
const ERRNO_13 = 'Permission denied (os error 13)';

/** A stand-in for whatever page the layout happens to be wrapping. The layout's
 *  own behaviour is what is under test, so the child is deliberately inert —
 *  and its ABSENCE is what proves the gate really gates. */
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
	bootStatusStore.status = null;
	bootStatusStore.askFailed = null;
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

function answer(dto: BootStatusDto): void {
	handlers.boot_status = () => dto;
}

/** Whether the wrapped page is in the DOM at all — not whether it is visible. */
function pageIsRendered(): boolean {
	return host.querySelector('[data-testid="page"]') !== null;
}

function takeover(): HTMLElement | null {
	return host.querySelector('[data-testid="boot-takeover"]');
}

describe('the layout’s boot ask', () => {
	// Vacuity, measured: emptying the layout's `onMount(() => { void
	// bootStatusStore.load(); })` — the whole of the wiring — reddened twelve of
	// this file's tests. The two that survived it were both assertions that could
	// not fail on a layout rendering no takeover at all, and both were rewritten
	// to state their premise first rather than left as measured.

	it('really does ask the backend how far the boot got', async () => {
		answer({ kind: 'ready' });
		await mountLayout();
		expect(asked).toContain('boot_status');
	});

	it('asks once, not once per settle turn', async () => {
		answer({ kind: 'ready' });
		await mountLayout();
		expect(asked.filter((c) => c === 'boot_status')).toHaveLength(1);
	});
});

describe('a ready boot', () => {
	it('renders the app, and no takeover at all', async () => {
		answer({ kind: 'ready' });
		await mountLayout();
		expect(pageIsRendered()).toBe(true);
		expect(takeover()).toBeNull();
	});
});

describe('a boot whose answer has not arrived yet', () => {
	// The transient state, and the ONLY place it can be observed: every other
	// test in this file asserts after the answer has landed.
	//
	// It is here because of a measured hole. Collapsing the layout's four-way
	// chain to the obvious two-way one —
	//
	//     {#if rendering.kind === 'takeover'} … {:else} {@render children()} {/if}
	//
	// which is the simplification any reader would reach for, and which renders
	// the app for a launch that has not answered yet — left the ENTIRE suite
	// green before this test existed. It is now the only test that fails on it.
	//
	// It matters because it is what makes "no page shows Tauri's `.manage()`
	// string" structural rather than a race: children rendered here would mount
	// the real pages and fire every command they load on before `boot_status` had
	// said whether any of them can answer.

	it('shows neither the app nor a takeover while the ask is still in flight', async () => {
		let release: (dto: BootStatusDto) => void = () => {};
		handlers.boot_status = () =>
			new Promise<BootStatusDto>((resolve) => {
				release = resolve;
			});
		await mountLayout();
		expect(asked).toContain('boot_status');
		expect(pageIsRendered()).toBe(false);
		expect(takeover()).toBeNull();

		// …and it is WAITING, not wedged. Without this half the test above would
		// also pass on a layout that renders nothing ever.
		release({ kind: 'ready' });
		for (let i = 0; i < 6; i++) {
			await Promise.resolve();
			await tick();
		}
		expect(pageIsRendered()).toBe(true);
	});
});

describe('each degraded state renders its own screen', () => {
	// Vacuity, measured: changing the layout's `{:else if rendering.kind ===
	// 'takeover'}` branch to render `{@render children()}` reddened all three
	// screens and all three gating tests here, and left `a ready boot` green.

	it('says another instance holds the lock, and names the folder', async () => {
		answer({ kind: 'alreadyRunning', home: HOME });
		await mountLayout();
		expect(takeover()?.querySelector('[data-testid="boot-already-running"]')).not.toBeNull();
		expect(host.textContent).toContain('OpenVHost is already running');
		expect(host.textContent).toContain(HOME);
	});

	it('names the run directory and the OS error, verbatim', async () => {
		answer({ kind: 'runDirUnusable', path: RUN_DIR, reason: ERRNO_13 });
		await mountLayout();
		expect(takeover()?.querySelector('[data-testid="boot-run-dir-unusable"]')).not.toBeNull();
		expect(host.querySelector('[data-testid="boot-run-dir"]')?.textContent).toBe(RUN_DIR);
		expect(host.querySelector('[data-testid="boot-reason"]')?.textContent).toBe(ERRNO_13);
	});

	it('passes a different path and errno all the way through the wire', async () => {
		// The control: a screen printing one canned string would satisfy the test
		// above just as well. The store slice's own technique — `os error 13`
		// present, a different errno absent — carried across the whole seam this
		// time rather than one component.
		answer({
			kind: 'runDirUnusable',
			path: '/Volumes/Data/openvhost/run',
			reason: 'Read-only file system (os error 30)'
		});
		await mountLayout();
		expect(host.querySelector('[data-testid="boot-run-dir"]')?.textContent).toBe(
			'/Volumes/Data/openvhost/run'
		);
		expect(host.querySelector('[data-testid="boot-reason"]')?.textContent).toBe(
			'Read-only file system (os error 30)'
		);
		expect(host.textContent).not.toContain('os error 13');
	});

	it('says where the files should go when the home would not resolve', async () => {
		answer({ kind: 'homeUnresolvable', reason: 'home directory unavailable' });
		await mountLayout();
		expect(takeover()?.querySelector('[data-testid="boot-home-unresolvable"]')).not.toBeNull();
		expect(host.querySelector('[data-testid="boot-reason"]')?.textContent).toBe(
			'home directory unavailable'
		);
		expect(host.textContent).toContain('OPENVHOST_HOME');
	});
});

describe('the gate really gates', () => {
	// The distinction SSR cannot draw. A takeover drawn OVER a page that is still
	// mounted has already let that page fire every command it loads on, at a
	// backend where almost none of them can answer — which is how Tauri's
	// `.manage()` string reaches a user in the first place.
	const degraded: Array<[string, BootStatusDto]> = [
		['alreadyRunning', { kind: 'alreadyRunning', home: HOME }],
		['runDirUnusable', { kind: 'runDirUnusable', path: RUN_DIR, reason: ERRNO_13 }],
		['homeUnresolvable', { kind: 'homeUnresolvable', reason: 'home directory unavailable' }]
	];

	it.each(degraded)('removes the app from the DOM entirely on %s', async (_name, dto) => {
		answer(dto);
		await mountLayout();
		expect(takeover()).not.toBeNull();
		expect(pageIsRendered()).toBe(false);
	});

	it('shows Tauri’s own refusal string nowhere on any of the three', async () => {
		for (const [, dto] of degraded) {
			answer(dto);
			await mountLayout();
			// The premise, asserted first: without it this test passes on a layout
			// that renders nothing at all, which is not what it claims to prove.
			expect(takeover()).not.toBeNull();
			expect(host.textContent).not.toContain('.manage()');
			if (instance !== null) unmount(instance);
			instance = null;
			bootStatusStore.status = null;
			bootStatusStore.askFailed = null;
		}
	});

	it('keeps the window movable on the takeover, not just on the app', async () => {
		// A degraded window the user can neither move nor position to reach the
		// close button is worse than the bug this screen exists to fix, and this is
		// the only assertion that sees the takeover in a real document.
		answer({ kind: 'runDirUnusable', path: RUN_DIR, reason: ERRNO_13 });
		await mountLayout();
		const screen = takeover();
		// Not `takeover()?.querySelector(…)`: optional chaining on a missing
		// takeover yields `undefined`, and `expect(undefined).not.toBeNull()`
		// PASSES — the assertion would survive the takeover disappearing entirely.
		expect(screen).not.toBeNull();
		expect(screen?.querySelector('[data-tauri-drag-region="deep"]')).not.toBeNull();
	});
});

describe('an ask that itself failed', () => {
	// Vacuity, measured: making `bootRendering` return `{ kind: 'pending' }` for
	// a failed ask — copying the store slice's silence into a gate — reddened
	// `renders the app anyway` here and left every other group in this file
	// green. `records the failure …` survived it, correctly: it pins the value
	// the banner reads, not the gate, and those are two different failures.

	it('renders the app anyway, never a blank window', async () => {
		// No handler for `boot_status` at all, so the invoke rejects.
		await mountLayout();
		expect(asked).toContain('boot_status');
		expect(pageIsRendered()).toBe(true);
		expect(takeover()).toBeNull();
	});

	it('records the failure where the app-level banner reads it', async () => {
		// The banner itself lives in AppShell (a `height: 100%` grid cannot take a
		// sibling), so what this seam owes is the value it renders from —
		// `routes.test.ts` proves the banner reaches every page.
		await mountLayout();
		expect(bootStatusStore.askFailed).not.toBeNull();
		expect(bootStatusStore.status).toBeNull();
	});
});
