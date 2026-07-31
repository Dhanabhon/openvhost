// SPDX-License-Identifier: GPL-3.0-or-later
//
// `onMount` does not run under SSR, so `LogsStore` (page-local, constructed
// fresh inside +page.svelte's own script) never loads anything here — this
// renders the page in its not-yet-loaded state, mirroring
// `routes.test.ts`'s identical `/web-server` coverage: the shell, the rail
// state, and the panel's empty state must all be right before any IPC has
// answered. The store's OWN states (each distinct LogBody/LogStatusLine
// rendering) are covered directly, with real props, in
// LogBody.svelte.test.ts / LogStatusLine.svelte.test.ts / etc. — this file
// is only the page-level wiring contract.

import { beforeEach, describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LogsPage from './+page.svelte';
import { servicesStore } from '$lib/services.shared.svelte';
import type { ServiceStatus } from '$lib/ipc';

const svc = (id: string, kind: 'running' | 'stopped'): ServiceStatus => ({
	id,
	displayName: id,
	endpoint: null,
	pid: kind === 'running' ? 1 : null,
	state: { kind }
});

beforeEach(() => {
	servicesStore.services = [];
	servicesStore.error = null;
});

describe('the /logs route', () => {
	it('renders the source picker, toolbar, body and status line', () => {
		const { body } = render(LogsPage);
		expect(body).toContain('data-testid="log-sources"');
		expect(body).toContain('data-testid="log-filter"');
		expect(body).toContain('data-testid="log-body"');
		// No source is selected before `onMount` runs, so the status line
		// (which only renders once something is targeted) is correctly absent —
		// pinned as a negative assertion so a future change cannot make it
		// render unconditionally without a test noticing.
		expect(body).not.toContain('data-testid="log-status-line"');
	});

	it('shows the page heading', () => {
		const { body } = render(LogsPage);
		expect(body).toContain('>Logs</h1>');
	});

	it('renders the no-selection state before anything has loaded', () => {
		const { body } = render(LogsPage);
		expect(body).toContain('data-testid="log-state-no-selection"');
	});

	it('marks Logs as the current rail destination', () => {
		const { body } = render(LogsPage);
		const anchor = [...body.matchAll(/<a\b([^>]*)>([\s\S]*?)<\/a>/g)].find(([, , inner]) =>
			inner.includes('Logs')
		);
		expect(anchor).toBeDefined();
		expect(anchor?.[1]).toContain('aria-current="page"');
	});

	it('reports the shared supervisor state in the titlebar, like every other route', () => {
		servicesStore.services = [svc('nginx', 'running'), svc('php-fpm', 'stopped')];
		const { body } = render(LogsPage);
		const shown = body.match(/([0-9]+) running/);
		expect(shown?.[1]).toBe('1');
	});
});
