<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script module lang="ts">
	/** The only characters a `.localhost` subdomain may contain, matching `Domain::parse`
	 * in crates/openvhost-core/src/site/model.rs: dot-joined labels of `[a-z0-9-]`. */
	const HOSTNAME_CHAR = /^[a-z0-9.-]$/;

	/** The only characters a site name may contain, matching `SiteName::parse`: a slug of
	 * `[a-z0-9-]`. NO dot — that is the one difference from `HOSTNAME_CHAR`, and mixing the
	 * two up would let `my.site` reach a field whose backend rejects it. */
	const SLUG_CHAR = /^[a-z0-9-]$/;

	/**
	 * Keep only the characters `allowed` accepts, lowercasing as we go.
	 *
	 * Per code point, and independent of position, which is what makes the caret arithmetic
	 * in `filterInput` exact: filtering a prefix of `s` always yields a prefix of filtering
	 * `s`, so the number of surviving characters before the caret is simply the length of
	 * the filtered prefix. A regex `.replace()` over the whole string could not promise
	 * that once `toLowerCase()` is allowed to change a string's length (`İ` → `i` +
	 * combining dot).
	 *
	 * Downcasing rather than rejecting is deliberate: both fields are case-insensitive
	 * identifiers whose parsers demand lowercase, so an uppercase keystroke has exactly one
	 * sane meaning. Dropping it instead would look like a broken keyboard.
	 */
	function filterChars(s: string, allowed: RegExp): string {
		let out = '';
		for (const ch of s) {
			const lower = ch.toLowerCase();
			if (allowed.test(lower)) out += lower;
		}
		return out;
	}

	/** Slug charset, then drop any leading `-`, because `SiteName::parse` requires the first
	 * character to be alphanumeric.
	 *
	 * Stripping a LEADING run is the only positional rule that keeps the prefix property
	 * `filter(prefix)` is a prefix of `filter(whole)` — which the caret formula depends on.
	 * Proof: let A = filterChars(prefix), B = filterChars(whole), A a prefix of B. If A is
	 * all dashes then stripping gives `''`, a prefix of anything. Otherwise A = `-`×k + rest
	 * with rest[0] not a dash; because A is a prefix of B, B shares that same `-`×k run, so
	 * both strip exactly k and A[k..] stays a prefix of B[k..].
	 *
	 * A TRAILING-dash rule could NOT be enforced this way — and must not be, since a user
	 * typing `my-` is mid-word. That, and the 1..=63 length bound (see `maxlength` on the
	 * input), are what the inline server error still covers. */
	function filterSlug(s: string): string {
		return filterChars(s, SLUG_CHAR).replace(/^-+/, '');
	}

	/**
	 * Filter one input event's raw value and say where the caret belongs afterwards.
	 *
	 * Exported for `SiteDrawer.svelte.test.ts`: the caret arithmetic is the part of this
	 * change that can actually be tested in the DOM-less `node` vitest project, and it is
	 * also the part most likely to be wrong. These are TYPING AFFORDANCES ONLY —
	 * `SiteName::parse`/`Domain::parse` remain the authority, and nothing here decides
	 * whether a name or domain is valid.
	 */
	function filterInput(
		raw: string,
		caret: number,
		filter: (s: string) => string
	): { value: string; caret: number } {
		return { value: filter(raw), caret: filter(raw.slice(0, caret)).length };
	}

	export function filterDomainInput(raw: string, caret: number): { value: string; caret: number } {
		return filterInput(raw, caret, (s) => filterChars(s, HOSTNAME_CHAR));
	}

	export function filterNameInput(raw: string, caret: number): { value: string; caret: number } {
		return filterInput(raw, caret, filterSlug);
	}
</script>

<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { open } from '@tauri-apps/plugin-dialog';
	import { resolve } from '$app/paths';
	import type { SiteDto, SiteInput } from '$lib/ipc';
	import {
		composeDomain,
		splitDomain,
		defaultPhpVersion,
		phpVersionOptions,
		scaffoldPreview,
		WEB_SERVERS,
		type WebServerKind
	} from '$lib/sites.derive';
	import Button from './Button.svelte';
	import Select from './Select.svelte';
	import WebServerIcon from './WebServerIcon.svelte';

	let {
		site,
		fieldErrors,
		installed,
		onSave,
		onDelete,
		onClose
	}: {
		site: SiteDto | null;
		fieldErrors: Record<string, string>;
		/** PHP majors actually installed on this machine (`phpEnvironment()`'s runtimes
		 * filtered to `installed`), threaded down from `+page.svelte`. See
		 * `phpVersionOptions`/`defaultPhpVersion` in `$lib/sites.derive` for why this
		 * replaced a hardcoded list. */
		installed: readonly string[];
		onSave: (id: string | null, input: SiteInput, createFolder: boolean) => Promise<boolean>;
		onDelete: (id: string) => Promise<boolean>;
		onClose: () => void;
	} = $props();

	// `SiteDto.webServer` crosses IPC as a bare string (specta exports the Rust enum's wire
	// form, not a TS union), so narrow it here — anything unrecognised falls back to the one
	// web server OpenVHost can actually configure.
	function initialWebServer(dto: SiteDto | null): WebServerKind {
		const found = WEB_SERVERS.find((server) => server === dto?.webServer);
		return found ?? WEB_SERVERS[0];
	}

	// Form state is seeded ONCE from `site` (edit) or blank defaults (create), via `untrack`
	// (https://svelte.dev/e/state_referenced_locally) — an explicit "read this prop exactly
	// once, do not track it" signal, not a live mirror. That is safe because the drawer is
	// modal: the backdrop blocks every pointer path to another row's Edit button, so `site`
	// cannot change under an already-mounted instance.
	let name = $state(untrack(() => site?.name ?? ''));
	let subdomain = $state(untrack(() => (site ? splitDomain(site.domain) : '')));
	let docroot = $state(untrack(() => site?.docroot ?? ''));
	// NOT seeded from `site` like the fields above: the checkbox this backs only ever
	// renders in create mode (see the Project-folder field below), so an existing site
	// has nothing to read here — always starting unchecked is the spec's explicit
	// decision, not a stand-in default.
	let createFolder = $state(false);
	let webServer = $state<WebServerKind>(untrack(() => initialWebServer(site)));
	let phpVersion = $state(untrack(() => site?.phpVersion ?? defaultPhpVersion(installed) ?? ''));
	let enabled = $state(untrack(() => site?.enabled ?? true));

	// Built once, from the site's STORED version rather than from the live `phpVersion`
	// state above, for two reasons: the same read-once rationale as the fields (`site`
	// cannot change under a mounted drawer), and because deriving it from the live value
	// would delete an unlisted version's option the moment the user clicked away from it,
	// stranding them with no way back. See `phpVersionOptions` for why the option has to
	// exist at all.
	//
	// `installed` is read the same read-once way: by the time this drawer can mount at
	// all, `+page.svelte`'s own `phpEnvironment()` call (fired at page load, well before
	// any Add/Edit button exists to click) has settled, so there is nothing to gain from
	// tracking it live — and doing so would reopen the same "list changes out from under
	// an open selection" problem `phpVersion` above is already written to avoid.
	const phpOptions = phpVersionOptions(
		untrack(() => site?.phpVersion),
		untrack(() => installed)
	);

	// Only true for a brand-new site on a machine with nothing installed: an existing
	// site's stored version always yields at least its own (possibly "not available")
	// entry, so `phpOptions` is only ever actually empty here. Save is disabled in this
	// state (see the drawer-foot below) rather than letting a doomed, invisible PHP
	// version reach the backend — the exact trap this task closes.
	const phpUnavailable = phpOptions.length === 0;

	let submitting = $state(false);
	let confirmingDelete = $state(false);
	let deleting = $state(false);
	// Surfaces a rejected `open()` call (ACL denial, plugin/runtime error) next to the
	// Project-folder field — a user *cancel* resolves `null` and is handled in `browse()`
	// below without touching this; only a genuine failure sets it. Cleared at the start of
	// every `browse()` attempt so a stale message can't outlive a later success.
	let pickerError = $state<string | null>(null);

	let drawerEl: HTMLElement | undefined = $state();
	let nameInput: HTMLInputElement | undefined = $state();
	let previouslyFocused: HTMLElement | null = null;
	// Plain (non-reactive — nothing renders off this) flag, flipped synchronously at the very
	// start of the unmount cleanup below, before either of its `.focus()` calls. See that
	// cleanup's comment for why `onFocusIn` needs to know a close is already in progress.
	let closing = false;

	const heading = $derived(site === null ? 'Add site' : `Edit site — ${site.name}`);

	// Focus management (behaviour contract): move focus into the drawer on mount, restore it
	// to whatever triggered the drawer (the clicked Add/Edit button) on unmount. Every close
	// path — Esc, backdrop click, Cancel, a successful save, a successful delete — flows
	// through the parent flipping `drawerOpen = false`, which unmounts this component and
	// runs this same cleanup, so focus restoration has exactly one code path to verify.
	onMount(() => {
		previouslyFocused = document.activeElement as HTMLElement | null;
		nameInput?.focus();
		return () => {
			// Flip `closing` first, synchronously, before any `.focus()` call below: those calls
			// synchronously dispatch a `focusin` that `onFocusIn` — possibly still attached for
			// this same teardown pass, since this cleanup's order relative to the
			// `<svelte:window>` binding's own teardown isn't something to depend on — would
			// otherwise see and "helpfully" recapture focus right back into the drawer that is
			// in the middle of closing.
			closing = true;
			// On the delete path, `previouslyFocused` is the just-deleted row's own Edit
			// button: `store.remove()` → `store.load()` removes that row (and its button) from
			// the DOM *before* `onClose()` unmounts this drawer and runs this cleanup, so
			// `.focus()` on it would silently no-op, dumping a keyboard user on `<body>`. Guard
			// it, and fall back to a real, still-present focusable element: the Sites page's
			// own primary action, `SitesPanel.svelte`'s "Add site" button, reached via the
			// `data-vh-focus-fallback` hook it sets through `Button`'s `focusFallback` prop.
			if (previouslyFocused && document.contains(previouslyFocused)) {
				previouslyFocused.focus();
				return;
			}
			const fallback = document.querySelector<HTMLElement>('[data-vh-focus-fallback]');
			if (fallback) {
				fallback.focus();
				return;
			}
			// Neither the original trigger nor the page-level fallback exists (e.g. this
			// drawer somehow rendered on a page with no such hook) — nothing focusable to hand
			// off to. Explicit, acknowledged no-op, not a silently swallowed bug.
		};
	});

	/** Currently visible, enabled, tab-reachable descendants — recomputed live so the
	 * danger zone's confirm step (which swaps in new buttons) is always covered. */
	function focusable(): HTMLElement[] {
		if (!drawerEl) return [];
		const selector =
			'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';
		return Array.from(drawerEl.querySelectorAll<HTMLElement>(selector)).filter(
			(el) => el.offsetParent !== null
		);
	}

	/** Window-scoped (see `<svelte:window>` below, not the `<aside>`) so Esc/Tab keep working
	 * even if focus has drifted outside the drawer entirely — the realistic trigger is the
	 * native folder picker (Browse) handing first-responder back to the webview body instead
	 * of the Browse button, which could not be verified in this sandbox. This only runs while
	 * `<svelte:window>` is part of the mounted tree, i.e. only while this component itself is
	 * mounted: the parent renders `<SiteDrawer>` exclusively inside `{#if drawerOpen}`, so
	 * there is no window where this listener is live but the drawer is "closed" — mount
	 * implies open, and Svelte tears down `<svelte:window>` bindings on unmount like any other
	 * effect, so nothing is left active afterward and nothing here interferes with any other
	 * page (e.g. Services). `!drawerEl`/`closing` are cheap extra guards for the instant
	 * before `bind:this` resolves and for the brief close transition (see the onMount cleanup
	 * above). */
	function onKeydown(e: KeyboardEvent): void {
		if (!drawerEl || closing) return;
		if (e.key === 'Escape') {
			e.preventDefault();
			onClose();
			return;
		}
		if (e.key !== 'Tab') return;
		const els = focusable();
		if (els.length === 0) return;
		const first = els[0];
		const last = els[els.length - 1];
		if (!drawerEl.contains(document.activeElement)) {
			// Focus is outside the drawer altogether — not merely at one of the two wrap
			// boundaries checked below — so there is nothing sane to wrap from. Recapture it
			// instead of letting Tab move focus wherever the untracked activeElement would
			// send it next.
			e.preventDefault();
			first.focus();
			return;
		}
		if (e.shiftKey && document.activeElement === first) {
			e.preventDefault();
			last.focus();
		} else if (!e.shiftKey && document.activeElement === last) {
			e.preventDefault();
			first.focus();
		}
	}

	/** Belt-and-braces recapture for the one path `onKeydown` above can't see at all: focus
	 * landing outside the drawer with no Tab keypress involved (again, the native picker
	 * returning focus to the webview body is the concrete, unverified-in-sandbox trigger).
	 * Fires on every focus change anywhere in the document while mounted; a no-op the
	 * overwhelming majority of the time, since focus moving between the drawer's own fields
	 * already satisfies `contains`. Same mount-scoped lifetime and guards as `onKeydown`. */
	function onFocusIn(): void {
		if (!drawerEl || closing || drawerEl.contains(document.activeElement)) return;
		const els = focusable();
		if (els.length > 0) els[0].focus();
	}

	/**
	 * Restrict the Domain field to hostname characters as the user types.
	 *
	 * The whole point of doing this by hand — rather than reassigning a `bind:value` and
	 * letting Svelte write the input back — is the caret. Assigning `input.value` moves the
	 * text entry cursor to the end whenever the string actually changes (HTML standard,
	 * `value` IDL setter), so mid-string editing becomes impossible unless the caret is
	 * restored in the same synchronous turn. Hence: read the raw value AND the caret, filter
	 * both together (`filterDomainInput`), write the element, then put the caret back.
	 *
	 * The field is therefore `value={subdomain}` + this handler, NOT `bind:value`: with the
	 * element already holding the filtered string, Svelte's own `value` update is a no-op
	 * (it compares `element.value` first), so nothing else ever touches the selection.
	 *
	 * Covers paste, drag-and-drop and autofill for free — they all raise `input`. `My_Site.COM`
	 * pasted becomes `mysite.com` with the caret after it, rather than being rejected wholesale.
	 */
	function applyFilter(
		el: HTMLInputElement,
		filter: (raw: string, caret: number) => { value: string; caret: number }
	): string {
		const next = filter(el.value, el.selectionStart ?? el.value.length);
		if (el.value !== next.value) {
			el.value = next.value;
			el.setSelectionRange(next.caret, next.caret);
		}
		return next.value;
	}

	function filterDomainField(el: HTMLInputElement): void {
		subdomain = applyFilter(el, filterDomainInput);
	}

	function filterNameField(el: HTMLInputElement): void {
		name = applyFilter(el, filterNameInput);
	}

	/** True while an IME is mid-composition. Rewriting `.value` under a live composition
	 * fights the IME's own buffer, so those events are left alone and filtered once at
	 * `compositionend` instead (which every composed character then goes through). Defensive:
	 * this sandbox cannot drive a real IME. */
	function isComposing(e: Event): boolean {
		return 'isComposing' in e && e.isComposing === true;
	}

	async function browse(): Promise<void> {
		pickerError = null;
		try {
			const picked = await open({
				directory: true,
				multiple: false,
				title: 'Choose project folder'
			});
			if (typeof picked === 'string') docroot = picked;
		} catch (e) {
			pickerError = `Could not open the folder picker: ${String(e)}`;
		}
	}

	// Project-folder field can carry two independent, non-exclusive errors — the backend's
	// `fieldErrors.docroot` (from Save) and `pickerError` (from Browse) — so their `id`s are
	// combined, space-separated, for `aria-describedby`; either alone still works standalone.
	// Like the web-server group below and unlike the docroot field above, the hint is a
	// permanent description of the control, so this is never `undefined`. Error first: readers
	// announce the list in order and the error is the urgent half.
	const nameDescribedBy = $derived(
		[fieldErrors.name ? 'f-name-error' : null, 'f-name-hint']
			.filter((id): id is string => id !== null)
			.join(' ')
	);

	const rootDescribedBy = $derived(
		[
			fieldErrors.docroot ? 'f-root-error' : null,
			pickerError ? 'f-root-picker-error' : null,
			// Mirrors the create-folder preview's own render gate below EXACTLY
			// (`{#if site === null}` wrapping `{#if createFolder}`) — 'f-root-preview' must
			// join this list only in the one case that paragraph is actually on the page.
			// Last, per the same error-first ordering the other four fields use: it is a
			// live description of what Save will do, not an error.
			site === null && createFolder ? 'f-root-preview' : null
		]
			.filter((id): id is string => id !== null)
			.join(' ') || undefined
	);

	// Web-server group: the Apache-not-supported notice is a permanent description of the
	// control, so unlike the fields above this is never `undefined`. A backend error, when
	// there is one, is listed FIRST — it is the urgent half, and screen readers read
	// `aria-describedby` in the order given.
	const serverDescribedBy = $derived(
		[fieldErrors.web_server ? 'f-server-error' : null, 'f-server-hint']
			.filter((id): id is string => id !== null)
			.join(' ')
	);

	async function submit(): Promise<void> {
		if (submitting) return;
		submitting = true;
		try {
			const input: SiteInput = {
				name,
				domain: composeDomain(subdomain),
				docroot,
				webServer,
				phpVersion,
				enabled
			};
			// `createFolder` unconditionally: in edit mode the checkbox above never renders,
			// so this state can only ever be its `false` default there — no need to gate the
			// call on `site === null` a second time here.
			const ok = await onSave(site?.id ?? null, input, createFolder);
			if (ok) onClose();
		} finally {
			submitting = false;
		}
	}

	async function confirmDelete(): Promise<void> {
		if (site === null || deleting) return;
		const target = site;
		deleting = true;
		try {
			const ok = await onDelete(target.id);
			if (ok) onClose();
		} finally {
			deleting = false;
		}
	}
</script>

<!-- Window-scoped, not on the `<aside>` below, so Esc/Tab keep working even once focus has
     drifted outside the drawer — see the `onKeydown`/`onFocusIn` doc comments above. -->
<svelte:window onkeydown={onKeydown} onfocusin={onFocusIn} />
<div class="drawer-backdrop" aria-hidden="true" onclick={onClose}></div>
<!-- An explicit `role` always overrides an element's implicit ARIA semantics (the mechanism
     is designed for exactly this); `<aside>` — tangentially related content, arguably the
     more correct tag for a side drawer than a bare `<div>` — is also site-editor.html's own
     literal tag choice for this dialog. The W3C APG Dialog (Modal) pattern does not mandate
     any specific host element. -->
<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
<aside
	class="drawer"
	role="dialog"
	aria-modal="true"
	aria-labelledby="drawer-title"
	bind:this={drawerEl}
>
	<div class="drawer-head">
		<h2 id="drawer-title">{heading}</h2>
		<button type="button" class="btn btn-quiet btn-icon" aria-label="Close" onclick={onClose}>
			<svg
				width="14"
				height="14"
				viewBox="0 0 14 14"
				fill="none"
				stroke="currentColor"
				stroke-width="1.8"
				stroke-linecap="round"
				aria-hidden="true"><path d="M3 3l8 8M11 3l-8 8" /></svg
			>
		</button>
	</div>

	<div class="drawer-body">
		<div class="field">
			<label for="f-name">Name</label>
			<!-- `value=` + `oninput` rather than `bind:value`, for the caret reason documented on
			     `applyFilter`. `maxlength` is 63 because `SiteName::parse` bounds `s.len()` — BYTES,
			     not characters — and the filter above guarantees ASCII-only, so here the two
			     coincide exactly. -->
			<input
				class="input mono"
				id="f-name"
				value={name}
				maxlength="63"
				oninput={(e) => {
					if (!isComposing(e)) filterNameField(e.currentTarget);
				}}
				oncompositionend={(e) => filterNameField(e.currentTarget)}
				bind:this={nameInput}
				aria-invalid={fieldErrors.name ? 'true' : undefined}
				aria-describedby={nameDescribedBy}
			/>
			<!-- The rule has to be VISIBLE, not just enforced. Filtering means a Thai (or any
			     non-slug) keystroke produces nothing at all — no character, no error — which
			     without this line reads as a broken keyboard or a frozen app. The owner hit
			     exactly this field with Thai text. -->
			<p class="hint" id="f-name-hint">
				Lowercase letters, numbers and dashes only — it names this site's generated config files.
				Use Domain for the address you'll visit.
			</p>
			<!-- `fieldErrors` keys are the BACKEND's snake_case field names (commands.rs /
			     openvhost-core's `invalid("php_version", ...)` etc.), not the camelCase
			     `SiteInput` keys — `name`/`domain`/`docroot` happen to read the same either
			     way, but `php_version`/`web_server` below do NOT: a camelCase lookup there
			     would silently never match. -->
			{#if fieldErrors.name}
				<p class="field-error" id="f-name-error">{fieldErrors.name}</p>
			{/if}
		</div>

		<div class="field">
			<label for="f-domain">Domain</label>
			<div class="input-group">
				<!-- Holds the LABEL only — `.localhost` is the static suffix beside it, and
				     `composeDomain`/`splitDomain` do the joining. See `filterDomainField` above
				     for why this is `value=` + `oninput` instead of `bind:value`. -->
				<input
					class="input mono"
					id="f-domain"
					value={subdomain}
					oninput={(e) => {
						if (!isComposing(e)) filterDomainField(e.currentTarget);
					}}
					oncompositionend={(e) => filterDomainField(e.currentTarget)}
					aria-invalid={fieldErrors.domain ? 'true' : undefined}
					aria-describedby={fieldErrors.domain ? 'f-domain-error' : undefined}
				/>
				<span class="input-suffix">.localhost</span>
			</div>
			<p class="hint">Resolves in modern browsers without touching the hosts file.</p>
			{#if fieldErrors.domain}
				<p class="field-error" id="f-domain-error">{fieldErrors.domain}</p>
			{/if}
		</div>

		<div class="field">
			<label for="f-root">Project folder</label>
			<div class="input-group">
				<input
					class="input mono"
					id="f-root"
					bind:value={docroot}
					aria-invalid={fieldErrors.docroot || pickerError ? 'true' : undefined}
					aria-describedby={rootDescribedBy}
				/>
				<button type="button" class="input-suffix input-suffix--btn" onclick={() => void browse()}>
					Browse
				</button>
			</div>
			{#if site === null}
				<!-- Create mode only — an existing site's folder already exists (that is what
				     "edit" means), so there is nothing to scaffold and this control would be a
				     lie. Same wrapping-label convention as the Enabled checkbox below
				     (`.checkbox-field`), not a new pattern. -->
				<label class="checkbox-field">
					<input id="f-root-create" type="checkbox" bind:checked={createFolder} />
					Create a site folder inside this folder
				</label>
				{#if createFolder}
					<!-- Live preview of exactly what Save will do to the path above — kept in sync
					     with `rootDescribedBy` (this paragraph's id joins that list, last, only
					     while both this renders and the checkbox is on). The fallback copy covers
					     BOTH reasons `scaffoldPreview` can return null (blank parent or no name
					     yet), not just the name half its wording names. -->
					<p class="hint mono" id="f-root-preview">
						{scaffoldPreview(docroot, name) ?? 'Enter a name to see the final path'}
					</p>
				{/if}
			{/if}
			{#if fieldErrors.docroot}
				<p class="field-error" id="f-root-error">{fieldErrors.docroot}</p>
			{/if}
			{#if pickerError}
				<p class="field-error" id="f-root-picker-error">{pickerError}</p>
			{/if}
		</div>

		<div class="field">
			<!-- An external group label via `aria-labelledby` (not a `for`-associated single
			     control) is the standard WAI-ARIA pattern for naming a `role="group"` of toggle
			     buttons; site-editor.html uses this exact `<label id="…">` + `aria-labelledby`
			     pairing for the same segmented control. -->
			<!-- svelte-ignore a11y_label_has_associated_control -->
			<label id="f-server-label">Web server</label>
			<!-- F4 (review fix-wave) note: `aria-invalid` was requested here "for parity" with
			     the other four fields, but WAI-ARIA's Supported States and Properties table
			     does not list `aria-invalid` for `role="group"` — nor for the two toggle
			     `<button>`s' own implicit `button` role. Confirmed two ways, not assumed: (1)
			     `aria-query`'s role data (the exact table `svelte-check`'s
			     `a11y_role_supports_aria_props` rule consults) lists `aria-invalid` only for
			     value-holding roles (checkbox, combobox, gridcell, listbox, radiogroup, slider,
			     spinbutton, textbox, tree, application) — `group`/`button` are not among them,
			     even after conglomerating their superclass (`roletype`/`structure`/`section`
			     and `roletype`/`widget`/`command`) props; (2) empirically, adding it fired
			     `a11y_role_supports_aria_props` at both `svelte-check` and `vite build`. Per
			     spec, AT behaviour for an unsupported state/role pairing is undefined, and this
			     project's gate requires 0 warnings — so it stays out rather than being forced
			     through with a suppression comment. `aria-describedby` below already gives this
			     group the same accessible error-association the other four fields get from
			     their own `aria-describedby`; that IS the spec-compliant equivalent for a role
			     that doesn't support `aria-invalid`. -->
			<div
				class="seg"
				role="group"
				aria-labelledby="f-server-label"
				aria-describedby={serverDescribedBy}
			>
				{#each WEB_SERVERS as server (server)}
					<!-- Brand marks stay in their real colours in both states. Recolouring a
					     trademark to suit our accent fill is exactly what WebServerIcon.svelte's
					     header asks us not to do, and the mark is `aria-hidden` reinforcement
					     anyway — the visible label carries the meaning. -->
					<button
						type="button"
						aria-pressed={webServer === server}
						onclick={() => (webServer = server)}
					>
						<WebServerIcon {server} />
						{server}
					</button>
				{/each}
			</div>
			<!-- Apache stays selectable (owner's call) but OpenVHost genuinely cannot serve it:
			     there is an NginxAdapter and nginx templates, no Apache counterpart. Saying so
			     here — as a capability statement about this product, not a guess about the
			     user's machine — is the honest alternative to a control that quietly produces
			     an unservable site. Associated with the group via `aria-describedby` (below,
			     after any error) so it is announced when the group is reached, not just seen. -->
			<p class="hint" id="f-server-hint">
				OpenVHost cannot serve Apache sites yet — it only generates nginx config. An Apache site
				will save, but it won't be served.
			</p>
			<!-- Backend field name for the web server is `web_server` (snake_case) — see the
			     note above the Name field. -->
			{#if fieldErrors.web_server}
				<p class="field-error" id="f-server-error">{fieldErrors.web_server}</p>
			{/if}
		</div>

		<div class="field">
			{#if !phpUnavailable}
				<!-- `for` AND `id` on the same label: `for` names the `<button role="combobox">`
				     Select renders (a `<button>` is a labelable element, and `combobox` takes no
				     name from its content), while the `id` lets Select name its popup listbox with
				     the same words. -->
				<label for="f-php" id="f-php-label">PHP version</label>
				<Select
					id="f-php"
					labelId="f-php-label"
					options={phpOptions}
					bind:value={phpVersion}
					invalid={Boolean(fieldErrors.php_version)}
					describedBy={fieldErrors.php_version ? 'f-php-error' : undefined}
					mono
				/>
				<p class="hint">Applies to this site only. Other sites keep their own version.</p>
				<!-- Backend field name for the PHP version is `php_version` (snake_case) — see the
				     note above the Name field. -->
				{#if fieldErrors.php_version}
					<p class="field-error" id="f-php-error">{fieldErrors.php_version}</p>
				{/if}
			{:else}
				<!-- No installed PHP version, and none stored yet either (a brand-new site) — the
				     one case `phpVersionOptions` can actually return empty. An empty `<select>`
				     above an enabled Save button would let this site be born pointing at nothing,
				     exactly the trap this task closes: every version in a fixed dropdown that a
				     later Apply refused. Point at the one place that fixes it instead. -->
				<p class="hint" id="f-php-unavailable">
					No PHP version is installed yet — <a href={resolve('/languages')}
						>install one on the Languages page</a
					> before adding a site.
				</p>
			{/if}
		</div>

		<div class="field">
			<label class="checkbox-field">
				<input type="checkbox" bind:checked={enabled} />
				Enabled
			</label>
		</div>

		{#if site !== null}
			<div class="danger-zone">
				<h3>Delete site</h3>
				<p class="consequence">
					This removes the site from OpenVHost. Your project files in
					<span class="mono">{site.docroot}</span> are not touched.
				</p>
				{#if !confirmingDelete}
					<button type="button" class="btn btn-danger" onclick={() => (confirmingDelete = true)}>
						Delete site…
					</button>
				{:else}
					<p class="confirm-prompt">Really delete <strong>{site.name}</strong>?</p>
					<div class="confirm-actions">
						<Button variant="quiet" onclick={() => (confirmingDelete = false)}>Cancel</Button>
						<button
							type="button"
							class="btn btn-danger"
							disabled={deleting}
							onclick={() => void confirmDelete()}
						>
							Delete
						</button>
					</div>
				{/if}
			</div>
		{/if}
	</div>

	<div class="drawer-foot">
		<Button variant="quiet" onclick={onClose}>Cancel</Button>
		<Button
			variant="primary"
			testId="drawer-save"
			disabled={submitting || phpUnavailable}
			onclick={() => void submit()}
		>
			Save
		</Button>
	</div>
</aside>

<style>
	/* Ported from docs/design/site-editor.html (lines 87-142) and the matching docs/design/
	   mock.css rules (.drawer-backdrop, .drawer, .drawer-head/body/foot, .field, .input,
	   .input-group, .input-suffix, .seg, .danger-zone, .consequence), tokens only. Two
	   deliberate deviations from the literal mock CSS (both explained in the task report):

	   1. `.drawer`'s width is `min(420px, 100%)`, not a bare `420px` — the fixed mock value
	      would overflow/clip at this project's 380px-minimum-panel-width floor; clamping to
	      100% degrades gracefully in a narrow window without changing anything at the app's
	      normal (960px default, no minWidth floor stopping a resize) size.
	   2. A MINIMAL `.btn`/`.btn-quiet`/`.btn-danger`/`.btn:disabled`/`.btn-icon` subset
	      (mock.css:114-135) is carried locally, because two controls here — the icon-only
	      Close button and the two danger-zone buttons — fall outside `Button.svelte`'s
	      existing `variant`/`size` surface (icon-sized quiet, and a danger variant it does
	      not have). Save/Cancel still reuse the shared `Button` component, matching how
	      SitesPanel/SiteListRow already use it. `Button.svelte`'s variant/size surface itself
	      was not extended for these two controls. (Review fix-wave note: `Button.svelte` did
	      later gain one small, unrelated, opt-in `focusFallback` prop — an F1 fix, see this
	      file's onMount cleanup — which is a non-visual `data-*` hook, not a new variant.) */
	.drawer-backdrop {
		position: absolute;
		inset: 0;
		background: var(--vh-scrim);
		z-index: var(--vh-z-drawer-backdrop);
	}
	.drawer {
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: min(420px, 100%);
		background: var(--vh-surface);
		border-left: 1px solid var(--vh-border);
		z-index: var(--vh-z-drawer);
		display: flex;
		flex-direction: column;
		box-shadow: var(--vh-shadow-overlay);
		animation: vh-slide-in var(--vh-dur-normal) var(--vh-ease-out);
	}
	@keyframes vh-slide-in {
		from {
			transform: translateX(24px);
			opacity: 0;
		}
	}
	.drawer-head {
		display: flex;
		align-items: center;
		gap: var(--vh-space-3);
		padding: var(--vh-space-3) var(--vh-space-6);
		border-bottom: 1px solid var(--vh-border);
	}
	.drawer-head h2 {
		font-size: var(--vh-text-section);
		font-weight: 600;
		flex: 1;
	}
	.drawer-body {
		flex: 1;
		overflow: auto;
		padding: var(--vh-space-4) var(--vh-space-6);
	}
	.drawer-foot {
		display: flex;
		justify-content: flex-end;
		gap: var(--vh-space-2);
		padding: var(--vh-space-4) var(--vh-space-6);
		border-top: 1px solid var(--vh-border);
		background: var(--vh-surface-2);
	}
	.danger-zone {
		margin-top: var(--vh-space-4);
		padding-top: var(--vh-space-3);
		border-top: 1px solid var(--vh-border);
	}
	/* mock.css inlines these two rules as a `style=""` attribute on its `<h3>` — moved into a
	   real scoped rule here so nothing in this component relies on an inline style. */
	.danger-zone h3 {
		font-size: var(--vh-text-table);
		font-weight: 600;
		margin: 0;
	}
	.danger-zone .consequence {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		margin: 4px 0 8px;
	}
	.confirm-prompt {
		margin: 0 0 8px;
		font-size: var(--vh-text-table);
	}
	.confirm-actions {
		display: flex;
		gap: var(--vh-space-2);
	}

	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-bottom: 10px;
	}
	.field label {
		font-weight: 600;
		font-size: var(--vh-text-table);
	}
	.field .hint {
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
		margin: 0;
	}
	/* The Languages pointer inside `#f-php-unavailable`'s `.hint` — same link colour as
	   the Browse control's `.input-suffix--btn` above, since both are "go elsewhere to
	   fix this" affordances. */
	.field .hint a {
		color: var(--vh-link);
	}
	/* No mock.css precedent for an inline field error (the mockups never render one) — this
	   mirrors `.field .hint`'s size/spacing but uses the shared failure-semantic token. */
	.field-error {
		color: var(--vh-fail);
		font-size: var(--vh-text-caption);
		margin: 0;
	}
	.checkbox-field {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	/* No `select.input` companion rule any more: the PHP-version field is a `Select`
	   component (its own scoped styles reproduce this same recipe), and Svelte flags a
	   scoped selector that matches nothing in this file. */
	.input {
		font: inherit;
		color: var(--vh-text);
		background: var(--vh-surface);
		border: 1px solid var(--vh-border-strong);
		border-radius: var(--vh-radius-control);
		padding: 7px 10px;
		transition: border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.input:hover {
		border-color: color-mix(in oklab, var(--vh-text) 40%, transparent);
	}
	/* An invalid field is never marked by colour alone — the message below it
	   carries the same information (brand guidelines §4.2). Copied from
	   `WebServerSettingsForm.svelte`'s identical rule: this file sets
	   `aria-invalid` on its name/domain/docroot fields (above) but, until
	   now, had no CSS rule for it at all, so an invalid field here read as
	   grey — indistinguishable from a valid one except for the message
	   underneath. Both forms must mark errors the same way. */
	.input[aria-invalid='true'] {
		border-color: var(--vh-fail);
	}
	/* The global `:focus-visible` in tokens.css (`outline: 2px solid
	   var(--vh-focus-ring)` at a 2px offset) stays the single source for the
	   ring itself — its size and its ordinary colour, for every focusable
	   control in the app. `.input` has a visible 1px border of its own,
	   though, so that same offset stacks three concentric edges on it —
	   border, gap, ring — the doubled frame `.btn-quiet` in Button.svelte was
	   fixed for. Closing the gap and letting the border carry the ring's
	   colour merges them into one 3px band; this rule is a fact about THIS
	   control's own border, which the global rule — written once for every
	   focusable element — has no way to know. It does not move where the
	   ring is drawn or what it is coloured in the ordinary case.

	   `focus-ring.test.ts` asserts this rule (and the invalid-focus rule
	   below it) are IDENTICAL, byte for byte, to the matching rules in
	   `WebServerSettingsForm.svelte`'s `.input` and `Select.svelte`'s
	   `.trigger` — the three controls this same doubling affects. A future
	   edit to one that misses the others fails that test instead of
	   quietly drifting apart. */
	.input:focus-visible {
		border-color: var(--vh-focus-ring);
		outline-offset: 0;
	}
	/* Red wins: an invalid field stays the failure colour even while
	   focused, never the focus ring's colour. Both the border AND the
	   outline are set to `--vh-fail` so the whole band is red, not a red
	   border beside a green ring. Focus is still signalled — the band
	   GROWS from a 1px border to a 3px band — so this does not cost the
	   keyboard user anything, it just refuses to let the ring repaint an
	   error green. */
	.input[aria-invalid='true']:focus-visible {
		border-color: var(--vh-fail);
		outline-color: var(--vh-fail);
	}
	.input.mono {
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
	}
	.input-group {
		display: flex;
	}
	.input-group .input {
		flex: 1;
		border-radius: var(--vh-radius-control) 0 0 var(--vh-radius-control);
	}
	.input-suffix {
		display: inline-flex;
		align-items: center;
		padding: 0 10px;
		border: 1px solid var(--vh-border-strong);
		border-left: 0;
		border-radius: 0 var(--vh-radius-control) var(--vh-radius-control) 0;
		background: var(--vh-surface-2);
		color: var(--vh-text-2);
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
	}
	/* The mock uses a `<span role="button" tabindex="0">` for Browse; this component renders
	   a real `<button>` instead (same classes, so it looks identical) — free native keyboard
	   activation (Enter/Space) instead of hand-rolled keydown handling for a non-native
	   button, the same "prefer a real control" convention Rail.svelte's `.stop-all` already
	   follows. `appearance: none` plus the explicit border/background/padding above strip the
	   browser's default button chrome so it matches the mock's span-based look exactly. */
	.input-suffix--btn {
		appearance: none;
		font-family: var(--vh-font-ui);
		font-weight: 500;
		color: var(--vh-link);
		cursor: pointer;
	}
	.seg {
		display: inline-flex;
		align-self: flex-start;
		border: 1px solid var(--vh-border-strong);
		border-radius: var(--vh-radius-control);
		overflow: hidden;
	}
	/* inline-flex + gap so each brand mark sits on the text baseline block beside its
	   label; `justify-content: center` keeps the pair centred in the 88px cell rather than
	   sliding left as the mark widens the content. */
	.seg button {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--vh-space-2);
		min-width: 88px;
		font: inherit;
		font-weight: 500;
		padding: 6px 14px;
		background: var(--vh-surface);
		color: var(--vh-text-2);
		border: 0;
		cursor: pointer;
	}
	.seg button + button {
		border-left: 1px solid var(--vh-border);
	}
	.seg button[aria-pressed='true'] {
		background: var(--vh-accent);
		color: var(--vh-accent-contrast);
	}

	/* Minimal `.btn` subset — see the deviation note at the top of this block. */
	.btn {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		font: inherit;
		font-weight: 500;
		padding: 7px 14px;
		border-radius: var(--vh-radius-control);
		border: 1px solid transparent;
		cursor: pointer;
		transition:
			background var(--vh-dur-fast) var(--vh-ease-out),
			border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.btn-quiet {
		background: transparent;
		color: var(--vh-text);
		border-color: var(--vh-border-strong);
	}
	.btn-quiet:hover {
		background: color-mix(in oklab, var(--vh-text) 6%, transparent);
	}
	.btn-danger {
		background: transparent;
		color: var(--vh-fail);
		border-color: color-mix(in oklab, var(--vh-fail) 45%, transparent);
	}
	.btn-danger:hover {
		background: var(--vh-fail-tint);
	}
	.btn:disabled {
		color: var(--vh-text-disabled);
		border-color: var(--vh-border);
		background: transparent;
		cursor: not-allowed;
	}
	.btn-icon {
		padding: 7px;
	}
</style>
