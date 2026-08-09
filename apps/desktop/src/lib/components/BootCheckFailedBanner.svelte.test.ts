// SPDX-License-Identifier: GPL-3.0-or-later
//
// The visible half of the one rule that must NOT be copied from the store
// slice: `boot_status` failing renders the app AND says so, where
// `state_store_status` failing renders silence.
//
// Rendered through `svelte/server`, which needs no DOM, so it runs in the
// existing `node` vitest project (the pattern `StoreUnavailableBanner.svelte.
// test.ts` established).

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import BootCheckFailedBanner from './BootCheckFailedBanner.svelte';
import type { IpcError } from '$lib/ipc';

const FAILURE: IpcError = { kind: 'core', message: 'transport died' };

/** Everything the user can actually read, tags stripped and whitespace
 *  collapsed. */
function visible(body: string): string {
	return body
		.replace(/<[^>]*>/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('when the boot check itself failed', () => {
	// Vacuity, measured: replacing the component's `{#if error !== null}` with
	// `{#if true}` — a banner permanently on, crying wolf at every healthy
	// launch — reddened `renders nothing at all` below and the six `is silent on
	// …` route cases, and left this group green; a component that never rendered
	// would do the reverse. Neither group can stand alone, which is why both are
	// here.

	it('leads with what could not be established, in the user’s terms', () => {
		const { body } = render(BootCheckFailedBanner, { props: { error: FAILURE } });
		expect(visible(body)).toContain('OpenVHost could not check how far this launch got');
		// The sentence this whole line of work exists to delete.
		expect(body).not.toContain('.manage()');
	});

	it('carries the transport’s own words, not a generic sentence', () => {
		const { body } = render(BootCheckFailedBanner, { props: { error: FAILURE } });
		expect(visible(body)).toContain('transport died');
	});

	it('passes a different failure through unchanged, so nothing is hardcoded', () => {
		// The control for the assertion above: a banner printing one canned string
		// would satisfy it just as well.
		const { body } = render(BootCheckFailedBanner, {
			props: { error: { kind: 'core', message: 'ipc channel closed' } satisfies IpcError }
		});
		expect(visible(body)).toContain('ipc channel closed');
		expect(visible(body)).not.toContain('transport died');
	});

	it('renders something readable even for a failure that carries no message', () => {
		// `IpcError`'s `simulated` variant has no `message` field at all, so the
		// banner goes through `errorMessage` rather than reaching for `.message` —
		// otherwise the one line meant to point at a cause renders "undefined".
		const { body } = render(BootCheckFailedBanner, {
			props: { error: { kind: 'simulated' } satisfies IpcError }
		});
		expect(visible(body)).toContain('The command failed.');
		expect(visible(body)).not.toContain('undefined');
	});

	it('says the app below is still showing, and why that is deliberate', () => {
		const { body } = render(BootCheckFailedBanner, { props: { error: FAILURE } });
		expect(visible(body)).toContain(
			'The rest of this window is showing as usual, because hiding a working app over one ' +
				'unanswered question would be the worse mistake.'
		);
	});

	it('says what a user should make of a page that then misbehaves', () => {
		const { body } = render(BootCheckFailedBanner, { props: { error: FAILURE } });
		expect(visible(body)).toContain(
			'If pages refuse to load or report errors that name things you have never heard of, this ' +
				'is why OpenVHost cannot say so plainly. Reopening it is the thing to try.'
		);
	});

	it('is one banner, not a stack of them', () => {
		const { body } = render(BootCheckFailedBanner, { props: { error: FAILURE } });
		expect(body.match(/data-testid="boot-check-failed-banner"/g)).toHaveLength(1);
	});

	it('is an assertive live region, because this is a failure', () => {
		// Matches `StoreUnavailableBanner`, not `SiteReadinessBanner`: what this
		// failed to establish is whether the rest of the window can be trusted.
		const { body } = render(BootCheckFailedBanner, { props: { error: FAILURE } });
		expect(body).toContain('role="alert"');
		expect(body).not.toContain('role="status"');
	});
});

describe('when the boot check answered', () => {
	it('renders nothing at all', () => {
		const { body } = render(BootCheckFailedBanner, { props: { error: null } });
		expect(body).not.toContain('boot-check-failed-banner');
		expect(visible(body)).toBe('');
	});
});
