// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node` vitest project —
// same approach as PendingChangesBanner.svelte.test.ts.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import ScaffoldNoticeBanner from './ScaffoldNoticeBanner.svelte';
import type { ScaffoldOutcomeDto } from '$lib/ipc';

function bannerHtml(outcome: ScaffoldOutcomeDto): string {
	return render(ScaffoldNoticeBanner, {
		props: { siteName: 'hello', docroot: '/srv/www/hello', outcome, onDismiss: () => {} }
	}).body;
}

/** Visible text with tags stripped and Svelte's source indentation collapsed. */
function text(markup: string): string {
	return markup
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('ScaffoldNoticeBanner', () => {
	it('created outcome renders role="status" with the starter-page path', () => {
		const body = bannerHtml({ kind: 'created' });
		expect(body).toContain('role="status"');
		expect(body).toContain('data-tone="ok"');
		expect(text(body)).toContain('/srv/www/hello/index.html');
	});

	it('keptExisting outcome names the file it kept', () => {
		const body = bannerHtml({ kind: 'keptExisting', existing: 'index.php' });
		expect(body).toContain('role="status"');
		expect(body).toContain('data-tone="ok"');
		expect(text(body)).toContain('index.php');
	});

	it('failed outcome renders role="alert" and the reason', () => {
		const body = bannerHtml({
			kind: 'failed',
			step: 'createDir',
			reason: 'Permission denied (os error 13)'
		});
		expect(body).toContain('role="alert"');
		expect(body).toContain('data-tone="warn"');
		expect(text(body)).toContain('Permission denied (os error 13)');
	});

	it('renders a dismiss button', () => {
		const body = bannerHtml({ kind: 'created' });
		expect(text(body)).toContain('Dismiss');
	});
});
