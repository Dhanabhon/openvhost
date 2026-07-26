// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node` vitest project —
// same approach as SiteDrawer.svelte.test.ts and QuitDialog.svelte.test.ts.
//
// WHAT THIS FILE CANNOT COVER: no DOM, so focus handling, Tab wrapping and Escape are manual
// click-through items in the PR, same caveat as QuitDialog.svelte.test.ts.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import ApplyDialog from './ApplyDialog.svelte';
import type { ApplyOutcomeDto, FileChangeDto } from '$lib/ipc';

const c = (path: string, kind: string): FileChangeDto => ({
	path,
	kind,
	diff: `--- a${path}\n+++ b${path}\n+${path}\n`
});

function renderDialog(props: {
	changes: FileChangeDto[];
	applying?: boolean;
	error?: string;
	outcome?: ApplyOutcomeDto | null;
}): string {
	return render(ApplyDialog, {
		props: {
			changes: props.changes,
			applying: props.applying ?? false,
			error: props.error ?? '',
			outcome: props.outcome ?? null,
			onApply: () => {},
			onClose: () => {}
		}
	}).body;
}

describe('ApplyDialog', () => {
	it('renders a badge for every change kind', () => {
		const body = renderDialog({
			changes: [
				c('/nginx.conf', 'modified'),
				c('/sites/a.conf', 'added'),
				c('/sites/b.conf', 'removed')
			]
		});
		expect(body).toContain('data-kind="modified"');
		expect(body).toContain('data-kind="added"');
		expect(body).toContain('data-kind="removed"');
	});

	it('shows the diff text for each file', () => {
		const body = renderDialog({ changes: [c('/sites/a.conf', 'added')] });
		expect(body).toContain('+/sites/a.conf');
	});

	it('shows the validator error with its line breaks preserved', () => {
		const body = renderDialog({ changes: [], error: 'nginx: [emerg] line 1\nline 2' });
		expect(body).toContain('line 2');
		expect(body).toMatch(/white-space:\s*pre-wrap/);
	});

	it('disables the apply button while an apply is in flight', () => {
		expect(renderDialog({ changes: [c('/a.conf', 'added')], applying: true })).toContain(
			'disabled'
		);
		expect(renderDialog({ changes: [c('/a.conf', 'added')], applying: false })).not.toContain(
			'disabled'
		);
	});

	it('names the services it restarted', () => {
		const body = renderDialog({
			changes: [],
			outcome: {
				applied: 2,
				restarted: ['php-fpm-8.4', 'nginx'],
				notStarted: [],
				needsAttention: []
			}
		});
		expect(body).toContain('php-fpm-8.4');
		expect(body).toContain('nginx');
	});

	it('says when a changed service was not running', () => {
		const body = renderDialog({
			changes: [],
			outcome: { applied: 1, restarted: [], notStarted: ['nginx'], needsAttention: [] }
		});
		expect(body).toMatch(/next time|not running/i);
	});

	// `needsAttention` was added after a review found Apply could stop a service and
	// fail to bring it back while still reporting success. A non-empty list means
	// the apply did NOT fully succeed, so it must be visible and must NOT share the
	// `.ok`/success styling — asserted both ways so deleting either half of the
	// markup change would fail this test.
	it('renders a prominent, non-success warning for a service that needs attention', () => {
		const body = renderDialog({
			changes: [],
			outcome: {
				applied: 1,
				restarted: [],
				notStarted: [],
				needsAttention: [{ id: 'nginx', reason: 'nginx stopped and could not be started again.' }]
			}
		});
		expect(body).toContain('nginx stopped and could not be started again.');
		expect(body).toContain('data-testid="needs-attention"');
		expect(body).toMatch(/data-testid="needs-attention"[^>]*role="alert"/);
		// A needsAttention outcome is not a success and must not render as one — the
		// plain-success block must not appear alongside it.
		expect(body).not.toContain('data-testid="apply-success"');
	});

	it('renders the plain success block, not the warning, when nothing needs attention', () => {
		const body = renderDialog({
			changes: [],
			outcome: { applied: 1, restarted: ['nginx'], notStarted: [], needsAttention: [] }
		});
		expect(body).toContain('data-testid="apply-success"');
		expect(body).not.toContain('data-testid="needs-attention"');
	});

	// The IPC surface review that added `needsAttention` also flagged the diff
	// text itself: it carries generated config and user-controlled docroot paths,
	// so it must go through Svelte's escaping `{...}` interpolation, never
	// `{@html}`. This proves the escaping happens rather than just asserting the
	// source doesn't say `{@html}` — a value that LOOKS like markup must come out
	// escaped.
	it('escapes diff content instead of rendering it as HTML', () => {
		const body = renderDialog({
			changes: [{ path: '/a.conf', kind: 'added', diff: '+<script>alert(1)</script>\n' }]
		});
		// The literal tag must never appear un-escaped — that would be the `{@html}`
		// mistake. `<` is what HTML text-node escaping actually guards (a raw `>` in
		// text is not itself dangerous and Svelte's server renderer leaves it as-is),
		// so assert on the `<` escaping rather than demanding the whole tag be
		// percent-escaped.
		expect(body).not.toContain('<script>alert(1)</script>');
		expect(body).toContain('&lt;script');
		expect(body).toContain('&lt;/script');
	});

	// A failed `plan_site_apply` (MissingRuntime / NotAPlainFile) can leave the
	// dialog with an empty change list and nothing else to show — the error is the
	// only thing telling the user why. Rendered even with zero changes.
	it('shows the plan error even when there are no changes to display', () => {
		const body = renderDialog({ changes: [], error: 'no PHP runtime installed for 8.9' });
		expect(body).toContain('no PHP runtime installed for 8.9');
	});
});
