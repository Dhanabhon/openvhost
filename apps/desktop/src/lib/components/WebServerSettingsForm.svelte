<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { WebServerSettingsDto } from '$lib/ipc';
	import {
		errorKey,
		CONNECTION_NUMBERS,
		GZIP_LEVEL,
		PHASE3_FIELDS,
		PHASE3_REASON,
		TIMEOUT_NUMBERS,
		type BoolKey,
		type NumberFieldSpec,
		type NumberKey,
		type TextKey
	} from '$lib/websettings.derive';
	import Button from './Button.svelte';

	// PURELY PRESENTATIONAL, like `WebServerPanel.svelte` above it: every value
	// arrives as a prop and every change leaves as a callback. That is what lets
	// the whole surface be asserted by an SSR test in this project's DOM-less
	// vitest project, and it keeps `WebSettingsStore` the only thing that decides
	// what a value becomes (see `onNumber` — the string→number conversion is the
	// store's, not this file's).
	let {
		values,
		fieldErrors,
		error = '',
		saving = false,
		dirty = false,
		canSave = true,
		onNumber,
		onBool,
		onText,
		onSave
	}: {
		/** `null` until the first read settles, and while it has failed — the form
		 * must never render the built-in defaults as if they were the stored row. */
		values: WebServerSettingsDto | null;
		/** Keyed by the BACKEND's snake_case field names (`fastcgi_read_timeout`),
		 * exactly like `SiteDrawer`'s. `errorKey` below is the only place that
		 * conversion happens; a camelCase lookup would mark nothing at all, and a
		 * rejected save would read as a save that silently did nothing. */
		fieldErrors: Record<string, string>;
		/** Page-level failure, or the caveat left by a save whose follow-up read
		 * failed — hence rendered ABOVE the fields, not instead of them. */
		error?: string;
		saving?: boolean;
		dirty?: boolean;
		/** False while a number box holds something that is not a number. */
		canSave?: boolean;
		/** Raw `input.value` — a string even on `type="number"`. The store
		 * converts it; see `WebSettingsStore.setNumber`. */
		onNumber: (key: NumberKey, raw: string) => void;
		onBool: (key: BoolKey, value: boolean) => void;
		onText: (key: TextKey, value: string) => void;
		onSave: () => void;
	} = $props();

	/** The one place the Phase 3 reason is named, so every inert control can point
	 * at it with `aria-describedby` — a disabled control is still reachable in a
	 * screen reader's browse mode, and "disabled" with no reason is the oversight
	 * this group exists to avoid. */
	const PHASE3_REASON_ID = 'ws-phase3-reason';

	const controlId = (name: string): string => `ws-${name}`;
	const errorId = (name: string): string => `ws-${name}-error`;
	const hintId = (name: string): string => `ws-${name}-hint`;

	/** Error first, then the hint: screen readers announce `aria-describedby` in
	 * the order given and the error is the urgent half (the same ordering
	 * `SiteDrawer.svelte` settled on). */
	function describedBy(name: string, hasHint: boolean): string | undefined {
		const ids = [fieldErrors[name] ? errorId(name) : null, hasHint ? hintId(name) : null];
		return ids.filter((id): id is string => id !== null).join(' ') || undefined;
	}
</script>

{#snippet fieldError(name: string)}
	{#if fieldErrors[name]}
		<p class="field-error" id={errorId(name)} data-testid="error-{name}">{fieldErrors[name]}</p>
	{/if}
{/snippet}

{#snippet numberField(spec: NumberFieldSpec, value: number)}
	{@const name = errorKey(spec.key)}
	<div class="field">
		<label for={controlId(name)}>{spec.label}</label>
		<div class="input-group">
			<input
				class="input num"
				type="number"
				inputmode="numeric"
				step="1"
				min={spec.min}
				max={spec.max}
				id={controlId(name)}
				data-testid="field-{name}"
				{value}
				aria-invalid={fieldErrors[name] ? 'true' : undefined}
				aria-describedby={describedBy(name, true)}
				oninput={(e) => onNumber(spec.key, e.currentTarget.value)}
			/>
			{#if spec.unit}<span class="input-suffix">{spec.unit}</span>{/if}
		</div>
		<p class="hint" id={hintId(name)}>{spec.hint}</p>
		{@render fieldError(name)}
	</div>
{/snippet}

<!-- A real `<button role="switch">` rather than a styled checkbox: it takes
     Enter and Space natively, announces its on/off state through `aria-checked`,
     and needs no hidden input to stay in step with.
     `toggle` is `null` for the Phase 3 switches, which are inert — the handler
     is passed in rather than looked up from `name`, so a renamed field is a type
     error here instead of a switch that quietly stops working. -->
{#snippet switchField(
	name: string,
	label: string,
	on: boolean,
	hint: string,
	toggle: (() => void) | null
)}
	<div class="field">
		<span class="label" id="{controlId(name)}-label">{label}</span>
		<button
			type="button"
			class="switch"
			role="switch"
			aria-checked={on}
			aria-labelledby="{controlId(name)}-label"
			aria-describedby={toggle === null ? PHASE3_REASON_ID : describedBy(name, hint !== '')}
			data-testid="field-{name}"
			disabled={toggle === null}
			onclick={() => toggle?.()}
		>
			<span class="track"><span class="thumb"></span></span>
			<span class="state">{on ? 'On' : 'Off'}</span>
		</button>
		{#if hint !== ''}<p class="hint" id={hintId(name)}>{hint}</p>{/if}
		{@render fieldError(name)}
	</div>
{/snippet}

<section
	class="panel settings"
	aria-labelledby="ws-settings-title"
	data-testid="web-server-settings"
>
	<header class="settings-head">
		<h3 id="ws-settings-title">Settings</h3>
		<p class="sub">
			How nginx behaves for every site. Saving stores a value; the live config only changes when you
			apply.
		</p>
	</header>

	{#if error !== ''}
		<!-- Above the fields rather than instead of them: after a successful save
		     whose follow-up read failed, this is a caveat about values that are
		     still perfectly renderable. `pre-wrap` inline as well as in the scoped
		     rule, because this project's SSR test harness never sees scoped CSS
		     (see ApplyDialog.svelte's note on the same duplication). -->
		<p class="form-error" role="alert" data-testid="settings-error" style="white-space: pre-wrap">
			{error}
		</p>
	{/if}

	{#if values === null}
		<!-- Deliberately not "no settings": until the read settles this page has
		     been told nothing, and the defaults are the BACKEND's answer, never a
		     guess made here. -->
		<p class="empty" data-testid="settings-unloaded">
			{error === '' ? 'Reading the stored settings…' : 'The stored settings could not be read.'}
		</p>
	{:else}
		<!-- Bound once here so every field below reads a NON-NULL snapshot: without
		     it each `values.x` needs a non-null assertion, and an event handler
		     closing over the nullable prop would need one too. -->
		{@const v = values}
		<div class="groups">
			<fieldset class="group">
				<legend>Connections</legend>
				<div class="grid">
					{#each CONNECTION_NUMBERS as spec (spec.key)}
						{@render numberField(spec, v[spec.key])}
					{/each}

					<div class="field">
						<label for={controlId('client_max_body_size')}>Max upload size</label>
						<input
							class="input mono"
							type="text"
							id={controlId('client_max_body_size')}
							data-testid="field-client_max_body_size"
							value={v.clientMaxBodySize}
							aria-invalid={fieldErrors.client_max_body_size ? 'true' : undefined}
							aria-describedby={describedBy('client_max_body_size', true)}
							oninput={(e) => onText('clientMaxBodySize', e.currentTarget.value)}
						/>
						<p class="hint" id={hintId('client_max_body_size')}>
							The largest request body nginx accepts — a database import or a media upload dies here
							first. A number, optionally followed by k, m or g. Careful with 0: nginx reads it as
							no limit at all, not as “reject every upload”.
						</p>
						{@render fieldError('client_max_body_size')}
					</div>

					{@render switchField(
						'tcp_nodelay',
						'TCP no-delay',
						v.tcpNodelay,
						'Sends small responses immediately instead of waiting to fill a packet.',
						() => onBool('tcpNodelay', !v.tcpNodelay)
					)}
				</div>
			</fieldset>

			<fieldset class="group">
				<legend>Timeouts</legend>
				<div class="grid">
					{#each TIMEOUT_NUMBERS as spec (spec.key)}
						{@render numberField(spec, v[spec.key])}
					{/each}
				</div>
			</fieldset>

			<fieldset class="group">
				<legend>Compression</legend>
				<div class="grid">
					{@render switchField(
						'gzip',
						'gzip',
						v.gzip,
						'Off by default for local work — compressed responses are harder to read in a network inspector.',
						() => onBool('gzip', !v.gzip)
					)}
					{@render numberField(GZIP_LEVEL, v.gzipCompLevel)}

					<div class="field field--wide">
						<label for={controlId('gzip_types')}>Compressed types</label>
						<textarea
							class="input mono types"
							rows="3"
							spellcheck="false"
							id={controlId('gzip_types')}
							data-testid="field-gzip_types"
							value={v.gzipTypes}
							aria-invalid={fieldErrors.gzip_types ? 'true' : undefined}
							aria-describedby={describedBy('gzip_types', true)}
							oninput={(e) => onText('gzipTypes', e.currentTarget.value)}></textarea>
						<p class="hint" id={hintId('gzip_types')}>
							MIME types to compress, separated by spaces. text/html is always compressed by nginx
							itself. One malformed type rejects the whole field and names the type — nothing is
							dropped quietly.
						</p>
						{@render fieldError('gzip_types')}
					</div>
				</div>
			</fieldset>

			<!-- Rendered, disabled, with one shared reason (design §3). Last, not
			     first as ServBay has it: a form that opens with six dead controls
			     reads as a broken page, and everything above is genuinely editable. -->
			<fieldset class="group group--disabled">
				<legend>Ports and HTTPS</legend>
				<p class="group-note" id={PHASE3_REASON_ID} data-testid="phase3-reason">{PHASE3_REASON}</p>
				<div class="grid">
					{#each PHASE3_FIELDS as spec (spec.id)}
						{#if spec.kind === 'switch'}
							{@render switchField(spec.id, spec.label, false, '', null)}
						{:else}
							<div class="field">
								<label for={controlId(spec.id)}>{spec.label}</label>
								<!-- No `value`: a port number here would be a claim about how the
								     server is listening today, and this page has not been told
								     that. The placeholder shows what the field WILL take. -->
								<input
									class="input {spec.kind === 'text' ? 'mono' : 'num'}"
									type={spec.kind === 'number' ? 'number' : 'text'}
									id={controlId(spec.id)}
									data-testid="field-{spec.id}"
									placeholder={spec.placeholder}
									aria-describedby={PHASE3_REASON_ID}
									disabled
								/>
							</div>
						{/if}
					{/each}
				</div>
			</fieldset>
		</div>

		<footer class="settings-foot">
			<p class="foot-copy">
				Save stores these values and then shows you the diff to review. nginx only sees them once
				you apply — if it rejects the result, OpenVHost rolls back and shows you what it said.
			</p>
			<div class="foot-actions">
				{#if dirty}
					<span class="dirty" data-testid="settings-dirty">Unsaved changes</span>
				{/if}
				<Button
					variant="primary"
					testId="settings-save"
					disabled={saving || !canSave}
					onclick={onSave}
				>
					{saving ? 'Saving…' : 'Save'}
				</Button>
			</div>
		</footer>
	{/if}
</section>

<style>
	/* Same panel vocabulary as `WebServerPanel.svelte` directly above it on the
	   page (mock.css's `.panel`), and the same field/input/hint recipe as
	   `SiteDrawer.svelte` — this is the product's existing form idiom applied to
	   a page rather than a drawer, not a new one. Tokens only. */
	.panel {
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		margin: 0 var(--vh-space-6) var(--vh-space-6);
		overflow: hidden;
	}
	.settings-head {
		padding: var(--vh-space-4) var(--vh-space-6) var(--vh-space-3);
		border-bottom: 1px solid var(--vh-border);
	}
	.settings-head h3 {
		font-size: var(--vh-text-section);
		font-weight: 600;
	}
	.settings-head .sub {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		margin: 2px 0 0;
	}
	.empty {
		padding: var(--vh-space-8) var(--vh-space-6);
		text-align: center;
		color: var(--vh-text-2);
	}
	.form-error {
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border-bottom: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		padding: var(--vh-space-3) var(--vh-space-6);
		margin: 0;
		font-size: var(--vh-text-table);
	}

	.groups {
		display: flex;
		flex-direction: column;
	}
	/* `<fieldset>` for the real grouping semantics, with the browser's own frame
	   stripped — the visual separation is a rule between groups, which keeps the
	   rhythm of the row-list panels elsewhere in the app. */
	.group {
		border: 0;
		border-top: 1px solid var(--vh-border);
		margin: 0;
		padding: var(--vh-space-4) var(--vh-space-6) var(--vh-space-6);
	}
	.group:first-child {
		border-top: 0;
	}
	.group legend {
		/* The same uppercase strip heading the shell uses for its regions
		   (`.section-label` in tokens.css), inlined here because a `<legend>`
		   cannot take that global class without also taking its page padding. */
		font-size: var(--vh-text-caption);
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--vh-text-2);
		padding: 0;
		margin-bottom: var(--vh-space-3);
	}
	.group--disabled {
		background: var(--vh-surface-2);
	}
	.group-note {
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
		max-width: 68ch;
		margin: 0 0 var(--vh-space-3);
	}

	/* Two columns where there is room, one at this project's 380px panel floor.
	   `auto-fit` rather than a media query so it also degrades inside a narrow
	   window without a second breakpoint to keep in step. */
	.grid {
		display: grid;
		/* `min(240px, 100%)`, not a bare 240px: at this project's 380px panel
		   floor the content column is narrower than the track's minimum, and a
		   bare minmax would push the whole form into a horizontal scroll. */
		grid-template-columns: repeat(auto-fit, minmax(min(240px, 100%), 1fr));
		gap: var(--vh-space-4) var(--vh-space-6);
		align-items: start;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}
	.field--wide {
		grid-column: 1 / -1;
	}
	.field label,
	.field .label {
		font-weight: 600;
		font-size: var(--vh-text-table);
	}
	.field .hint {
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
		margin: 0;
		max-width: 52ch;
	}
	.field-error {
		color: var(--vh-fail);
		font-size: var(--vh-text-caption);
		margin: 0;
	}

	.input {
		font: inherit;
		color: var(--vh-text);
		background: var(--vh-surface);
		border: 1px solid var(--vh-border-strong);
		border-radius: var(--vh-radius-control);
		padding: 7px 10px;
		min-width: 0;
		transition: border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.input:hover:not(:disabled) {
		border-color: color-mix(in oklab, var(--vh-text) 40%, transparent);
	}
	.input:disabled {
		background: var(--vh-surface-2);
		color: var(--vh-text-disabled);
		border-color: var(--vh-border);
		cursor: not-allowed;
	}
	/* An invalid field is never marked by colour alone — the message below it
	   carries the same information (brand guidelines §4.2). */
	.input[aria-invalid='true'] {
		border-color: var(--vh-fail);
	}
	.input.mono,
	.input.num {
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
	}
	.input.num {
		font-variant-numeric: tabular-nums;
	}
	.types {
		resize: vertical;
		line-height: 1.5;
	}
	/* The unit sits against the number, the way SiteDrawer's `.localhost` sits
	   against the domain. */
	.input-group {
		display: flex;
		min-width: 0;
	}
	.input-group .input {
		flex: 1;
		border-radius: var(--vh-radius-control) 0 0 var(--vh-radius-control);
	}
	.input-group .input:only-child {
		border-radius: var(--vh-radius-control);
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

	.switch {
		display: inline-flex;
		align-items: center;
		gap: var(--vh-space-2);
		align-self: flex-start;
		font: inherit;
		font-size: var(--vh-text-table);
		color: var(--vh-text-2);
		background: transparent;
		border: 0;
		padding: 2px 0;
		cursor: pointer;
	}
	.switch .track {
		position: relative;
		width: 38px;
		height: 22px;
		border-radius: var(--vh-radius-pill);
		background: color-mix(in oklab, var(--vh-ink) 18%, transparent);
		border: 1px solid var(--vh-border-strong);
		transition: background var(--vh-dur-fast) var(--vh-ease-out);
	}
	.switch .thumb {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		background: var(--vh-surface);
		box-shadow: 0 1px 2px rgb(23 26 33 / 0.25);
		/* Transform, not `left`: the compositor can carry it, and the thumb is the
		   one thing on this page that moves. */
		transition: transform var(--vh-dur-fast) var(--vh-ease-out);
	}
	.switch[aria-checked='true'] .track {
		background: var(--vh-accent);
		border-color: var(--vh-accent);
	}
	.switch[aria-checked='true'] .thumb {
		transform: translateX(16px);
	}
	.switch:hover:not(:disabled) .track {
		border-color: color-mix(in oklab, var(--vh-text) 45%, transparent);
	}
	/* A press the finger can feel, on a compositor-friendly property only — the
	   checked variant repeats the travel because `transform` does not compose. */
	.switch:active:not(:disabled) .thumb {
		transform: scale(0.88);
	}
	.switch[aria-checked='true']:active:not(:disabled) .thumb {
		transform: translateX(16px) scale(0.88);
	}
	.switch .state {
		font-variant-numeric: tabular-nums;
	}
	.switch:disabled {
		color: var(--vh-text-disabled);
		cursor: not-allowed;
	}
	.switch:disabled .track {
		background: color-mix(in oklab, var(--vh-ink) 8%, transparent);
		border-color: var(--vh-border);
	}
	/* No `:focus-visible` rule anywhere in this file: tokens.css sets one ring
	   for the whole app (`outline: 2px solid var(--vh-focus-ring)` at a 2px
	   offset), and a second one here is how the two drift apart — this project
	   has already fixed a focus ring once. */

	.settings-foot {
		display: flex;
		align-items: center;
		gap: var(--vh-space-4);
		flex-wrap: wrap;
		padding: var(--vh-space-4) var(--vh-space-6);
		border-top: 1px solid var(--vh-border);
		background: var(--vh-surface-2);
	}
	.foot-copy {
		/* Basis, not `min-width`: the copy wraps under the button in a narrow
		   panel instead of forcing the footer wider than the panel. */
		flex: 1 1 240px;
		min-width: 0;
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
		max-width: 68ch;
		margin: 0;
	}
	.foot-actions {
		display: flex;
		align-items: center;
		gap: var(--vh-space-3);
		margin-left: auto;
	}
	.dirty {
		font-size: var(--vh-text-caption);
		font-weight: 600;
		color: var(--vh-start);
		background: color-mix(in oklab, var(--vh-state-starting) 14%, var(--vh-surface));
		border: 1px solid color-mix(in oklab, var(--vh-state-starting) 40%, transparent);
		border-radius: var(--vh-radius-pill);
		padding: 2px 10px;
	}
</style>
