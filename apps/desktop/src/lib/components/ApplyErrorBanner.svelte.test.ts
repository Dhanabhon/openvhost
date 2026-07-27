// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node` vitest project —
// same approach as SiteListRow.svelte.test.ts.
//
// WHAT THIS FILE CANNOT COVER: no DOM, so clicking "Edit hello" and confirming it calls
// `onEditSite` with the right site is a manual click-through item in the PR, same caveat as
// every other `svelte/server`-rendered component test in this codebase.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import ApplyErrorBanner from './ApplyErrorBanner.svelte';
import type { SiteDto } from '$lib/ipc';

const site = (overrides: Partial<SiteDto> = {}): SiteDto => ({
	id: 'a1',
	name: 'hello',
	domain: 'hello.localhost',
	docroot: '/srv/www/hello',
	webServer: 'nginx',
	phpVersion: '8.4',
	enabled: true,
	createdAt: 1,
	updatedAt: 1,
	...overrides
});

function renderBanner(props: { error: string; missing: SiteDto | null }): string {
	return render(ApplyErrorBanner, {
		props: {
			error: props.error,
			missing: props.missing,
			onEditSite: () => {}
		}
	}).body;
}

describe('ApplyErrorBanner', () => {
	// Stating the problem without an exit is what left the user stuck (spec).
	it('offers both ways out of a missing-runtime failure', () => {
		const body = renderBanner({
			error: 'site hello needs PHP 8.4, which is not installed (installed: 8.5)',
			missing: site({ name: 'hello', phpVersion: '8.4' })
		});
		expect(body).toContain('data-testid="go-install-8.4"');
		expect(body).toContain('data-testid="edit-site-hello"');
	});

	// A nginx -t syntax error has no "install this" remedy; offering one would be
	// worse than offering nothing.
	it('shows no actions for a failure that is not about a missing runtime', () => {
		const body = renderBanner({ error: 'nginx: [emerg] unknown directive', missing: null });
		expect(body).not.toMatch(/data-testid="go-install-/);
		expect(body).not.toMatch(/data-testid="edit-site-/);
	});

	it('always shows the error text, whether or not it has a remedy', () => {
		expect(renderBanner({ error: 'nginx: [emerg] unknown directive', missing: null })).toContain(
			'nginx: [emerg] unknown directive'
		);
	});

	// Multi-line errors (`ValidationFailed`'s nginx stderr) must stay pre-wrap —
	// the existing banner already does this; this is a regression guard for the
	// extraction into its own component.
	it('keeps a multi-line error pre-wrap', () => {
		const body = renderBanner({ error: 'line one\nline two', missing: null });
		expect(body).toMatch(/white-space:\s*pre-wrap/);
	});
});
