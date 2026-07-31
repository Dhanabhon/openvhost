// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: genuine RED-first — LogSourcePicker.svelte does not exist
// yet, so this file fails on the import until it is written (confirmed by
// the run recorded in task-6-report.md).
//
// Rendered via `svelte/server` (no DOM) — clicks are asserted only through
// the `onSelect` callback firing with the expected argument, which needs no
// DOM event dispatch; anything requiring real layout/scroll is a manual
// click-list item.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import LogSourcePicker from './LogSourcePicker.svelte';
import type { LogSourceDto, LogSourceRowDto } from '$lib/ipc';

function row(source: LogSourceDto, label: string, kind: 'file' | 'ring' = 'file'): LogSourceRowDto {
	return {
		source,
		label,
		kind,
		exists: true,
		sizeBytes: kind === 'file' ? 10 : null,
		serviceId: null
	};
}

function renderPicker(props: {
	services?: LogSourceRowDto[];
	siteDomains?: string[];
	selected?: LogSourceDto | null;
	failedServiceIds?: ReadonlySet<string>;
}): string {
	return render(LogSourcePicker, {
		props: {
			services: props.services ?? [],
			siteDomains: props.siteDomains ?? [],
			selected: props.selected ?? null,
			failedServiceIds: props.failedServiceIds ?? new Set<string>(),
			onSelect: () => {}
		}
	}).body;
}

describe('LogSourcePicker grouping', () => {
	it('renders a Services group and a Sites group', () => {
		const body = renderPicker({});
		expect(body).toContain('data-testid="log-sources"');
		expect(body).toMatch(/>Services</);
		expect(body).toMatch(/>Sites</);
	});

	it('lists every service row as its own chip, labelled from the catalogue', () => {
		const body = renderPicker({
			services: [
				row({ kind: 'nginxError' }, 'nginx error log'),
				row({ kind: 'phpFpm', major: '8.4' }, 'PHP 8.4 pool log')
			]
		});
		expect(body).toContain('data-testid="log-source-nginx-error"');
		expect(body).toContain('nginx error log');
		expect(body).toContain('data-testid="log-source-php-fpm:8.4"');
		expect(body).toContain('PHP 8.4 pool log');
	});

	it('lists a chip per distinct site domain, not per access/error row', () => {
		const body = renderPicker({ siteDomains: ['shop.localhost', 'blog.localhost'] });
		expect(body).toContain('data-testid="log-source-domain-shop.localhost"');
		expect(body).toContain('data-testid="log-source-domain-blog.localhost"');
		expect(body.match(/data-testid="log-source-domain-/g)).toHaveLength(2);
	});

	it('says so when there are no sites yet, rather than an empty gap', () => {
		const body = renderPicker({ siteDomains: [] });
		expect(body).toMatch(/no sites/i);
	});
});

describe('LogSourcePicker selection state', () => {
	it('marks the currently selected service chip aria-pressed', () => {
		const body = renderPicker({
			services: [
				row({ kind: 'nginxError' }, 'nginx error log'),
				row({ kind: 'nginxAccess' }, 'nginx access log')
			],
			selected: { kind: 'nginxError' }
		});
		const errorChip = body.match(/<button[^>]*data-testid="log-source-nginx-error"[^>]*>/)?.[0];
		const accessChip = body.match(/<button[^>]*data-testid="log-source-nginx-access"[^>]*>/)?.[0];
		expect(errorChip).toContain('aria-pressed="true"');
		expect(accessChip).toContain('aria-pressed="false"');
	});

	it('marks a domain chip pressed when either of its streams is selected', () => {
		const body = renderPicker({
			siteDomains: ['shop.localhost'],
			selected: { kind: 'siteAccess', domain: 'shop.localhost' }
		});
		const chip = body.match(
			/<button[^>]*data-testid="log-source-domain-shop.localhost"[^>]*>/
		)?.[0];
		expect(chip).toContain('aria-pressed="true"');
	});

	it('leaves every chip unpressed when nothing is selected', () => {
		const body = renderPicker({
			services: [row({ kind: 'nginxError' }, 'nginx error log')],
			siteDomains: ['shop.localhost'],
			selected: null
		});
		expect(body).not.toContain('aria-pressed="true"');
	});
});

describe('LogSourcePicker failed-service indicator', () => {
	// Scoped to `serviceRing` rows only — `LogSourceRowDto.serviceId` is
	// `None` for every `"file"` row (including PhpFpm), so there is no
	// non-fragile way to cross-reference a php-fpm pool chip to its
	// ServiceState from this DTO alone (see logs.derive.ts's own doc
	// comment on why guessing an id format is deliberately avoided).
	it('flags a ring chip whose service has failed', () => {
		const body = renderPicker({
			services: [row({ kind: 'serviceRing', id: 'mysql' }, 'mysql output', 'ring')],
			failedServiceIds: new Set(['mysql'])
		});
		const chip = body.match(
			/<button[^>]*data-testid="log-source-service-ring:mysql"[^>]*>[\s\S]*?<\/button>/
		)?.[0];
		expect(chip).toContain('data-testid="chip-fail-mysql"');
	});

	it('does not flag a ring chip whose service is fine', () => {
		const body = renderPicker({
			services: [row({ kind: 'serviceRing', id: 'mysql' }, 'mysql output', 'ring')],
			failedServiceIds: new Set()
		});
		expect(body).not.toContain('data-testid="chip-fail-mysql"');
	});
});
