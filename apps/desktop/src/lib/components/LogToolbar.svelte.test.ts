// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: genuine RED-first — LogToolbar.svelte does not exist yet.
//
// WHAT THIS FILE CANNOT COVER: the filter input debounces (`FILTER_DEBOUNCE_MS`,
// mirroring `Select.svelte`'s own untested typeahead-reset timer — see that
// component's file header for the identical, already-accepted carve-out) and
// every switch/select/button needs a real click or keypress to fire its
// callback — none of that is reachable under `svelte/server`. Only the
// STRUCTURAL contract (initial values reflected, controls present, bound
// input) is asserted here; the rest is the manual click-list.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LogToolbar from './LogToolbar.svelte';
import type { LogSourceDto } from '$lib/ipc';

/** The opening tag of the first element carrying `data-testid={id}`, so an
 *  attribute-order assumption in a test can never be the reason it fails —
 *  matches the `checkAgainHeaderButtonTag` pattern already established in
 *  `languages-page.test.ts`. */
function tagFor(body: string, testId: string): string {
	const match = body.match(new RegExp(`<[a-z]+[^>]*data-testid="${testId}"[^>]*>`));
	if (!match) throw new Error(`expected an element with data-testid="${testId}"`);
	return match[0];
}

function renderToolbar(props: {
	needle?: string;
	caseSensitive?: boolean;
	minLevel?: 'warn' | 'error' | null;
	follow?: boolean;
	newRowsWhilePaused?: boolean;
	selected?: LogSourceDto | null;
}): string {
	return render(LogToolbar, {
		props: {
			needle: props.needle ?? '',
			caseSensitive: props.caseSensitive ?? false,
			minLevel: props.minLevel ?? null,
			follow: props.follow ?? true,
			newRowsWhilePaused: props.newRowsWhilePaused ?? false,
			selected: props.selected ?? null,
			onNeedle: () => {},
			onCaseSensitive: () => {},
			onMinLevel: () => {},
			onSetFollow: () => {},
			onJumpToLatest: () => {},
			onSelectStream: () => {}
		}
	}).body;
}

describe('LogToolbar controls present', () => {
	it('renders the filter input, case toggle, level select and follow toggle', () => {
		const body = renderToolbar({});
		expect(body).toContain('data-testid="log-filter"');
		expect(body).toContain('data-testid="log-case-sensitive"');
		expect(body).toContain('data-testid="log-level"');
		expect(body).toContain('data-testid="log-follow"');
	});

	it('bounds the filter input so a paste cannot exceed the server cap', () => {
		const body = renderToolbar({});
		const tag = body.match(/<input[^>]*data-testid="log-filter"[^>]*>/)?.[0] ?? '';
		expect(tag).toContain('maxlength="256"');
	});

	it('reflects the current needle as the input value', () => {
		const body = renderToolbar({ needle: 'FastCGI sent in stderr' });
		const tag = body.match(/<input[^>]*data-testid="log-filter"[^>]*>/)?.[0] ?? '';
		expect(tag).toContain('value="FastCGI sent in stderr"');
	});

	it('reflects caseSensitive/follow as aria-checked on their switches', () => {
		const on = renderToolbar({ caseSensitive: true, follow: false });
		expect(tagFor(on, 'log-case-sensitive')).toContain('aria-checked="true"');
		expect(tagFor(on, 'log-follow')).toContain('aria-checked="false"');

		const off = renderToolbar({ caseSensitive: false, follow: true });
		expect(tagFor(off, 'log-case-sensitive')).toContain('aria-checked="false"');
		expect(tagFor(off, 'log-follow')).toContain('aria-checked="true"');
	});

	it('selects the option matching minLevel, defaulting to "All levels"', () => {
		const all = renderToolbar({ minLevel: null });
		expect(all).toMatch(/<option value="all"[^>]*selected/);

		const warn = renderToolbar({ minLevel: 'warn' });
		expect(warn).toMatch(/<option value="warn"[^>]*selected/);

		const error = renderToolbar({ minLevel: 'error' });
		expect(error).toMatch(/<option value="error"[^>]*selected/);
	});

	it('renders the privacy note (spec D5: no false redaction promise)', () => {
		const body = renderToolbar({});
		expect(body).toContain('data-testid="log-privacy-note"');
		expect(body).toMatch(/local/i);
		expect(body).toMatch(/sensitive/i);
	});
});

describe('LogToolbar Jump to latest', () => {
	it('is absent while following', () => {
		const body = renderToolbar({ follow: true, newRowsWhilePaused: false });
		expect(body).not.toContain('data-testid="log-jump-to-latest"');
	});

	it('is absent even with newRowsWhilePaused if follow is somehow already on', () => {
		// Defensive: `newRowsWhilePaused` should never be true while `follow`
		// is true in practice (the store clears it on setFollow(true)), but
		// the component must not show a "resume following" button while
		// already following even if that invariant were ever violated.
		const body = renderToolbar({ follow: true, newRowsWhilePaused: true });
		expect(body).not.toContain('data-testid="log-jump-to-latest"');
	});

	it('appears once paused with new rows waiting', () => {
		const body = renderToolbar({ follow: false, newRowsWhilePaused: true });
		expect(body).toContain('data-testid="log-jump-to-latest"');
	});

	it('does not appear while paused with nothing new', () => {
		const body = renderToolbar({ follow: false, newRowsWhilePaused: false });
		expect(body).not.toContain('data-testid="log-jump-to-latest"');
	});
});

describe('LogToolbar stream toggle ("then the stream" — spec D6)', () => {
	it('is absent for a non-site source', () => {
		const body = renderToolbar({ selected: { kind: 'nginxError' } });
		expect(body).not.toContain('data-testid="log-stream-toggle"');
	});

	it('is absent when nothing is selected', () => {
		const body = renderToolbar({ selected: null });
		expect(body).not.toContain('data-testid="log-stream-toggle"');
	});

	it('appears for a site source, with the current stream pressed', () => {
		const errorBody = renderToolbar({ selected: { kind: 'siteError', domain: 'shop.localhost' } });
		expect(errorBody).toContain('data-testid="log-stream-toggle"');
		expect(tagFor(errorBody, 'log-stream-error')).toContain('aria-pressed="true"');
		expect(tagFor(errorBody, 'log-stream-access')).toContain('aria-pressed="false"');

		const accessBody = renderToolbar({
			selected: { kind: 'siteAccess', domain: 'shop.localhost' }
		});
		expect(tagFor(accessBody, 'log-stream-access')).toContain('aria-pressed="true"');
		expect(tagFor(accessBody, 'log-stream-error')).toContain('aria-pressed="false"');
	});
});
