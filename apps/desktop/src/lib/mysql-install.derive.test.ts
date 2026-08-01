// SPDX-License-Identifier: GPL-3.0-or-later
//
// The load-bearing shape here is PAIRWISE INEQUALITY, not "each string is
// non-empty". A renderer that collapsed `verified` into `extracted` would pass
// every per-variant existence check ever written, and it is exactly the bug
// that would make a checksum-verified download and an unchecked one look
// identical on screen — the thing golden rule 6 buys, made unobservable. So
// each of the three unions is asserted distinct across every pair.
//
// Vacuity: each pairwise test was proved able to fail by making the two
// variants it protects return the same value and watching it go red (recorded
// in the slice report).

import { describe, expect, it } from 'vitest';
import type {
	MysqlInstallProgressDto,
	MysqlInstallResultDto,
	MysqlLedgerWriteDto,
	MysqlPackageOfferDto,
	MysqlRuntimeSourceDto
} from './ipc';
import {
	PACKAGED_UNINSTALL_UNAVAILABLE,
	mysqlCancelLabel,
	mysqlInstallDeclaredTotal,
	mysqlInstallOffered,
	mysqlInstallProgressLabel,
	mysqlInstallProgressPercent,
	mysqlInstallResultNotice,
	mysqlLedgerNotice,
	mysqlPackageOfferNotice,
	mysqlSourceBadge,
	mysqlUninstallOffered
} from './mysql-install.derive';

const EVERY_PROGRESS: MysqlInstallProgressDto[] = [
	{ kind: 'started', total: 1024 },
	{ kind: 'downloaded', bytes: 512 },
	{ kind: 'verified' },
	{ kind: 'extracted' },
	{ kind: 'linked' }
];

const EVERY_RESULT: MysqlInstallResultDto[] = [
	{ kind: 'installed', version: '8.4.11', detected: true, ledger: { kind: 'recorded' } },
	{ kind: 'installed', version: '8.4.11', detected: false, ledger: { kind: 'recorded' } },
	{ kind: 'alreadyInstalled', version: '8.4.11' },
	{ kind: 'cancelled' },
	{ kind: 'verificationFailed', expected: 'a'.repeat(64), actual: 'b'.repeat(64) },
	{ kind: 'stalled', detail: 'download stalled after 1024 of 2048 bytes' },
	{ kind: 'unavailable', target: 'macos-x86_64' },
	{ kind: 'failed', reason: 'io error opening /tmp/x' }
];

/** Every unordered pair of indices into `xs`, so a "these all differ" claim is
 *  actually checked rather than asserted one item at a time. */
function pairs<T>(xs: readonly T[]): [T, T][] {
	const out: [T, T][] = [];
	for (let i = 0; i < xs.length; i += 1) {
		for (let j = i + 1; j < xs.length; j += 1) out.push([xs[i], xs[j]]);
	}
	return out;
}

describe('mysqlInstallProgressLabel', () => {
	it('renders all five pipeline states pairwise-distinctly', () => {
		const labels = EVERY_PROGRESS.map((p) => mysqlInstallProgressLabel(p, 1024));
		for (const [a, b] of pairs(labels)) expect(a).not.toBe(b);
	});

	// Stated on its own as well as pairwise: this is the pair that carries the
	// verification guarantee, and it should fail by name if it ever collapses.
	it('never says the same thing for a verified download as for an extracted one', () => {
		expect(mysqlInstallProgressLabel({ kind: 'verified' }, null)).not.toBe(
			mysqlInstallProgressLabel({ kind: 'extracted' }, null)
		);
	});

	it('says the checksum was checked, in words a user can act on', () => {
		const label = mysqlInstallProgressLabel({ kind: 'verified' }, null);
		expect(label).toMatch(/checksum/i);
		expect(label).toMatch(/SHA-256/);
	});

	it('names the declared size when the server gave one', () => {
		expect(mysqlInstallProgressLabel({ kind: 'started', total: 1536 }, null)).toContain('1.50 KiB');
	});

	it('says so honestly when the server declared no size, and invents no number', () => {
		const label = mysqlInstallProgressLabel({ kind: 'started', total: null }, null);
		expect(label).toMatch(/did not say how large/i);
		expect(label).not.toMatch(/\d/);
	});

	it('shows progress against the total carried forward from the started event', () => {
		expect(mysqlInstallProgressLabel({ kind: 'downloaded', bytes: 512 }, 2048)).toBe(
			'Downloading — 512 B of 2.00 KiB'
		);
	});

	it('falls back to a "so far" reading rather than a fabricated denominator', () => {
		const label = mysqlInstallProgressLabel({ kind: 'downloaded', bytes: 512 }, null);
		expect(label).toContain('so far');
		expect(label).not.toContain(' of ');
	});
});

describe('mysqlInstallProgressPercent', () => {
	it('is a real percentage only while bytes are arriving against a known total', () => {
		expect(mysqlInstallProgressPercent({ kind: 'downloaded', bytes: 512 }, 2048)).toBe(25);
	});

	it('is null with no declared total, so no bar is drawn on a guess', () => {
		expect(mysqlInstallProgressPercent({ kind: 'downloaded', bytes: 512 }, null)).toBeNull();
	});

	it('is null for a zero or negative total rather than dividing by it', () => {
		expect(mysqlInstallProgressPercent({ kind: 'downloaded', bytes: 512 }, 0)).toBeNull();
	});

	it('never exceeds 100 even if more bytes arrive than the server declared', () => {
		expect(mysqlInstallProgressPercent({ kind: 'downloaded', bytes: 4096 }, 2048)).toBe(100);
	});

	it('is null for the steps that are moments rather than durations', () => {
		expect(mysqlInstallProgressPercent({ kind: 'started', total: 10 }, 10)).toBeNull();
		expect(mysqlInstallProgressPercent({ kind: 'verified' }, 10)).toBeNull();
		expect(mysqlInstallProgressPercent({ kind: 'extracted' }, 10)).toBeNull();
		expect(mysqlInstallProgressPercent({ kind: 'linked' }, 10)).toBeNull();
	});
});

describe('mysqlInstallDeclaredTotal', () => {
	it('carries the declared length off the started event and nothing else', () => {
		expect(mysqlInstallDeclaredTotal({ kind: 'started', total: 4096 })).toBe(4096);
		expect(mysqlInstallDeclaredTotal({ kind: 'started', total: null })).toBeNull();
		expect(mysqlInstallDeclaredTotal({ kind: 'downloaded', bytes: 4096 })).toBeNull();
		expect(mysqlInstallDeclaredTotal({ kind: 'verified' })).toBeNull();
	});
});

describe('mysqlInstallResultNotice', () => {
	it('gives every settled outcome its own title', () => {
		const titles = EVERY_RESULT.map((r) => mysqlInstallResultNotice(r).title);
		for (const [a, b] of pairs(titles)) expect(a).not.toBe(b);
	});

	it('gives every settled outcome its own body too', () => {
		const bodies = EVERY_RESULT.map((r) => mysqlInstallResultNotice(r).body);
		for (const [a, b] of pairs(bodies)) expect(a).not.toBe(b);
	});

	// The mandatory one. A hash mismatch must not read as a connection problem:
	// the bytes arrived intact and are simply not ours, and "network error"
	// invites the exact wrong response.
	it('reports a checksum mismatch as a checksum mismatch, never as a network problem', () => {
		const notice = mysqlInstallResultNotice({
			kind: 'verificationFailed',
			expected: 'a'.repeat(64),
			actual: 'b'.repeat(64)
		});
		expect(notice.tone).toBe('error');
		expect(notice.title).toMatch(/checksum/i);
		expect(notice.body).toContain('a'.repeat(64));
		expect(notice.body).toContain('b'.repeat(64));
		expect(notice.body).toMatch(/not a slow or broken connection/i);
		// And it must not be confusable with the two failures that ARE about
		// the transfer.
		expect(notice.title).not.toBe(mysqlInstallResultNotice({ kind: 'stalled', detail: 'x' }).title);
		expect(notice.title).not.toBe(
			mysqlInstallResultNotice({ kind: 'failed', reason: 'network error: reset' }).title
		);
	});

	it('says nothing was installed on every failure, so a half-state is never implied', () => {
		for (const result of EVERY_RESULT) {
			const notice = mysqlInstallResultNotice(result);
			if (notice.tone !== 'error') continue;
			expect(notice.body).toMatch(/nothing was installed|stopped before unpacking/i);
		}
	});

	it('treats a cancel as a clean stop, not a failure', () => {
		const notice = mysqlInstallResultNotice({ kind: 'cancelled' });
		expect(notice.tone).toBe('warn');
		expect(notice.body).toMatch(/no half-downloaded files/i);
		expect(notice.body).toMatch(/data directories and passwords are untouched/i);
	});

	it('reports an unavailable target as an absence naming Homebrew, not as an error', () => {
		const notice = mysqlInstallResultNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(notice.tone).not.toBe('error');
		expect(notice.title).toContain('macos-x86_64');
		expect(notice.body).toContain('Homebrew');
	});

	it('distinguishes an install whose programs were missing from a clean one', () => {
		const ok = mysqlInstallResultNotice({
			kind: 'installed',
			version: '8.4.11',
			detected: true,
			ledger: { kind: 'recorded' }
		});
		const missing = mysqlInstallResultNotice({
			kind: 'installed',
			version: '8.4.11',
			detected: false,
			ledger: { kind: 'recorded' }
		});
		expect(ok.tone).toBe('ok');
		expect(missing.tone).toBe('warn');
		expect(ok.title).not.toBe(missing.title);
	});

	it('names the exact version it installed, never the major', () => {
		const notice = mysqlInstallResultNotice({
			kind: 'installed',
			version: '8.4.11',
			detected: true,
			ledger: { kind: 'recorded' }
		});
		expect(notice.title).toContain('8.4.11');
	});
});

describe('mysqlPackageOfferNotice / mysqlInstallOffered', () => {
	it('offers an install only where a verified download exists', () => {
		expect(mysqlInstallOffered({ kind: 'available', version: '8.4.11' })).toBe(true);
		expect(mysqlInstallOffered({ kind: 'unavailable', target: 'macos-x86_64' })).toBe(false);
	});

	it('says exactly what pressing Install will do, including that brew is not used', () => {
		const notice = mysqlPackageOfferNotice({ kind: 'available', version: '8.4.11' });
		expect(notice.title).toContain('8.4.11');
		expect(notice.body).toMatch(/SHA-256/);
		expect(notice.body).toMatch(/Homebrew is not used/i);
	});

	it('renders an unavailable target as an honest absence with a real alternative', () => {
		const notice = mysqlPackageOfferNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(notice.tone).toBe('warn');
		expect(notice.title).toContain('macos-x86_64');
		expect(notice.body).toMatch(/Homebrew is the way to install MySQL on macos-x86_64/i);
		// Not an error, and not a claim that the build does not exist — Oracle
		// publishes one; OpenVHost has simply not verified it.
		expect(notice.body).toMatch(/Oracle does publish a build/i);
	});

	it('tells the two offer states apart', () => {
		const available = mysqlPackageOfferNotice({ kind: 'available', version: '8.4.11' });
		const unavailable = mysqlPackageOfferNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(available.title).not.toBe(unavailable.title);
		expect(available.body).not.toBe(unavailable.body);
		expect(available.tone).not.toBe(unavailable.tone);
	});
});

describe('mysqlSourceBadge', () => {
	const packaged: MysqlRuntimeSourceDto = { kind: 'packaged', version: '8.4.11' };
	const homebrew: MysqlRuntimeSourceDto = { kind: 'homebrew' };

	it('shows nothing for a major that is not installed', () => {
		expect(mysqlSourceBadge(null)).toBeNull();
	});

	it('labels the two sources distinctly, so a migration is legible', () => {
		const a = mysqlSourceBadge(packaged);
		const b = mysqlSourceBadge(homebrew);
		expect(a).not.toBeNull();
		expect(b).not.toBeNull();
		expect(a?.label).not.toBe(b?.label);
		expect(a?.title).not.toBe(b?.title);
	});

	it('shows the exact version for a runtime OpenVHost installed', () => {
		expect(mysqlSourceBadge(packaged)?.label).toBe('OpenVHost 8.4.11');
	});

	// The lie this rules out: printing the MAJOR where a full version belongs.
	// Brew's exact patch release is not knowable without executing mysqld, so
	// the badge carries no number at all.
	it('invents no version for a Homebrew runtime — its label carries no digits', () => {
		const badge = mysqlSourceBadge(homebrew);
		expect(badge?.label).toBe('Homebrew');
		expect(badge?.label).not.toMatch(/\d/);
		expect(badge?.title).toMatch(/will not guess/i);
	});
});

describe('mysqlUninstallOffered', () => {
	it('offers Uninstall for a Homebrew keg, which the brew path can really remove', () => {
		expect(mysqlUninstallOffered({ kind: 'homebrew' })).toBe(true);
	});

	// Absent rather than present-and-failing: `openvhost-pkg` has no uninstall
	// counterpart at all yet, so the brew-driven dialog could only fail on a
	// packaged runtime.
	it('offers no Uninstall for a runtime OpenVHost installed itself', () => {
		expect(mysqlUninstallOffered({ kind: 'packaged', version: '8.4.11' })).toBe(false);
	});

	it('offers no Uninstall when nothing is installed', () => {
		expect(mysqlUninstallOffered(null)).toBe(false);
	});

	it('explains the absence rather than leaving it looking like an oversight', () => {
		expect(PACKAGED_UNINSTALL_UNAVAILABLE).toMatch(/not built yet/i);
	});
});

describe('mysqlLedgerNotice', () => {
	it('says nothing when the row was written', () => {
		expect(mysqlLedgerNotice({ kind: 'recorded' })).toBeNull();
	});

	it('reports a failed row as provenance lost, never as a failed install', () => {
		const recorded: MysqlLedgerWriteDto = { kind: 'recorded' };
		const failed: MysqlLedgerWriteDto = { kind: 'failed', reason: 'database is locked' };
		expect(mysqlLedgerNotice(recorded)).toBeNull();
		const notice = mysqlLedgerNotice(failed);
		expect(notice).toContain('database is locked');
		expect(notice).toMatch(/installed and usable/i);
	});
});

describe('mysqlCancelLabel', () => {
	it('distinguishes an idle Cancel from one already in flight', () => {
		expect(mysqlCancelLabel(false)).not.toBe(mysqlCancelLabel(true));
		expect(mysqlCancelLabel(true)).toBe('Cancelling…');
	});
});

describe('offer and result copy agree with each other', () => {
	// Two paths tell the user the same fact — the row's pre-install note and a
	// settled `unavailable` outcome — and they must not tell different stories.
	it('uses one explanation for an unavailable target', () => {
		const fromOffer = mysqlPackageOfferNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		const fromResult = mysqlInstallResultNotice({ kind: 'unavailable', target: 'macos-x86_64' });
		expect(fromOffer.body).toBe(fromResult.body);
	});
});

// A `MysqlPackageOfferDto` union member added without a branch above fails
// typecheck at the `never` arm; this keeps the type imported so `pnpm check`
// sees the reference even though every runtime assertion is above.
const _offerTypeIsUsed: MysqlPackageOfferDto = { kind: 'available', version: '8.4.11' };
void _offerTypeIsUsed;
