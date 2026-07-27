// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node` vitest project —
// same approach as ApplyDialog.svelte.test.ts and QuitDialog.svelte.test.ts.
//
// WHAT THIS FILE CANNOT COVER: no DOM, so the "Check again" click and the brew.sh link's
// actual navigation are manual click-through items in the PR, same caveat as those files.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LanguagesEmpty from './LanguagesEmpty.svelte';

function renderEmpty(props: {
	brewFound: boolean;
	anyInstalled?: boolean;
	brewSearched?: string[];
}): string {
	return render(LanguagesEmpty, {
		props: {
			brewFound: props.brewFound,
			anyInstalled: props.anyInstalled ?? false,
			brewSearched: props.brewSearched ?? [],
			onRescan: () => {}
		}
	}).body;
}

describe('LanguagesEmpty', () => {
	it('invites the user to install when brew is present and no PHP is', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: false });
		expect(body).toContain('data-testid="languages-no-php"');
		expect(body).toMatch(/install/i);
		expect(body).not.toContain('data-testid="languages-no-brew"');
	});

	it('explains the dependency and how to satisfy it when brew is missing', () => {
		// Otherwise the user came here to solve a problem and was handed a
		// different one with no way forward.
		const body = renderEmpty({
			brewFound: false,
			brewSearched: ['/opt/homebrew/bin/brew', '/usr/local/bin/brew']
		});
		expect(body).toContain('data-testid="languages-no-brew"');
		expect(body).toContain('/opt/homebrew/bin/brew');
		expect(body).toContain('brew.sh');
		expect(body).toContain('data-testid="languages-check-again"');
	});

	it('offers the brew install command as copyable text, never as a button that runs it', () => {
		// A curl | bash that asks for sudo is the machine owner's decision, and
		// our spawned process has no tty to answer the prompt anyway.
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('/bin/bash -c');
		expect(body).not.toMatch(/data-testid="install-homebrew"/);
	});

	it('shows neither empty state once a version is installed', () => {
		const body = renderEmpty({ brewFound: true, anyInstalled: true });
		expect(body).not.toContain('data-testid="languages-no-php"');
		expect(body).not.toContain('data-testid="languages-no-brew"');
	});

	// The no-brew state must name every path actually searched rather than a
	// hardcoded guess — an Intel Mac's /usr/local and an Apple Silicon Mac's
	// /opt/homebrew are different paths, and a wrong one sends the user
	// checking a location brew was never going to be at.
	it('says so, rather than rendering an empty gap, when no paths were searched', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toContain('data-testid="languages-no-brew"');
		expect(body).toMatch(/no (install )?paths?/i);
	});

	it('links to the official Homebrew site rather than only naming it', () => {
		const body = renderEmpty({ brewFound: false, brewSearched: [] });
		expect(body).toMatch(/href="https:\/\/brew\.sh"/);
	});
});
