// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers for the Logs UI (P1 live-log-viewer design, spec D6/D7:
// docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md). No IPC, no
// Svelte state, no `$app/paths` — every function here takes plain data and
// returns plain data, so the store, the deep-link codec and the grouped
// picker's logic are all testable without a component or a fake api.
// `resolve('/logs')` itself is deliberately NOT wrapped in here: eslint's
// `svelte/no-navigation-without-resolve` inspects an `href` attribute's own
// expression and cannot see through an imported helper's function body, so
// each `href`-building call site keeps its own visible `resolve(...)` call
// (`logSourceQuery` below hands it the query string to append).
import type { IpcError, LogLevel, LogSourceDto, LogSourceRowDto } from './ipc';

/** Spec D3's cap on a filter query, mirrored here so the UI can bound the
 *  input BEFORE it ever reaches IPC (`openvhost_core::logs::LogQuery::needle`
 *  applies no bound of its own; `LogNeedle::parse` at the command layer is
 *  the actual enforcement). Kept in sync by convention, not by a shared
 *  constant across the IPC boundary — Rust does not export it. */
export const LOG_NEEDLE_MAX_BYTES = 256;

/**
 * Truncate `s` to at most `maxBytes` UTF-8 bytes without splitting a
 * multi-byte character in half.
 *
 * A plain `<input maxlength>` counts UTF-16 code units, not bytes — this app
 * ships Thai UI text from Phase 2 (CLAUDE.md), and a Thai character is 3
 * bytes in UTF-8, so a 256-CHARACTER filter could be well over 256 BYTES and
 * still slip past a naive length check while the server's `LogNeedle::parse`
 * (spec D3) rejects it anyway. This is the client-side half of "bound the
 * input" (the other half is `LogsStore.setNeedle` calling this before
 * storing/sending anything).
 */
export function truncateToUtf8Bytes(s: string, maxBytes: number): string {
	const bytes = new TextEncoder().encode(s);
	if (bytes.length <= maxBytes) return s;
	let end = maxBytes;
	// Back off until `end` does not land inside a multi-byte sequence: a UTF-8
	// continuation byte always has its top two bits set to `10`.
	while (end > 0 && (bytes[end] & 0xc0) === 0x80) end -= 1;
	return new TextDecoder().decode(bytes.subarray(0, end));
}

/** Spec D8's threshold for the status line's on-disk-growth warning. A UI
 *  constant, not a mirror of a Rust one — the server sends only `sizeBytes`
 *  and never a threshold. */
export const SIZE_WARNING_BYTES = 100 * 1024 * 1024;

/** `1536 -> "1.50 KiB"`. Binary units throughout (KiB/MiB/GiB), matching how
 *  the spec itself talks about the limits (512 KiB payload, 16 MiB scan,
 *  100 MiB warning). */
export function formatBytes(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	const units = ['KiB', 'MiB', 'GiB', 'TiB'];
	let value = bytes / 1024;
	let unitIndex = 0;
	while (value >= 1024 && unitIndex < units.length - 1) {
		value /= 1024;
		unitIndex += 1;
	}
	return `${value.toFixed(value < 10 ? 2 : 1)} ${units[unitIndex]}`;
}

/**
 * The ONE level → CSS-class mapping for a file-backed log row, extracted
 * from `LogPane.svelte`'s previously-inline `levelClass` (spec D6: "the row
 * renderer is extracted so level colours cannot drift between the two
 * surfaces"). `LogPane` (ring rows) and `LogBody` (file rows) both render
 * through `LogLevelBadge.svelte`, which calls this — a level's colour is
 * therefore a type-checked fact in ONE place, not two components that could
 * quietly disagree.
 */
export function levelClass(level: LogLevel): string {
	return level === 'error' ? 'lvl-error' : level === 'warn' ? 'lvl-warn' : 'lvl-info';
}

/** Structural equality for the closed `LogSourceDto` union — object identity
 *  is useless here since every DTO crossing IPC is freshly deserialized.
 *  Exhaustive over `kind` with no `default:`, so a fifth/sixth variant added
 *  later fails typecheck here instead of silently comparing `true`. */
export function sameSource(a: LogSourceDto, b: LogSourceDto): boolean {
	switch (a.kind) {
		case 'nginxError':
		case 'nginxAccess':
			return b.kind === a.kind;
		case 'phpFpm':
			return b.kind === 'phpFpm' && b.major === a.major;
		case 'siteAccess':
			return b.kind === 'siteAccess' && b.domain === a.domain;
		case 'siteError':
			return b.kind === 'siteError' && b.domain === a.domain;
		case 'serviceRing':
			return b.kind === 'serviceRing' && b.id === a.id;
	}
}

/** The domain a site-scoped source names, or `null` for every other kind
 *  (including `source === null`) — "then the stream" (spec D6) reads this to
 *  know whether the Access/Error toggle applies at all. */
export function sourceDomain(source: LogSourceDto | null): string | null {
	if (source === null) return null;
	return source.kind === 'siteAccess' || source.kind === 'siteError' ? source.domain : null;
}

/** Which of a site's two streams `source` names, or `null` for a non-site
 *  source. */
export function sourceStream(source: LogSourceDto | null): 'access' | 'error' | null {
	if (source === null) return null;
	if (source.kind === 'siteAccess') return 'access';
	if (source.kind === 'siteError') return 'error';
	return null;
}

/** Build the `LogSourceDto` for `domain`'s given stream — the inverse of
 *  `sourceDomain`/`sourceStream` combined, used by the picker's domain chip
 *  and the toolbar's stream toggle. */
export function siteSource(domain: string, stream: 'access' | 'error'): LogSourceDto {
	return stream === 'access' ? { kind: 'siteAccess', domain } : { kind: 'siteError', domain };
}

/**
 * A human label for a source that is NOT (or is no longer) in the
 * catalogue — the "unavailable source" state has no `LogSourceRowDto.label`
 * to show, since the whole point is that no such row exists. Mirrors the
 * server's own `list_log_sources` labels (`commands.rs`) for the common
 * cases; a `serviceRing` source falls back to the bare id since this side
 * has no `display_name` for a service it does not currently know about.
 */
export function describeSource(source: LogSourceDto): string {
	switch (source.kind) {
		case 'nginxError':
			return 'nginx error log';
		case 'nginxAccess':
			return 'nginx access log';
		case 'phpFpm':
			return `PHP ${source.major} pool log`;
		case 'siteAccess':
			return `${source.domain} access log`;
		case 'siteError':
			return `${source.domain} error log`;
		case 'serviceRing':
			return `${source.id} output`;
	}
}

/** Everything `list_log_sources` returned, split into the two picker groups
 *  (spec D6): every non-site row stays flat under "Services" in the SAME
 *  order the server listed them; site rows collapse to their distinct
 *  domains (dropping the access/error split — that becomes the toolbar's
 *  stream toggle, "then the stream"), sorted for a scan-friendly list
 *  regardless of insertion order in state.db. */
export interface GroupedSources {
	services: readonly LogSourceRowDto[];
	siteDomains: readonly string[];
}

export function groupSources(sources: readonly LogSourceRowDto[]): GroupedSources {
	const services = sources.filter(
		(s) => s.source.kind !== 'siteAccess' && s.source.kind !== 'siteError'
	);
	const domains = new Set<string>();
	for (const s of sources) {
		const domain = sourceDomain(s.source);
		if (domain !== null) domains.add(domain);
	}
	return { services, siteDomains: [...domains].sort() };
}

/** Whether `source` names a row genuinely present in `sources` right now —
 *  the client-side half of the "unavailable source" state (spec D6): a
 *  deep-linked site that was deleted, or a PHP major that was uninstalled,
 *  is caught here before any read is even attempted. */
export function isSourceListed(sources: readonly LogSourceRowDto[], source: LogSourceDto): boolean {
	return sources.some((s) => sameSource(s.source, source));
}

// ---- deep-link codec (?source=…) ------------------------------------------
//
// A small, stable, hand-written string encoding — not `JSON.stringify`,
// which would produce an unreadable, key-order-fragile blob in the address
// bar for what is meant to be a shareable link.

/** Exported for `logsHref` (the query-param value) AND `LogSourcePicker`
 *  (a stable, unique chip `data-testid`) — one stable string identity for
 *  a source, reused rather than duplicated for its second purpose. */
export function encodeLogSource(source: LogSourceDto): string {
	switch (source.kind) {
		case 'nginxError':
			return 'nginx-error';
		case 'nginxAccess':
			return 'nginx-access';
		case 'phpFpm':
			return `php-fpm:${source.major}`;
		case 'siteAccess':
			return `site-access:${source.domain}`;
		case 'siteError':
			return `site-error:${source.domain}`;
		case 'serviceRing':
			return `service-ring:${source.id}`;
	}
}

/**
 * The inverse of `encodeLogSource`. Only the FIRST `:` splits tag from
 * value, so a value that itself contains `:` (not expected for a `Domain`
 * or a service id today, but not this function's job to assume) still
 * round-trips. Returns `null` for anything unrecognized rather than
 * throwing — a stale or hand-edited link must degrade to "no source
 * requested", never crash the page.
 */
export function decodeLogSource(raw: string): LogSourceDto | null {
	if (raw === 'nginx-error') return { kind: 'nginxError' };
	if (raw === 'nginx-access') return { kind: 'nginxAccess' };
	const sep = raw.indexOf(':');
	if (sep === -1) return null;
	const tag = raw.slice(0, sep);
	const value = raw.slice(sep + 1);
	if (value === '') return null;
	switch (tag) {
		case 'php-fpm':
			return { kind: 'phpFpm', major: value };
		case 'site-access':
			return { kind: 'siteAccess', domain: value };
		case 'site-error':
			return { kind: 'siteError', domain: value };
		case 'service-ring':
			return { kind: 'serviceRing', id: value };
		default:
			return null;
	}
}

/** Read `?source=…` out of a raw query string (`location.search`'s shape,
 *  leading `?` optional — `URLSearchParams` accepts either). `null` for "no
 *  source requested" as well as for "not parseable" — both mean the page
 *  falls back to its own default (`LogsStore.selectFromDeepLink`). */
export function parseSourceParam(search: string): LogSourceDto | null {
	const raw = new URLSearchParams(search).get('source');
	return raw === null ? null : decodeLogSource(raw);
}

/**
 * The `?source=…` query string for `source` (leading `?` included) — spec
 * D6's deep-link value, consumed by the Sites and Services row actions.
 * Deliberately does NOT resolve `/logs` itself (see this file's header):
 * each `href`-building call site does `` `${resolve('/logs')}${logSourceQuery(source)}` ``
 * so eslint's `svelte/no-navigation-without-resolve` can see the `resolve`
 * call it requires.
 */
export function logSourceQuery(source: LogSourceDto): string {
	const params = new URLSearchParams({ source: encodeLogSource(source) });
	return `?${params.toString()}`;
}

// ---- rendered body state ----------------------------------------------

/** Every distinct way the log body can render (spec D6). Exactly one is
 *  true at a time — `logBodyState` below is the ONE place that decides
 *  which. */
export type LogBodyState =
	| 'no-selection'
	| 'unavailable'
	| 'permission-denied'
	| 'error'
	| 'not-yet-created'
	| 'empty'
	| 'rows';

/**
 * Classify a failed `read_log_window` call. `openvhost_core::CoreError::Io`'s
 * `Display` embeds `std::io::Error`'s own message verbatim (`error.rs`), and
 * on a `chmod 000` file that message is exactly `"Permission denied (os
 * error 13)"` — there is no structured `IpcError` variant for this (a plain
 * `core`-kind error carries only a string), so a case-insensitive substring
 * check is the only signal available without a Rust change, which is out of
 * scope for this slice (frontend-only task). Windows is deferred (project
 * scope memo) and would say something else here; not a regression for a
 * platform this app does not run on yet.
 */
export function classifyReadError(e: IpcError | null): 'none' | 'permission' | 'other' {
	if (e === null) return 'none';
	const message = 'message' in e ? e.message : '';
	return /permission denied/i.test(message) ? 'permission' : 'other';
}

/** The ONE place body-state precedence is decided (spec D6's "distinct
 *  states"), so every renderer (SSR tests included) reads the same
 *  priority order instead of each re-deriving it. `unavailable` wins over
 *  everything, including a `selected` fallback that may already have loaded
 *  fine — see `LogsStore.selectFromDeepLink`'s doc comment for why no
 *  fallback content is ever selected behind that banner. */
export function logBodyState(input: {
	selected: LogSourceDto | null;
	requestedUnavailable: LogSourceDto | null;
	readError: IpcError | null;
	exists: boolean;
	rowCount: number;
}): LogBodyState {
	if (input.requestedUnavailable !== null) return 'unavailable';
	if (input.selected === null) return 'no-selection';
	const errKind = classifyReadError(input.readError);
	if (errKind === 'permission') return 'permission-denied';
	if (errKind === 'other') return 'error';
	if (!input.exists) return 'not-yet-created';
	if (input.rowCount === 0) return 'empty';
	return 'rows';
}
