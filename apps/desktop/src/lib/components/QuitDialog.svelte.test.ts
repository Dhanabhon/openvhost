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
	installingMajor?: string | null;
	quitting?: boolean;
	error?: string;
}): string {
	return render(QuitDialog, {
		props: {
			pending: props.pending ?? [],
			installingMajor: props.installingMajor ?? null,
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
		const t = text(html({ pending: [], installingMajor: '8.4' }));
		expect(t).toContain('PHP 8.4');
		expect(t).toContain('is still installing');
		expect(t).not.toContain('No services are running');
	});

	it('offers Stop and quit when only an install is in flight, no services', () => {
		const m = html({ pending: [], installingMajor: '8.4' });
		expect(m).toContain('Stop and quit');
		expect(m).not.toContain('>Quit<');
	});

	it('combines a running service and an in-flight install in one sentence', () => {
		const t = text(html({ pending: ['nginx'], installingMajor: '8.4' }));
		expect(t).toContain('nginx is running');
		expect(t).toContain('PHP 8.4');
		expect(t).toContain('is still installing');
	});

	it('says nothing is running and mentions no install when both are idle', () => {
		const t = text(html({ pending: [], installingMajor: null }));
		expect(t).toContain('No services are running');
		expect(t).not.toContain('PHP');
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
