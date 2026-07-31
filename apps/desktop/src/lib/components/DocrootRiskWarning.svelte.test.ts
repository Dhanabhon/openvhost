// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), same approach as
// ScaffoldNoticeBanner.svelte.test.ts. This component is what makes "renders for
// a risky docroot in CREATE mode" honestly SSR-testable at all: SiteDrawer.svelte
// seeds its own `docroot` state from `site?.docroot ?? ''`, which is unconditionally
// blank in create mode (`site === null`) at mount — there is no way to hand an SSR
// render of the full drawer a non-blank docroot in create mode without simulating a
// user typing/browsing, which (per SiteDrawer.svelte.test.ts's own header) this
// project's DOM-less `node` vitest project cannot do. Taking `risk`/`mode` as plain
// props sidesteps that entirely, the same reason `ScaffoldNoticeBanner` takes an
// already-classified `outcome` rather than deriving it itself.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import DocrootRiskWarning from './DocrootRiskWarning.svelte';
import type { DocrootRisk, DocrootWarningMode } from '$lib/sites.derive';

function warningHtml(risk: DocrootRisk, mode: DocrootWarningMode): string {
	return render(DocrootRiskWarning, { props: { risk, mode } }).body;
}

function text(markup: string): string {
	return markup
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('DocrootRiskWarning', () => {
	it('renders in CREATE mode for a personal-folder risk, naming the folder and the checkbox fix', () => {
		const html = warningHtml({ kind: 'personalFolder', folder: 'Downloads' }, 'create');
		const body = text(html);
		expect(body).toContain('Downloads');
		expect(body).toContain("reachable at this site's domain");
		expect(body).toContain('.php');
		expect(body).toContain('Create a site folder inside this folder');
	});

	it('renders in EDIT mode for the same risk, naming the folder and the subfolder fix instead', () => {
		const html = warningHtml({ kind: 'personalFolder', folder: 'Downloads' }, 'edit');
		const body = text(html);
		expect(body).toContain('Downloads');
		expect(body).toContain('subfolder');
		expect(body).not.toContain('Create a site folder inside this folder');
	});

	it('renders the homeItself tier in create mode', () => {
		const body = text(warningHtml({ kind: 'homeItself' }, 'create'));
		expect(body).toContain('home folder');
	});

	it('renders the systemRoot tier in edit mode, naming the actual root', () => {
		const body = text(warningHtml({ kind: 'systemRoot', root: '/etc' }, 'edit'));
		expect(body).toContain('/etc');
	});

	it('exposes a stable id for aria-describedby wiring on the Project-folder input', () => {
		const html = warningHtml({ kind: 'homeItself' }, 'create');
		expect(html).toContain('id="f-root-risk"');
	});

	it('carries a testid so integration tests can find it without matching on copy', () => {
		const html = warningHtml({ kind: 'homeItself' }, 'create');
		expect(html).toContain('data-testid="docroot-risk-warning"');
	});
});
