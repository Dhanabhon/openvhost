// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: genuine RED-first — LogBody.svelte does not exist yet.
//
// WHAT THIS FILE CANNOT COVER: the auto-scroll-on-follow `$effect` and the
// onscroll near-bottom detection both need a real, laid-out DOM (scrollTop/
// scrollHeight/clientHeight are all 0 under `svelte/server`) — those are
// manual click-list items. Every DISTINCT STATE (spec D6) is reachable
// through props alone, which is what this file actually proves.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LogBody from './LogBody.svelte';
import type { IpcError, LogResetDto, LogRowDto, LogSourceDto } from '$lib/ipc';

function renderBody(props: {
	selected?: LogSourceDto | null;
	requestedUnavailable?: LogSourceDto | null;
	readError?: IpcError | null;
	exists?: boolean;
	rows?: LogRowDto[];
	filtered?: boolean;
	reset?: LogResetDto | null;
	follow?: boolean;
}): string {
	return render(LogBody, {
		props: {
			// `'selected' in props`, NOT `props.selected ?? default`: `??`
			// cannot tell "not provided" apart from "explicitly null", and
			// `selected: null` is a real, distinct case this suite tests —
			// collapsing it into the default silently tested the wrong thing.
			selected: 'selected' in props ? (props.selected ?? null) : { kind: 'nginxError' },
			requestedUnavailable: props.requestedUnavailable ?? null,
			readError: props.readError ?? null,
			exists: props.exists ?? true,
			rows: props.rows ?? [],
			filtered: props.filtered ?? false,
			reset: props.reset ?? null,
			follow: props.follow ?? true,
			onRevealFolder: () => {},
			onScroll: () => {}
		}
	}).body;
}

describe('LogBody distinct states (spec D6)', () => {
	it('no-selection: nothing chosen and nothing requested', () => {
		const body = renderBody({ selected: null });
		expect(body).toContain('data-testid="log-state-no-selection"');
	});

	it('unavailable: a deep link named something not in the catalogue', () => {
		const body = renderBody({
			selected: null,
			requestedUnavailable: { kind: 'phpFpm', major: '8.1' }
		});
		expect(body).toContain('data-testid="log-state-unavailable"');
		expect(body).toContain('PHP 8.1 pool log');
		expect(body).not.toContain('data-testid="log-state-no-selection"');
	});

	it('permission-denied: a read failed with a permission error', () => {
		const body = renderBody({
			exists: false,
			readError: { kind: 'core', message: 'open x: Permission denied (os error 13)' }
		});
		expect(body).toContain('data-testid="log-state-permission-denied"');
		expect(body).toMatch(/permission/i);
	});

	it('error: a read failed for a non-permission reason, message included', () => {
		const body = renderBody({
			exists: false,
			readError: { kind: 'core', message: 'disk fell over' }
		});
		expect(body).toContain('data-testid="log-state-error"');
		expect(body).toContain('disk fell over');
	});

	it('not-yet-created: the file does not exist and nothing failed', () => {
		const body = renderBody({ exists: false });
		expect(body).toContain('data-testid="log-state-not-yet-created"');
	});

	it('empty: the file exists but nothing matched, filter-aware copy', () => {
		const plain = renderBody({ rows: [] });
		expect(plain).toContain('data-testid="log-state-empty"');
		expect(plain).toMatch(/empty/i);

		const filtered = renderBody({ rows: [], filtered: true });
		expect(filtered).toMatch(/match/i);
	});

	it('rows: renders every row with its level and text', () => {
		const body = renderBody({
			rows: [
				{ level: 'info', text: 'nginx started' },
				{ level: 'error', text: 'FastCGI sent in stderr: fatal' }
			]
		});
		expect(body).not.toContain('data-testid="log-state-empty"');
		expect(body).toContain('nginx started');
		expect(body).toContain('FastCGI sent in stderr: fatal');
		expect(body).toContain('lvl-error');
	});
});

describe('LogBody reset notice', () => {
	it('shows a distinct notice for rotated vs truncated', () => {
		const rotated = renderBody({ reset: 'rotated', rows: [{ level: 'info', text: 'x' }] });
		expect(rotated).toContain('data-testid="log-reset-notice"');
		expect(rotated).toMatch(/rotated|replaced/i);

		const truncated = renderBody({ reset: 'truncated', rows: [{ level: 'info', text: 'x' }] });
		expect(truncated).toMatch(/truncated/i);
	});

	it('is absent when nothing was reset', () => {
		const body = renderBody({ reset: null });
		expect(body).not.toContain('data-testid="log-reset-notice"');
	});
});
