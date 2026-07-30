// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), same approach as
// `LanguageRow.svelte.test.ts`. WHAT THIS FILE CANNOT COVER: no DOM, so click
// handlers are exercised only through the `onclick` prop wiring, not by
// simulating a real click.
//
// MANDATORY (spec D3/D6): the root password is never fetched eagerly and
// never appears in a test snapshot. Every fixture below uses an obviously
// fake, clearly-not-a-real-credential string — never anything shaped like the
// real 32-hex generated password — and the "masked by default" tests assert
// the ABSENCE of any password-like value when one was never revealed.
//
// `confirmingReset` is a PROP here, not local component state: unlike
// `SiteListRow.svelte`'s delete confirm (kept local because that row lives in
// a store-refetchable `#each` list, so lifting it risks a confirm landing on
// the wrong row after a refetch), `MysqlRow.svelte` owns exactly one
// `MysqlCredentials` instance per major with no equivalent refetch-reordering
// risk — so this lives as a prop instead, which is what lets THIS file assert
// the consequence copy directly rather than leaving it entirely to the manual
// click-list.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import MysqlCredentials from './MysqlCredentials.svelte';
import type { MysqlConnectionProofDto, MysqlResetOutcomeDto } from '$lib/ipc';

const FAKE_REVEALED = 'not-a-real-password';

function renderCredentials(
	props: Partial<{
		major: string;
		socketPath: string;
		password?: string;
		revealed: boolean;
		revealing: boolean;
		passwordError: string;
		confirmingReset: boolean;
		resetting: boolean;
		resetOutcome?: MysqlResetOutcomeDto;
		resetError: string;
		verifying: boolean;
		verifyResult?: MysqlConnectionProofDto;
		verifyError: string;
	}> = {}
): string {
	return render(MysqlCredentials, {
		props: {
			major: props.major ?? '8.4',
			socketPath: props.socketPath ?? '/Users/x/.openvhost/run/mysql-8.4.sock',
			password: props.password,
			revealed: props.revealed ?? false,
			revealing: props.revealing ?? false,
			passwordError: props.passwordError ?? '',
			confirmingReset: props.confirmingReset ?? false,
			resetting: props.resetting ?? false,
			resetOutcome: props.resetOutcome,
			resetError: props.resetError ?? '',
			verifying: props.verifying ?? false,
			verifyResult: props.verifyResult,
			verifyError: props.verifyError ?? '',
			onReveal: () => {},
			onHide: () => {},
			onCopyPassword: () => {},
			onRequestReset: () => {},
			onCancelReset: () => {},
			onConfirmReset: () => {},
			onVerify: () => {}
		}
	}).body;
}

describe('MysqlCredentials — connection block', () => {
	it('shows the fixed host, port and user, and the real socket path, all copyable', () => {
		const body = renderCredentials({ socketPath: '/Users/x/.openvhost/run/mysql-8.4.sock' });
		expect(body).toContain('127.0.0.1');
		expect(body).toContain('3306');
		expect(body).toContain('root');
		expect(body).toContain('/Users/x/.openvhost/run/mysql-8.4.sock');
		expect(body.match(/aria-label="Copy /g)?.length).toBeGreaterThanOrEqual(4);
	});
});

describe('MysqlCredentials — password field (masked by default)', () => {
	it('renders a fixed placeholder, never a real value, when nothing has been revealed', () => {
		const body = renderCredentials();
		expect(body).toContain('data-testid="password-field-8.4"');
		expect(body).toMatch(/value="[•]+"/);
		expect(body).toContain('type="password"');
	});

	it('never renders any password-shaped string when unrevealed, whatever a caller might pass', () => {
		// Defense in depth: the component's OWN masking must not depend on
		// trusting the caller. This is the literal SSR proof for spec D3/D6's
		// MANDATORY "assert masked rendering by default".
		const body = renderCredentials({ password: undefined });
		expect(body).not.toContain(FAKE_REVEALED);
	});

	it('switches to the real value and a text input once revealed=true AND a password is supplied', () => {
		const body = renderCredentials({ password: FAKE_REVEALED, revealed: true });
		expect(body).toContain(FAKE_REVEALED);
		expect(body).toContain('type="text"');
	});

	// Review fix (the actual regression this task shipped and a reviewer
	// caught): masking must be gated SOLELY by `revealed`, never by
	// `password !== undefined` alone. `DatabasesStore.copyPassword()` fetches
	// and caches the very same value `reveal()` does — so if the component
	// treated a defined `password` as "show it", a Copy click (which never
	// sets the display gate) would still silently un-mask the field the
	// instant its cache-fill resolved. Screen-share scenario: user clicks
	// Copy meaning clipboard-only, and the root password renders in
	// cleartext on the shared screen.
	it('stays masked when a password IS cached but revealed is false — the exact Copy-without-Reveal case', () => {
		const body = renderCredentials({ password: FAKE_REVEALED, revealed: false });
		expect(body).not.toContain(FAKE_REVEALED);
		expect(body).toMatch(/value="[•]+"/);
		expect(body).toContain('type="password"');
		expect(body).toMatch(/>\s*Reveal\s*</);
		expect(body).not.toMatch(/>\s*Hide\s*</);
	});

	it('offers Reveal when masked and Hide once revealed', () => {
		expect(renderCredentials({ password: undefined, revealed: false })).toMatch(/>\s*Reveal\s*</);
		expect(renderCredentials({ password: FAKE_REVEALED, revealed: true })).toMatch(/>\s*Hide\s*</);
	});

	it('never shows Hide while revealed is false, even with a password cached (Copy must not relabel the toggle)', () => {
		const body = renderCredentials({ password: FAKE_REVEALED, revealed: false });
		expect(body).not.toMatch(/>\s*Hide\s*</);
	});

	it('disables Reveal/Copy while a reveal is in flight', () => {
		const body = renderCredentials({ revealing: true });
		const reveal = body.match(/<button[^>]*data-testid="reveal-toggle-8\.4"[^>]*>/)?.[0] ?? '';
		const copy = body.match(/<button[^>]*data-testid="copy-password-8\.4"[^>]*>/)?.[0] ?? '';
		expect(reveal).toContain('disabled');
		expect(copy).toContain('disabled');
	});

	it('surfaces a reveal failure without ever showing a password', () => {
		const body = renderCredentials({ passwordError: 'no stored root password' });
		expect(body).toContain('no stored root password');
		expect(body).toMatch(/value="[•]+"/);
	});
});

describe('MysqlCredentials — reset (confirmed, states its consequence)', () => {
	it('does not reset without a confirm step', () => {
		const body = renderCredentials({ confirmingReset: false });
		expect(body).toContain('data-testid="reset-password-8.4"');
		expect(body).not.toContain('data-testid="reset-confirm-8.4"');
	});

	it('states plainly, once confirming, that this regenerates the password and where it lives', () => {
		const body = renderCredentials({ confirmingReset: true });
		expect(body).toContain('data-testid="reset-confirm-8.4"');
		expect(body).toMatch(/regenerat/i);
		expect(body).toMatch(/stops working|no longer work/i);
		expect(body).toMatch(/OpenVHost's local database|state\.db/i);
	});

	it('offers Cancel and a distinctly danger-styled confirm action, not a bare click-to-reset', () => {
		const body = renderCredentials({ confirmingReset: true });
		expect(body).toContain('data-testid="cancel-reset-8.4"');
		expect(body).toContain('data-testid="confirm-reset-8.4"');
		expect(body).toMatch(/btn-danger/);
	});

	it('disables the confirm action while resetting is in flight', () => {
		const body = renderCredentials({ confirmingReset: true, resetting: true });
		const btn = body.match(/<button[^>]*data-testid="confirm-reset-8\.4"[^>]*>/)?.[0] ?? '';
		expect(btn).toContain('disabled');
	});

	it('reports a successful reset', () => {
		const body = renderCredentials({ resetOutcome: { kind: 'reset' } });
		expect(body).toContain('data-testid="reset-ok-8.4"');
	});

	it('renders a stale-credential auth failure with manual-recovery copy, not a generic error', () => {
		const body = renderCredentials({
			resetOutcome: { kind: 'authFailed', detail: 'Access denied for user root' }
		});
		expect(body).toContain('data-testid="reset-auth-failed-8.4"');
		expect(body).toContain('Access denied for user root');
		expect(body).toMatch(/restored from a backup|changed outside OpenVHost/i);
	});

	it('surfaces a genuine spawn/IPC failure distinctly from an auth failure', () => {
		const body = renderCredentials({ resetError: 'could not write the ephemeral credential file' });
		expect(body).toContain('could not write the ephemeral credential file');
	});
});

describe('MysqlCredentials — verify connection', () => {
	it('offers a Verify connection control', () => {
		expect(renderCredentials()).toContain('data-testid="verify-connection-8.4"');
	});

	it('reports the server version and port on success', () => {
		const body = renderCredentials({
			verifyResult: { kind: 'ok', version: '8.4.11', port: 3306 }
		});
		expect(body).toContain('8.4.11');
		expect(body).toContain('3306');
	});

	it('renders an auth failure with manual-recovery copy, not a generic error', () => {
		const body = renderCredentials({
			verifyResult: { kind: 'authFailed', detail: 'Access denied for user root' }
		});
		expect(body).toContain('Access denied for user root');
		expect(body).toMatch(/restored from a backup|changed outside OpenVHost/i);
	});

	it('renders a plain connection failure verbatim', () => {
		const body = renderCredentials({
			verifyResult: { kind: 'failed', detail: "Can't connect to local MySQL server" }
		});
		expect(body).toContain("Can't connect to local MySQL server");
	});

	it('disables the button while verifying', () => {
		const body = renderCredentials({ verifying: true });
		const btn = body.match(/<button[^>]*data-testid="verify-connection-8\.4"[^>]*>/)?.[0] ?? '';
		expect(btn).toContain('disabled');
	});
});
