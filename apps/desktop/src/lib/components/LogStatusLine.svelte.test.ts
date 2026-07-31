// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: genuine RED-first — LogStatusLine.svelte does not exist yet.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LogStatusLine from './LogStatusLine.svelte';
import type { LogSourceDto } from '$lib/ipc';

function renderStatus(props: {
	selected?: LogSourceDto | null;
	requestedUnavailable?: LogSourceDto | null;
	sizeBytes?: number;
	truncatedLines?: number;
	scanBoundReached?: boolean;
	follow?: boolean;
}): string {
	return render(LogStatusLine, {
		props: {
			selected: 'selected' in props ? (props.selected ?? null) : { kind: 'nginxError' },
			requestedUnavailable: props.requestedUnavailable ?? null,
			sizeBytes: props.sizeBytes ?? 1024,
			truncatedLines: props.truncatedLines ?? 0,
			scanBoundReached: props.scanBoundReached ?? false,
			follow: props.follow ?? true,
			onRevealFolder: () => {}
		}
	}).body;
}

describe('LogStatusLine visibility', () => {
	it('renders nothing when no real source is targeted', () => {
		expect(renderStatus({ selected: null })).not.toContain('data-testid="log-status-line"');
	});

	it('renders nothing for an unavailable deep-link target', () => {
		const body = renderStatus({
			selected: null,
			requestedUnavailable: { kind: 'nginxError' }
		});
		expect(body).not.toContain('data-testid="log-status-line"');
	});

	it('renders for an ordinary selected source', () => {
		expect(renderStatus({})).toContain('data-testid="log-status-line"');
	});

	// Not reachable through `LogsStore`'s actual call pattern today
	// (`selectFromDeepLink`'s not-found branch never touches an
	// already-`null` `selected` — see that method's doc comment), but the
	// two props are independent at the TYPE level, and a component must not
	// rely on a caller-side invariant it cannot see. Without this, a chip
	// selection error that left a stale `requestedUnavailable` set (see
	// `LogsStore.selectSource` clearing it) could show BOTH a real
	// selection's status line — file size, Open log folder for a source
	// the picker no longer treats as available — right underneath the
	// "unavailable" banner in LogBody.
	it('stays hidden even if a selection exists alongside a stale requestedUnavailable', () => {
		const body = renderStatus({
			selected: { kind: 'nginxError' },
			requestedUnavailable: { kind: 'phpFpm', major: '8.1' }
		});
		expect(body).not.toContain('data-testid="log-status-line"');
	});
});

describe('LogStatusLine facts', () => {
	it('shows the file size in binary units', () => {
		const body = renderStatus({ sizeBytes: 1536 });
		expect(body).toContain('1.50 KiB');
	});

	it('says Following or Paused based on follow', () => {
		expect(renderStatus({ follow: true })).toMatch(/Following/);
		expect(renderStatus({ follow: false })).toMatch(/Paused/);
	});

	it('offers Open log folder for the selected source', () => {
		const body = renderStatus({});
		expect(body).toContain('data-testid="log-reveal-folder"');
		expect(body).toContain('>Open log folder<');
	});
});

describe('LogStatusLine warnings (spec D8/D3)', () => {
	it('warns above the 100 MiB threshold and stays quiet below it', () => {
		const big = renderStatus({ sizeBytes: 101 * 1024 * 1024 });
		expect(big).toContain('data-testid="log-size-warning"');
		expect(big).toMatch(/100 ?MiB/i);

		const small = renderStatus({ sizeBytes: 99 * 1024 * 1024 });
		expect(small).not.toContain('data-testid="log-size-warning"');
	});

	it('notes an early scan-bound stop, honestly, only when it happened', () => {
		const stopped = renderStatus({ scanBoundReached: true });
		expect(stopped).toContain('data-testid="log-scan-bound-note"');

		const notStopped = renderStatus({ scanBoundReached: false });
		expect(notStopped).not.toContain('data-testid="log-scan-bound-note"');
	});

	it('notes truncated lines only when at least one was truncated', () => {
		const some = renderStatus({ truncatedLines: 3 });
		expect(some).toContain('data-testid="log-truncated-note"');
		expect(some).toContain('3');

		const none = renderStatus({ truncatedLines: 0 });
		expect(none).not.toContain('data-testid="log-truncated-note"');
	});
});
