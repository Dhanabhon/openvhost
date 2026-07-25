<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script module lang="ts">
	/** The only characters a `.localhost` subdomain may contain, matching `Domain::parse`
	 * in crates/openvhost-core/src/site/model.rs: dot-joined labels of `[a-z0-9-]`. */
	const HOSTNAME_CHAR = /^[a-z0-9.-]$/;

	/**
	 * Keep only hostname characters, lowercasing as we go.
	 *
	 * Per code point, and independent of position, which is what makes the caret
	 * arithmetic in `filterDomainInput` exact: filtering a prefix of `s` always yields a
	 * prefix of filtering `s`, so the number of surviving characters before the caret is
	 * simply the length of the filtered prefix. A regex `.replace()` over the whole string
	 * could not promise that once `toLowerCase()` is allowed to change a string's length
	 * (`İ` → `i` + combining dot).
	 *
	 * Downcasing rather than rejecting is deliberate: hostnames are case-insensitive and
	 * `Domain::parse` demands lowercase, so an uppercase keystroke has exactly one sane
	 * meaning. Dropping it instead would look like a broken keyboard.
	 */
	function filterHostname(s: string): string {
		let out = '';
		for (const ch of s) {
			const lower = ch.toLowerCase();
			if (HOSTNAME_CHAR.test(lower)) out += lower;
		}
		return out;
	}

	/**
	 * Filter one input event's raw value and say where the caret belongs afterwards.
	 *
	 * Exported for `SiteDrawer.svelte.test.ts`: the caret arithmetic is the part of this
	 * change that can actually be tested in the DOM-less `node` vitest project, and it is
	 * also the part most likely to be wrong. This is a TYPING AFFORDANCE ONLY —
	 * `Domain::parse` is still the authority, and nothing here decides whether a domain is
	 * valid.
	 */
	export function filterDomainInput(raw: string, caret: number): { value: string; caret: number } {
		return { value: filterHostname(raw), caret: filterHostname(raw.slice(0, caret)).length };
	}
</script>

<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { open } from '@tauri-apps/plugin-dialog';
	import type { SiteDto, SiteInput } from '$lib/ipc';
	import { composeDomain, splitDomain, phpVersionOptions, PHP_VERSIONS } from '$lib/sites.derive';
	import Button from './Button.svelte';

	let {
		site,
		fieldErrors,
		onSave,
		onDelete,
		onClose
	}: {
		site: SiteDto | null;
		fieldErrors: Record<string, string>;
		onSave: (id: string | null, input: SiteInput) => Promise<boolean>;
		onDelete: (id: string) => Promise<boolean>;
		onClose: () => void;
	} = $props();

	function initialWebServer(dto: SiteDto | null): 'nginx' | 'apache' {
		return dto?.webServer === 'apache' ? 'apache' : 'nginx';
	}

	// Form state is seeded ONCE from `site` (edit) or blank defaults (create), via `untrack`
	// (https://svelte.dev/e/state_referenced_locally) — an explicit "read this prop exactly
	// once, do not track it" signal, not a live mirror. That is safe because the drawer is
	// modal: the backdrop blocks every pointer path to another row's Edit button, so `site`
	// cannot change under an already-mounted instance.
	let name = $state(untrack(() => site?.name ?? ''));
	let subdomain = $state(untrack(() => (site ? splitDomain(site.domain) : '')));
	let docroot = $state(untrack(() => site?.docroot ?? ''));
	let webServer = $state<'nginx' | 'apache'>(untrack(() => initialWebServer(site)));
	let phpVersion = $state(untrack(() => site?.phpVersion ?? PHP_VERSIONS[0]));
	let enabled = $state(untrack(() => site?.enabled ?? true));

	// Built once, from the site's STORED version rather than from the live `phpVersion`
	// state above, for two reasons: the same read-once rationale as the fields (`site`
	// cannot change under a mounted drawer), and because deriving it from the live value
	// would delete an unlisted version's option the moment the user clicked away from it,
	// stranding them with no way back. See `phpVersionOptions` for why the option has to
	// exist at all.
	const phpOptions = phpVersionOptions(untrack(() => site?.phpVersion));

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
	function filterDomainField(el: HTMLInputElement): void {
		const next = filterDomainInput(el.value, el.selectionStart ?? el.value.length);
		if (el.value !== next.value) {
			el.value = next.value;
			el.setSelectionRange(next.caret, next.caret);
		}
		subdomain = next.value;
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
	const rootDescribedBy = $derived(
		[fieldErrors.docroot ? 'f-root-error' : null, pickerError ? 'f-root-picker-error' : null]
			.filter((id): id is string => id !== null)
			.join(' ') || undefined
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
			const ok = await onSave(site?.id ?? null, input);
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
			<input
				class="input"
				id="f-name"
				bind:value={name}
				bind:this={nameInput}
				aria-invalid={fieldErrors.name ? 'true' : undefined}
				aria-describedby={fieldErrors.name ? 'f-name-error' : undefined}
			/>
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
				aria-describedby={fieldErrors.web_server ? 'f-server-error' : undefined}
			>
				<button
					type="button"
					aria-pressed={webServer === 'nginx'}
					onclick={() => (webServer = 'nginx')}>nginx</button
				>
				<button
					type="button"
					aria-pressed={webServer === 'apache'}
					onclick={() => (webServer = 'apache')}>apache</button
				>
			</div>
			<!-- Backend field name for the web server is `web_server` (snake_case) — see the
			     note above the Name field. -->
			{#if fieldErrors.web_server}
				<p class="field-error" id="f-server-error">{fieldErrors.web_server}</p>
			{/if}
		</div>

		<div class="field">
			<label for="f-php">PHP version</label>
			<select
				class="input mono"
				id="f-php"
				bind:value={phpVersion}
				aria-invalid={fieldErrors.php_version ? 'true' : undefined}
				aria-describedby={fieldErrors.php_version ? 'f-php-error' : undefined}
			>
				{#each phpOptions as opt (opt.value)}
					<option value={opt.value}>{opt.label}</option>
				{/each}
			</select>
			<p class="hint">Applies to this site only. Other sites keep their own version.</p>
			<!-- Backend field name for the PHP version is `php_version` (snake_case) — see the
			     note above the Name field. -->
			{#if fieldErrors.php_version}
				<p class="field-error" id="f-php-error">{fieldErrors.php_version}</p>
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
		<Button variant="primary" disabled={submitting} onclick={() => void submit()}>Save</Button>
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
	.input,
	select.input {
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
	.seg button {
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
