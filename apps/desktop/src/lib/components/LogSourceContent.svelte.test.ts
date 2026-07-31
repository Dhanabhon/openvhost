// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: genuine RED-first — LogSourceContent.svelte does not exist
// yet, so this file fails on the import until it is written.
//
// This is the "page-level SSR" proof the whole-branch-review CRITICAL
// finding asks for, given the real page's `LogsStore` is page-local and
// `onMount` never runs under `svelte/server` (see `logs-page.test.ts`'s own
// header): `+page.svelte` composes this component with EXACTLY the props
// `store`'s fields would supply, so rendering it directly with a ring-shaped
// `selected` is what proves "a ring deep link renders the live-output
// surface, not the error state" without needing the page's own onMount to
// run.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LogSourceContent from './LogSourceContent.svelte';
import type { LogSourceDto, ServiceLogEvent } from '$lib/ipc';

function renderContent(props: {
	selected?: LogSourceDto | null;
	ringLogs?: ServiceLogEvent[];
}): string {
	return render(LogSourceContent, {
		props: {
			selected: 'selected' in props ? (props.selected ?? null) : { kind: 'nginxError' },
			ringLogs: props.ringLogs ?? [],
			requestedUnavailable: null,
			readError: null,
			exists: true,
			rows: [],
			filtered: false,
			reset: null,
			follow: true,
			newRowsWhilePaused: false,
			needle: '',
			caseSensitive: false,
			minLevel: null,
			sizeBytes: 0,
			truncatedLines: 0,
			scanBoundReached: false,
			onNeedle: () => {},
			onCaseSensitive: () => {},
			onMinLevel: () => {},
			onSetFollow: () => {},
			onJumpToLatest: () => {},
			onSelectStream: () => {},
			onRevealFolder: () => {},
			onScroll: () => {}
		}
	}).body;
}

describe('LogSourceContent (spec D7: two mechanisms, deliberately)', () => {
	// The CRITICAL bug this component exists to make impossible: before this
	// fix, a ring source flowed through the SAME LogToolbar+LogBody+
	// LogStatusLine path as a file source, and since `readLogWindow` rejects
	// a ring source by design, the rendered result was LogBody's own
	// generic error state — internal plumbing text, not a working log view.
	it('renders the live-output surface (LogPane) for a ring source, never the file toolbar/body', () => {
		const body = renderContent({
			selected: { kind: 'serviceRing', id: 'mysql' },
			ringLogs: [{ id: 'mysql', tsMs: 1, level: 'info', line: 'ready for connections' }]
		});
		expect(body).toContain('data-testid="log"'); // LogPane's own container
		expect(body).toContain('ready for connections');
		expect(body).not.toContain('data-testid="log-body"'); // LogBody did not render
		expect(body).not.toContain('data-testid="log-state-error"');
		expect(body).not.toContain('data-testid="log-filter"'); // LogToolbar did not render
		expect(body).not.toContain('data-testid="log-status-line"'); // LogStatusLine did not render
	});

	it('renders an empty LogPane, not an error, when the ring tail has not arrived yet', () => {
		const body = renderContent({ selected: { kind: 'serviceRing', id: 'mysql' }, ringLogs: [] });
		expect(body).toContain('data-testid="log"');
		expect(body).not.toContain('data-testid="log-state-error"');
	});

	// Security audit L3: `privacyNoteCopy()` used to render only via
	// `LogToolbar` on the file-source branch — a ring source rendered
	// `LogPane` with no toolbar and therefore no note at all, even though
	// ring output (raw child stdout/stderr — mysqld/php-fpm startup noise)
	// is at least as likely to carry a connection string as a file log.
	it('renders the privacy note for a ring source too (spec D5: no false redaction promise)', () => {
		const body = renderContent({
			selected: { kind: 'serviceRing', id: 'mysql' },
			ringLogs: [{ id: 'mysql', tsMs: 1, level: 'info', line: 'ready for connections' }]
		});
		expect(body).toContain('data-testid="log-privacy-note"');
		expect(body).toMatch(/local/i);
		expect(body).toMatch(/sensitive/i);
	});

	it('still renders the file toolbar/body/status-line for a file source, unchanged', () => {
		const body = renderContent({ selected: { kind: 'nginxError' } });
		expect(body).toContain('data-testid="log-filter"');
		expect(body).toContain('data-testid="log-body"');
	});

	it('renders the file surface (no-selection state) when nothing is selected at all', () => {
		const body = renderContent({ selected: null });
		expect(body).toContain('data-testid="log-body"');
		expect(body).toContain('data-testid="log-state-no-selection"');
	});
});
