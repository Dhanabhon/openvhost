// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`).
//
// This file lost most of its old surface with the move off Homebrew, and the
// losses are the point: this component used to carry a "Homebrew is required to
// install MySQL" guide with a copyable `curl | bash`, a brew.sh button and its
// own Check-again control. Installing MySQL is now download → SHA-256 verify →
// extract, so that guide described a dependency that no longer exists, and the
// tests that pinned it were pinning a dead end. Whether a PARTICULAR host has a
// verified download is a per-row fact, covered in `MysqlRow.svelte.test.ts`.
//
// WHAT THIS FILE CANNOT COVER: `svelte/server` renders markup only, with no DOM
// and no event dispatch.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import DatabasesEmpty from './DatabasesEmpty.svelte';

function renderEmpty(anyInstalled: boolean): string {
	return render(DatabasesEmpty, { props: { anyInstalled } }).body;
}

describe('DatabasesEmpty', () => {
	it('invites the user to install when no MySQL is present', () => {
		const body = renderEmpty(false);
		expect(body).toContain('data-testid="databases-no-mysql"');
		expect(body).toMatch(/install/i);
	});

	it('shows nothing once a MySQL major is installed', () => {
		expect(renderEmpty(true)).not.toContain('data-testid="databases-no-mysql"');
	});

	it('names MySQL specifically, not a generic "database" placeholder', () => {
		expect(renderEmpty(false)).toMatch(/mysql/i);
	});

	// The load-bearing regression. A user with no Homebrew can install MySQL
	// now; telling them to go install brew first would be a dead end this
	// component invented, and it is exactly what it used to do.
	it('no longer demands Homebrew, and offers no brew guide of any kind', () => {
		const body = renderEmpty(false);
		expect(body).not.toContain('data-testid="databases-no-brew"');
		expect(body).not.toContain('data-testid="open-brew-site"');
		expect(body).not.toContain('/bin/bash -c');
		expect(body).not.toMatch(/homebrew is required/i);
	});

	it('says the install needs no Homebrew, rather than staying silent about it', () => {
		expect(renderEmpty(false)).toMatch(/no Homebrew required/i);
	});

	it('describes what the install actually does — fetch, check, unpack', () => {
		const body = renderEmpty(false);
		expect(body).toMatch(/downloads MySQL from Oracle/i);
		expect(body).toMatch(/checksum/i);
	});

	// The Check-again control lived inside the removed brew guide. The page now
	// renders exactly one, unconditionally, which is what stops it doubling up.
	it('renders no rescan control of its own', () => {
		expect(renderEmpty(false)).not.toContain('data-testid="databases-check-again"');
	});
});
