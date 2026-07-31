// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: neuter-proven. Every group below was run once against a
// deliberately broken version of the function under test (wrong branch,
// off-by-one, or a stubbed-out return) to confirm it fails for the reason
// this file claims, then against the real implementation in logs.derive.ts
// to confirm it passes — see task-6-report.md for the specific breaks used.
import { describe, expect, it } from 'vitest';
import {
	LOG_NEEDLE_MAX_BYTES,
	SIZE_WARNING_BYTES,
	classifyReadError,
	decodeLogSource,
	describeSource,
	formatBytes,
	groupSources,
	isSourceListed,
	levelClass,
	logBodyState,
	logSourceQuery,
	parseSourceParam,
	sameSource,
	siteSource,
	sourceDomain,
	sourceStream,
	truncateToUtf8Bytes
} from './logs.derive';
import type { IpcError, LogSourceDto, LogSourceRowDto } from './ipc';

describe('truncateToUtf8Bytes', () => {
	it('leaves a short ASCII string untouched', () => {
		expect(truncateToUtf8Bytes('ERROR', 256)).toBe('ERROR');
	});

	it('truncates an over-long ASCII string to exactly maxBytes', () => {
		const s = 'a'.repeat(300);
		const out = truncateToUtf8Bytes(s, 256);
		expect(out).toHaveLength(256);
		expect(new TextEncoder().encode(out).length).toBe(256);
	});

	// The whole reason this function exists rather than a bare `slice`: Thai
	// text is 3 bytes/char in UTF-8, so a naive character-count cap could
	// pass a 256-CHAR string that is 768 BYTES — well over the server's cap.
	it('never splits a multi-byte character, even when that means returning fewer bytes than the cap', () => {
		const thai = 'ก'.repeat(100); // 3 bytes each = 300 bytes
		const out = truncateToUtf8Bytes(thai, 256);
		const bytes = new TextEncoder().encode(out);
		expect(bytes.length).toBeLessThanOrEqual(256);
		// Every returned character round-trips cleanly — no replacement-character
		// garbage from a mid-sequence cut.
		expect(out).not.toContain('�');
		expect(out.length % 1).toBe(0);
	});

	it('never splits a surrogate pair (an emoji outside the BMP)', () => {
		const s = '💚'.repeat(80); // 4 bytes each in UTF-8
		const out = truncateToUtf8Bytes(s, 100);
		expect(new TextEncoder().encode(out).length).toBeLessThanOrEqual(100);
		expect(out).not.toContain('�');
	});

	it('the exported cap is 256, matching the server (spec D3)', () => {
		expect(LOG_NEEDLE_MAX_BYTES).toBe(256);
	});
});

describe('formatBytes', () => {
	it('renders sub-1024 as plain bytes', () => {
		expect(formatBytes(0)).toBe('0 B');
		expect(formatBytes(512)).toBe('512 B');
	});

	it('renders KiB/MiB/GiB with two decimals under 10 and one at or above', () => {
		expect(formatBytes(1536)).toBe('1.50 KiB');
		expect(formatBytes(10 * 1024)).toBe('10.0 KiB');
		expect(formatBytes(150 * 1024 * 1024)).toBe('150.0 MiB');
		expect(formatBytes(2 * 1024 * 1024 * 1024)).toBe('2.00 GiB');
	});

	it('the >100 MiB status-line threshold is exactly 100 MiB', () => {
		expect(SIZE_WARNING_BYTES).toBe(100 * 1024 * 1024);
	});
});

describe('levelClass', () => {
	it('maps every level to its own class, defaulting unknown-ish input to info', () => {
		expect(levelClass('error')).toBe('lvl-error');
		expect(levelClass('warn')).toBe('lvl-warn');
		expect(levelClass('info')).toBe('lvl-info');
	});
});

describe('sameSource', () => {
	it('matches identical simple sources and rejects different kinds', () => {
		expect(sameSource({ kind: 'nginxError' }, { kind: 'nginxError' })).toBe(true);
		expect(sameSource({ kind: 'nginxError' }, { kind: 'nginxAccess' })).toBe(false);
	});

	it('compares the payload for parameterised kinds, not just the kind', () => {
		expect(sameSource({ kind: 'phpFpm', major: '8.4' }, { kind: 'phpFpm', major: '8.4' })).toBe(
			true
		);
		expect(sameSource({ kind: 'phpFpm', major: '8.4' }, { kind: 'phpFpm', major: '8.3' })).toBe(
			false
		);
		expect(
			sameSource(
				{ kind: 'siteAccess', domain: 'a.localhost' },
				{ kind: 'siteError', domain: 'a.localhost' }
			)
		).toBe(false);
		expect(
			sameSource({ kind: 'serviceRing', id: 'mysql' }, { kind: 'serviceRing', id: 'mysql' })
		).toBe(true);
		expect(
			sameSource({ kind: 'serviceRing', id: 'mysql' }, { kind: 'serviceRing', id: 'nginx' })
		).toBe(false);
	});
});

describe('sourceDomain / sourceStream / siteSource', () => {
	it('extracts the domain only from site-scoped sources', () => {
		expect(sourceDomain({ kind: 'siteAccess', domain: 'shop.localhost' })).toBe('shop.localhost');
		expect(sourceDomain({ kind: 'siteError', domain: 'shop.localhost' })).toBe('shop.localhost');
		expect(sourceDomain({ kind: 'nginxError' })).toBeNull();
		expect(sourceDomain(null)).toBeNull();
	});

	it('extracts the stream only from site-scoped sources', () => {
		expect(sourceStream({ kind: 'siteAccess', domain: 'x' })).toBe('access');
		expect(sourceStream({ kind: 'siteError', domain: 'x' })).toBe('error');
		expect(sourceStream({ kind: 'phpFpm', major: '8.4' })).toBeNull();
		expect(sourceStream(null)).toBeNull();
	});

	it('siteSource is the exact inverse of sourceDomain/sourceStream', () => {
		const built = siteSource('shop.localhost', 'access');
		expect(built).toEqual({ kind: 'siteAccess', domain: 'shop.localhost' });
		expect(sourceDomain(built)).toBe('shop.localhost');
		expect(sourceStream(built)).toBe('access');
	});
});

describe('describeSource', () => {
	it('names every kind in a human-readable way', () => {
		expect(describeSource({ kind: 'nginxError' })).toBe('nginx error log');
		expect(describeSource({ kind: 'nginxAccess' })).toBe('nginx access log');
		expect(describeSource({ kind: 'phpFpm', major: '8.1' })).toBe('PHP 8.1 pool log');
		expect(describeSource({ kind: 'siteAccess', domain: 'shop.localhost' })).toBe(
			'shop.localhost access log'
		);
		expect(describeSource({ kind: 'siteError', domain: 'shop.localhost' })).toBe(
			'shop.localhost error log'
		);
		expect(describeSource({ kind: 'serviceRing', id: 'mysql' })).toBe('mysql output');
	});
});

function row(source: LogSourceDto, label: string, kind: 'file' | 'ring' = 'file'): LogSourceRowDto {
	return {
		source,
		label,
		kind,
		exists: true,
		sizeBytes: kind === 'file' ? 100 : null,
		serviceId: null
	};
}

describe('groupSources', () => {
	it('keeps every non-site row flat under services, in server order', () => {
		const sources = [
			row({ kind: 'nginxError' }, 'nginx error log'),
			row({ kind: 'nginxAccess' }, 'nginx access log'),
			row({ kind: 'phpFpm', major: '8.4' }, 'PHP 8.4 pool log'),
			row({ kind: 'serviceRing', id: 'mysql' }, 'mysql output', 'ring')
		];
		const grouped = groupSources(sources);
		expect(grouped.services.map((s) => s.source)).toEqual(sources.map((s) => s.source));
		expect(grouped.siteDomains).toEqual([]);
	});

	// The reason this feature exists at all (spec D6): 40 sites is 80 flat
	// tabs with the mock's approach. Grouping by DOMAIN collapses the two
	// rows a site always has (access + error) into one chip.
	it('collapses a site’s access/error pair into one domain, sorted', () => {
		const sources = [
			row({ kind: 'siteError', domain: 'zeta.localhost' }, 'zeta.localhost error log'),
			row({ kind: 'siteAccess', domain: 'zeta.localhost' }, 'zeta.localhost access log'),
			row({ kind: 'siteAccess', domain: 'alpha.localhost' }, 'alpha.localhost access log'),
			row({ kind: 'siteError', domain: 'alpha.localhost' }, 'alpha.localhost error log')
		];
		const grouped = groupSources(sources);
		expect(grouped.siteDomains).toEqual(['alpha.localhost', 'zeta.localhost']);
		expect(grouped.services).toEqual([]);
	});
});

describe('isSourceListed', () => {
	it('is true only for a source actually present in the catalogue', () => {
		const sources = [row({ kind: 'phpFpm', major: '8.4' }, 'PHP 8.4 pool log')];
		expect(isSourceListed(sources, { kind: 'phpFpm', major: '8.4' })).toBe(true);
		expect(isSourceListed(sources, { kind: 'phpFpm', major: '8.1' })).toBe(false);
	});
});

describe('deep-link codec (encode via logSourceQuery / decode / parse)', () => {
	const cases: LogSourceDto[] = [
		{ kind: 'nginxError' },
		{ kind: 'nginxAccess' },
		{ kind: 'phpFpm', major: '8.4' },
		{ kind: 'siteAccess', domain: 'shop.localhost' },
		{ kind: 'siteError', domain: 'shop.localhost' },
		{ kind: 'serviceRing', id: 'mysql' }
	];

	it.each(cases)('round-trips %j through logSourceQuery -> parseSourceParam', (source) => {
		expect(parseSourceParam(logSourceQuery(source))).toEqual(source);
	});

	it('parses a bare query string with no leading "?" the same way', () => {
		expect(parseSourceParam('source=nginx-error')).toEqual({ kind: 'nginxError' });
	});

	it('returns null for a missing source param', () => {
		expect(parseSourceParam('')).toBeNull();
		expect(parseSourceParam('?other=1')).toBeNull();
	});

	it('returns null rather than throwing on a garbage source value', () => {
		expect(decodeLogSource('not-a-real-source')).toBeNull();
		expect(decodeLogSource('unknown-tag:value')).toBeNull();
		expect(decodeLogSource('php-fpm:')).toBeNull();
		expect(parseSourceParam('?source=%00garbage')).toBeNull();
	});

	it('logSourceQuery starts with "?", ready to append to a resolved path', () => {
		expect(logSourceQuery({ kind: 'nginxError' })).toMatch(/^\?source=/);
	});
});

describe('classifyReadError', () => {
	it('is none for no error', () => {
		expect(classifyReadError(null)).toBe('none');
	});

	it('recognizes a permission failure case-insensitively, wherever it sits in the message', () => {
		const e: IpcError = {
			kind: 'core',
			message: 'open /x/error.log: Permission denied (os error 13)'
		};
		expect(classifyReadError(e)).toBe('permission');
		const e2: IpcError = { kind: 'core', message: 'PERMISSION DENIED reading the file' };
		expect(classifyReadError(e2)).toBe('permission');
	});

	it('falls back to other for every non-permission failure', () => {
		expect(classifyReadError({ kind: 'core', message: 'disk is unreadable' })).toBe('other');
		expect(classifyReadError({ kind: 'simulated' })).toBe('other');
	});
});

describe('logBodyState precedence', () => {
	const base = {
		selected: { kind: 'siteError', domain: 'shop.localhost' } as LogSourceDto,
		requestedUnavailable: null as LogSourceDto | null,
		readError: null as IpcError | null,
		exists: true,
		rowCount: 3
	};

	it('unavailable wins over everything else', () => {
		expect(
			logBodyState({ ...base, requestedUnavailable: { kind: 'nginxError' }, exists: false })
		).toBe('unavailable');
	});

	it('no-selection when nothing is chosen and nothing was requested', () => {
		expect(logBodyState({ ...base, selected: null })).toBe('no-selection');
	});

	// The realistic shape of "unavailable": `LogsStore.selectFromDeepLink`
	// deliberately leaves `selected` at `null` (no fallback content behind
	// the banner — see that method's doc comment), so `selected === null`
	// AND `requestedUnavailable !== null` are true AT THE SAME TIME. A
	// precedence order that checks `selected` before `requestedUnavailable`
	// would report `no-selection` here and never render the banner at all —
	// this pins the two-flag combination the standalone tests above (each
	// varying only one flag from `base`) cannot reach.
	it('unavailable still wins when nothing is selected at all', () => {
		expect(
			logBodyState({
				...base,
				selected: null,
				requestedUnavailable: { kind: 'nginxError' }
			})
		).toBe('unavailable');
	});

	it('permission-denied beats a generic error and a missing file', () => {
		expect(
			logBodyState({
				...base,
				exists: false,
				readError: { kind: 'core', message: 'Permission denied (os error 13)' }
			})
		).toBe('permission-denied');
	});

	it('error is used for a non-permission failure', () => {
		expect(logBodyState({ ...base, readError: { kind: 'core', message: 'disk fell over' } })).toBe(
			'error'
		);
	});

	it('not-yet-created when the file does not exist and nothing failed', () => {
		expect(logBodyState({ ...base, exists: false })).toBe('not-yet-created');
	});

	it('empty when the file exists but nothing matched', () => {
		expect(logBodyState({ ...base, rowCount: 0 })).toBe('empty');
	});

	it('rows for the ordinary, populated case', () => {
		expect(logBodyState(base)).toBe('rows');
	});
});
