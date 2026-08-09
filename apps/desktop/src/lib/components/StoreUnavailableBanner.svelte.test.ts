// SPDX-License-Identifier: GPL-3.0-or-later
//
// The rendering half of the store-unavailable banner: that it appears exactly
// when there is a reason, that the reason reaches the screen verbatim, and that
// the copy says all three of what is wrong / what does not work / what still
// does.
//
// Rendered through `svelte/server`, which needs no DOM, so it runs in the
// existing `node` vitest project (the pattern `SiteReadinessBanner.svelte.
// test.ts` established). This component has no interaction to drive.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import StoreUnavailableBanner from './StoreUnavailableBanner.svelte';

const REASON = 'unable to open database file (os error 14)';

/** Everything the user can actually read, tags stripped and whitespace
 *  collapsed — so a copy assertion cannot pass on markup the reader never sees,
 *  and cannot fail on how Svelte happened to wrap a line. */
function visible(body: string): string {
	return body
		.replace(/<[^>]*>/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('when the store is down', () => {
	// Vacuity, measured: replacing the component's `{#if reason !== null}` with
	// `{#if true}` — the banner permanently on, crying wolf at every healthy
	// machine — reddened `renders nothing at all` below and left this group
	// green; a component that never rendered would do the reverse. Neither group
	// can stand alone, which is why both are here.

	it('leads with what is wrong, in the user’s terms and not Tauri’s', () => {
		const { body } = render(StoreUnavailableBanner, { props: { reason: REASON } });
		expect(visible(body)).toContain("OpenVHost can't open its data store");
		// The sentence this whole slice exists to delete.
		expect(body).not.toContain('.manage()');
	});

	it('carries the real reason verbatim, not a generic sentence', () => {
		const { body } = render(StoreUnavailableBanner, { props: { reason: REASON } });
		expect(visible(body)).toContain(REASON);
	});

	it('passes a different reason through unchanged, so nothing is hardcoded', () => {
		// The control for the assertion above: a banner that printed one canned
		// string would satisfy it just as well.
		const { body } = render(StoreUnavailableBanner, {
			props: { reason: 'Permission denied (os error 13)' }
		});
		expect(visible(body)).toContain('Permission denied (os error 13)');
		expect(visible(body)).not.toContain('os error 14');
	});

	it('says what no longer works, including the lists that go quiet', () => {
		const { body } = render(StoreUnavailableBanner, { props: { reason: REASON } });
		const text = visible(body);
		expect(text).toContain(
			'Your sites, web server settings and database passwords are kept in it, ' +
				'so anything that reads or changes them refuses until it opens'
		);
		// The half D5 exists for: a short list that does not announce itself is
		// indistinguishable from an empty one.
		expect(text).toContain(
			'lists that draw on it, such as per-site logs, are short without saying so'
		);
	});

	it('says what still works, and how to try again', () => {
		const { body } = render(StoreUnavailableBanner, { props: { reason: REASON } });
		expect(visible(body)).toContain(
			'Starting and stopping services, installing versions, and the nginx and PHP logs are ' +
				'unaffected. Reopening OpenVHost tries again.'
		);
	});

	it('is one banner, not a stack of them', () => {
		const { body } = render(StoreUnavailableBanner, { props: { reason: REASON } });
		expect(body.match(/data-testid="store-unavailable-banner"/g)).toHaveLength(1);
	});

	// role="alert", not SiteReadinessBanner's role="status": something HAS
	// failed, and it silently changes what every page can answer. A user about
	// to press Save deserves to hear it before, not after.
	it('is an assertive live region, because this is a failure', () => {
		const { body } = render(StoreUnavailableBanner, { props: { reason: REASON } });
		expect(body).toContain('role="alert"');
		expect(body).not.toContain('role="status"');
	});
});

describe('when the store is fine', () => {
	it('renders nothing at all', () => {
		const { body } = render(StoreUnavailableBanner, { props: { reason: null } });
		expect(body).not.toContain('store-unavailable-banner');
		expect(visible(body)).toBe('');
	});
});
