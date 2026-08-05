// SPDX-License-Identifier: GPL-3.0-or-later
//
// Runs under the `dom` (jsdom) vitest project (vite.config.ts). The row's "More
// actions" menu (design spec 2026-08-05-sites-row-overflow-menu-design.md, D1/D3)
// needs a live DOM to open, so this file covers exactly what
// `SiteListRow.svelte.test.ts` (the `server`/svelte-server project) cannot: opening
// the menu, its item contents, and the handoff from "choose Delete in the menu" to
// the row's EXISTING inline confirm.
//
// `RowActionMenu.dom.test.ts` already proves the component's own generic contract
// (portal, keyboard, focus, ARIA) against a placeholder "View"/"Edit"/"Delete" item
// set — this file does not re-prove any of that, only what is specific to THIS row:
// which three actions are wired in, what each one does, and that Delete's `onSelect`
// reaches `confirming`, not a second delete path.
//
// WHAT THIS FILE CANNOT COVER: jsdom does no layout — the menu's on-screen position
// and whether it is visually clipped by `SitesPanel`'s `.panel` are not provable
// here (see `RowActionMenu.dom.test.ts`'s own header, and spec D4 item 4). That is a
// human, screenshot-based check.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import SiteListRow from './SiteListRow.svelte';
import SitesPanel from './SitesPanel.svelte';
import type { SiteDto } from '$lib/ipc';

interface Setup {
	host: HTMLElement;
	instance: object;
	onEdit: ReturnType<typeof vi.fn>;
	onDelete: ReturnType<typeof vi.fn>;
}

const site: SiteDto = {
	id: 'a1',
	name: 'shop',
	domain: 'shop.localhost',
	docroot: '/srv/www/shop',
	webServer: 'nginx',
	phpVersion: '8.3',
	enabled: true,
	createdAt: 1,
	updatedAt: 1
};

/** Same site, disabled — for the one property that differs by `enabled`: the row's
 * OPEN button is state-aware, but (design spec D1, replacing the old inline "View
 * logs" link's own "stays reachable even when the site is disabled, unlike Open")
 * View logs must not be. */
const disabledSite: SiteDto = { ...site, enabled: false };

/** Mounts a real `SiteListRow` for the given site (an enabled one by default). See
 * `RowActionMenu.dom.test.ts`'s own `setup()` for why the `await tick()` below is
 * required before any interaction: `bind:this` (here, inside the row's
 * `RowActionMenu`) populates via a deferred effect, not synchronously during
 * `mount()` — a synthetic `.click()` fired right after `mount()` would reach the
 * trigger's handler with its ref still `undefined`. */
async function setup(dto: SiteDto = site): Promise<Setup> {
	const host = document.createElement('div');
	document.body.appendChild(host);
	const onEdit = vi.fn();
	const onDelete = vi.fn();
	const instance = mount(SiteListRow, {
		target: host,
		props: {
			site: dto,
			installed: [dto.phpVersion],
			onEdit,
			onToggleEnabled: vi.fn(),
			onOpen: vi.fn(),
			onDelete
		}
	});
	await tick();
	return { host, instance, onEdit, onDelete };
}

async function teardown(s: Setup): Promise<void> {
	await unmount(s.instance);
	s.host.remove();
}

// Safety net only, not a substitute for each test's own `teardown` — see
// RowActionMenu.dom.test.ts's own identical note: the portalled menu node any one
// test forgot to clean up would otherwise bleed into whichever test runs next.
afterEach(() => {
	document.body.replaceChildren();
});

function getTrigger(host: HTMLElement): HTMLButtonElement {
	const el = host.querySelector<HTMLButtonElement>('button.trigger');
	if (el === null) throw new Error('SiteListRow rendered no "More actions" trigger');
	return el;
}

function menuItems(): HTMLElement[] {
	return Array.from(document.body.querySelectorAll<HTMLElement>('[role="menuitem"]'));
}

/** Clicks a real DOM node and waits for the resulting state change to reach the DOM
 * — see `RowActionMenu.dom.test.ts`'s file header for why the await is required. */
async function click(el: Element): Promise<void> {
	(el as HTMLElement).click();
	await tick();
}

describe('SiteListRow "More actions" trigger', () => {
	it("names the site, matching the row's other action-label wording", async () => {
		const s = await setup();
		const trigger = getTrigger(s.host);
		expect(trigger.getAttribute('aria-label')).toBe('More actions for shop');
		expect(trigger.getAttribute('aria-haspopup')).toBe('menu');
		await teardown(s);
	});

	// Replaces part of the old inline "View logs" test's own reasoning ("the icon has
	// no other text, so IT must carry the site's name") — that reasoning no longer
	// applies (every item now has visible text, so an item's own accessible name IS
	// that text; a per-item aria-label would be redundant labelling, which the design
	// review explicitly rejected), but the underlying need — a screen-reader user
	// hearing "View logs" alone must still know WHICH site — has to land somewhere.
	// It lands here: RowActionMenu.svelte reuses one `ariaLabel` prop for both the
	// trigger AND the open popup (`role="menu"` gets the same `aria-label`), so every
	// item is read inside a region already announced as "More actions for shop".
	it('the open menu carries the same name, so every item reads in that context', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const menu = document.body.querySelector('[role="menu"]');
		expect(menu?.getAttribute('aria-label')).toBe('More actions for shop');
		await teardown(s);
	});
});

describe('SiteListRow menu contents', () => {
	it('opens to exactly View logs, Edit, and Delete, in that order', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const items = menuItems();
		expect(items.map((el) => el.textContent?.trim())).toEqual(['View logs', 'Edit', 'Delete']);
		await teardown(s);
	});

	it('View logs is a real link to the site error log, not a button', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view] = menuItems();
		expect(view.tagName).toBe('A');
		expect(view.getAttribute('href')).toBe('/logs?source=site-error%3Ashop.localhost');
		await teardown(s);
	});

	// Replaces the old inline "Delete is the only framed control" test (design
	// 2026-08-05-sites-row-overflow-menu-design.md D1): with Delete no longer a
	// separate, bordered `<Button>`, "the destructive action is visually distinct
	// from the routine ones" now means RowActionMenu's own `.item.destructive` (a
	// text colour) marks Delete and ONLY Delete, of the menu's three items.
	it('marks Delete destructive and leaves Edit and View logs unmarked', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [view, edit, del] = menuItems();
		expect(del.classList.contains('destructive')).toBe(true);
		expect(edit.classList.contains('destructive')).toBe(false);
		expect(view.classList.contains('destructive')).toBe(false);
		await teardown(s);
	});

	it('choosing Edit calls onEdit with the site and closes the menu', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [, edit] = menuItems();
		await click(edit);
		expect(s.onEdit).toHaveBeenCalledWith(site);
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
		await teardown(s);
	});
});

// Replaces the old inline "View logs" link's own "stays reachable even when the site
// is disabled, unlike Open" (design 2026-08-05-sites-row-overflow-menu-design.md D1):
// a disabled site is not being served, so `Open` (still inline) is disabled for it —
// but its PAST logs are still worth reading, arguably more so. `RowActionMenuLinkItem`
// has no `disabled` field at all, so there is no way to gate a link item today; this
// pins the behaviour this row actually wants (View logs unconditionally present)
// rather than relying on that absence as an implementation accident.
describe('SiteListRow menu contents when the site is disabled', () => {
	it('still offers View logs, unlike Open', async () => {
		const s = await setup(disabledSite);
		await click(getTrigger(s.host));
		const items = menuItems();
		expect(items.map((el) => el.textContent?.trim())).toEqual(['View logs', 'Edit', 'Delete']);
		const [view] = items;
		expect(view.tagName).toBe('A');
		expect(view.getAttribute('href')).toBe('/logs?source=site-error%3Ashop.localhost');
		await teardown(s);
	});
});

describe('SiteListRow Delete menu item reaches the existing inline confirm', () => {
	// Design spec D3: choosing Delete in the menu only REVEALS the row's existing
	// two-step confirm (`confirming = true`) — it must not open a second dialog and
	// must not call `onDelete` itself.
	it("closes the menu and shows the row's own confirm, without calling onDelete", async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [, , del] = menuItems();
		await click(del);
		expect(document.body.querySelector('[role="menu"]')).toBeNull();
		expect(s.host.querySelector(`[data-testid="confirm-${site.id}"]`)).not.toBeNull();
		expect(s.onDelete).not.toHaveBeenCalled();
		await teardown(s);
	});
	// Vacuity-proved: temporarily changed the Delete item's `onSelect` in
	// SiteListRow.svelte's `rowMenuItems` from `confirming = true` to a no-op
	// (`() => {}`), leaving everything else (including RowActionMenu itself)
	// untouched, and re-ran this file. This test went red —
	// `[data-testid="confirm-a1"]` was `null` after the click — while "opens to
	// exactly View logs, Edit, and Delete" above and the menu-contents group stayed
	// green throughout, confirming the failure was specific to the wiring, not to
	// the menu's own rendering. Reverted before moving on.

	it('the revealed confirm still deletes through its own existing button, not a second path', async () => {
		const s = await setup();
		await click(getTrigger(s.host));
		const [, , del] = menuItems();
		await click(del);
		const confirmDelete = s.host.querySelector<HTMLButtonElement>(
			'[aria-label="Confirm deleting shop"]'
		);
		expect(confirmDelete).not.toBeNull();
		await click(confirmDelete as HTMLButtonElement);
		expect(s.onDelete).toHaveBeenCalledWith(site.id);
		await teardown(s);
	});
});

describe("SiteListRow's menu escapes the real SitesPanel .panel (design spec D2)", () => {
	// `.panel` (SitesPanel.svelte) sets `overflow: hidden` to clip ROWS against its
	// rounded corners. `RowActionMenu.dom.test.ts`'s own "portal" group already
	// proves the popup relocates to `<body>` generically — this proves that
	// guarantee holds for THIS row specifically, mounted inside the real
	// `.panel`/`.rowlist` ancestry rather than a bare host, which is what makes
	// changing `.panel`'s `overflow: hidden` unnecessary: the popup is never a
	// descendant of `.panel` once open, so the property has nothing left to clip.
	it('opens the popup outside .panel, leaving .panel unaffected by anything the popup does', async () => {
		const host = document.createElement('div');
		document.body.appendChild(host);
		const instance = mount(SitesPanel, {
			target: host,
			props: {
				sites: [site],
				installed: [site.phpVersion],
				onAdd: vi.fn(),
				onEdit: vi.fn(),
				onToggleEnabled: vi.fn(),
				onOpen: vi.fn(),
				onDelete: vi.fn()
			}
		});
		await tick();
		const panel = host.querySelector('[data-testid="sites"]');
		if (panel === null) throw new Error('SitesPanel rendered no .panel');
		await click(getTrigger(host));
		const menu = document.body.querySelector('[role="menu"]');
		expect(menu).not.toBeNull();
		expect(panel.contains(menu)).toBe(false);
		await unmount(instance);
		host.remove();
	});
	// Vacuity-proved: temporarily flipped the final assertion to
	// `expect(panel.contains(menu)).toBe(true)` and re-ran this file. It went
	// red — `panel.contains(menu)` really is `false` — confirming the version
	// committed here (`.toBe(false)`) is a genuine, non-vacuous check of the
	// portal's real effect on this row's actual ancestry, not an assertion that
	// happens to hold no matter what (e.g. from `menu` unexpectedly being `null`,
	// which `Node.contains(null)` also reports as `false` — already guarded one
	// line up by `expect(menu).not.toBeNull()`). Reverted before moving on.
});
