// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node` vitest project —
// same approach as SiteListRow.svelte.test.ts.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import PendingChangesBanner from './PendingChangesBanner.svelte';

function bannerHtml(count: number): string {
	return render(PendingChangesBanner, { props: { count, onReview: () => {} } }).body;
}

/** Visible text with tags stripped and Svelte's source indentation collapsed. */
function text(markup: string): string {
	return markup
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('PendingChangesBanner', () => {
	it('renders nothing at zero pending changes', () => {
		const body = bannerHtml(0);
		expect(body).not.toContain('data-testid="pending-changes"');
	});

	it('uses singular copy for exactly one change', () => {
		const t = text(bannerHtml(1));
		expect(t).toContain('1 change not applied yet');
		expect(t).not.toContain('1 changes');
	});

	it('uses plural copy for more than one change', () => {
		const t = text(bannerHtml(2));
		expect(t).toContain('2 changes not applied yet');
	});

	it('offers a review action once there is something pending', () => {
		const body = bannerHtml(3);
		expect(body).toContain('Review and apply');
	});
});
