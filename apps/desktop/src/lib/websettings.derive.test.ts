// SPDX-License-Identifier: GPL-3.0-or-later
//
// The two conversions that stand between a typed value and the backend: the
// camelCase→snake_case error key (get it wrong and a rejected field marks
// nothing) and the string→number input conversion (get it wrong and specta
// rejects the whole payload with an error naming no field at all).
import { describe, expect, it } from 'vitest';
import type { WebServerSettingsDto } from './ipc';
import {
	errorKey,
	parseCount,
	sameSettings,
	PHASE3_FIELDS,
	PHASE3_REASON
} from './websettings.derive';

/**
 * Every field name as `WebServerSettingsDto` (camelCase, the wire form) →
 * `IpcError::Validation.field` (snake_case, what `ConfError::InvalidField`
 * carries). Written out by hand from `crates/openvhost-conf/src/settings/value.rs`
 * and `commands.rs`'s `seconds_field` calls rather than generated from the same
 * rule the implementation uses — a test that reapplied `errorKey`'s own regex
 * could not catch `errorKey` being wrong.
 */
const BACKEND_NAMES: Record<keyof WebServerSettingsDto, string> = {
	workerConnections: 'worker_connections',
	clientMaxBodySize: 'client_max_body_size',
	keepaliveTimeout: 'keepalive_timeout',
	tcpNodelay: 'tcp_nodelay',
	fastcgiConnectTimeout: 'fastcgi_connect_timeout',
	fastcgiSendTimeout: 'fastcgi_send_timeout',
	fastcgiReadTimeout: 'fastcgi_read_timeout',
	gzip: 'gzip',
	gzipCompLevel: 'gzip_comp_level',
	gzipTypes: 'gzip_types'
};

const stored: WebServerSettingsDto = {
	workerConnections: 1024,
	clientMaxBodySize: '256m',
	keepaliveTimeout: 65,
	tcpNodelay: true,
	fastcgiConnectTimeout: 60,
	fastcgiSendTimeout: 300,
	fastcgiReadTimeout: 300,
	gzip: false,
	gzipCompLevel: 1,
	gzipTypes: 'text/plain text/css'
};

describe('errorKey', () => {
	// Table-driven so the failure names the field that drifted.
	for (const [dtoKey, backendName] of Object.entries(BACKEND_NAMES)) {
		it(`maps ${dtoKey} to ${backendName}`, () => {
			expect(errorKey(dtoKey as keyof WebServerSettingsDto)).toBe(backendName);
		});
	}

	it('covers every field the DTO carries', () => {
		// Guards the table above: a field added to `WebServerSettingsDto` without a
		// row here would otherwise be silently untested, and an unmapped field is
		// exactly the one whose validation error marks nothing.
		expect(Object.keys(BACKEND_NAMES).sort()).toEqual(Object.keys(stored).sort());
	});
});

describe('parseCount', () => {
	it('converts a number input string to a number', () => {
		// The trap: `"2048"` handed to a `u32` field fails inside specta, not
		// inside validation, so the user gets a transport error naming no field.
		expect(parseCount('2048')).toBe(2048);
		expect(typeof parseCount('2048')).toBe('number');
	});

	it('tolerates surrounding whitespace', () => {
		expect(parseCount(' 12 ')).toBe(12);
	});

	it('refuses an empty box rather than sending zero', () => {
		// A cleared input must not silently become `0`, which every one of these
		// settings rejects anyway — and would look like the user chose it.
		expect(parseCount('')).toBeNull();
		expect(parseCount('   ')).toBeNull();
	});

	it('refuses anything that is not a whole positive number', () => {
		for (const raw of ['abc', '1.5', '-1', '1e3', '12px', '٣']) {
			expect(parseCount(raw)).toBeNull();
		}
	});

	it('refuses a value too long to be a real setting', () => {
		// Ten digits is past `u32`; rejecting here keeps the overflow out of the
		// payload rather than letting serde reject the whole form.
		expect(parseCount('12345678901')).toBeNull();
	});
});

describe('sameSettings', () => {
	it('is true for an untouched copy', () => {
		expect(sameSettings(stored, { ...stored })).toBe(true);
	});

	it('notices a change in any single field', () => {
		// One case per field, so a field left out of the comparison fails here
		// rather than leaving the form permanently clean after an edit.
		const changed: WebServerSettingsDto[] = [
			{ ...stored, workerConnections: 2048 },
			{ ...stored, clientMaxBodySize: '512m' },
			{ ...stored, keepaliveTimeout: 30 },
			{ ...stored, tcpNodelay: false },
			{ ...stored, fastcgiConnectTimeout: 30 },
			{ ...stored, fastcgiSendTimeout: 30 },
			{ ...stored, fastcgiReadTimeout: 900 },
			{ ...stored, gzip: true },
			{ ...stored, gzipCompLevel: 6 },
			{ ...stored, gzipTypes: 'text/html' }
		];
		for (const other of changed) {
			expect(sameSettings(stored, other)).toBe(false);
		}
	});
});

describe('the Phase 3 fields', () => {
	it('names every control ServBay offers that we cannot honour yet', () => {
		expect(PHASE3_FIELDS.map((f) => f.id)).toEqual([
			'http-port',
			'https-port',
			'ssl-protocol',
			'ssl-prefer-server-ciphers',
			'http2',
			'http3'
		]);
	});

	it('carries no value, only a placeholder', () => {
		// A port number rendered as a real value would be a claim about how the
		// server is listening today — a fact this page does not have.
		for (const field of PHASE3_FIELDS) {
			expect(field).not.toHaveProperty('value');
		}
	});

	it('states why, not just that it is disabled', () => {
		expect(PHASE3_REASON).toMatch(/privileged helper/);
		expect(PHASE3_REASON).toMatch(/Phase 3/);
	});
});
