// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), mirroring
// `LanguagesEmpty.svelte.test.ts` almost exactly — same three states, same
// Homebrew guide, adapted for MySQL's copy.
//
// WHAT THIS FILE CANNOT COVER: `svelte/server` renders markup only, with no
// DOM and no event dispatch — see `LanguagesEmpty.svelte.test.ts`'s header
// for the identical caveat.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import DatabasesEmpty from './DatabasesEmpty.svelte';

function renderEmpty(props: {
	brewFound: boolean;
	anyInstalled?: boolean;
	brewSearched?: string[];
	installing?: string;
	onRescan?: () => void;
	onOpenBrewSite?: () => void;
}): string {
	return render(DatabasesEmpty, {
		props: {
			brewFound: props.brewFound,
			anyInstalled: props.anyInstalled ?? false,
			brewSearched: props.brewSearched ?? [],
			installing: props.installing ?? '',
			onRescan: props.onRescan ?? (() => {}),
			onOpenBrewSite: props.onOpenBrewSite ?? (() => {})
		}
	}).body;
}

function checkAgainButtonTag(body: string): string {
	const match = body.match(/<button[^>]*data-testid="databases-check-again"[^>]*>/);
	if (!match) throw new Error('expected the Check again button to render');
	return match[0];
}

describe('DatabasesEmpty', () => {
	it('invites the user to install when brew is present and no MySQL is', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: false });
		expect(body).toContain('data-testid="databases-no-mysql"');
		expect(body).toMatch(/install/i);
		expect(body).not.toContain('data-testid="databases-no-brew"');
	});

	it('explains the dependency and how to satisfy it when brew is missing', () => {
		const body = renderEmpty({
			brewFound: false,
			brewSearched: ['/opt/homebrew/bin/brew', '/usr/local/bin/brew']
		});
		expect(body).toContain('data-testid="databases-no-brew"');
		expect(body).toContain('/opt/homebrew/bin/brew');
		expect(body).toContain('brew.sh');
		expect(body).toContain('data-testid="databases-check-again"');
	});

	it('offers the brew install command as copyable text, never as a button that runs it', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('/bin/bash -c');
		expect(body).not.toMatch(/data-testid="install-homebrew"/);
	});

	it('shows neither empty state once a MySQL major is installed', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: true });
		expect(body).not.toContain('data-testid="databases-no-mysql"');
		expect(body).not.toContain('data-testid="databases-no-brew"');
	});

	it('says so, rather than rendering an empty gap, when no paths were searched', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('data-testid="databases-no-brew"');
		expect(body).toMatch(/no (install )?paths?/i);
	});

	it('offers a real control for the Homebrew site, not a bare link', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('data-testid="open-brew-site"');
		expect(body).not.toMatch(/href="https:\/\/brew\.sh"/);
	});

	it('calls out to open the Homebrew site when that control is used', () => {
		let called = 0;
		const body = renderEmpty({
			brewFound: false,
			brewSearched: [],
			onOpenBrewSite: () => {
				called += 1;
			}
		});
		expect(body).toContain('data-testid="open-brew-site"');
		expect(called).toBe(0); // rendering alone must not invoke it
	});

	it('disables Check again while an install is running', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [], installing: '8.4' });
		expect(checkAgainButtonTag(body)).toContain('disabled');
	});

	it('leaves Check again enabled when no install is running', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [], installing: '' });
		expect(checkAgainButtonTag(body)).not.toContain('disabled');
	});

	it('names MySQL specifically, not a generic "database" placeholder', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: false });
		expect(body).toMatch(/mysql/i);
	});
});
