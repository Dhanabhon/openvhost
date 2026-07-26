// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node`
// vitest project with no DOM — same approach as SiteDrawer/SiteListRow/QuitDialog.
//
// WHAT THIS FILE CANNOT COVER: there is no DOM, so the polling itself and the
// pause-on-hidden wiring are out of reach here. Those live in
// `stats.svelte.test.ts` (with fake timers) and in the PR's manual click-through.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import StatusBar from './StatusBar.svelte';

function html(props: {
	servicesBytes?: number | null;
	processCount?: number | null;
	homeBytes?: number | null;
	homePending?: boolean;
}): string {
	return render(StatusBar, {
		props: {
			servicesBytes: props.servicesBytes ?? null,
			processCount: props.processCount ?? null,
			homeBytes: props.homeBytes ?? null,
			homePending: props.homePending ?? false
		}
	}).body;
}

function text(markup: string): string {
	return markup
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('StatusBar', () => {
	it('shows all three segments when everything is known', () => {
		const t = text(html({ servicesBytes: 89128960, processCount: 2, homeBytes: 1288490188 }));
		expect(t).toContain('services 85 MB');
		expect(t).toContain('2 processes');
		expect(t).toContain('~/.openvhost');
		expect(t).toContain('1.2 GB');
	});

	// The failure mode this guards: a failed sample must not read as a measured
	// zero. "0 MB · no processes" is a specific claim; "—" is the truth.
	it('renders unknown figures as a dash, never as zero', () => {
		const t = text(html({ servicesBytes: null, processCount: null, homeBytes: null }));
		expect(t).toContain('—');
		expect(t).not.toContain('0 MB');
		expect(t).not.toContain('no processes');
	});

	it('says measuring while the first home walk is in flight', () => {
		const t = text(html({ homeBytes: null, homePending: true }));
		expect(t).toContain('measuring');
		// "measuring…" and "—" are different states and must not both show.
		expect(t).not.toMatch(/~\/\.openvhost\s+—/);
	});

	it('reports an idle app as a real zero, not as unknown', () => {
		const t = text(html({ servicesBytes: 0, processCount: 0, homeBytes: 1024 }));
		expect(t).toContain('0 MB');
		expect(t).toContain('no processes');
	});

	// A screen reader should not have this re-announced every 2 seconds.
	it('is a labelled, non-live region', () => {
		const m = html({ servicesBytes: 1024, processCount: 1, homeBytes: 1024 });
		expect(m).toContain('aria-label="Resource usage"');
		expect(m).not.toContain('aria-live');
	});

	// A bare <div> has the implicit ARIA role `generic`, and `generic` is in the
	// ARIA "name prohibited" category: `aria-label` alone would not compute an
	// accessible name, and the strip would not be reachable via landmark/rotor
	// navigation. `role="region"` is what makes the existing `aria-label` name
	// something and turns the strip into a landmark a screen-reader user can
	// reach on purpose.
	it('is a landmark, not a bare unnamed div', () => {
		const m = html({ servicesBytes: 1024, processCount: 1, homeBytes: 1024 });
		expect(m).toContain('role="region"');
	});
});
