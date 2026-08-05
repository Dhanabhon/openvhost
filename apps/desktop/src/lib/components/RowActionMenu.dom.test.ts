// SPDX-License-Identifier: GPL-3.0-or-later
//
// Runs under the `dom` (jsdom) vitest project (vite.config.ts), the first consumer of
// it: mounted via `mount`/`unmount` from 'svelte' (real client instances, real DOM,
// real event dispatch) rather than `svelte/server`, because every behaviour this file
// checks — focus, keyboard, portalling, listener cleanup — needs a live `document`.
//
// Every interaction helper below (`click`, `keydown`, `dispatch`) awaits `tick()`
// after dispatching. Svelte 5 applies a state change made inside an event handler to
// the DOM asynchronously (a microtask), so `document.body` right after a bare
// `el.click()` still shows the PREVIOUS render — confirmed directly: a throwaway probe
// component here showed `onclick` firing (the spy was called) while the `{#if}` block
// it flipped stayed unrendered until an awaited `tick()`. Skipping the await does not
// make a test fail cleanly; it makes it fail on the wrong line with a confusing "found
// nothing" error, so every helper below awaits it rather than leaving call sites to
// remember to.
//
// WHAT THIS FILE CANNOT COVER: jsdom does no layout, so `getBoundingClientRect()`
// returns zeros unless a test overrides it, and there is no paint at all. The
// positioning tests below prove the ARITHMETIC — given a rect, the popup's inline
// `top`/`right` are computed correctly from it — never that the result lands on
// screen in a visible, unclipped place. Whether the popup is actually clipped by
// `SitesPanel`'s `.panel` or runs off a real viewport edge is NOT provable here and
// is not asserted here; it is a human, screenshot-based check (spec D4, item 4).
//
// Items used below are deliberately generic (not "View logs"/"Edit"/"Delete" tied to
// Sites) — this component knows nothing about Sites, and neither should its tests.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import RowActionMenu, { type RowActionMenuItem } from './RowActionMenu.svelte';

interface Setup {
	host: HTMLElement;
	instance: object;
	onEdit: ReturnType<typeof vi.fn>;
	onDelete: ReturnType<typeof vi.fn>;
}

/** Mounts a fresh `RowActionMenu` with three generic items — a link ("View", an
 * `<a href>`, matching the shape task B's "View logs" needs) and two buttons, the
 * second marked destructive (matching "Delete"'s shape). */
async function setup(): Promise<Setup> {
	const host = document.createElement('div');
	document.body.appendChild(host);
	const onEdit = vi.fn();
	const onDelete = vi.fn();
	const items: RowActionMenuItem[] = [
		{ kind: 'link', label: 'View', href: '#view' },
		{ kind: 'button', label: 'Edit', onSelect: onEdit },
		{ kind: 'button', label: 'Delete', destructive: true, onSelect: onDelete }
	];
	const instance = mount(RowActionMenu, {
		target: host,
		props: { ariaLabel: 'Actions for example.test', items }
	});
	// `bind:this={triggerEl}` (and every other bind:this in the component) assigns
	// via an effect, not synchronously during the initial DOM creation `mount()`
	// performs — confirmed directly: a synthetic `.click()` fired right after
	// `mount()` returns reached `onTriggerClick` with `triggerEl` still
	// `undefined`, one tick before the binding effect had flushed. Awaiting a tick
	// here, once, before any test interacts with the trigger, is what makes every
	// bound ref reliably populated first.
	await tick();
	return { host, instance, onEdit, onDelete };
}

/** Tears down a `Setup` — every test calls this itself (rather than a shared
 * `afterEach`) so the exact moment of unmount is visible at the call site; several
 * tests below assert ON that moment. */
async function teardown(s: Setup): Promise<void> {
	await unmount(s.instance);
	s.host.remove();
}

// Safety net only, not a substitute for each test's own `teardown`: jsdom's
// `document`/`window` persist across tests in this file (unlike the `server`
// project, which renders a markup string per call and keeps no DOM at all), so a
// portal node any one test forgot to clean up would otherwise bleed into whichever
// test runs next instead of failing the test that actually caused it.
afterEach(() => {
	document.body.replaceChildren();
});

function getTrigger(host: HTMLElement): HTMLButtonElement {
	const el = host.querySelector('button');
	if (el === null) throw new Error('RowActionMenu rendered no trigger button');
	return el;
}

/** The open popup, found in `<body>` — never inside `host`, since it is portalled. */
function getMenu(): HTMLElement {
	const el = document.body.querySelector<HTMLElement>('[role="menu"]');
	if (el === null) throw new Error('no open RowActionMenu popup found in document.body');
	return el;
}

function menuItems(): HTMLElement[] {
	return Array.from(document.body.querySelectorAll<HTMLElement>('[role="menuitem"]'));
}

/** Clicks a real DOM node and waits for the resulting state change to reach the DOM.
 * See the file header. */
async function click(el: Element): Promise<void> {
	(el as HTMLElement).click();
	await tick();
}

async function keydown(target: EventTarget, key: string): Promise<void> {
	target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
	await tick();
}

async function dispatch(target: EventTarget, event: Event): Promise<void> {
	target.dispatchEvent(event);
	await tick();
}

describe('RowActionMenu portal', () => {
	it("moves the open popup to <body>, not into the trigger's own subtree", async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const menu = getMenu();
		expect(menu.parentElement).toBe(document.body);
		expect(s.host.contains(menu)).toBe(false);
		await teardown(s);
	});
	// Vacuity-proved: temporarily changed `portal()` to skip
	// `document.body.appendChild(node)` (rendering the menu where the template
	// placed it, a sibling of the trigger inside `host`) and re-ran this file.
	// This test went red — `menu.parentElement` was inside `host`'s own subtree,
	// not `document.body` — and `renders every item...` below stayed green
	// throughout, confirming the failure was specific to portalling. Reverted
	// before moving on.
});

describe('RowActionMenu ARIA wiring', () => {
	it('the trigger is a menu button, closed by default, with its own accessible name', async () => {
		const s = await setup();
		const trigger = getTrigger(s.host);
		expect(trigger.getAttribute('aria-haspopup')).toBe('menu');
		expect(trigger.getAttribute('aria-expanded')).toBe('false');
		expect(trigger.getAttribute('aria-label')).toBe('Actions for example.test');
		await teardown(s);
	});
	// Vacuity-proved: temporarily deleted `aria-haspopup="menu"` from the trigger's
	// markup and re-ran this file. This test went red on the first expectation;
	// restored immediately after.

	it('flips aria-expanded and links aria-controls to the open popup', async () => {
		const s = await setup();
		const trigger = getTrigger(s.host);
		await click(trigger);
		const menu = getMenu();
		expect(trigger.getAttribute('aria-expanded')).toBe('true');
		expect(trigger.getAttribute('aria-controls')).toBe(menu.id);
		expect(menu.getAttribute('role')).toBe('menu');
		await teardown(s);
	});

	it('gives every item role="menuitem", one per item, none extra', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const items = menuItems();
		expect(items).toHaveLength(3);
		await teardown(s);
	});
});

describe('RowActionMenu item kinds', () => {
	it('renders a link item as a real <a href>, never a button', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view] = menuItems();
		expect(view.tagName).toBe('A');
		expect(view.getAttribute('href')).toBe('#view');
		await teardown(s);
	});

	it('renders a button item as a real <button type="button">', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [, edit] = menuItems();
		expect(edit.tagName).toBe('BUTTON');
		expect(edit.getAttribute('type')).toBe('button');
		await teardown(s);
	});

	it('marks a destructive item, and leaves a non-destructive one unmarked', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [, edit, del] = menuItems();
		expect(del.classList.contains('destructive')).toBe(true);
		expect(edit.classList.contains('destructive')).toBe(false);
		await teardown(s);
	});
});

describe('RowActionMenu positioning arithmetic', () => {
	// jsdom performs no layout, so `getBoundingClientRect()` always returns zeros
	// unless overridden — these tests override it and check the FORMULA
	// (`top = rect.bottom + gap`, `right = innerWidth - rect.right`), not that the
	// popup ends up visible or unclipped on a real screen. See the file header.
	it("computes top/right from the trigger's own rect, not a fixed offset", async () => {
		const s = await setup();
		const trigger = getTrigger(s.host);
		trigger.getBoundingClientRect = () =>
			({
				top: 100,
				left: 50,
				right: 90,
				bottom: 120,
				width: 40,
				height: 20,
				x: 50,
				y: 100,
				toJSON: () => ({})
			}) as DOMRect;
		await click(trigger);
		const menu = getMenu();
		expect(menu.style.top).toBe('124px'); // rect.bottom(120) + the 4px gap
		expect(menu.style.right).toBe(`${window.innerWidth - 90}px`);
		await teardown(s);
	});
	// Vacuity-proved: temporarily swapped `rect.bottom` for `rect.top` in
	// `openMenu()`. `menu.style.top` then read `104px` instead of the expected
	// `124px` and this test went red; reverted immediately after.
});

describe('RowActionMenu keyboard', () => {
	it('opening moves focus to the first item', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view] = menuItems();
		expect(document.activeElement).toBe(view);
		await teardown(s);
	});

	it('ArrowDown moves focus forward and wraps past the last item', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view, edit, del] = menuItems();
		await keydown(view, 'ArrowDown');
		expect(document.activeElement).toBe(edit);
		await keydown(edit, 'ArrowDown');
		expect(document.activeElement).toBe(del);
		await keydown(del, 'ArrowDown');
		expect(document.activeElement).toBe(view);
		await teardown(s);
	});

	it('ArrowUp moves focus backward and wraps before the first item', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view, , del] = menuItems();
		await keydown(view, 'ArrowUp');
		expect(document.activeElement).toBe(del);
		await teardown(s);
	});

	it('Enter activates the focused BUTTON item', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [, edit] = menuItems();
		edit.focus();
		await keydown(edit, 'Enter');
		expect(s.onEdit).toHaveBeenCalledOnce();
		await teardown(s);
	});

	// The one case where native behaviour alone would NOT be enough: a browser
	// activates a focused <button> on Space for free, but not a focused <a> (Space
	// scrolls the page by default) — so this specifically exercises RowActionMenu's
	// own explicit Space handling, not something jsdom or the browser already does.
	it('Space activates the focused LINK item', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view] = menuItems();
		view.focus();
		await keydown(view, ' ');
		// A link item has no onSelect to spy on; the menu closing (part of
		// "choosing an item") is the observable proof the synthetic click ran.
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
		await teardown(s);
	});

	it('Escape closes the popup', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view] = menuItems();
		await keydown(view, 'Escape');
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
		await teardown(s);
	});
});

describe('RowActionMenu focus management', () => {
	// Each of the three tests below funnels through the same `closeAndRefocus`.
	// Vacuity-proved TOGETHER: temporarily removed the `triggerEl?.focus();` line
	// from `closeAndRefocus()` and re-ran this file. All three went red —
	// `document.activeElement` was `document.body`, not the trigger, in each case —
	// while every "...closes" assertion elsewhere stayed green (proving `open`
	// still correctly went false; only the refocus was broken). Reverted after.
	it('Escape returns focus to the trigger', async () => {
		const s = await setup();
		const trigger = getTrigger(s.host);
		await click(trigger);
		const [view] = menuItems();
		await keydown(view, 'Escape');
		expect(document.activeElement).toBe(trigger);
		await teardown(s);
	});

	it('choosing an item returns focus to the trigger', async () => {
		const s = await setup();
		const trigger = getTrigger(s.host);
		await click(trigger);
		const [, edit] = menuItems();
		await click(edit);
		expect(document.activeElement).toBe(trigger);
		await teardown(s);
	});

	it('clicking outside returns focus to the trigger', async () => {
		const s = await setup();
		const trigger = getTrigger(s.host);
		await click(trigger);
		await dispatch(document.body, new Event('pointerdown', { bubbles: true }));
		expect(document.activeElement).toBe(trigger);
		await teardown(s);
	});
});

describe('RowActionMenu click outside', () => {
	it('closes the popup', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		expect(getMenu()).toBeTruthy();
		await dispatch(document.body, new Event('pointerdown', { bubbles: true }));
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
		await teardown(s);
	});

	it('a pointerdown INSIDE the popup does not close it', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view] = menuItems();
		await dispatch(view, new Event('pointerdown', { bubbles: true }));
		expect(getMenu()).toBeTruthy();
		await teardown(s);
	});
});

describe('RowActionMenu scroll and resize', () => {
	it('closes on window resize', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		await dispatch(window, new Event('resize'));
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
		await teardown(s);
	});

	// A REAL scroll event does not bubble, so a listener on `window` only ever
	// observes a nested ancestor's scroll during the CAPTURE phase. Dispatching a
	// non-bubbling scroll on an element other than `window` itself is what makes
	// this test actually exercise that, rather than trivially passing because the
	// event's target and `window` happened to be the same object.
	it('closes when a non-bubbling scroll fires on a nested ancestor', async () => {
		const s = await setup();
		const scrollHost = document.createElement('div');
		s.host.appendChild(scrollHost);
		await click(getTrigger(s.host));
		await dispatch(scrollHost, new Event('scroll', { bubbles: false }));
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
		await teardown(s);
	});
	// Vacuity-proved: temporarily changed the trigger's `onscrollcapture` to
	// `onscroll` (dropping the capture flag) and re-ran this file. This test went
	// red — the popup stayed open, since a bubble-phase `window` listener never
	// sees a non-bubbling scroll dispatched on a different target — while "closes
	// on window resize" above stayed green. Reverted after.
});

describe('RowActionMenu cleanup on unmount', () => {
	it('removes the portalled popup node from <body> when unmounted while open', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const menu = getMenu();
		expect(document.body.contains(menu)).toBe(true);
		await unmount(s.instance);
		expect(document.body.contains(menu)).toBe(false);
		s.host.remove();
	});
	// Vacuity-proved: temporarily removed `node.remove()` from the portal action's
	// `destroy()` (leaving Svelte's own `{#if}` teardown to find the node, which —
	// per the component file's header comment — it cannot, because the action
	// already relocated it out from under the tree Svelte expects). Re-ran this
	// file: this test went red, `document.body.contains(menu)` stayed `true` after
	// unmount. Reverted immediately after.

	it('does not react to window/document events after unmount', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		await unmount(s.instance);
		s.host.remove();
		expect(() => {
			window.dispatchEvent(new Event('resize'));
			window.dispatchEvent(new Event('scroll'));
			document.dispatchEvent(new Event('pointerdown', { bubbles: true }));
		}).not.toThrow();
		await tick();
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
	});
});
