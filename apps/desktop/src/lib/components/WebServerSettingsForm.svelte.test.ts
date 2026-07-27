// SPDX-License-Identifier: GPL-3.0-or-later
//
// SSR-rendered (`svelte/server`), so no DOM is needed and this runs in the
// existing `node` vitest project — the same approach as
// `webserver.panel.test.ts`. Clicking Save, typing in a box and the switch's
// keyboard behaviour are out of reach here and are on the PR's click-through
// list; what IS reachable is every assertion below, and they are the ones that
// have silently failed in this project before: a field error that marks
// nothing, and a control that is disabled with no explanation.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import WebServerSettingsForm from './WebServerSettingsForm.svelte';
import type { WebServerSettingsDto } from '$lib/ipc';

const STORED: WebServerSettingsDto = {
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

function dto(over: Partial<WebServerSettingsDto> = {}): WebServerSettingsDto {
	return { ...STORED, ...over };
}

function renderForm(props: Record<string, unknown> = {}): string {
	return render(WebServerSettingsForm, {
		props: {
			values: dto(),
			fieldErrors: {},
			error: '',
			saving: false,
			dirty: false,
			onNumber: () => {},
			onBool: () => {},
			onText: () => {},
			onSave: () => {},
			...props
		}
	}).body;
}

/** The whole opening tag of the element carrying `data-testid`, so an assertion
 * can look at that element's OWN attributes instead of the page's. */
function control(body: string, testId: string): string {
	const tag = body.match(new RegExp(`<[a-z]+[^>]*data-testid="${testId}"[^>]*>`));
	if (tag === null) throw new Error(`no element carries data-testid="${testId}"`);
	return tag[0];
}

/** Visible text, tags stripped and Svelte's source indentation collapsed. */
function text(body: string): string {
	return body
		.replace(/<[^>]*>/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();
}

const EDITABLE = [
	'worker_connections',
	'client_max_body_size',
	'tcp_nodelay',
	'keepalive_timeout',
	'fastcgi_connect_timeout',
	'fastcgi_send_timeout',
	'fastcgi_read_timeout',
	'gzip',
	'gzip_comp_level',
	'gzip_types'
];

describe('the settings form', () => {
	it('renders every editable setting with its stored value', () => {
		const body = renderForm({ values: dto({ fastcgiReadTimeout: 900, gzip: true }) });
		expect(body).toContain('value="900"');
		expect(body).toContain('data-testid="field-gzip"');
		// A field the form forgot to render is a setting the user cannot change
		// at all, with nothing else failing.
		for (const name of EDITABLE) {
			expect(body).toContain(`data-testid="field-${name}"`);
		}
	});

	it('shows each stored number in its own box', () => {
		const body = renderForm({
			values: dto({
				workerConnections: 2048,
				keepaliveTimeout: 30,
				fastcgiConnectTimeout: 11,
				fastcgiSendTimeout: 22,
				fastcgiReadTimeout: 33,
				gzipCompLevel: 6
			})
		});
		expect(control(body, 'field-worker_connections')).toContain('value="2048"');
		expect(control(body, 'field-keepalive_timeout')).toContain('value="30"');
		expect(control(body, 'field-fastcgi_connect_timeout')).toContain('value="11"');
		expect(control(body, 'field-fastcgi_send_timeout')).toContain('value="22"');
		expect(control(body, 'field-fastcgi_read_timeout')).toContain('value="33"');
		expect(control(body, 'field-gzip_comp_level')).toContain('value="6"');
	});

	it('renders the switches in their stored positions', () => {
		const on = renderForm({ values: dto({ gzip: true, tcpNodelay: false }) });
		expect(control(on, 'field-gzip')).toContain('aria-checked="true"');
		expect(control(on, 'field-tcp_nodelay')).toContain('aria-checked="false"');
	});

	it('renders the free-text settings, gzip types in a textarea', () => {
		const body = renderForm({ values: dto({ clientMaxBodySize: '512m' }) });
		expect(control(body, 'field-client_max_body_size')).toContain('value="512m"');
		expect(control(body, 'field-gzip_types')).toMatch(/^<textarea/);
		expect(body).toContain('text/plain text/css');
	});

	it('marks only the field that failed', () => {
		const body = renderForm({
			values: dto(),
			fieldErrors: { gzip_comp_level: 'must be between 1 and 9' }
		});
		expect(body).toMatch(/data-testid="error-gzip_comp_level"/);
		expect(body).not.toMatch(/data-testid="error-keepalive_timeout"/);
		expect(control(body, 'field-gzip_comp_level')).toContain('aria-invalid="true"');
		expect(control(body, 'field-keepalive_timeout')).not.toContain('aria-invalid');
	});

	it('marks the FastCGI read timeout when the backend rejects fastcgi_read_timeout', () => {
		// The single most likely way this whole slice fails silently. Validation
		// errors arrive keyed by the backend's snake_case name
		// (`fastcgi_read_timeout`); the DTO field is `fastcgiReadTimeout`. A
		// camelCase lookup here would find nothing: no mark, no message, and a
		// rejected save reads to the user as a save that did nothing at all.
		const message = '"99999999" must be between 1 and 86400';
		const body = renderForm({
			values: dto(),
			fieldErrors: { fastcgi_read_timeout: message }
		});
		expect(body).toContain('data-testid="error-fastcgi_read_timeout"');
		expect(text(body)).toContain(message);
		const input = control(body, 'field-fastcgi_read_timeout');
		expect(input).toContain('aria-invalid="true"');
		// Marked AND announced: the message has to be reachable from the input
		// itself, not merely somewhere on the page.
		expect(input).toContain('aria-describedby="ws-fastcgi_read_timeout-error');
	});

	it('shows the Phase 3 fields disabled with a reason rather than hiding them', () => {
		// A missing field reads as an oversight; a disabled one with a reason
		// tells the user the product knows.
		const body = renderForm({ values: dto() });
		for (const id of [
			'http-port',
			'https-port',
			'ssl-protocol',
			'ssl-prefer-server-ciphers',
			'http2',
			'http3'
		]) {
			expect(body).toContain(`data-testid="field-${id}"`);
			expect(control(body, `field-${id}`)).toContain('disabled');
		}
		expect(text(body)).toMatch(/privileged helper/i);
		expect(text(body)).toMatch(/Phase 3/i);
	});

	it('states no value in a field it cannot honour', () => {
		// `80` in a dead port box would be a claim about how the server listens
		// today — and it listens on a port this page has never been told.
		const body = renderForm({ values: dto() });
		expect(control(body, 'field-http-port')).not.toMatch(/[^-]value="/);
		expect(control(body, 'field-http-port')).toContain('placeholder="80"');
	});

	it('disables Save while a save is in flight', () => {
		// Scoped to the Save button's own tag: the Phase 3 inputs are disabled
		// in every render, so a bare `toContain('disabled')` would pass whatever
		// `saving` was.
		expect(control(renderForm({ saving: true }), 'settings-save')).toContain('disabled');
		expect(control(renderForm({ saving: false }), 'settings-save')).not.toContain('disabled');
	});

	it('leaves Save available on an unchanged form', () => {
		// A first launch has changes pending that the user did not make (every
		// setting is written explicitly now), so Save is the way to the diff even
		// with nothing edited.
		expect(control(renderForm({ dirty: false }), 'settings-save')).not.toContain('disabled');
	});

	it('says when there is something unsaved', () => {
		expect(renderForm({ dirty: true })).toContain('data-testid="settings-dirty"');
		expect(renderForm({ dirty: false })).not.toContain('data-testid="settings-dirty"');
	});

	it('never implies Save applies anything, or that nginx has agreed to it', () => {
		// Two traps in one line of copy: settings do not reach nginx until Apply,
		// and no `nginx -t` runs at save time — a combination nginx rejects
		// surfaces at Apply, which validates and rolls back.
		const t = text(renderForm());
		expect(t).toMatch(/review|diff/i);
		expect(t).toMatch(/apply/i);
		// The form renders no Apply control of its own — applying goes through
		// the shared dialog, which shows the diff first.
		expect(renderForm()).not.toContain('data-testid="settings-apply"');
	});

	it('renders a page-level failure without pretending the values are loaded', () => {
		const body = renderForm({ values: null, error: 'state.db is locked' });
		expect(body).toContain('data-testid="settings-error"');
		expect(text(body)).toContain('state.db is locked');
		expect(body).not.toContain('data-testid="field-gzip"');
	});

	it('still shows the values when a save reports a caveat', () => {
		// A failed re-read after a successful save must not blank the form.
		const body = renderForm({ values: dto(), error: 'could not be read back' });
		expect(body).toContain('data-testid="settings-error"');
		expect(body).toContain('data-testid="field-gzip"');
	});
});
