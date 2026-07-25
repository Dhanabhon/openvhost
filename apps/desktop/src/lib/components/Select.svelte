<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Single-select dropdown drawn from the design tokens — the replacement for a native
  `<select>`, whose popup the OS renders itself (unstyleable, and visibly not part of
  this app).

  Pattern: WAI-ARIA APG "Select-Only Combobox" — a `role="combobox"` trigger that KEEPS
  DOM FOCUS while open and tracks the highlighted row with `aria-activedescendant`,
  rather than moving real focus into the popup. Two reasons that choice matters here:

  1. The only consumer so far, `SiteDrawer.svelte`, runs a focus trap with a
     window-scoped `focusin` handler that recaptures focus the moment it leaves the
     drawer, plus a Tab handler that wraps at the drawer's first/last focusable
     element. Keeping focus parked on one element the trap already knows about means
     none of that has to know this component exists.
  2. The popup therefore never needs to be portalled out of the drawer's DOM subtree —
     which would be fatal: that same `focusin` handler would yank focus straight back
     the instant the popup opened.

  `role="combobox"` on the `<button>` (not the bare implicit `button` role) is
  deliberate and checked, not decorative: `button` supports neither
  `aria-activedescendant` nor `aria-invalid`, so the ARIA-in-HTML mapping for
  `<button>` — which explicitly permits `role="option"`, `role="combobox"` and friends —
  is what makes both attributes legal here. Verified against aria-query's role table
  (the same data `svelte-check`'s `a11y_role_supports_aria_props` consults); it is also
  what Radix/Bits-style select triggers ship.

  The popup stays in the DOM when closed (`hidden`), so `aria-controls` always resolves
  to a real element and the option set is present in server-rendered markup — which is
  what makes it assertable in the `node` vitest project (no DOM there).
-->
<script lang="ts">
	let {
		id,
		labelId,
		options,
		value = $bindable(),
		invalid = false,
		describedBy,
		mono = false
	}: {
		/** DOM id for the trigger. Keep a `<label for={id}>` pointed at it: `<button>` is a
		 * labelable element, so that label alone gives the combobox its accessible name
		 * (`combobox` does not take its name from content, so the visible selected value
		 * cannot stand in for one). */
		id: string;
		/** id of that same `<label>` element, reused to name the popup listbox. */
		labelId: string;
		options: readonly { value: string; label: string }[];
		value: string;
		/** Renders `aria-invalid="true"` on the trigger, matching what the native
		 * `<select>` carried. `combobox` supports it; a bare `button` role would not. */
		invalid?: boolean;
		describedBy?: string;
		/** Monospace + dense-table size, for values that are really code (version
		 * numbers, ports, paths). */
		mono?: boolean;
	} = $props();

	/** Reset window for the type-to-jump buffer, matching the APG examples. */
	const TYPEAHEAD_RESET_MS = 500;

	let open = $state(false);
	/** Highlighted row (APG "visual focus"), NOT the committed value — `-1` = none. */
	let activeIndex = $state(-1);
	let rootEl: HTMLElement | undefined = $state();
	let triggerEl: HTMLButtonElement | undefined = $state();
	let listEl: HTMLElement | undefined = $state();

	// Plain, non-reactive (nothing renders off any of these): the typeahead buffer and its
	// reset timer, plus whether the highlight was last moved by the keyboard. Same
	// convention as SiteDrawer's `closing` flag — and for `scrollHighlight` it is load
	// bearing, because the scroll effect below must be able to read it WITHOUT taking a
	// dependency on it.
	let typed = '';
	let typedTimer: ReturnType<typeof setTimeout> | undefined;
	let scrollHighlight = false;

	const listboxId = $derived(`${id}-listbox`);
	const selectedIndex = $derived(options.findIndex((o) => o.value === value));
	/** Falls back to the raw `value` when it matches no option: showing the real stored
	 * string is honest, where a blank trigger would hide it. Callers should not rely on
	 * this — see `phpVersionOptions` in `$lib/sites.derive` for the supported way to keep
	 * an unlisted value selectable. */
	const triggerLabel = $derived(options[selectedIndex]?.label ?? value);
	const activeOptionId = $derived(open && activeIndex >= 0 ? optionId(activeIndex) : undefined);

	function optionId(index: number): string {
		// Index, not value: ids stay collision-free and syntactically safe whatever a
		// stored value happens to contain.
		return `${id}-opt-${index}`;
	}

	function clampIndex(index: number): number {
		if (options.length === 0) return -1;
		return Math.min(Math.max(index, 0), options.length - 1);
	}

	/** Row to highlight when the popup opens: the current value, else the first row. */
	function initialIndex(): number {
		return selectedIndex >= 0 ? selectedIndex : 0;
	}

	/**
	 * Move the highlight. `scroll` says whether the row should be scrolled into view:
	 * true for keyboard moves and for opening (the current value must be visible even in a
	 * long list), false for pointer hover — a partially visible row scrolling itself out
	 * from under the cursor is exactly the jumpiness to avoid.
	 */
	function setActive(index: number, scroll: boolean): void {
		scrollHighlight = scroll;
		activeIndex = clampIndex(index);
	}

	function openAt(index: number): void {
		setActive(index, true);
		open = true;
	}

	function closeList(): void {
		open = false;
		clearTypeahead();
	}

	/** Close and hand focus back to the trigger. Used by every path the user drove
	 * deliberately from the keyboard or by clicking a row, so they resume from the
	 * control they were on instead of from `<body>`. */
	function closeAndRefocus(): void {
		closeList();
		triggerEl?.focus();
	}

	function commit(index: number): void {
		const option = options[index];
		if (option !== undefined) value = option.value;
	}

	function clearTypeahead(): void {
		typed = '';
		if (typedTimer !== undefined) clearTimeout(typedTimer);
		typedTimer = undefined;
	}

	// Effects, not lifecycle hooks: neither runs during SSR, so both are inert in the
	// `node` test project.
	$effect(() => {
		// `activeIndex` is the tracked dependency; `scrollHighlight` is deliberately a plain
		// variable, so a hover-driven move re-runs this and finds it false rather than
		// scrolling.
		const index = activeIndex;
		if (!open || !scrollHighlight) return;
		// `block: 'nearest'` so it only scrolls when the row really is out of view — and,
		// because the popup cannot escape the drawer's scrolling body (no portal), this
		// also nudges that body far enough to reveal a popup opening near its bottom edge.
		listEl?.children.item(index)?.scrollIntoView({ block: 'nearest' });
	});
	// Teardown-only (no reactive reads, so it runs once): cancel a pending typeahead
	// reset when the component goes away.
	$effect(() => {
		return clearTypeahead;
	});

	function onTriggerKeydown(e: KeyboardEvent): void {
		if (e.key === 'Escape') {
			// Swallowed ONLY while the popup is open. Closed, Esc has to keep bubbling to
			// whatever owns the surrounding dialog (SiteDrawer listens on `window`) so it
			// still dismisses it; open, it must not, or one keypress would close both.
			if (!open) return;
			e.preventDefault();
			e.stopPropagation();
			closeAndRefocus();
			return;
		}
		if (e.key === 'Tab') {
			// APG select-only combobox, and every native `<select>`: Tab takes the
			// highlighted row with it. Deliberately NOT prevented — the surrounding
			// dialog's own Tab handling still has to run.
			if (open) {
				commit(activeIndex);
				closeList();
			}
			return;
		}
		// Ctrl/Cmd combinations belong to the OS and the app, never to this widget.
		if (e.ctrlKey || e.metaKey) return;
		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				if (open) setActive(activeIndex + 1, true);
				else openAt(initialIndex());
				return;
			case 'ArrowUp':
				e.preventDefault();
				if (open) setActive(activeIndex - 1, true);
				else openAt(initialIndex());
				return;
			case 'Home':
				e.preventDefault();
				openAt(0);
				return;
			case 'End':
				e.preventDefault();
				openAt(options.length - 1);
				return;
			case 'Enter':
			case ' ':
				// preventDefault also suppresses the button's own activation behaviour — the
				// browser synthesises a `click` from Enter/Space on a `<button>` — which keeps
				// `onTriggerClick` a pointer-only path that cannot double-toggle the popup.
				e.preventDefault();
				if (open) {
					commit(activeIndex);
					closeAndRefocus();
				} else {
					openAt(initialIndex());
				}
				return;
		}
		// Alt is still allowed through to the arrows above (Alt+Down opens, as on a native
		// select) but must not be read as typing.
		if (e.altKey) return;
		if (e.key.length === 1) {
			e.preventDefault();
			typeahead(e.key);
		}
	}

	/** Type-to-jump: moves the highlight, never the value, and opens the popup so the
	 * jump is visible rather than silently rewriting the field. */
	function typeahead(char: string): void {
		typed += char;
		if (typedTimer !== undefined) clearTimeout(typedTimer);
		typedTimer = setTimeout(() => {
			typed = '';
			typedTimer = undefined;
		}, TYPEAHEAD_RESET_MS);

		// APG: repeating one character cycles through the rows that start with it; a
		// genuine multi-character run is matched as a prefix from the top of the list.
		const repeating = typed.length > 1 && [...typed].every((c) => c === typed[0]);
		const needle = (repeating ? typed[0] : typed).toLowerCase();
		const from = repeating ? activeIndex + 1 : 0;
		const match = matchFrom(needle, from);
		if (match === -1) return;
		openAt(match);
	}

	/** First option whose label starts with `needle`, searched circularly from `from`. */
	function matchFrom(needle: string, from: number): number {
		const count = options.length;
		for (let step = 0; step < count; step++) {
			const index = (from + step) % count;
			if (options[index].label.toLowerCase().startsWith(needle)) return index;
		}
		return -1;
	}

	function onTriggerClick(e: MouseEvent): void {
		// `detail === 0` is a keyboard-synthesised click. Those are already handled (and
		// prevented) in `onTriggerKeydown`; ignoring them here means that even on an engine
		// that fires the click anyway, one keypress cannot toggle the popup twice.
		if (e.detail === 0) return;
		if (open) closeList();
		else openAt(initialIndex());
	}

	function onWindowPointerDown(e: Event): void {
		if (!open || rootEl === undefined) return;
		const target = e.target;
		if (target instanceof Node && rootEl.contains(target)) return;
		// No refocus here: the user is on their way somewhere else, and pulling focus back
		// to the trigger would fight whatever they just pressed.
		closeList();
	}
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="combo" bind:this={rootEl}>
	<button
		{id}
		type="button"
		class="trigger"
		class:mono
		role="combobox"
		aria-haspopup="listbox"
		aria-controls={listboxId}
		aria-expanded={open}
		aria-activedescendant={activeOptionId}
		aria-invalid={invalid ? 'true' : undefined}
		aria-describedby={describedBy}
		bind:this={triggerEl}
		onclick={onTriggerClick}
		onkeydown={onTriggerKeydown}
	>
		<span class="trigger-value">{triggerLabel}</span>
		<svg
			class="chev"
			viewBox="0 0 10 10"
			fill="none"
			stroke="currentColor"
			stroke-width="1.6"
			stroke-linecap="round"
			stroke-linejoin="round"
			aria-hidden="true"><path d="M2 3.6l3 3 3-3" /></svg
		>
	</button>

	<!-- `tabindex="-1"`: required by `a11y_interactive_supports_focus` for an element with
	     the `listbox` role and a mouse handler, and `-1` keeps it out of the surrounding
	     dialog's focus-trap query (which excludes `[tabindex="-1"]`) — focus stays on the
	     trigger by design.
	     `onmousedown` + preventDefault: pressing a row must not move focus off the trigger.
	     Without it the press blurs the trigger, `aria-activedescendant` loses its host, and
	     SiteDrawer's `focusin` recapture drags focus to the top of the drawer mid-click. -->
	<div
		id={listboxId}
		class="listbox"
		role="listbox"
		aria-labelledby={labelId}
		tabindex="-1"
		hidden={!open}
		bind:this={listEl}
		onmousedown={(e) => e.preventDefault()}
	>
		{#each options as option, index (option.value)}
			<!-- `<button role="option">`: the ARIA-in-HTML mapping lists `option` among the
			     roles `<button>` may take, and a real button keeps this a genuine control
			     (pointer cursor, click semantics) instead of a div with handlers bolted on.
			     `tabindex="-1"` keeps every row out of the tab order — this pattern never
			     moves real focus into the popup. -->
			<button
				type="button"
				class="option"
				class:mono
				role="option"
				id={optionId(index)}
				aria-selected={option.value === value}
				data-active={open && index === activeIndex}
				tabindex="-1"
				onclick={() => {
					commit(index);
					closeAndRefocus();
				}}
				onmouseenter={() => setActive(index, false)}
			>
				<span class="check" aria-hidden="true">
					{#if option.value === value}
						<svg
							viewBox="0 0 10 10"
							fill="none"
							stroke="currentColor"
							stroke-width="1.8"
							stroke-linecap="round"
							stroke-linejoin="round"><path d="M1.5 5.4l2.4 2.4L8.5 2.6" /></svg
						>
					{/if}
				</span>
				<span class="option-label">{option.label}</span>
			</button>
		{/each}
	</div>
</div>

<style>
	/* The trigger reproduces docs/design/mock.css's `.input` recipe rather than importing
	   it — Svelte's scoped styles mean SiteDrawer's own `.input` rule cannot reach in here,
	   and the two must stay visually identical or this control would not line up with the
	   Name/Domain fields stacked above it. That is why the 7px/10px padding is carried
	   verbatim from the mock instead of being rounded to a --vh-space-* step: a 2px
	   difference is plainly visible in a column of fields. Colour, radius, motion and
	   elevation are all tokens. */
	.combo {
		position: relative;
	}
	.trigger {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--vh-space-2);
		width: 100%;
		font: inherit;
		text-align: left;
		color: var(--vh-text);
		background: var(--vh-surface);
		border: 1px solid var(--vh-border-strong);
		border-radius: var(--vh-radius-control);
		padding: 7px 10px;
		cursor: pointer;
		transition: border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.trigger:hover {
		border-color: color-mix(in oklab, var(--vh-text) 40%, transparent);
	}
	/* Two-class selectors on purpose: `.trigger` and `.option` both set `font: inherit`
	   (a shorthand that resets family AND size), so a bare `.mono` rule would win or lose
	   purely on source order. */
	.trigger.mono,
	.option.mono {
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
	}
	/* Truncate rather than widen: panels have to stay usable at 380px. */
	.trigger-value {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	/* Marks sized in em so they track whatever type scale the host field uses, the same
	   convention WebServerIcon.svelte follows. */
	.chev {
		width: 0.7em;
		height: 0.7em;
		flex: none;
		color: var(--vh-text-2);
		transition: transform var(--vh-dur-fast) var(--vh-ease-out);
	}
	.trigger[aria-expanded='true'] .chev {
		transform: rotate(180deg);
	}

	.listbox {
		position: absolute;
		top: calc(100% + var(--vh-space-1));
		left: 0;
		right: 0;
		/* Above the drawer's own body content; the drawer itself already sits on
		   --vh-z-drawer, so this only has to win inside that stacking context. */
		z-index: var(--vh-z-sticky);
		/* ~6 rows, then scroll internally. The popup deliberately stays inside the
		   drawer's DOM subtree (see the file header), so it cannot escape that scroll
		   container — capping it keeps a long list from being clipped instead of scrolled. */
		max-height: 15em;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 1px;
		padding: var(--vh-space-1);
		background: var(--vh-surface);
		border: 1px solid var(--vh-border-strong);
		border-radius: var(--vh-radius-control);
		box-shadow: var(--vh-shadow-overlay);
		animation: vh-pop-in var(--vh-dur-fast) var(--vh-ease-out);
	}
	/* `hidden` alone would lose to the `display: flex` above. */
	.listbox[hidden] {
		display: none;
	}
	@keyframes vh-pop-in {
		from {
			transform: translateY(-4px);
			opacity: 0;
		}
	}
	/* tokens.css already flattens every animation globally under reduced motion; this
	   states it locally too, so the intent survives a refactor of that global rule. */
	@media (prefers-reduced-motion: reduce) {
		.listbox {
			animation: none;
		}
		.chev {
			transition: none;
		}
	}

	.option {
		display: flex;
		align-items: center;
		gap: var(--vh-space-2);
		width: 100%;
		font: inherit;
		text-align: left;
		color: var(--vh-text);
		background: transparent;
		border: 0;
		border-radius: var(--vh-radius-control);
		padding: 5px var(--vh-space-2);
		cursor: pointer;
	}
	/* One highlight, one source of truth: the row under the pointer and the row the
	   arrow keys are on are the same state, so hover is driven by `data-active` (set on
	   mouseenter) instead of a separate `:hover` rule that could disagree with
	   `aria-activedescendant`. */
	.option[data-active='true'] {
		background: var(--vh-selected);
	}
	.option[aria-selected='true'] {
		font-weight: 600;
	}
	.check {
		width: 0.85em;
		flex: none;
		display: inline-flex;
		align-items: center;
		color: var(--vh-accent);
	}
	.check svg {
		width: 0.85em;
		height: 0.85em;
	}
	.option-label {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
