// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`) in the existing `node` vitest project —
// same approach as SiteDrawer.svelte.test.ts and SiteListRow.svelte.test.ts.
//
// WHAT THIS FILE CANNOT COVER: no DOM, so the interactive half is out of reach —
// initial focus landing on Cancel, Tab wrapping between the two buttons, and
// Escape cancelling. Those are manual click-through items in the PR. What IS
// covered is everything the markup asserts on its own: the dialog's role and
// labelling, which copy each state produces, and that the buttons are disabled
// while a quit is in flight.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import QuitDialog from './QuitDialog.svelte';

function html(props: {
	pending?: string[];
	pendingInstall?: { kind: 'php' | 'mysql'; label: string } | null;
	quitting?: boolean;
	error?: string;
}): string {
	return render(QuitDialog, {
		props: {
			pending: props.pending ?? [],
			pendingInstall: props.pendingInstall ?? null,
			quitting: props.quitting ?? false,
			error: props.error ?? '',
			onCancel: () => {},
			onConfirm: () => {}
		}
	}).body;
}

/** Visible text with tags stripped and Svelte's source indentation collapsed. */
function text(markup: string): string {
	return markup
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('where focus lands when the dialog opens', () => {
	// The dialog focuses ITSELF on mount rather than the Cancel button. Focusing a
	// control put a focus ring on a button the user had never navigated to —
	// whether `:focus-visible` matches a script-focused element is a browser
	// heuristic, so the ring came and went depending on how the dialog was
	// reached. Reported three times before the cause was found.
	//
	// SSR cannot verify where focus actually goes — see this file's header. What
	// it CAN pin is the thing that makes it possible: the container is focusable
	// at all. `dialog?.focus()` on an element without `tabindex` silently does
	// nothing and focus stays on `<body>`, which would leave the dialog unfocused
	// AND untrapped with no error anywhere.
	it('makes the container focusable, without putting it in the tab order', () => {
		const m = html({});
		expect(m).toContain('tabindex="-1"');
	});

	it('still puts Cancel before the destructive button in DOM order', () => {
		// This became load-bearing with the focus change. The old code focused the
		// first button EXPLICITLY, so it landed on Cancel however the markup was
		// ordered. Focus now starts on the container and reaches a control by
		// ordinary Tab, so "the safe choice comes first" is enforced by DOM order
		// alone — swapping the two buttons would put a stray Enter one keypress
		// from quitting, with nothing else to catch it.
		//
		// `lastIndexOf` for the confirm label: "Quit" also appears in the dialog's
		// own title ("Quit OpenVHost?"), which precedes both buttons. Matching the
		// first occurrence would compare against the heading and pass regardless of
		// the buttons' order.
		const t = text(html({ pending: [] }));
		expect(t.indexOf('Cancel')).toBeLessThan(t.lastIndexOf('Quit'));
	});
});

describe('QuitDialog', () => {
	it('is a labelled modal dialog', () => {
		const m = html({});
		expect(m).toContain('role="dialog"');
		expect(m).toContain('aria-modal="true"');
		// Labelled BY the heading rather than aria-label: the visible title and the
		// announced name cannot drift apart if they are the same string.
		expect(m).toContain('aria-labelledby="quit-title"');
		expect(m).toContain('id="quit-title"');
		expect(m).toContain('aria-describedby="quit-body"');
		expect(m).toContain('id="quit-body"');
	});

	// The whole reason this dialog exists: quitting used to abandon running
	// services silently. It must say which ones.
	it('names the running services and says they will be stopped', () => {
		const t = text(html({ pending: ['nginx', 'PHP-FPM'] }));
		expect(t).toContain('nginx and PHP-FPM');
		expect(t).toContain('are running');
		expect(t).toContain('Quitting stops');
	});

	it('agrees in number for a single service', () => {
		const t = text(html({ pending: ['nginx'] }));
		expect(t).toContain('nginx is running');
		expect(t).not.toContain('are running');
	});

	// With nothing running there is nothing to stop, so promising to stop things
	// would be a lie — the copy AND the button label both change.
	it('says nothing is running, and offers plain Quit, when idle', () => {
		const m = html({ pending: [] });
		expect(text(m)).toContain('No services are running');
		expect(m).toContain('>Quit<');
		expect(m).not.toContain('Stop and quit');
	});

	it('offers Stop and quit when something is running', () => {
		expect(html({ pending: ['nginx'] })).toContain('Stop and quit');
	});

	// The C1 audit fix: a PHP install is invisible to `pending` (it is not a
	// supervised service), so without its own copy the dialog would promise
	// "nothing will be interrupted" while a twenty-minute build was about to be
	// silently discarded.
	it('names an in-flight PHP install and warns it will be discarded', () => {
		const t = text(html({ pending: [], pendingInstall: { kind: 'php', label: '8.4' } }));
		expect(t).toContain('PHP 8.4');
		expect(t).toContain('is still installing');
		expect(t).not.toContain('No services are running');
	});

	it('offers Stop and quit when only an install is in flight, no services', () => {
		const m = html({ pending: [], pendingInstall: { kind: 'php', label: '8.4' } });
		expect(m).toContain('Stop and quit');
		expect(m).not.toContain('>Quit<');
	});

	it('combines a running service and an in-flight install in one sentence', () => {
		const t = text(html({ pending: ['nginx'], pendingInstall: { kind: 'php', label: '8.4' } }));
		expect(t).toContain('nginx is running');
		expect(t).toContain('PHP 8.4');
		expect(t).toContain('is still installing');
	});

	it('says nothing is running and mentions no install when both are idle', () => {
		const t = text(html({ pending: [], pendingInstall: null }));
		expect(t).toContain('No services are running');
		expect(t).not.toContain('PHP');
	});

	// Review fix wave, Important 1: the quit dialog used to be blind to a
	// MySQL install/init in flight entirely (the old query was PHP-only).
	// The label already reads as a complete phrase ("MySQL 8.4"), so the
	// rendered sentence must not double the word "MySQL".
	it('names an in-flight MySQL install and warns it will be discarded, without doubling the word MySQL', () => {
		const t = text(html({ pending: [], pendingInstall: { kind: 'mysql', label: 'MySQL 8.4' } }));
		expect(t).toContain('MySQL 8.4 is still installing');
		expect(t).not.toContain('MySQL MySQL');
		expect(t).not.toContain('No services are running');
	});

	it('names an in-flight MySQL initialization using its own complete label', () => {
		const t = text(
			html({
				pending: [],
				pendingInstall: { kind: 'mysql', label: 'MySQL 8.4 initialization' }
			})
		);
		expect(t).toContain('MySQL 8.4 initialization is still installing');
	});

	it('offers Stop and quit when only a MySQL install is in flight, no services', () => {
		const m = html({ pending: [], pendingInstall: { kind: 'mysql', label: 'MySQL 8.4' } });
		expect(m).toContain('Stop and quit');
		expect(m).not.toContain('>Quit<');
	});

	// Both buttons disabled: the confirm because stopping is already underway, and
	// Cancel because the services are already going down — offering to cancel
	// something that cannot be undone is the worse lie.
	it('disables both buttons while the quit is in flight', () => {
		const m = html({ pending: ['nginx'], quitting: true });
		expect(m.match(/disabled/g)?.length).toBe(2);
		expect(m).toContain('Stopping services…');
	});

	it('leaves the buttons enabled when idle', () => {
		expect(html({ pending: ['nginx'] })).not.toContain('disabled');
	});

	// A failure has nowhere else to go: the dialog is modal, so a page banner
	// would render behind the scrim where it cannot be read or dismissed.
	it('announces a failed quit inside the dialog', () => {
		const m = html({ error: 'nginx would not stop' });
		expect(m).toContain('role="alert"');
		expect(m).toContain('nginx would not stop');
		expect(html({})).not.toContain('role="alert"');
	});
});
