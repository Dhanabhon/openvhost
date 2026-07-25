// SPDX-License-Identifier: GPL-3.0-or-later
//
// SSR-rendered (`svelte/server`), so no DOM is needed and this runs in the
// existing `node` vitest project. Interactive behaviour — the config disclosure
// toggling, the Validate round-trip — is out of reach here and is on the PR's
// click-through list.
//
// No `beforeEach`: the panel is purely presentational, so every case states its
// whole world in the `html()` call and there is no module-level state to reset
// (unlike routes.test.ts, which shares the services store singleton).
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import WebServerPanel from './WebServerPanel.svelte';
import type { WebServerDto } from '$lib/ipc';

const nginx: WebServerDto = {
	id: 'nginx',
	displayName: 'nginx',
	supported: true,
	serviceId: 'nginx',
	binaryPath: '/opt/homebrew/opt/nginx/bin/nginx',
	version: '1.27.3',
	supportsHotReload: true,
	configPath: '/x/.openvhost/conf/nginx.conf'
};
const apache: WebServerDto = {
	id: 'apache',
	displayName: 'Apache',
	supported: false,
	serviceId: null,
	binaryPath: null,
	version: null,
	supportsHotReload: false,
	configPath: null
};

function html(props: Record<string, unknown>): string {
	return render(WebServerPanel, {
		props: {
			servers: [nginx, apache],
			services: [],
			configText: {},
			configError: {},
			reports: {},
			validating: {},
			onShowConfig: () => {},
			onValidate: () => {},
			...props
		}
	}).body;
}

function text(s: string): string {
	return s
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('WebServerPanel', () => {
	it('shows the resolved binary, version and config path for nginx', () => {
		const t = text(html({}));
		expect(t).toContain('/opt/homebrew/opt/nginx/bin/nginx');
		expect(t).toContain('1.27.3');
		expect(t).toContain('/x/.openvhost/conf/nginx.conf');
	});

	// An unknown version must read as unknown, not as an empty gap the user
	// cannot interpret.
	it('says the version is unknown rather than rendering a blank', () => {
		const t = text(html({ servers: [{ ...nginx, version: null }] }));
		expect(t.toLowerCase()).toContain('unknown');
	});

	it('states plainly that Apache is not served yet', () => {
		expect(text(html({})).toLowerCase()).toContain('cannot serve apache');
	});

	it('offers neither the config view nor Validate for an unsupported brand', () => {
		const body = html({ servers: [apache] });
		expect(body).not.toContain('data-testid="validate-apache"');
		expect(body).not.toContain('data-testid="show-config-apache"');
	});

	it('offers both for nginx', () => {
		const body = html({ servers: [nginx] });
		expect(body).toContain('data-testid="validate-nginx"');
		expect(body).toContain('data-testid="show-config-nginx"');
	});

	it('renders a per-row failure on that row', () => {
		const t = text(html({ configError: { nginx: 'cannot read /x/nginx.conf' } }));
		expect(t).toContain('cannot read /x/nginx.conf');
	});

	// nginx's own diagnostic is the useful part; it must not be summarized away.
	it('shows the validator stderr verbatim', () => {
		const t = text(
			html({
				reports: { nginx: { ok: false, stderr: 'nginx: [emerg] unknown directive "bogus"' } }
			})
		);
		expect(t).toContain('unknown directive');
	});

	it('shows the config text once it has been read', () => {
		const t = text(html({ configText: { nginx: 'daemon off; worker_processes 1;' } }));
		expect(t).toContain('worker_processes 1;');
	});
});

// The store clears `configError[id]` when a read starts but never clears
// `configText[id]`, so a row that read its config once and later failed a
// RE-read holds stale text and a fresh error at the same time. The store does
// not prevent that combination, so the rendering precedence is decided here and
// pinned. See WebServerRow.svelte's own comment for the reasoning.
describe('a read error next to previously-read content', () => {
	const stale = {
		configText: { nginx: 'daemon off; worker_processes 1;' },
		configError: { nginx: 'cannot read /x/.openvhost/conf/nginx.conf: No such file' }
	};

	it('shows the error and NOT the stale config text', () => {
		const t = text(html(stale));
		expect(t).toContain('cannot read /x/.openvhost/conf/nginx.conf: No such file');
		expect(t).not.toContain('worker_processes 1;');
	});

	// Asymmetric on purpose: a failed read does not contradict the result of a
	// validator run that DID complete, so the report is not suppressed with the
	// text. Both statements can be true at once.
	it('keeps a completed validation report visible beside that error', () => {
		const t = text(
			html({
				...stale,
				reports: { nginx: { ok: true, stderr: 'syntax is ok\ntest is successful' } }
			})
		);
		expect(t).toContain('cannot read /x/.openvhost/conf/nginx.conf: No such file');
		expect(t).toContain('test is successful');
	});
});
