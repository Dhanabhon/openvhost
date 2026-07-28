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
import type { ServiceStatus, WebServerDto } from '$lib/ipc';

const nginx: WebServerDto = {
	id: 'nginx',
	displayName: 'nginx',
	supported: true,
	serviceId: 'nginx',
	binaryPath: '/opt/homebrew/opt/nginx/bin/nginx',
	version: '1.27.3',
	supportsHotReload: true,
	configPath: '/x/.openvhost/conf/nginx.conf',
	configExists: true
};
const apache: WebServerDto = {
	id: 'apache',
	displayName: 'Apache',
	supported: false,
	serviceId: null,
	binaryPath: null,
	version: null,
	supportsHotReload: false,
	configPath: null,
	configExists: false
};

/** A supervisor snapshot entry. Takes the whole `ServiceState` rather than a bare
 * kind so `failed` — which carries `exit`/`stderrTail` — is expressible here. */
const svc = (id: string, state: ServiceStatus['state']): ServiceStatus => ({
	id,
	displayName: id,
	endpoint: null,
	pid: state.kind === 'running' ? 1 : null,
	state
});

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
			onStart: () => {},
			onStop: () => {},
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

/** The opening `<button …>` tag carrying `testId`, so an attribute assertion is about
 * THAT control rather than about the attribute appearing anywhere on the page. */
function buttonTag(body: string, testId: string): string {
	const found = body.match(new RegExp(`<button[^>]*data-testid="${testId}"[^>]*>`));
	if (found === null) {
		throw new Error(`no <button> with data-testid="${testId}" in:\n${body}`);
	}
	return found[0];
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

	// "On that row" is the load-bearing half and needs the negative: the panel indexes
	// the per-id maps so each row gets only its own slice, and mutating that to
	// `Object.values(configError)[0]` shows nginx's failure on Apache too — which the
	// message assertion alone cannot tell apart from correct behaviour.
	it('renders a per-row failure on that row and on no other', () => {
		const body = html({ configError: { nginx: 'cannot read /x/nginx.conf' } });
		expect(text(body)).toContain('cannot read /x/nginx.conf');
		expect(body).toContain('data-testid="config-error-nginx"');
		expect(body).not.toContain('data-testid="config-error-apache"');
	});

	// nginx's own diagnostic is the useful part; it must be neither summarized nor
	// truncated. A single short assertion cannot say that — `stderr.slice(0, 40)`
	// would satisfy it — so the fixture is long and multi-line and BOTH ends are
	// asserted. (Whitespace fidelity is not observable through `text()`; the
	// unmodified `<pre>` is what carries it.)
	it('shows the validator stderr verbatim, first line through last', () => {
		const first = 'nginx: [emerg] unexpected end of file, expecting } in /x/nginx.conf:120';
		const last = 'nginx: configuration file /x/.openvhost/conf/nginx.conf test failed';
		const stderr = [
			first,
			'nginx: [warn] the "user" directive makes sense only if the master process runs',
			'nginx: [warn] duplicate MIME type text/plain in /x/.openvhost/conf/mime.types:31',
			'nginx: [emerg] the server directive is not allowed here in /x/nginx.conf:44',
			last
		].join('\n');
		const t = text(html({ reports: { nginx: { ok: false, stderr } } }));
		expect(t).toContain(first);
		expect(t).toContain(last);
	});

	// Same per-row slicing hole as the failure above, on the other map.
	it('shows the config text on the row it was read for and on no other', () => {
		const body = html({ configText: { nginx: 'daemon off; worker_processes 1;' } });
		expect(text(body)).toContain('worker_processes 1;');
		expect(body).toContain('data-testid="config-nginx"');
		expect(body).not.toContain('data-testid="config-apache"');
	});
});

// The shared `Button` was widened with `expanded`/`controls` FOR this disclosure, and
// `disabled` is the only thing bounding a second `nginx -t` spawn — yet deleting
// `expanded={showConfig}` and `disabled={validating}` from WebServerRow left
// `pnpm test` at 129/129 and `pnpm check` at 0/0, with no dead code created. Task 5's
// review proved the hard half (the 9 pre-existing Button consumers emit no
// `aria-expanded` at all); nobody checked that the new consumer actually USES what the
// component was widened for.
describe('the disclosure and Validate controls', () => {
	it('marks the disclosure expanded and points it at the region it reveals', () => {
		const open = buttonTag(html({ configText: { nginx: 'daemon off;' } }), 'show-config-nginx');
		expect(open).toContain('aria-expanded="true"');
		// The IDREF must name the `<pre>` that is actually in the DOM in this state.
		expect(open).toContain('aria-controls="ws-config-nginx"');
	});

	// The closed state is the one a screen-reader user meets first, and `false` must be
	// PRESENT rather than the attribute being absent: absent means "not a disclosure",
	// which is what a plain Button emits and what this row must not look like.
	it('still announces aria-expanded="false" while the config is hidden', () => {
		expect(buttonTag(html({}), 'show-config-nginx')).toContain('aria-expanded="false"');
	});

	// Without this, a double-click fires a SECOND `nginx -t` spawn while the first is in
	// flight — `validate()` has no re-entrancy guard of its own (the security auditor
	// found the same thing independently), so this attribute is the whole bound.
	it('disables Validate while a validation is in flight, and only then', () => {
		expect(buttonTag(html({ validating: { nginx: true } }), 'validate-nginx')).toMatch(
			/\sdisabled/
		);
		expect(buttonTag(html({}), 'validate-nginx')).not.toMatch(/\sdisabled/);
	});
});

// The copy added by this slice is the only place the product tells anyone to edit
// `<home>/config/generated/nginx/nginx.conf` — and `site::apply` regenerates that
// file from the user's sites every time Apply runs, so the advice has to say so
// (and name the `config/custom/` escape hatch) or it is a trap.
describe('the failed-validation next step', () => {
	it('warns that Apply regenerates the file from the sites', () => {
		const t = text(html({ reports: { nginx: { ok: false, stderr: 'nginx: [emerg] boom' } } }));
		// Not "this page is read-only" any more: the settings form below this
		// panel edits exactly how that file is generated. The claim belongs to
		// the config VIEW, which still cannot be typed into.
		expect(t).toContain('it does not edit it');
		expect(t).not.toContain('this page is read-only');
		expect(t.toLowerCase()).toContain('regenerates this file from your sites');
		expect(t).toContain('config/custom/');
	});

	// Nothing to warn about when the config IS valid: no next-step block renders.
	it('says none of it when the config is valid', () => {
		const t = text(html({ reports: { nginx: { ok: true, stderr: 'syntax is ok' } } }));
		expect(t.toLowerCase()).not.toContain('regenerates this file');
	});
});

// The one thing this page exists to get right, and the one thing every case above
// leaves unexercised: they all pass `services: []`, so no pill renders at all.
// Mutating the panel to `statusFor(services, server.serviceId ?? 'nginx')` puts
// nginx's live status on Apache's row — telling the user OpenVHost is serving
// Apache when it cannot — and before this block every other test in the file, and
// in routes.test.ts, still passed. `webservers.derive.test.ts` pins the HELPER;
// these pin the WIRING, which is where that bug would actually live.
describe('the status pill', () => {
	const nginxRunning = [svc('nginx', { kind: 'running' })];

	it('appears on the row that owns the service and on no other row', () => {
		const body = html({ services: nginxRunning });
		expect(body).toContain('data-testid="ws-pill-nginx"');
		// Apache has no supervised service, so it gets no pill at all — least of all
		// a neighbour's.
		expect(body).not.toContain('data-testid="ws-pill-apache"');
	});

	// Two different states down the same path: a constant that matched one of them
	// cannot pass both. `failed` is included because it is the state the user most
	// needs to be told the truth about.
	it('reads the state off the snapshot rather than any constant', () => {
		expect(html({ services: nginxRunning })).toContain('pill-running');
		const failed = [svc('nginx', { kind: 'failed', exit: 1, stderrTail: ['bind() failed'] })];
		expect(html({ services: failed })).toContain('pill-failed');
	});

	// Before the layout's first `listServices` answers there is no state to report,
	// and a pill that guessed one would be a fabricated claim about the machine.
	it('renders no pill at all before the first supervisor snapshot arrives', () => {
		expect(html({ services: [] })).not.toContain('data-testid="ws-pill-');
	});
});

// `onMount` does not run under SSR and the route paints before `list_web_servers`
// resolves, so this state is on screen for a frame on every single visit. The copy
// therefore must not assert anything the app has not checked yet.
describe('the empty state', () => {
	it('does not claim there are no web servers, since the list may still be loading', () => {
		const t = text(html({ servers: [] }));
		expect(t).toContain('Nothing to show yet');
		expect(t.toLowerCase()).not.toContain('no web servers');
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

describe('the service control', () => {
	it('renders nothing at all before the services snapshot arrives', () => {
		// An empty services array is what the very first frame of every visit
		// looks like. Neither a Start nor a Stop may appear.
		const out = html({ servers: [nginx], services: [] });
		expect(out).not.toContain('data-testid="ws-start-nginx"');
		expect(out).not.toContain('data-testid="ws-stop-nginx"');
	});

	it('offers Start when nginx is stopped and a config exists', () => {
		const out = html({ servers: [nginx], services: [svc('nginx', { kind: 'stopped' })] });
		expect(out).toContain('data-testid="ws-start-nginx"');
		// Asserting the REASON is absent, not that the word "disabled" is absent
		// anywhere in the panel: this is a whole-panel string, and any other
		// control acquiring a `disabled` attribute later would fail this case for
		// a reason that has nothing to do with Start.
		expect(out).not.toContain('ws-start-reason-nginx');
	});

	it('disables Start and says why when no config has been generated', () => {
		const out = html({
			servers: [{ ...nginx, configExists: false }],
			services: [svc('nginx', { kind: 'stopped' })]
		});
		expect(out).toContain('data-testid="ws-start-nginx"');
		expect(out).toContain('disabled');
		expect(out).toContain('ws-start-reason-nginx');
		expect(out).toContain('No config generated yet');
	});

	it('offers Start with no reason when config existence could not be determined', () => {
		// `configExists: null` is what the backend reports when its own stat
		// could not be performed — NOT the same fact as "confirmed absent". Start
		// stays enabled and, critically, the reason line does not render: naming
		// NO_CONFIG_REASON here would tell the user to re-Apply for a problem
		// (a permission error, say) that Apply cannot fix.
		const out = html({
			servers: [{ ...nginx, configExists: null }],
			services: [svc('nginx', { kind: 'stopped' })]
		});
		expect(out).toContain('data-testid="ws-start-nginx"');
		// The same reason-absence assertion the enabled case above makes: a whole-
		// panel string, so this is about Start specifically, not about "disabled"
		// never appearing anywhere in the markup.
		expect(out).not.toContain('ws-start-reason-nginx');
	});

	it('offers Stop while running', () => {
		const out = html({ servers: [nginx], services: [svc('nginx', { kind: 'running' })] });
		expect(out).toContain('data-testid="ws-stop-nginx"');
		expect(out).not.toContain('data-testid="ws-start-nginx"');
	});

	it('gives Apache no service control, since it supervises nothing', () => {
		const out = html({ servers: [apache], services: [] });
		expect(out).not.toContain('ws-start-apache');
		expect(out).not.toContain('ws-stop-apache');
	});
});

describe('the pool warning', () => {
	it('warns that a site will 502 when its pool is down', () => {
		const out = html({
			servers: [nginx],
			services: [svc('nginx', { kind: 'running' }), svc('php-fpm-8.4', { kind: 'stopped' })],
			stoppedPools: ['8.4']
		});
		expect(out).toContain('data-testid="ws-pool-warning"');
		expect(out).toContain('502');
		expect(out).toContain('8.4');
	});
});

describe('a failed nginx', () => {
	it("shows nginx's own words, not just a failed pill", () => {
		// The whole point. Asserting on the CONTENT, not on the presence of a
		// block: an empty <pre> would satisfy a weaker assertion and tell the
		// user nothing about why their web server did not start.
		const out = html({
			servers: [nginx],
			services: [
				svc('nginx', {
					kind: 'failed',
					exit: 1,
					stderrTail: ['nginx: [emerg] bind() to 0.0.0.0:8080 failed (48: Address already in use)']
				})
			]
		});
		expect(out).toContain('Address already in use');
		expect(out).toContain('data-testid="ws-failed-nginx"');
	});

	it('offers Retry rather than Start after a failure', () => {
		const out = html({
			servers: [nginx],
			services: [svc('nginx', { kind: 'failed', exit: 1, stderrTail: ['boom'] })]
		});
		expect(out).toContain('data-testid="ws-retry-nginx"');
	});

	it('says a failure happened even when nginx said nothing', () => {
		// A service killed by a signal has an empty tail. Rendering only the
		// <pre> would leave a failed row that looks identical to a healthy one.
		const out = html({
			servers: [nginx],
			services: [svc('nginx', { kind: 'failed', exit: null, stderrTail: [] })]
		});
		expect(out).toContain('data-testid="ws-failed-nginx"');
		expect(out).toContain('nginx failed');
	});
});
