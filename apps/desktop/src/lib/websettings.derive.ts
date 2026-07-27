// SPDX-License-Identifier: GPL-3.0-or-later
//
// Pure helpers and field metadata for the Web server settings form. No runes,
// no IPC — everything here is a plain function or a constant, so it is
// testable on its own and the form component stays markup.
//
// It is also where every user-visible label, hint and unit for these fields
// lives: one table to hand to the i18n layer in Phase 2, rather than ten
// strings scattered through markup.
import type { WebServerSettingsDto } from './ipc';

export type SettingKey = keyof WebServerSettingsDto;

/** Settings the form edits with a number input. */
export type NumberKey =
	| 'workerConnections'
	| 'keepaliveTimeout'
	| 'fastcgiConnectTimeout'
	| 'fastcgiSendTimeout'
	| 'fastcgiReadTimeout'
	| 'gzipCompLevel';

/** Settings the form edits with a switch. */
export type BoolKey = 'tcpNodelay' | 'gzip';

/** Settings the form edits with an input or textarea. */
export type TextKey = 'clientMaxBodySize' | 'gzipTypes';

/**
 * The BACKEND's name for a setting — snake_case — from the camelCase name the
 * DTO uses on the wire.
 *
 * This is the whole reason this function exists rather than the form indexing
 * `fieldErrors` with the DTO key directly. `IpcError::Validation.field` is
 * `fastcgi_read_timeout`, the DTO field is `fastcgiReadTimeout`, and a
 * camelCase lookup would find nothing: the input would not be marked, no
 * message would appear, and a rejected save would read to the user as a save
 * that silently did nothing. `SiteDrawer.svelte` already keys its own
 * `fieldErrors` off the same snake_case names (`web_server`, `php_version`) —
 * this keeps one convention instead of inventing a second.
 *
 * Derived rather than tabulated because `#[serde(rename_all = "camelCase")]`
 * is exactly this transformation in reverse; `websettings.derive.test.ts`
 * pins every one of the ten names against the Rust struct so a drift fails a
 * test rather than a click.
 */
export function errorKey(key: SettingKey): string {
	return key.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
}

/**
 * A whole, non-negative count from an `<input type="number">`'s value.
 *
 * `input.value` is a STRING even when `type="number"`, and `WebServerSettingsDto`'s
 * numeric fields are `u32` on the Rust side. Handing `"2048"` straight to
 * `save_web_server_settings` fails in specta's deserializer, which surfaces as a
 * transport-shaped error naming no field at all — not the friendly "must be
 * between 1 and 65535" the user should get. So the conversion happens here, at
 * the input boundary.
 *
 * `null` means "not a number I can send": an empty box (the user cleared it
 * mid-edit), or anything with a sign, a decimal point or stray text. The store
 * turns that into a field error and leaves the last good value in place, rather
 * than storing `NaN` — which `JSON.stringify` would quietly turn into `null` on
 * the wire.
 */
export function parseCount(raw: string): number | null {
	const trimmed = raw.trim();
	if (!/^\d{1,9}$/.test(trimmed)) return null;
	const parsed = Number(trimmed);
	return Number.isSafeInteger(parsed) ? parsed : null;
}

/** Whether two settings snapshots hold the same values, field by field. Used
 * for the form's dirty flag — a key-order-independent comparison, unlike
 * `JSON.stringify`. */
export function sameSettings(a: WebServerSettingsDto, b: WebServerSettingsDto): boolean {
	return (
		a.workerConnections === b.workerConnections &&
		a.clientMaxBodySize === b.clientMaxBodySize &&
		a.keepaliveTimeout === b.keepaliveTimeout &&
		a.tcpNodelay === b.tcpNodelay &&
		a.fastcgiConnectTimeout === b.fastcgiConnectTimeout &&
		a.fastcgiSendTimeout === b.fastcgiSendTimeout &&
		a.fastcgiReadTimeout === b.fastcgiReadTimeout &&
		a.gzip === b.gzip &&
		a.gzipCompLevel === b.gzipCompLevel &&
		a.gzipTypes === b.gzipTypes
	);
}

export interface NumberFieldSpec {
	key: NumberKey;
	label: string;
	hint: string;
	/** Mirrors the Rust newtype's own range (`WorkerConnections`, `Seconds`,
	 * `GzipLevel`) so the browser's spinner stops where the parser does. It is
	 * an affordance, not the authority: `parse` on the Rust side still decides,
	 * and a value typed past these bounds comes back as a marked field. */
	min: number;
	max: number;
	/** Rendered as an input suffix, the way `SiteDrawer`'s domain field renders
	 * `.localhost` — the unit belongs beside the number, not buried in a hint. */
	unit?: string;
}

/** Seconds range shared by `keepalive_timeout` and the three FastCGI timeouts —
 * `Seconds::parse` accepts 1..=86400 for all four. */
const SECONDS_MIN = 1;
const SECONDS_MAX = 86400;

export const CONNECTION_NUMBERS: readonly NumberFieldSpec[] = [
	{
		key: 'workerConnections',
		label: 'Worker connections',
		hint: 'How many connections one nginx worker will handle at once. 1–65535.',
		min: 1,
		max: 65535
	}
];

export const TIMEOUT_NUMBERS: readonly NumberFieldSpec[] = [
	{
		key: 'keepaliveTimeout',
		label: 'Keepalive timeout',
		hint: 'How long an idle connection stays open. 1–86400.',
		min: SECONDS_MIN,
		max: SECONDS_MAX,
		unit: 'seconds'
	},
	{
		key: 'fastcgiConnectTimeout',
		label: 'FastCGI connect timeout',
		hint: 'How long nginx waits for PHP-FPM to accept a connection. A long value here hides a dead pool. 1–86400.',
		min: SECONDS_MIN,
		max: SECONDS_MAX,
		unit: 'seconds'
	},
	{
		key: 'fastcgiSendTimeout',
		label: 'FastCGI send timeout',
		hint: 'How long nginx waits while sending the request to PHP-FPM. 1–86400.',
		min: SECONDS_MIN,
		max: SECONDS_MAX,
		unit: 'seconds'
	},
	{
		key: 'fastcgiReadTimeout',
		label: 'FastCGI read timeout',
		hint: 'How long nginx waits for PHP to answer. Raise it if requests paused on a debugger breakpoint get cut off. 1–86400.',
		min: SECONDS_MIN,
		max: SECONDS_MAX,
		unit: 'seconds'
	}
];

export const GZIP_LEVEL: NumberFieldSpec = {
	key: 'gzipCompLevel',
	label: 'Compression level',
	hint: '1 is fastest, 9 compresses hardest. Only has an effect while gzip is on.',
	min: 1,
	max: 9
};

/**
 * The controls ServBay offers that OpenVHost cannot honour yet (design §3).
 *
 * They are rendered, disabled, with one shared reason, rather than left out: a
 * missing field looks like an oversight, a disabled one with a reason tells the
 * user the product knows. None of them carries a value — a number in a dead
 * port box would be a claim about how the server is listening today, and this
 * page has no such fact to state. The placeholder shows the value the field
 * WILL take, as ghost text in an inert control.
 */
export interface Phase3FieldSpec {
	/** Also the `data-testid` suffix and the input id. */
	id: string;
	label: string;
	kind: 'number' | 'text' | 'switch';
	placeholder?: string;
}

export const PHASE3_FIELDS: readonly Phase3FieldSpec[] = [
	{ id: 'http-port', label: 'HTTP port', kind: 'number', placeholder: '80' },
	{ id: 'https-port', label: 'HTTPS port', kind: 'number', placeholder: '443' },
	{ id: 'ssl-protocol', label: 'SSL protocol', kind: 'text', placeholder: 'TLSv1.2 TLSv1.3' },
	{ id: 'ssl-prefer-server-ciphers', label: 'Prefer server ciphers', kind: 'switch' },
	{ id: 'http2', label: 'HTTP/2', kind: 'switch' },
	{ id: 'http3', label: 'HTTP/3', kind: 'switch' }
];

export const PHASE3_REASON =
	'Not editable yet — binding ports 80 and 443 needs the privileged helper, and HTTPS needs the local certificate authority. Both arrive in Phase 3; sites are served over plain HTTP until then.';

/** Shown under a number input whose content is not a whole number — the store
 * keeps the last good value rather than sending `NaN`. */
export const NOT_A_NUMBER = 'Enter a whole number.';
