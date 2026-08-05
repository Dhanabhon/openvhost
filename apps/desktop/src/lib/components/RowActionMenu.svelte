<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Reusable icon-only trigger + `role="menu"` popup for a row's secondary actions.
  docs/superpowers/specs/2026-08-05-sites-row-overflow-menu-design.md (D2-D4) — the
  Sites row is the first consumer (task B wires View logs / Edit / Delete into it), but
  this file knows nothing about Sites: `items` is a plain, generic list, and every
  action it takes (open/close, portal, focus, keyboard) is driven by that list alone.

  PORTALLED TO `<body>` (D2), not left in place with `position: fixed`. The design
  spec's own stated reason was a claim, not a fact, and it explicitly asked for that
  claim to be checked before relying on it: PR #45 gave `.rowlist`
  (`SitesPanel.svelte`) `container-type: inline-size`, and the spec worried that
  containment would make `.rowlist` a containing block for a `position: fixed`
  descendant, trapping the menu inside `.panel`'s `overflow: hidden` anyway.

  MEASURED IN CHROME 151.0.7922.75, and the claim did not hold THERE, for the rule
  actually in this codebase: a `position: fixed` descendant of a `container-type:
  inline-size` (no other `contain`) ancestor resolved against the VIEWPORT exactly
  as an ordinary fixed element would — `getComputedStyle` reported `contain: none`
  for that ancestor, and the fixed descendant painted and hit-tested at its true
  viewport coordinates, not clipped by an outer `overflow: hidden`. A `contain:
  layout` ancestor (not what `.rowlist` carries) DID trap it — confirmed as a
  positive control, alongside `transform`, which is the textbook case, so the
  method itself was validated, not just asserted.

  NOT RECONCILED with the spec text: a later review of this file read the CSS
  Containment / Container Queries spec as defining a non-`normal` `container-type`
  to itself imply layout containment — which, if that reading is right, WOULD
  establish a containing block for a fixed descendant regardless of what
  `getComputedStyle().contain` reports (the specific thing the measurement above
  read). That reviewer could not verify live and did not assert the measurement
  above is wrong; this file does not assert the spec reading is wrong either. Both
  are recorded because the decision below did not change either way — treat the
  Chrome-151 finding as version- or reading-specific, not as a settled fact about
  the platform.

  Portalling is still the correct call, for the reasons the spec gave that do NOT
  depend on the containment question: it is immune to any ANCESTOR later gaining
  `transform`, `filter`, or an explicit `contain: layout` (any of which WOULD then
  trap an in-place fixed element with no warning), and — unlike the Popover API,
  which would also escape via the top layer — moving a real node into `<body>` is
  something a DOM test can assert directly (`node.parentElement === document.body`),
  where "is it visually clipped" is not.

  POSITIONED from the trigger's `getBoundingClientRect()`, computed once when the
  menu opens. Closes on scroll and on resize (both window-level, `scroll` captured so
  a nested scrollable ancestor's non-bubbling `scroll` event is still caught) rather
  than tracking a moving trigger — matching the spec's explicit choice not to chase
  the anchor around.

  ITEMS are a plain discriminated union (`kind: 'button' | 'link'`), not a snippet:
  this component renders every item itself, so `role="menuitem"` and the keyboard/
  click wiring are guaranteed correct by construction instead of trusted to whatever
  markup a caller hands in. A `link` item renders a real `<a href>` — never coerced
  into a `<button>`, which would lose its navigation semantics (right-click "open in
  new tab", middle-click, status-bar preview, etc.).

  KEYBOARD follows the APG Menu Button pattern: real DOM focus moves onto the items
  themselves (roving, via `.focus()`), not virtual/`aria-activedescendant` like
  `Select.svelte`'s combobox — a menu's items are read as one flat action list, not a
  value to select, so nothing needs to stay parked on the trigger while open. Arrow
  keys WRAP at the ends (Down past the last item goes to the first, and back), which
  is the standard menu behaviour and deliberately different from `Select.svelte`'s
  own clamp-at-the-edges choice — the two are different ARIA patterns (menu vs.
  listbox combobox) and this component is not required to match Select's edge
  behaviour just because it is a nearby sibling.

  Enter AND Space are both handled explicitly here (`preventDefault` + a manual
  `.click()` on the focused item) rather than left to native per-element defaults.
  Buttons get Space activation for free from the browser; anchors do not (Space
  scrolls the page by default) — handling both keys the same way for every item kind
  is what makes "Enter/Space activate" true uniformly, and it is verified in jsdom
  rather than assumed, since jsdom does not reliably reproduce a live browser's
  default keyboard-activation behaviour for focused elements.

  TAB is never trapped (review fix wave — the original cut of this file had no `Tab`
  case at all, which is a real gap this comment used to say nothing about). Matches
  `Select.svelte`'s own `onTriggerKeydown` precedent exactly: closes the menu and
  does NOT `preventDefault`, so the browser's native focus navigation runs
  uninterrupted rather than this widget deciding where Tab goes. Without it the menu
  stayed open — stale `open`/`aria-expanded="true"` — even once focus had visibly
  moved on, which every OTHER deliberate close path (Escape, an item, outside click)
  already handled correctly.

  FOCUS RETURNS to the trigger on every path that closes the menu deliberately —
  Escape, choosing an item (button or link), and clicking outside — funnelled through
  one `closeAndRefocus`. Scroll/resize close WITHOUT refocusing: the user is
  scrolling or resizing, not finishing an interaction with this control, and yanking
  focus back to a trigger that may itself have scrolled off-screen would be worse
  than leaving focus where the browser's own default handling puts it (matches
  `Select.svelte`'s own reasoning for not refocusing on its outside-pointerdown
  close, applied here to the scroll/resize case instead — outside-click on THIS
  component's menu is the one path where the spec explicitly asks for refocus even
  though Select does not do that for its own outside click; that is Select's own
  clamp-vs-menu distinction, not a mismatch to "fix").

  CLEANUP: `<svelte:window>`'s listeners are tied to Svelte's own guaranteed
  component-destroy lifecycle (they exist for the component's whole life, each
  guarded by `if (!open) return`, exactly `Select.svelte`'s own convention) — that
  part cannot leak without a compiler bug. The one piece of manual DOM surgery this
  file owns is the portal action's `document.body.appendChild` — its `destroy()`
  explicitly `.remove()`s the node rather than trusting Svelte's own `{#if}` teardown
  to find it, because once an action has relocated a node elsewhere in the DOM,
  Svelte's ordinary removal logic is not guaranteed to reach across that portal
  boundary. That is the actual leak risk this file carries, and it is what the test
  file's unmount-cleanup group is aimed at.
-->
<script lang="ts">
	export interface RowActionMenuButtonItem {
		readonly kind: 'button';
		readonly label: string;
		/** Styles the item with the app's failure colour — for Delete-shaped actions.
		 * Never gates keyboard/click handling, only appearance. */
		readonly destructive?: boolean;
		readonly onSelect: () => void;
	}

	export interface RowActionMenuLinkItem {
		readonly kind: 'link';
		readonly label: string;
		readonly href: string;
		readonly destructive?: boolean;
	}

	export type RowActionMenuItem = RowActionMenuButtonItem | RowActionMenuLinkItem;

	/** Gap between the trigger's bottom edge and the menu, in px — same value as
	 * `--vh-space-1` (tokens.css), matching `Select.svelte`'s own popup gap
	 * (`top: calc(100% + var(--vh-space-1))`). Computed in JS for the portalled
	 * `position: fixed` offset, so it cannot be expressed as that CSS custom
	 * property directly; kept numerically in sync with it instead. */
	const MENU_GAP_PX = 4;

	let {
		ariaLabel,
		items,
		testId
	}: {
		/** Accessible name for the icon-only trigger (it carries no visible text) —
		 * and reused as the popup `role="menu"`'s own name, so both the button and
		 * the region it opens announce the same "actions for what". */
		ariaLabel: string;
		/** The menu's contents. Generic on purpose: this component owns no knowledge
		 * of what any item does, only how to render, position, and operate the list. */
		items: readonly RowActionMenuItem[];
		/** Test hook on the trigger's real DOM node, following `Button.svelte`'s
		 * `testId` precedent. Opt-in: omitted means no attribute is emitted at all. */
		testId?: string;
	} = $props();

	// Per-instance, SSR-hydration-safe id (see the `$props.id()` rune) — avoids
	// asking every caller to invent a unique id per row just to link
	// `aria-controls` to the popup, which is the class of bug ("two rows share one
	// id") a hand-supplied id would risk.
	const uid = $props.id();
	const menuId = `${uid}-menu`;

	let open = $state(false);
	let position = $state<{ top: number; right: number }>({ top: 0, right: 0 });
	let triggerEl: HTMLButtonElement | undefined = $state();
	let menuEl: HTMLDivElement | undefined = $state();

	/** Every operable row in the (currently rendered) popup, in document order.
	 * Queried live from the DOM rather than tracked as a bound array — mirrors
	 * `Select.svelte`'s own `listEl?.children.item(index)` convention: one ref to
	 * the container is enough, and the item list never gets out of sync with what
	 * is actually on screen. */
	function itemElements(): HTMLElement[] {
		return menuEl ? Array.from(menuEl.querySelectorAll<HTMLElement>('[role="menuitem"]')) : [];
	}

	function openMenu(): void {
		if (triggerEl === undefined) return;
		const rect = triggerEl.getBoundingClientRect();
		// Right-aligned to the trigger rather than left-aligned: this trigger sits
		// at the END of a row (the row-actions strip is `justify-content: flex-end`),
		// so anchoring the menu's RIGHT edge to the trigger's right edge is what
		// keeps it from running off toward the row's far edge. Using `right`
		// (not `left` + a measured menu width) sidesteps needing to know the
		// menu's own width before it has rendered.
		position = { top: rect.bottom + MENU_GAP_PX, right: window.innerWidth - rect.right };
		open = true;
	}

	function closeMenu(): void {
		open = false;
	}

	/** Close and hand focus back to the trigger — every path the user closed the
	 * menu FROM (Escape, choosing an item, clicking outside), never the ambient
	 * scroll/resize close (see the file header). */
	function closeAndRefocus(): void {
		closeMenu();
		triggerEl?.focus();
	}

	function onTriggerClick(): void {
		if (open) closeMenu();
		else openMenu();
	}

	function selectItem(item: RowActionMenuItem): void {
		if (item.kind === 'button') item.onSelect();
		// A link item still closes and refocuses here: the browser's own navigation
		// (already under way from the real `<a href>` the click landed on) is
		// unaffected by moving focus elsewhere in the same synchronous handler —
		// the two are independent, and this keeps "choosing an item" behaving
		// identically for both item kinds.
		closeAndRefocus();
	}

	function focusItemAt(index: number): void {
		const els = itemElements();
		if (els.length === 0) return;
		const wrapped = ((index % els.length) + els.length) % els.length;
		els[wrapped]?.focus();
	}

	function currentItemIndex(): number {
		return itemElements().findIndex((el) => el === document.activeElement);
	}

	function onMenuKeydown(e: KeyboardEvent): void {
		switch (e.key) {
			case 'Escape':
				// Always swallowed here: unlike Select.svelte's combobox trigger (which
				// only owns Escape while ITS popup is open, and must let it bubble to a
				// surrounding dialog otherwise), this handler is on the popup itself and
				// only exists in the DOM at all while `open` is true — there is no
				// "closed" state in which it could wrongly swallow an ancestor's Escape.
				e.preventDefault();
				e.stopPropagation();
				closeAndRefocus();
				return;
			case 'ArrowDown':
				e.preventDefault();
				focusItemAt(currentItemIndex() + 1);
				return;
			case 'ArrowUp':
				e.preventDefault();
				focusItemAt(currentItemIndex() - 1);
				return;
			case 'Enter':
			case ' ':
				// See the file header: both keys are handled the same way for every
				// item kind, rather than relying on each element's own native default.
				e.preventDefault();
				(e.target as HTMLElement | null)?.click();
				return;
			case 'Tab':
				// Matches Select.svelte's own onTriggerKeydown precedent (this file's own
				// header cites Select as its keyboard model): Tab is never trapped here,
				// deliberately NOT `preventDefault`-ed, so the browser's native focus
				// navigation runs uninterrupted. The menu still has to close itself,
				// though — without this case it stayed open (stale `open`/
				// `aria-expanded="true"`) even once focus had visibly left it, which
				// Escape/click-outside/choosing an item all already handle but Tab did
				// not. `closeMenu()`, not `closeAndRefocus()`: an explicit `.focus()`
				// here would fight the tab key's own default action instead of getting
				// out of its way.
				closeMenu();
				return;
		}
	}

	function onWindowPointerDown(e: PointerEvent): void {
		if (!open) return;
		const target = e.target;
		if (target instanceof Node && (triggerEl?.contains(target) || menuEl?.contains(target))) {
			return;
		}
		closeAndRefocus();
	}

	function onWindowScroll(): void {
		if (!open) return;
		closeMenu();
	}

	function onWindowResize(): void {
		if (!open) return;
		closeMenu();
	}

	/** Moves `node` to `<body>` on mount and removes it on destroy — see the file
	 * header for why `destroy()` must do the removal itself rather than trust
	 * Svelte's own `{#if}` teardown to find a node an action has relocated. */
	function portal(node: HTMLElement): { destroy(): void } {
		document.body.appendChild(node);
		return {
			destroy() {
				node.remove();
			}
		};
	}

	// Move focus into the menu the moment it opens (APG Menu Button: opening moves
	// focus to the first item). `open` and `menuEl` both end up as tracked
	// dependencies simply by being read here — same convention as Select.svelte's
	// own effect — and the `if (!open)` guard keeps every other rerun (including the
	// one `menuEl` flipping from undefined to bound causes) a cheap no-op.
	$effect(() => {
		if (!open) return;
		itemElements()[0]?.focus();
	});
</script>

<button
	bind:this={triggerEl}
	type="button"
	class="trigger"
	aria-haspopup="menu"
	aria-expanded={open}
	aria-controls={open ? menuId : undefined}
	aria-label={ariaLabel}
	data-testid={testId}
	onclick={onTriggerClick}
>
	<svg class="kebab" viewBox="0 0 4 16" aria-hidden="true">
		<circle cx="2" cy="2" r="1.6" fill="currentColor" />
		<circle cx="2" cy="8" r="1.6" fill="currentColor" />
		<circle cx="2" cy="14" r="1.6" fill="currentColor" />
	</svg>
</button>

{#if open}
	<!-- `tabindex="-1"`: required by `a11y_interactive_supports_focus` for an
	     element with the `menu` role (unlike `Select.svelte`'s `.listbox`, whose
	     `-1` also keeps it out of a surrounding focus trap's query — there is no
	     such trap here). This container itself is never a tab stop; real focus
	     moves onto its `role="menuitem"` children instead (see the file header). -->
	<div
		bind:this={menuEl}
		id={menuId}
		class="menu"
		role="menu"
		aria-label={ariaLabel}
		style="top: {position.top}px; right: {position.right}px;"
		use:portal
		onkeydown={onMenuKeydown}
		tabindex="-1"
	>
		{#each items as item, index (index)}
			{#if item.kind === 'link'}
				<!-- `item.href` is caller-supplied and generic — this component has no idea
				     whether it is an internal SvelteKit route, an external URL, or (as in its
				     own tests) a bare hash — so it cannot itself call `resolve()` on it; the
				     caller is responsible for passing an already-resolved href, exactly as
				     `SiteListRow.svelte`'s own "View logs" link does today. Block
				     disable/enable, not `-next-line`, matching that file's own caution: prettier
				     may re-wrap this element, and `-next-line` would then silently stop covering
				     the `href` line. -->
				<!-- eslint-disable svelte/no-navigation-without-resolve -->
				<a
					class="item"
					class:destructive={item.destructive === true}
					role="menuitem"
					href={item.href}
					onclick={() => selectItem(item)}
				>
					{item.label}
				</a>
				<!-- eslint-enable svelte/no-navigation-without-resolve -->
			{:else}
				<button
					type="button"
					class="item"
					class:destructive={item.destructive === true}
					role="menuitem"
					onclick={() => selectItem(item)}
				>
					{item.label}
				</button>
			{/if}
		{/each}
	</div>
{/if}

<svelte:window
	onpointerdown={onWindowPointerDown}
	onscrollcapture={onWindowScroll}
	onresize={onWindowResize}
/>

<style>
	/* Reproduces Button.svelte's `.btn`/`.btn-ghost`/`.btn-sm` recipe locally, same
	   precedent SiteListRow.svelte already uses twice (`.icon-link`, `.btn-danger`)
	   for a control Button.svelte cannot express — here, `aria-haspopup`, which
	   Button.svelte has no prop for. Sized and padded to match the `.btn-ghost.btn-sm`
	   icon buttons this trigger sits beside in a row-actions strip (Open, View logs). */
	.trigger {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		font: inherit;
		padding: 4px 10px;
		color: var(--vh-text);
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--vh-radius-control);
		cursor: pointer;
		transition:
			background var(--vh-dur-fast) var(--vh-ease-out),
			border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.trigger:hover {
		background: color-mix(in oklab, var(--vh-text) 6%, transparent);
		border-color: var(--vh-border-strong);
	}
	/* Same doubled-frame fix as `.btn-ghost`/`.icon-link` elsewhere in the app
	   (Button.svelte, SiteListRow.svelte): merges the ring into the border instead
	   of stacking a second ring outside it, and keeps this trigger visually part of
	   the same row-actions family as its neighbours. */
	.trigger:focus-visible {
		border-color: var(--vh-focus-ring);
		outline-offset: 0;
	}
	.kebab {
		width: 0.85em;
		height: 0.85em;
	}

	/* The popup. No local `:focus-visible` override on `.item` below — unlike
	   `.trigger`, an item has no border of its own at rest, so the global ring
	   (tokens.css, 2px outline at a 2px offset) is already the single, undoubled
	   focus indicator Button.svelte's own header comment describes as correct for a
	   borderless control; overriding it here would be solving a problem this
	   element does not have. */
	.menu {
		position: fixed;
		z-index: var(--vh-z-dialog);
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 160px;
		max-height: 15em;
		overflow-y: auto;
		padding: var(--vh-space-1);
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		box-shadow: var(--vh-shadow-overlay);
		animation: vh-pop-in var(--vh-dur-fast) var(--vh-ease-out);
	}
	@keyframes vh-pop-in {
		from {
			transform: translateY(-4px);
			opacity: 0;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.menu {
			animation: none;
		}
	}

	.item {
		display: flex;
		align-items: center;
		width: 100%;
		font: inherit;
		font-size: var(--vh-text-table);
		text-align: left;
		text-decoration: none;
		white-space: nowrap;
		color: var(--vh-text);
		background: transparent;
		border: 0;
		border-radius: var(--vh-radius-control);
		padding: 6px var(--vh-space-2);
		cursor: pointer;
	}
	.item:hover {
		background: color-mix(in oklab, var(--vh-text) 6%, transparent);
	}
	/* Text-only red, not a bordered danger button (SiteListRow's `.btn-danger`) —
	   this is a row inside a flat list of rows, not a standalone control, so the
	   established menu-item vocabulary (colour signals the action, no boxed frame)
	   fits better here than a hand-rolled second `.btn-danger` copy. */
	.item.destructive {
		color: var(--vh-fail);
	}
	/* Same tint SiteListRow's `.btn-danger:hover` already uses for exactly this
	   purpose — reused, not invented. */
	.item.destructive:hover {
		background: var(--vh-fail-tint);
	}
</style>
