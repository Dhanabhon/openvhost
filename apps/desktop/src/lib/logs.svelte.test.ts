// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: genuine RED-first — `logs.svelte.ts` does not exist yet,
// so every test in this file fails with a module-not-found error until the
// store is implemented (see task-6-report.md for the run that proved it).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LogsStore, MAX_RENDERED_ROWS, POLL_INTERVAL_MS } from './logs.svelte';
import type { LogsApi } from './logs.svelte';
import type { LogRowDto, LogSourceRowDto, LogWindowDto } from './ipc';

function sourceRow(
	source: LogSourceRowDto['source'],
	label: string,
	overrides: Partial<LogSourceRowDto> = {}
): LogSourceRowDto {
	return {
		source,
		label,
		kind: 'file',
		exists: true,
		sizeBytes: 100,
		serviceId: null,
		...overrides
	};
}

const nginxErrorRow = sourceRow({ kind: 'nginxError' }, 'nginx error log');
const phpRow = sourceRow({ kind: 'phpFpm', major: '8.4' }, 'PHP 8.4 pool log');

function row(text: string, level: LogRowDto['level'] = 'info'): LogRowDto {
	return { level, text };
}

function windowOf(overrides: Partial<LogWindowDto> = {}): LogWindowDto {
	return {
		rows: [],
		cursor: 'c0',
		exists: true,
		reset: null,
		hasMore: false,
		sizeBytes: 1000,
		scannedBytes: 100,
		truncatedLines: 0,
		scanBoundReached: false,
		...overrides
	};
}

function api(overrides: Partial<Record<string, unknown>> = {}): LogsApi {
	return {
		listLogSources: vi.fn(async () => [nginxErrorRow, phpRow]),
		readLogWindow: vi.fn(async () => windowOf()),
		revealLogFolder: vi.fn(async () => {}),
		...overrides
	} as unknown as LogsApi;
}

describe('LogsStore.loadSources', () => {
	it('fills sources on success', async () => {
		const store = new LogsStore(api());
		await store.loadSources();
		expect(store.sources).toEqual([nginxErrorRow, phpRow]);
		expect(store.sourcesError).toBeNull();
	});

	it('captures a failure on sourcesError instead of throwing', async () => {
		const store = new LogsStore(
			api({
				listLogSources: vi.fn(async () => {
					throw { kind: 'proc', message: 'catalogue unavailable' };
				})
			})
		);
		await expect(store.loadSources()).resolves.toBeUndefined();
		expect(store.sourcesError).toEqual({ kind: 'proc', message: 'catalogue unavailable' });
		expect(store.sources).toEqual([]);
	});
});

describe('LogsStore.selectFromDeepLink', () => {
	it('selects the requested source when it is in the catalogue', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.loadSources();
		await store.selectFromDeepLink({ kind: 'phpFpm', major: '8.4' });
		expect(store.selected).toEqual({ kind: 'phpFpm', major: '8.4' });
		expect(store.requestedUnavailable).toBeNull();
		expect(a.readLogWindow).toHaveBeenCalledWith(
			expect.objectContaining({ source: { kind: 'phpFpm', major: '8.4' } })
		);
	});

	// The point of the deep-link feature: a stale/garbage link must not crash
	// or silently substitute unrelated content — it names what was requested
	// and leaves the picker for the user, per `logBodyState`'s 'unavailable'
	// precedence (logs.derive.ts).
	it('flags an unlisted requested source as unavailable and selects nothing', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.loadSources();
		await store.selectFromDeepLink({ kind: 'phpFpm', major: '8.1' });
		expect(store.requestedUnavailable).toEqual({ kind: 'phpFpm', major: '8.1' });
		expect(store.selected).toBeNull();
		expect(a.readLogWindow).not.toHaveBeenCalled();
	});

	it('falls back to nginx error log when nothing was requested', async () => {
		const store = new LogsStore(api());
		await store.loadSources();
		await store.selectFromDeepLink(null);
		expect(store.selected).toEqual({ kind: 'nginxError' });
		expect(store.requestedUnavailable).toBeNull();
	});
});

describe('LogsStore.selectSource', () => {
	it('resets prior state and loads a fresh window', async () => {
		const a = api({
			readLogWindow: vi
				.fn<() => Promise<LogWindowDto>>()
				.mockResolvedValueOnce(windowOf({ rows: [row('old')], cursor: 'c1' }))
				.mockResolvedValueOnce(windowOf({ rows: [row('new')], cursor: 'c2' }))
		});
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		expect(store.rows.map((r) => r.text)).toEqual(['old']);

		await store.selectSource({ kind: 'phpFpm', major: '8.4' });
		expect(store.rows.map((r) => r.text)).toEqual(['new']);
		expect(store.follow).toBe(true);
		expect(store.newRowsWhilePaused).toBe(false);
	});

	it('clears a stale requestedUnavailable once a real selection is made', async () => {
		const store = new LogsStore(api());
		await store.loadSources();
		await store.selectFromDeepLink({ kind: 'phpFpm', major: '8.1' }); // unavailable
		expect(store.requestedUnavailable).not.toBeNull();
		await store.selectSource({ kind: 'nginxError' });
		expect(store.requestedUnavailable).toBeNull();
	});
});

describe('LogsStore cursor / append / reset (spec D3: never double-print)', () => {
	it('appends rows across polls using the cursor the server returned', async () => {
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockResolvedValueOnce(windowOf({ rows: [row('a')], cursor: 'c1' }))
			.mockResolvedValueOnce(windowOf({ rows: [row('b')], cursor: 'c2' }));
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'nginxError' });
		await store.refresh();

		expect(store.rows.map((r) => r.text)).toEqual(['a', 'b']);
		expect(readLogWindow).toHaveBeenLastCalledWith(expect.objectContaining({ cursor: 'c1' }));
	});

	// The exact bug this contract exists to prevent: a `reset` window's rows
	// are a FRESH tail, not a continuation — appending them onto what is
	// already on screen can print the same lines twice.
	it('a reset REPLACES accumulated rows rather than appending them', async () => {
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockResolvedValueOnce(windowOf({ rows: [row('a'), row('b')], cursor: 'c1' }))
			.mockResolvedValueOnce(
				windowOf({ rows: [row('a'), row('b'), row('c')], cursor: 'c2', reset: 'rotated' })
			);
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'nginxError' });
		await store.refresh();

		expect(store.rows.map((r) => r.text)).toEqual(['a', 'b', 'c']);
		expect(store.reset).toBe('rotated');
	});

	it('exists:false clears rows rather than leaving stale content on screen', async () => {
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockResolvedValueOnce(windowOf({ rows: [row('a')], cursor: 'c1' }))
			.mockResolvedValueOnce(windowOf({ rows: [], cursor: null, exists: false }));
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'siteError', domain: 'gone.localhost' });
		await store.refresh();

		expect(store.rows).toEqual([]);
		expect(store.exists).toBe(false);
	});

	it('caps accumulated rows at MAX_RENDERED_ROWS, evicting the oldest first', async () => {
		expect(MAX_RENDERED_ROWS).toBe(500);
		const first = windowOf({
			rows: Array.from({ length: 500 }, (_, i) => row(`l${i}`)),
			cursor: 'c1'
		});
		const second = windowOf({ rows: [row('overflow')], cursor: 'c2' });
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockResolvedValueOnce(first)
			.mockResolvedValueOnce(second);
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'nginxError' });
		await store.refresh();

		expect(store.rows).toHaveLength(500);
		expect(store.rows[0]?.text).toBe('l1'); // l0 evicted
		expect(store.rows[499]?.text).toBe('overflow');
	});

	it('reflects size/scan facts from the latest window', async () => {
		const store = new LogsStore(
			api({
				readLogWindow: vi.fn(async () =>
					windowOf({
						sizeBytes: 12345,
						scannedBytes: 999,
						truncatedLines: 2,
						scanBoundReached: true
					})
				)
			})
		);
		await store.selectSource({ kind: 'nginxError' });
		expect(store.sizeBytes).toBe(12345);
		expect(store.scannedBytes).toBe(999);
		expect(store.truncatedLines).toBe(2);
		expect(store.scanBoundReached).toBe(true);
	});
});

describe('LogsStore follow toggling', () => {
	it('setFollow(false) turns follow off without touching rows', async () => {
		const store = new LogsStore(api());
		await store.selectSource({ kind: 'nginxError' });
		store.setFollow(false);
		expect(store.follow).toBe(false);
	});

	// This is the property the "Jump to latest" affordance depends on: rows
	// keep arriving in the background while the user is reading history, and
	// the store only needs to REMEMBER that they arrived — the viewport's
	// own auto-scroll decision is the component's job (DOM-only, manual
	// click-list), not this store's.
	it('flags newRowsWhilePaused when a poll delivers rows while follow is off', async () => {
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockResolvedValueOnce(windowOf({ rows: [row('seed')], cursor: 'c1' }))
			.mockResolvedValueOnce(windowOf({ rows: [row('while-paused')], cursor: 'c2' }));
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'nginxError' });
		store.setFollow(false);
		await store.refresh();
		expect(store.newRowsWhilePaused).toBe(true);
	});

	it('does not flag newRowsWhilePaused while following', async () => {
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockResolvedValueOnce(windowOf({ rows: [row('seed')], cursor: 'c1' }))
			.mockResolvedValueOnce(windowOf({ rows: [row('more')], cursor: 'c2' }));
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'nginxError' });
		await store.refresh(); // still following
		expect(store.newRowsWhilePaused).toBe(false);
	});

	it('jumpToLatest turns follow back on, clears the flag, and refreshes', async () => {
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockResolvedValueOnce(windowOf({ rows: [row('seed')], cursor: 'c1' }))
			.mockResolvedValueOnce(windowOf({ rows: [row('paused')], cursor: 'c2' }))
			.mockResolvedValueOnce(windowOf({ rows: [row('caught-up')], cursor: 'c3' }));
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'nginxError' });
		store.setFollow(false);
		await store.refresh();
		expect(store.newRowsWhilePaused).toBe(true);

		await store.jumpToLatest();
		expect(store.follow).toBe(true);
		expect(store.newRowsWhilePaused).toBe(false);
		expect(store.rows.map((r) => r.text)).toEqual(['seed', 'paused', 'caught-up']);
	});
});

describe('LogsStore filter round-trip', () => {
	it('setNeedle sends the needle on the next call and restarts from a fresh cursor', async () => {
		const a = api({
			readLogWindow: vi
				.fn<() => Promise<LogWindowDto>>()
				.mockResolvedValueOnce(windowOf({ rows: [row('a')], cursor: 'c1' }))
				.mockResolvedValueOnce(windowOf({ rows: [row('match')], cursor: 'c2' }))
		});
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		await store.setNeedle('ERROR 500');
		expect(a.readLogWindow).toHaveBeenLastCalledWith(
			expect.objectContaining({ needle: 'ERROR 500', cursor: null })
		);
		expect(store.rows.map((r) => r.text)).toEqual(['match']); // replaced, not appended
	});

	it('an empty needle is sent as null, not an empty string', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		await store.setNeedle('');
		expect(a.readLogWindow).toHaveBeenLastCalledWith(expect.objectContaining({ needle: null }));
	});

	it('setNeedle truncates an over-long value to the 256-byte cap before storing or sending it', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		await store.setNeedle('a'.repeat(500));
		expect(store.needle).toHaveLength(256);
		expect(a.readLogWindow).toHaveBeenLastCalledWith(
			expect.objectContaining({ needle: 'a'.repeat(256) })
		);
	});

	it('setCaseSensitive round-trips through the api', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		await store.setCaseSensitive(true);
		expect(a.readLogWindow).toHaveBeenLastCalledWith(
			expect.objectContaining({ caseSensitive: true })
		);
	});

	it('setMinLevel round-trips through the api', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		await store.setMinLevel('warn');
		expect(a.readLogWindow).toHaveBeenLastCalledWith(expect.objectContaining({ minLevel: 'warn' }));
	});
});

describe('LogsStore stale-response guard', () => {
	// Without this guard, a slow response for the SOURCE the user just
	// navigated away from could resolve after a new selection and get
	// merged into the new source's rows — cross-source contamination that
	// looks exactly like a double-print bug but has a different cause.
	it('discards a read response for a selection that has since been superseded', async () => {
		let resolveFirst: (w: LogWindowDto) => void = () => {};
		const first = new Promise<LogWindowDto>((resolve) => {
			resolveFirst = resolve;
		});
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockReturnValueOnce(first)
			.mockResolvedValueOnce(windowOf({ rows: [row('second-source')], cursor: 'c2' }));
		const store = new LogsStore(api({ readLogWindow }));

		const firstSelect = store.selectSource({ kind: 'nginxError' });
		const secondSelect = store.selectSource({ kind: 'phpFpm', major: '8.4' });
		await secondSelect;
		expect(store.rows.map((r) => r.text)).toEqual(['second-source']);

		// The slow first request now resolves — its rows must NOT retroactively
		// land on top of (or alongside) the second source's already-rendered rows.
		resolveFirst(windowOf({ rows: [row('first-source-late')], cursor: 'c1' }));
		await firstSelect;
		expect(store.rows.map((r) => r.text)).toEqual(['second-source']);
	});
});

describe('LogsStore.start/stop polling (teardown is a tested requirement)', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('the interval is exactly 500ms', () => {
		expect(POLL_INTERVAL_MS).toBe(500);
	});

	it('polls on the configured interval while started', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		const before = (a.readLogWindow as ReturnType<typeof vi.fn>).mock.calls.length;

		store.start();
		await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 3);
		store.stop();

		expect((a.readLogWindow as ReturnType<typeof vi.fn>).mock.calls.length - before).toBe(3);
	});

	// The whole point of a tested teardown: an app left open all day must not
	// keep polling a page the user is no longer on (an "orphaned interval is
	// a permanent battery wakeup" — spec D3).
	it('issues no further calls after stop — the route-unmount contract', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		store.start();
		await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
		const before = (a.readLogWindow as ReturnType<typeof vi.fn>).mock.calls.length;

		store.stop();
		await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 5);

		expect((a.readLogWindow as ReturnType<typeof vi.fn>).mock.calls.length).toBe(before);
	});

	it('is idempotent — a second start() does not double the polling rate', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		const before = (a.readLogWindow as ReturnType<typeof vi.fn>).mock.calls.length;

		store.start();
		store.start();
		await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 2);
		store.stop();

		expect((a.readLogWindow as ReturnType<typeof vi.fn>).mock.calls.length - before).toBe(2);
	});

	it('stop() is safe to call when never started', () => {
		const store = new LogsStore(api());
		expect(() => store.stop()).not.toThrow();
	});
});

describe('LogsStore.revealFolder', () => {
	it('reveals the folder for the selected source', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		await store.revealFolder();
		expect(a.revealLogFolder).toHaveBeenCalledWith({ kind: 'nginxError' });
	});

	it('is a no-op when nothing is selected', async () => {
		const a = api();
		const store = new LogsStore(a);
		await store.revealFolder();
		expect(a.revealLogFolder).not.toHaveBeenCalled();
	});

	it('captures a failure without throwing', async () => {
		const a = api({
			revealLogFolder: vi.fn(async () => {
				throw { kind: 'proc', message: 'opener unavailable' };
			})
		});
		const store = new LogsStore(a);
		await store.selectSource({ kind: 'nginxError' });
		await expect(store.revealFolder()).resolves.toBeUndefined();
		expect(store.readError).toEqual({ kind: 'proc', message: 'opener unavailable' });
	});
});

describe('LogsStore read failures', () => {
	it('a failed refresh lands on readError without throwing', async () => {
		const store = new LogsStore(
			api({
				readLogWindow: vi.fn(async () => {
					throw { kind: 'core', message: 'boom' };
				})
			})
		);
		await expect(store.selectSource({ kind: 'nginxError' })).resolves.toBeUndefined();
		expect(store.readError).toEqual({ kind: 'core', message: 'boom' });
	});

	it('a later successful refresh clears a previous readError', async () => {
		const readLogWindow = vi
			.fn<() => Promise<LogWindowDto>>()
			.mockRejectedValueOnce({ kind: 'core', message: 'boom' })
			.mockResolvedValueOnce(windowOf());
		const store = new LogsStore(api({ readLogWindow }));
		await store.selectSource({ kind: 'nginxError' });
		expect(store.readError).not.toBeNull();
		await store.refresh();
		expect(store.readError).toBeNull();
	});
});
