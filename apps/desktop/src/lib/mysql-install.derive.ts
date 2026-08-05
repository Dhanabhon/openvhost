// SPDX-License-Identifier: GPL-3.0-or-later
// Pure copy + classification for installing MySQL from the upstream tarball
// (MySQL-from-tarball design D2/D3). Every user-visible string on that path
// lives here, once, so the page and its tests share one wording and a future
// i18n extraction has a single file to walk.
//
// The rule this module exists to keep: NO WILDCARD ARM over any of the three
// unions it consumes. Each `switch` ends in `const unreachable: never = x`, so
// a sixth pipeline stage, a third install source or an eighth outcome fails
// TYPECHECK here rather than silently rendering as whichever branch happened to
// come last. This codebase has shipped a state-collapsed-into-a-boolean bug
// five times; this is the cheapest place to stop the sixth.

import { formatBytes } from './logs.derive';
import type {
	MysqlInstallProgressDto,
	MysqlInstallResultDto,
	MysqlLedgerWriteDto,
	MysqlPackageOfferDto,
	MysqlRuntimeSourceDto
} from './ipc';

/** How a notice reads: a fact, something to look at, or something that failed.
 *  Mirrors the tones `MysqlRow.svelte`'s own `.note`/`.note.warn`/`.error`
 *  classes already carry, named here so the derive layer decides the tone and
 *  the template only paints it. */
export type NoticeTone = 'ok' | 'warn' | 'error';

export interface Notice {
	tone: NoticeTone;
	title: string;
	body: string;
}

/**
 * One line describing the pipeline step the user is currently watching.
 *
 * `total` is the length the server declared, carried forward from the
 * `started` event (the `downloaded` events do not repeat it) — `null` when the
 * server declared none, which is a real case and renders as an honest
 * "so far" rather than a fabricated percentage.
 *
 * MANDATORY: `verified` and `extracted` must never render the same sentence.
 * They are the difference between a download that was checked against the
 * compiled-in SHA-256 and one that merely arrived, which is precisely what
 * golden rule 6 buys — and a guarantee nobody can see is a guarantee nobody
 * has. `mysql-install.derive.test.ts` asserts every pair of these five is
 * distinct, not merely that each is non-empty.
 */
export function mysqlInstallProgressLabel(
	progress: MysqlInstallProgressDto,
	total: number | null
): string {
	switch (progress.kind) {
		case 'started':
			return progress.total === null
				? 'Starting the download — the server did not say how large it is'
				: `Starting the download — ${formatBytes(progress.total)} to fetch`;
		case 'downloaded':
			return total === null
				? `Downloading — ${formatBytes(progress.bytes)} so far`
				: `Downloading — ${formatBytes(progress.bytes)} of ${formatBytes(total)}`;
		case 'verified':
			return 'Checksum verified — the download matches the SHA-256 built into OpenVHost';
		case 'extracted':
			return 'Extracted — the archive was unpacked into a staging folder';
		case 'linked':
			return 'Installed — the files are in place and this version is now selected';
		default: {
			const unreachable: never = progress;
			return unreachable;
		}
	}
}

/**
 * How far along the transfer is, 0–100, or `null` when there is nothing
 * honest to draw: before the first byte, when the server declared no length,
 * and for every step after the download (which are moments, not durations, and
 * would otherwise animate a bar that is really just waiting).
 */
export function mysqlInstallProgressPercent(
	progress: MysqlInstallProgressDto,
	total: number | null
): number | null {
	switch (progress.kind) {
		case 'started':
			return null;
		case 'downloaded': {
			if (total === null || total <= 0) return null;
			return Math.min(100, Math.round((progress.bytes / total) * 100));
		}
		case 'verified':
		case 'extracted':
		case 'linked':
			return null;
		default: {
			const unreachable: never = progress;
			return unreachable;
		}
	}
}

/** The declared total a `started` event carries, for the store to hold on to —
 *  every later `downloaded` event needs it and none of them repeats it. */
export function mysqlInstallDeclaredTotal(progress: MysqlInstallProgressDto): number | null {
	return progress.kind === 'started' ? progress.total : null;
}

/**
 * What to say once an install has settled — one notice per outcome, each with
 * its own title, so no two outcomes can be mistaken for each other.
 *
 * The one that matters most: a **verification failure is not a network
 * error**. The bytes arrived; they simply are not the bytes we pinned. Calling
 * that "network error" would both hide the single event the SHA-256 check
 * exists to catch and invite exactly the wrong response (retry until it
 * works), so it gets its own title, its own tone and its own explanation.
 */
export function mysqlInstallResultNotice(result: MysqlInstallResultDto): Notice {
	switch (result.kind) {
		case 'installed':
			return result.detected
				? {
						tone: 'ok',
						title: `MySQL ${result.version} installed`,
						body:
							"Downloaded from Oracle, checked against OpenVHost's built-in SHA-256, and unpacked " +
							"into OpenVHost's own packages folder. Homebrew was not involved."
					}
				: {
						tone: 'warn',
						title: `MySQL ${result.version} unpacked, but its programs were not found`,
						body:
							'The download and the checksum were fine, but mysqld, mysql or mysqladmin is missing ' +
							'from the extracted files, so OpenVHost cannot run this version. Nothing else on this ' +
							'machine was changed.'
					};
		case 'alreadyInstalled':
			return {
				tone: 'ok',
				title: `MySQL ${result.version} was already installed`,
				body: 'Nothing was downloaded — that exact version is already in the packages folder.'
			};
		case 'cancelled':
			return {
				tone: 'warn',
				title: 'Install cancelled',
				body:
					'Nothing was installed and no half-downloaded files were left behind. Your existing ' +
					'MySQL, data directories and passwords are untouched.'
			};
		case 'verificationFailed':
			return {
				tone: 'error',
				title: 'Checksum did not match — nothing was installed',
				body:
					`The download finished, but its SHA-256 was ${result.actual} instead of the expected ` +
					`${result.expected}. OpenVHost stopped before unpacking anything. This is not a slow or ` +
					'broken connection: the bytes that arrived are not the bytes OpenVHost pinned.'
			};
		case 'stalled':
			return {
				tone: 'error',
				title: 'The download stopped making progress',
				body: `${result.detail}. Nothing was installed. Check the connection and try again.`
			};
		case 'unavailable':
			return {
				tone: 'warn',
				title: `No verified MySQL download for ${result.target}`,
				body: unavailableBody(result.target)
			};
		case 'failed':
			return {
				tone: 'error',
				title: 'The install did not finish',
				body: `${result.reason}. Nothing was installed.`
			};
		default: {
			const unreachable: never = result;
			return unreachable;
		}
	}
}

/**
 * The Homebrew-fallback sentence for a target this build has no verified
 * MySQL download for — Homebrew remains that machine's only source today.
 *
 * Exported (P1 MariaDB UI design D2/D1) so the shared engine descriptor
 * (`databases.derive.ts`) can point at this exact sentence rather than a
 * second copy of the words. MariaDB's own descriptor entry returns `null`
 * instead: there is no Homebrew fallback for MariaDB anywhere in this app, so
 * that fact belongs here, on MySQL's side, and nowhere generic.
 */
export function mysqlUnavailableFallback(target: string): string {
	return `Homebrew is the way to install MySQL on ${target} today.`;
}

/** The honest-absence copy, shared by the row's own "you cannot install this
 *  here" note and by the (rarer) settled `unavailable` outcome, so the two
 *  cannot tell the user different stories about the same fact. */
function unavailableBody(target: string): string {
	return (
		`OpenVHost only ships a MySQL download whose checksum it has verified, and it has none for ` +
		`${target}. Oracle does publish a build for it — OpenVHost has just not verified those bytes, ` +
		`and shipping an unchecked download is the one thing this pipeline exists to refuse. ` +
		`${mysqlUnavailableFallback(target)}`
	);
}

/**
 * What the row says under the Install control before anything is installed:
 * exactly what pressing it will do, or exactly why there is nothing to press.
 */
export function mysqlPackageOfferNotice(offer: MysqlPackageOfferDto): Notice {
	switch (offer.kind) {
		case 'available':
			return {
				tone: 'ok',
				title: `Installs MySQL ${offer.version}`,
				body:
					"Downloads it from Oracle, checks it against OpenVHost's built-in SHA-256, and unpacks it " +
					"into OpenVHost's own packages folder. Homebrew is not used and nothing outside OpenVHost " +
					'is changed.'
			};
		case 'unavailable':
			return {
				tone: 'warn',
				title: `MySQL cannot be installed on ${offer.target}`,
				body: unavailableBody(offer.target)
			};
		default: {
			const unreachable: never = offer;
			return unreachable;
		}
	}
}

/** Whether the Install control exists at all. `false` is an ABSENCE — the row
 *  renders {@link mysqlPackageOfferNotice}'s explanation instead of a button
 *  that would throw. */
export function mysqlInstallOffered(offer: MysqlPackageOfferDto): boolean {
	switch (offer.kind) {
		case 'available':
			return true;
		case 'unavailable':
			return false;
		default: {
			const unreachable: never = offer;
			return unreachable;
		}
	}
}

/**
 * The small provenance badge beside an installed runtime's name — which
 * install put those binaries there.
 *
 * Two sources coexist by design during the migration (D3/D7), and the owner
 * will be running a brew-installed 8.4 and a packaged 8.4 at the same time, so
 * "which mysqld am I actually running" needs an answer on screen.
 *
 * A Homebrew badge shows **no version at all**. OpenVHost does not know brew's
 * exact patch release — finding out means executing a 55 MB `mysqld`, the
 * measurement that put design D4 in the spec — and printing the major (`8.4`)
 * where a full version belongs would be a lie the user could not detect. The
 * row's own heading already says `MySQL 8.4`; the badge adds provenance, never
 * a guess.
 */
export function mysqlSourceBadge(
	source: MysqlRuntimeSourceDto | null
): { label: string; title: string } | null {
	if (source === null) return null;
	switch (source.kind) {
		case 'packaged':
			return {
				label: `OpenVHost ${source.version}`,
				title:
					`Installed by OpenVHost from Oracle's tarball and checksum-verified. Exact version ` +
					`${source.version}, recorded at install time.`
			};
		case 'homebrew':
			return {
				label: 'Homebrew',
				title:
					'Installed by Homebrew, not by OpenVHost. OpenVHost does not know its exact patch ' +
					'version and will not guess one.'
			};
		default: {
			const unreachable: never = source;
			return unreachable;
		}
	}
}

/**
 * Whether an Uninstall control may be offered for this runtime.
 *
 * `false` for a packaged runtime, and that is a real limit rather than a
 * styling choice: the uninstall slice drives `brew uninstall`, and
 * `openvhost-pkg` has **no uninstall counterpart at all** yet — removing a
 * packaged version is its own slice. An affordance that is present and fails is
 * worse than one that is absent, so the row omits it.
 */
export function mysqlUninstallOffered(source: MysqlRuntimeSourceDto | null): boolean {
	if (source === null) return false;
	switch (source.kind) {
		case 'packaged':
			return false;
		case 'homebrew':
			return true;
		default: {
			const unreachable: never = source;
			return unreachable;
		}
	}
}

/** The one-line explanation for a packaged runtime that has no Uninstall
 *  button, so its absence reads as a known limit rather than an oversight. */
export const PACKAGED_UNINSTALL_UNAVAILABLE =
	'Removing a version OpenVHost installed itself is not built yet — it is the next slice of the ' +
	'move off Homebrew. Nothing here can delete it by accident in the meantime.';

/** A ledger row that could not be written. `null` on the happy path: the tree
 *  is the inventory, so a missing row costs provenance, never the install —
 *  reporting a demonstrably installed MySQL as a failure would be the bigger
 *  lie. */
export function mysqlLedgerNotice(ledger: MysqlLedgerWriteDto): string | null {
	switch (ledger.kind) {
		case 'recorded':
			return null;
		case 'failed':
			return (
				`MySQL is installed and usable, but OpenVHost could not record it in its own database: ` +
				`${ledger.reason}. The version is still detected from the packages folder.`
			);
		default: {
			const unreachable: never = ledger;
			return unreachable;
		}
	}
}

/** The Cancel control's label. A state, not a boolean dressed as copy: a
 *  cancel that has been asked for but not yet taken effect must not look
 *  clickable-and-idle, which invites a second press at the exact moment the
 *  first one is unwinding a staging directory. */
export function mysqlCancelLabel(cancelling: boolean): string {
	return cancelling ? 'Cancelling…' : 'Cancel';
}
