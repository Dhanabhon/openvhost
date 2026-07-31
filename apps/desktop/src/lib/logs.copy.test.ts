// SPDX-License-Identifier: GPL-3.0-or-later
// Vacuity method: neuter-proven (see logs.derive.test.ts's header for the
// general approach) — the exhaustiveness guard on `resetNoticeCopy` was
// checked by temporarily removing a `case` and confirming `tsc`/svelte-check
// rejects it (a runtime test cannot exercise a compile-time exhaustiveness
// failure), and each string's content was checked against a deliberately
// wrong copy value to confirm the assertion is specific, not just "is a
// string".
import { describe, expect, it } from 'vitest';
import {
	emptyCopy,
	genericReadErrorCopy,
	noSelectionCopy,
	notYetCreatedCopy,
	permissionDeniedCopy,
	privacyNoteCopy,
	resetNoticeCopy,
	scanBoundCopy,
	sizeWarningCopy,
	unavailableSourceCopy
} from './logs.copy';

describe('logs.copy', () => {
	it('emptyCopy distinguishes "nothing written" from "nothing matched the filter"', () => {
		expect(emptyCopy(false)).toMatch(/empty/i);
		expect(emptyCopy(true)).toMatch(/match/i);
		expect(emptyCopy(false)).not.toBe(emptyCopy(true));
	});

	it('noSelectionCopy invites picking a source', () => {
		expect(noSelectionCopy()).toMatch(/pick/i);
	});

	it('notYetCreatedCopy points forward rather than just stating absence', () => {
		const c = notYetCreatedCopy();
		expect(c).toMatch(/hasn.t been created/i);
		expect(c).toMatch(/will appear/i);
	});

	it('permissionDeniedCopy names the actual problem and a next step', () => {
		const c = permissionDeniedCopy();
		expect(c).toMatch(/permission/i);
		expect(c).toMatch(/open log folder/i);
	});

	it('genericReadErrorCopy carries the underlying message through, not just a generic banner', () => {
		expect(genericReadErrorCopy('disk fell over')).toContain('disk fell over');
	});

	it('unavailableSourceCopy names the source and explains why it may be gone', () => {
		const c = unavailableSourceCopy('PHP 8.1 pool log');
		expect(c).toContain('PHP 8.1 pool log');
		expect(c).toMatch(/removed/i);
	});

	it('resetNoticeCopy differs between rotated and truncated', () => {
		expect(resetNoticeCopy('rotated')).toMatch(/rotated|replaced/i);
		expect(resetNoticeCopy('truncated')).toMatch(/truncated/i);
		expect(resetNoticeCopy('rotated')).not.toBe(resetNoticeCopy('truncated'));
	});

	it('scanBoundCopy is an honest "may be incomplete" note, not a claim of completeness', () => {
		expect(scanBoundCopy()).toMatch(/stopped|early|further/i);
	});

	it('sizeWarningCopy names the 100 MiB threshold and the one recourse this slice ships', () => {
		const c = sizeWarningCopy();
		expect(c).toMatch(/100 ?MiB/i);
		expect(c).toMatch(/folder/i);
	});

	it('privacyNoteCopy makes no false redaction promise', () => {
		const c = privacyNoteCopy();
		expect(c).toMatch(/local/i);
		expect(c).toMatch(/sensitive/i);
	});

	it('no copy string uses banned hype vocabulary (brand guidelines §6.1)', () => {
		const all = [
			emptyCopy(false),
			emptyCopy(true),
			noSelectionCopy(),
			notYetCreatedCopy(),
			permissionDeniedCopy(),
			genericReadErrorCopy('x'),
			unavailableSourceCopy('x'),
			resetNoticeCopy('rotated'),
			resetNoticeCopy('truncated'),
			scanBoundCopy(),
			sizeWarningCopy(),
			privacyNoteCopy()
		].join(' ');
		expect(all).not.toMatch(/blazingly|supercharge|magical|seamless|revolutionary|oops/i);
	});
});
