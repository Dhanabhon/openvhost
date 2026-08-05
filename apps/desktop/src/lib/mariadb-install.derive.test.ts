// SPDX-License-Identifier: GPL-3.0-or-later
//
// The load-bearing shape here, mirroring `mysql-install.derive.test.ts`'s own
// header: PAIRWISE INEQUALITY across every settled outcome, not "each string
// is non-empty" — a renderer that collapsed two distinct outcomes onto one
// sentence would pass every per-variant existence check ever written.
//
// Vacuity: `mariadbPackageOfferNotice`'s `available` case was temporarily
// changed to return the SAME title string as MySQL's own
// `mysqlPackageOfferNotice` (hardcoding "Installs MySQL …" instead of
// "Installs MariaDB …") to confirm `'says exactly what pressing Install will
// do, naming MariaDB and no Homebrew fallback'` below goes red; reverted
// immediately after.

import { describe, expect, it } from 'vitest';
import type { MariadbInstallResultDto, MariadbLedgerWriteDto } from './ipc';
import {
	mariadbInstallResultNotice,
	mariadbLedgerNotice,
	mariadbPackageOfferNotice
} from './mariadb-install.derive';
import { engineAwaitingReleaseNotice, engineDescriptor } from './databases.derive';

const EVERY_RESULT: MariadbInstallResultDto[] = [
	{ kind: 'installed', version: '11.4.9', detected: true, ledger: { kind: 'recorded' } },
	{ kind: 'installed', version: '11.4.9', detected: false, ledger: { kind: 'recorded' } },
	{ kind: 'alreadyInstalled', version: '11.4.9' },
	{ kind: 'cancelled' },
	{ kind: 'verificationFailed', expected: 'a'.repeat(64), actual: 'b'.repeat(64) },
	{ kind: 'stalled', detail: 'download stalled after 1024 of 2048 bytes' },
	{ kind: 'awaitingRelease', tag: 'mariadb-11.4.9' },
	{ kind: 'unavailable', target: 'macos-x86_64' },
	{ kind: 'failed', reason: 'io error opening /tmp/x' }
];

/** Every unordered pair of indices into `xs` — mirrors
 *  `mysql-install.derive.test.ts`'s own `pairs` helper exactly. */
function pairs<T>(xs: readonly T[]): [T, T][] {
	const out: [T, T][] = [];
	for (let i = 0; i < xs.length; i += 1) {
		for (let j = i + 1; j < xs.length; j += 1) out.push([xs[i], xs[j]]);
	}
	return out;
}

describe('mariadbInstallResultNotice', () => {
	it('gives every settled outcome its own title, across all nine values', () => {
		const titles = EVERY_RESULT.map((r) => mariadbInstallResultNotice(r).title);
		for (const [a, b] of pairs(titles)) expect(a).not.toBe(b);
	});

	it('gives every settled outcome its own body too', () => {
		const bodies = EVERY_RESULT.map((r) => mariadbInstallResultNotice(r).body);
		for (const [a, b] of pairs(bodies)) expect(a).not.toBe(b);
	});

	it('names MariaDB, never MySQL, in a clean install', () => {
		const notice = mariadbInstallResultNotice({
			kind: 'installed',
			version: '11.4.9',
			detected: true,
			ledger: { kind: 'recorded' }
		});
		expect(notice.title).toContain('MariaDB 11.4.9');
		expect(notice.body).not.toMatch(/mysql/i);
		expect(notice.body).not.toMatch(/oracle/i);
	});

	it('reports a checksum mismatch as a checksum mismatch, never as a network problem', () => {
		const notice = mariadbInstallResultNotice({
			kind: 'verificationFailed',
			expected: 'a'.repeat(64),
			actual: 'b'.repeat(64)
		});
		expect(notice.tone).toBe('error');
		expect(notice.title).toMatch(/checksum/i);
		expect(notice.body).toContain('a'.repeat(64));
		expect(notice.body).toContain('b'.repeat(64));
		expect(notice.body).toMatch(/not a slow or broken connection/i);
	});

	it('says nothing was installed on every error-toned failure', () => {
		for (const result of EVERY_RESULT) {
			const notice = mariadbInstallResultNotice(result);
			if (notice.tone !== 'error') continue;
			expect(notice.body).toMatch(/nothing was installed|stopped before unpacking/i);
		}
	});

	it('treats a cancel as a clean stop naming MariaDB, not a failure', () => {
		const notice = mariadbInstallResultNotice({ kind: 'cancelled' });
		expect(notice.tone).toBe('warn');
		expect(notice.body).toMatch(/no half-downloaded files/i);
		expect(notice.body).toMatch(/MariaDB, data directory and password are untouched/i);
	});

	it('reports an unavailable target as an absence, naming no working Homebrew fallback', () => {
		const notice = mariadbInstallResultNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(notice.tone).not.toBe('error');
		expect(notice.title).toContain('macos-x86_64');
		// Unlike MySQL's own "Homebrew is the way to install …" fallback, this
		// must never suggest a working Homebrew route — the word "Homebrew" may
		// still appear, stating plainly that there is no such route (design D2).
		expect(notice.body).not.toMatch(/homebrew is the way|brew install|brew services/i);
		expect(notice.body).toMatch(/never gone through homebrew/i);
	});

	it('distinguishes an install whose programs were missing from a clean one', () => {
		const ok = mariadbInstallResultNotice({
			kind: 'installed',
			version: '11.4.9',
			detected: true,
			ledger: { kind: 'recorded' }
		});
		const missing = mariadbInstallResultNotice({
			kind: 'installed',
			version: '11.4.9',
			detected: false,
			ledger: { kind: 'recorded' }
		});
		expect(ok.tone).toBe('ok');
		expect(missing.tone).toBe('warn');
		expect(ok.title).not.toBe(missing.title);
		expect(missing.body).toMatch(/mariadbd, mariadb or mariadb-admin/i);
	});

	// The ninth (well, eighth for MariaDB) state (design D2/D5): a build
	// exists but the release has not been published. Reuses the row's own
	// `awaitingRelease` notice verbatim, so the settled-outcome banner can
	// never drift from the not-yet-installed row's own explanation.
	describe('awaitingRelease', () => {
		it('is identical to the row-state notice for the same tag', () => {
			const fromResult = mariadbInstallResultNotice({
				kind: 'awaitingRelease',
				tag: 'mariadb-11.4.9'
			});
			const fromRowState = engineAwaitingReleaseNotice(
				engineDescriptor('mariadb'),
				'mariadb-11.4.9'
			);
			expect(fromResult).toEqual(fromRowState);
		});

		it('names the tag and points at no user action', () => {
			const notice = mariadbInstallResultNotice({ kind: 'awaitingRelease', tag: 'mariadb-11.4.9' });
			expect(notice.body).toContain('mariadb-11.4.9');
			expect(notice.body).not.toMatch(/homebrew/i);
		});
	});
});

describe('mariadbPackageOfferNotice', () => {
	it('says exactly what pressing Install will do, naming MariaDB and no Homebrew fallback', () => {
		const notice = mariadbPackageOfferNotice({ kind: 'available', version: '11.4.9' });
		expect(notice.title).toContain('Installs MariaDB 11.4.9');
		expect(notice.body).toMatch(/SHA-256/);
		expect(notice.body).toMatch(/Homebrew is not used/i);
		expect(notice.body).not.toMatch(/oracle/i);
	});

	it('renders an unavailable target as an honest absence with no working Homebrew route', () => {
		const notice = mariadbPackageOfferNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(notice.tone).toBe('warn');
		expect(notice.title).toContain('macos-x86_64');
		expect(notice.body).not.toMatch(/homebrew is the way|brew install|brew services/i);
		expect(notice.body).toMatch(/never gone through homebrew/i);
	});

	it('tells the two offer states apart', () => {
		const available = mariadbPackageOfferNotice({ kind: 'available', version: '11.4.9' });
		const unavailable = mariadbPackageOfferNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(available.title).not.toBe(unavailable.title);
		expect(available.body).not.toBe(unavailable.body);
		expect(available.tone).not.toBe(unavailable.tone);
	});

	// The other half of "offer and result copy agree with each other" —
	// mirrors `mysql-install.derive.test.ts`'s own cross-check.
	it('agrees with the settled unavailable outcome on the same target', () => {
		const fromOffer = mariadbPackageOfferNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		const fromResult = mariadbInstallResultNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(fromOffer.body).toBe(fromResult.body);
	});
});

describe('mariadbLedgerNotice', () => {
	it('says nothing when the row was written', () => {
		expect(mariadbLedgerNotice({ kind: 'recorded' })).toBeNull();
	});

	it('reports a failed row as provenance lost, never as a failed install', () => {
		const recorded: MariadbLedgerWriteDto = { kind: 'recorded' };
		const failed: MariadbLedgerWriteDto = { kind: 'failed', reason: 'database is locked' };
		expect(mariadbLedgerNotice(recorded)).toBeNull();
		const notice = mariadbLedgerNotice(failed);
		expect(notice).toContain('database is locked');
		expect(notice).toMatch(/installed and usable/i);
	});
});
