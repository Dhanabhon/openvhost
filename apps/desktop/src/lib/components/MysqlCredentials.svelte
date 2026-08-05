<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { MysqlConnectionProofDto, MysqlResetOutcomeDto } from '$lib/ipc';
	import {
		MASKED_PASSWORD_PLACEHOLDER,
		engineDescriptor,
		type EngineKind
	} from '$lib/databases.derive';
	import { copyToClipboard } from '$lib/utils/clipboard';
	import Button from './Button.svelte';

	// PURELY PRESENTATIONAL, like `WebServerSettingsForm.svelte`: every value
	// arrives as a prop and every state change leaves as a callback —
	// including `confirmingReset`, which `SiteListRow.svelte`'s equivalent
	// keeps as internal local state for a reason that does not apply here
	// (see this file's own test header). That is what lets an SSR test drive
	// every rendered state directly, rather than leaving the confirm step's
	// consequence copy to the manual click-list.
	let {
		engine = 'mysql',
		major,
		host = '127.0.0.1',
		port,
		socketPath,
		user = 'root',
		password,
		revealed = false,
		revealing = false,
		passwordError = '',
		confirmingReset = false,
		resetting = false,
		resetOutcome,
		resetError = '',
		verifying = false,
		verifyResult,
		verifyError = '',
		onReveal,
		onHide,
		onCopyPassword,
		onRequestReset,
		onCancelReset,
		onConfirmReset,
		onVerify
	}: {
		/** Which engine this block belongs to (P1 MariaDB UI design D1) —
		 *  defaults to `'mysql'` so every existing caller/test is unaffected.
		 *  Resolved ONCE into {@link descriptor}; drives `resolvedPort`'s default
		 *  and the "MySQL"/"MariaDB" word in this block's own copy. */
		engine?: EngineKind;
		major: string;
		host?: string;
		/** The fixed port to show, or `undefined` to fall back to the engine's
		 *  own default (3306 for MySQL, 3307 for MariaDB) — see
		 *  {@link resolvedPort}. */
		port?: number;
		socketPath: string;
		user?: string;
		/** The cached value once fetched — `undefined` before any fetch. NOT
		 *  the display gate on its own (see `revealed`): `copyPassword()`
		 *  fetches into this SAME cache without ever revealing on screen, so a
		 *  defined `password` alone must never be read as "show it". The real
		 *  value only ever arrives here via `DatabasesStore.reveal()`/
		 *  `copyPassword()`, both explicit user actions — NEVER fetched by
		 *  this component itself. */
		password?: string;
		/** The DISPLAY gate — MANDATORY (spec D3/D6, review fix): whether the
		 *  field currently shows `password` in plaintext. Set ONLY by an
		 *  explicit Reveal action (`DatabasesStore.reveal`), cleared ONLY by
		 *  Hide (`forgetPassword`). Deliberately INDEPENDENT of whether
		 *  `password` is cached — a Copy click populates the cache (so the
		 *  clipboard write has a value) but must NEVER flip this, or Copy
		 *  would silently un-mask the field on screen (e.g. during a screen
		 *  share) even though the user asked only to copy it. */
		revealed?: boolean;
		revealing?: boolean;
		passwordError?: string;
		confirmingReset?: boolean;
		resetting?: boolean;
		resetOutcome?: MysqlResetOutcomeDto;
		resetError?: string;
		verifying?: boolean;
		verifyResult?: MysqlConnectionProofDto;
		verifyError?: string;
		/** Reveal/Hide toggle: fetch-if-needed then turn `revealed` on, or
		 *  forget the cached value and turn `revealed` off — wired to
		 *  `DatabasesStore.reveal`/`forgetPassword`. */
		onReveal: () => void;
		onHide: () => void;
		/** Fetch-if-needed then copy to the clipboard — does NOT itself flip
		 *  `revealed` (spec D6 MANDATORY: Reveal and Copy are separate
		 *  affordances, and Copy must never un-mask the on-screen field). */
		onCopyPassword: () => void;
		onRequestReset: () => void;
		onCancelReset: () => void;
		onConfirmReset: () => void;
		onVerify: () => void;
	} = $props();

	/** The static, per-engine facts (P1 MariaDB UI design D1) — resolved ONCE,
	 *  here, from the closed {@link EngineKind}. */
	const descriptor = $derived(engineDescriptor(engine));
	/** The port actually shown: the caller's explicit override, or the
	 *  engine's own default — never a bare literal `3306` (that was MySQL's
	 *  own hardcoded default before this block was shared). */
	const resolvedPort = $derived(port ?? descriptor.defaultPort);

	/** Both signals must agree: `revealed` is the user's ask, `password !==
	 *  undefined` is a defensive floor so a `revealed: true` with nothing yet
	 *  cached (a race, or a caller bug) can never try to render `undefined`
	 *  as plaintext. */
	const isRevealed = $derived(revealed && password !== undefined);
	const displayValue = $derived(isRevealed ? password : MASKED_PASSWORD_PLACEHOLDER);
</script>

{#snippet connField(label: string, value: string, testId: string)}
	<div class="conn-field">
		<span class="conn-label">{label}</span>
		<span class="conn-value mono" data-testid="conn-value-{testId}">{value}</span>
		<button
			type="button"
			class="copy-btn"
			data-testid="copy-{testId}"
			aria-label="Copy {label}"
			onclick={() => void copyToClipboard(value)}
		>
			Copy
		</button>
	</div>
{/snippet}

<div class="credentials" data-testid="{descriptor.idPrefix}-credentials-{major}">
	<div class="conn-block">
		<h3 class="block-title">Connection</h3>
		<div class="conn-grid">
			{@render connField('Host', host, `host-${major}`)}
			{@render connField('Port', String(resolvedPort), `port-${major}`)}
			{@render connField('Socket', socketPath, `socket-${major}`)}
			{@render connField('User', user, `user-${major}`)}
		</div>
	</div>

	<div class="password-block">
		<label for="{descriptor.idPrefix}-password-{major}">Root password</label>
		<div class="input-group">
			<input
				class="input mono"
				id="{descriptor.idPrefix}-password-{major}"
				type={isRevealed ? 'text' : 'password'}
				readonly
				value={displayValue}
				data-testid="password-field-{major}"
				aria-label="{descriptor.label} {major} root password"
			/>
			<button
				type="button"
				class="input-suffix input-suffix--btn"
				data-testid="reveal-toggle-{major}"
				disabled={revealing}
				onclick={() => (isRevealed ? onHide() : onReveal())}
			>
				{isRevealed ? 'Hide' : revealing ? 'Revealing…' : 'Reveal'}
			</button>
			<button
				type="button"
				class="input-suffix input-suffix--btn"
				data-testid="copy-password-{major}"
				disabled={revealing}
				onclick={onCopyPassword}
			>
				Copy
			</button>
		</div>
		{#if passwordError !== ''}
			<p class="error" role="alert" data-testid="password-error-{major}">{passwordError}</p>
		{/if}

		{#if confirmingReset}
			<div class="reset-confirm" data-testid="reset-confirm-{major}">
				<p>
					This regenerates {descriptor.label}
					{major}'s root password. The current password stops working immediately, and the new one
					is stored in OpenVHost's local database (state.db), not in Keychain.
				</p>
				<div class="reset-actions">
					<Button variant="quiet" size="sm" testId="cancel-reset-{major}" onclick={onCancelReset}>
						Cancel
					</Button>
					<button
						type="button"
						class="btn btn-danger btn-sm"
						data-testid="confirm-reset-{major}"
						disabled={resetting}
						onclick={onConfirmReset}
					>
						{resetting ? 'Resetting…' : 'Reset password'}
					</button>
				</div>
			</div>
		{:else}
			<Button variant="quiet" size="sm" testId="reset-password-{major}" onclick={onRequestReset}>
				Reset password
			</Button>
		{/if}

		{#if resetOutcome?.kind === 'reset'}
			<p class="ok" role="status" data-testid="reset-ok-{major}">Password regenerated.</p>
		{:else if resetOutcome?.kind === 'authFailed'}
			<p class="error" role="alert" data-testid="reset-auth-failed-{major}">
				Reset failed: {resetOutcome.detail}. {descriptor.staleCredentialRecovery}
			</p>
		{/if}
		{#if resetError !== ''}
			<p class="error" role="alert" data-testid="reset-error-{major}">{resetError}</p>
		{/if}
	</div>

	<div class="verify-block">
		<Button
			variant="quiet"
			size="sm"
			testId="verify-connection-{major}"
			disabled={verifying}
			onclick={onVerify}
		>
			{verifying ? 'Verifying…' : 'Verify connection'}
		</Button>
		{#if verifyResult?.kind === 'ok'}
			<p class="ok" role="status" data-testid="verify-ok-{major}">
				Connected — {descriptor.label}
				{verifyResult.version} on port {verifyResult.port}.
			</p>
		{:else if verifyResult?.kind === 'authFailed'}
			<p class="error" role="alert" data-testid="verify-auth-failed-{major}">
				{verifyResult.detail}. {descriptor.staleCredentialRecovery}
			</p>
		{:else if verifyResult?.kind === 'failed'}
			<p class="error" role="alert" data-testid="verify-failed-{major}">{verifyResult.detail}</p>
		{/if}
		{#if verifyError !== ''}
			<p class="error" role="alert" data-testid="verify-ipc-error-{major}">{verifyError}</p>
		{/if}
	</div>
</div>

<style>
	.credentials {
		display: flex;
		flex-direction: column;
		gap: var(--vh-space-4);
		padding: var(--vh-space-3) var(--vh-space-4) var(--vh-space-4);
	}
	.block-title {
		margin: 0 0 var(--vh-space-2);
		font-size: var(--vh-text-table);
		font-weight: 600;
		color: var(--vh-text-2);
	}
	.conn-grid {
		display: grid;
		/* `min(220px, 100%)`, not a bare 220px: at this project's 380px panel
		   floor a bare minmax would push the grid into horizontal scroll —
		   same technique `WebServerSettingsForm.svelte`'s `.grid` already uses. */
		grid-template-columns: repeat(auto-fit, minmax(min(220px, 100%), 1fr));
		gap: var(--vh-space-2) var(--vh-space-4);
	}
	.conn-field {
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
	}
	.conn-label {
		flex: none;
		width: 44px;
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
	.conn-value {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--vh-text-table);
	}
	.copy-btn {
		flex: none;
		font: inherit;
		font-size: var(--vh-text-caption);
		font-weight: 500;
		color: var(--vh-link);
		background: none;
		border: 0;
		padding: 0;
		cursor: pointer;
	}
	.copy-btn:hover {
		text-decoration: underline;
	}
	.password-block {
		display: flex;
		flex-direction: column;
		gap: 6px;
		max-width: 480px;
	}
	.password-block label {
		font-weight: 600;
		font-size: var(--vh-text-table);
	}
	/* `.input`/`.input-group`/`.input-suffix`/`.input-suffix--btn`: the exact
	   recipe `SiteDrawer.svelte`'s docroot field uses for its "Browse" button
	   (an input with a trailing real `<button>`, not a decorative suffix) —
	   including the focus-ring double-frame fix (`focus-ring.test.ts` tracks
	   this file as a fourth bordered control alongside `WebServerSettingsForm`/
	   `SiteDrawer`/`Select`, so a future edit that misses one fails a test
	   instead of quietly drifting apart). */
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
	.input:focus-visible {
		border-color: var(--vh-focus-ring);
		outline-offset: 0;
	}
	.input.mono {
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
	}
	.input-group {
		display: flex;
		min-width: 0;
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
		background: var(--vh-surface-2);
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
	}
	.input-suffix:last-child {
		border-radius: 0 var(--vh-radius-control) var(--vh-radius-control) 0;
	}
	.input-suffix--btn {
		appearance: none;
		font-family: var(--vh-font-ui);
		font-weight: 500;
		color: var(--vh-link);
		cursor: pointer;
	}
	.input-suffix--btn:disabled {
		color: var(--vh-text-disabled);
		cursor: not-allowed;
	}
	/* Success and failure use DIFFERENT background recipes on purpose — both
	   numbers checked against this theme's actual tokens (light-only today;
	   the reserved dark block inherits the same obligation once filled in,
	   same caveat `ScaffoldNoticeBanner.svelte` records for its own tones):

	   `--vh-fail` (#c13832) on `--vh-fail-tint` (9% mix into --vh-surface)
	   measures 4.89:1 — clears WCAG AA, and matches the exact recipe already
	   used unmodified by `LanguageRow.svelte`'s `.error` and
	   `ServiceRow.svelte`'s `.fail-detail`, so this reuses a verified pairing
	   rather than inventing a new one.

	   `--vh-run` (#2b8139) on `--vh-add-tint` (12% mix) measures only 4.41:1 —
	   the known AA failure `tokens.css`'s own `--vh-diff-add-text` comment
	   already documents for this exact tint (it exists BECAUSE `--vh-run`
	   fails on it). `LanguageRow.svelte`'s pre-existing `.ok` class uses that
	   failing pairing; it is not repeated here. `.ok` below instead uses the
	   pairing `ScaffoldNoticeBanner.svelte` vouches for: `--vh-run` directly on
	   plain `--vh-surface` (no tint), which measures 4.88:1. */
	.error {
		margin: 0;
		font-size: var(--vh-text-table);
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
	}
	.ok {
		margin: 0;
		font-size: var(--vh-text-table);
		color: var(--vh-run);
		background: var(--vh-surface);
		border: 1px solid color-mix(in oklab, var(--vh-run) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
	}
	.reset-confirm {
		display: flex;
		flex-direction: column;
		gap: var(--vh-space-2);
		padding: var(--vh-space-3);
		background: var(--vh-surface-2);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
	}
	.reset-confirm p {
		margin: 0;
		font-size: var(--vh-text-table);
		color: var(--vh-text-2);
	}
	.reset-actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--vh-space-2);
	}
	/* Minimal `.btn` subset for the danger confirm only — `Button.svelte` has
	   no danger variant, mirrored verbatim from `SiteListRow.svelte`'s
	   identical deviation note. */
	.btn {
		display: inline-flex;
		align-items: center;
		font: inherit;
		font-weight: 500;
		border-radius: var(--vh-radius-control);
		border: 1px solid transparent;
		cursor: pointer;
		transition:
			background var(--vh-dur-fast) var(--vh-ease-out),
			border-color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.btn-sm {
		padding: 4px 10px;
		font-size: var(--vh-text-table);
	}
	.btn-danger {
		background: transparent;
		color: var(--vh-fail);
		border-color: color-mix(in oklab, var(--vh-fail) 45%, transparent);
	}
	.btn-danger:hover:not(:disabled) {
		background: var(--vh-fail-tint);
	}
	.btn:disabled {
		opacity: 0.55;
		cursor: default;
	}
	.verify-block {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--vh-space-2);
	}
</style>
