// SPDX-License-Identifier: GPL-3.0-or-later
// Pure copy + classification for installing MariaDB from OpenVHost's own
// GitHub release (P1 MariaDB UI design). The MariaDB mirror of
// `mysql-install.derive.ts` — a SEPARATE file rather than widened functions
// there, because `MariadbInstallResultDto` is not a subtype of
// `MysqlInstallResultDto` (it adds `awaitingRelease`, design D2/D5) and the
// wording genuinely differs (OpenVHost's own GitHub release, never Oracle;
// no Homebrew fallback anywhere in this app). Reusing `mysqlInstallResultNotice`
// unchanged for MySQL and adding this file for MariaDB is what keeps MySQL's
// own copy — and its tests — byte-for-byte untouched by this slice.
//
// Same rule `mysql-install.derive.ts` states for itself: NO WILDCARD ARM over
// any union this file consumes. Each `switch` ends in
// `const unreachable: never = x`, so a ninth pipeline stage or an added
// offer/result state fails TYPECHECK here rather than silently rendering
// nothing.
//
// Task 3 finding: `MysqlRow.svelte`'s settled-install banner
// (`mysqlInstallResultNotice`) and offer notice (`mysqlPackageOfferNotice`)
// were called directly, unconditionally, for EVERY engine — so a MariaDB
// install rendered "MySQL 11.4.9 installed" / "Downloads it from Oracle…"
// until this file existed and `MysqlRow.svelte` was taught to dispatch on
// `engine`. See that file's own `outcomeNotice`/`offerNotice` doc comments.

import type { MariadbInstallResultDto, MariadbLedgerWriteDto, MysqlPackageOfferDto } from './ipc';
import { engineAwaitingReleaseNotice, engineDescriptor } from './databases.derive';
import type { Notice } from './mysql-install.derive';

/**
 * The honest-absence copy for a target this build has no verified MariaDB
 * download for. Unlike `mysql-install.derive.ts`'s own `unavailableBody`,
 * this names NO fallback route: there is no Homebrew fallback for MariaDB
 * anywhere in this app (design D2), so the sentence stops at the fact rather
 * than inventing a next step that does not exist.
 */
function mariadbUnavailableBody(target: string): string {
	return (
		`OpenVHost only ships a MariaDB download whose checksum it has verified, and it has none for ` +
		`${target}. MariaDB has never gone through Homebrew in this app, so there is no other route to ` +
		'install it on this machine today.'
	);
}

/**
 * What the row says under the Install control before anything is installed
 * — the MariaDB mirror of `mysqlPackageOfferNotice`. Typed against the same
 * two-member `MysqlPackageOfferDto` shape that function takes (rather than
 * `MariadbPackageOfferDto` directly): both engines' rows build this value
 * fresh from `MysqlRowState`'s own `unavailable`/`notInstalled` payload,
 * never from the raw wire DTO, and `awaitingRelease` is never passed here —
 * it renders through {@link engineAwaitingReleaseNotice} instead, because a
 * build existing but unpublished is a materially different fact (design D2),
 * not a naming variant of this one.
 */
export function mariadbPackageOfferNotice(offer: MysqlPackageOfferDto): Notice {
	switch (offer.kind) {
		case 'available':
			return {
				tone: 'ok',
				title: `Installs MariaDB ${offer.version}`,
				body:
					"Downloads it from OpenVHost's own GitHub release, checks it against OpenVHost's built-in " +
					"SHA-256, and unpacks it into OpenVHost's own packages folder. Homebrew is not used and " +
					'nothing outside OpenVHost is changed.'
			};
		case 'unavailable':
			return {
				tone: 'warn',
				title: `MariaDB cannot be installed on ${offer.target}`,
				body: mariadbUnavailableBody(offer.target)
			};
		default: {
			const unreachable: never = offer;
			return unreachable;
		}
	}
}

/**
 * What to say once a MariaDB install has settled — the mirror of
 * `mysqlInstallResultNotice`, exhaustive over `MariadbInstallResultDto`'s
 * EIGHT members (MySQL's seven plus `awaitingRelease`, design D5). The
 * `awaitingRelease` arm reuses {@link engineAwaitingReleaseNotice} verbatim
 * rather than a near-duplicate sentence, so the settled-outcome banner and
 * the row's own not-yet-installed notice can never drift apart.
 *
 * `awaitingRelease` here is defensive rather than reachable in practice:
 * `install_mariadb` consults the SAME compiled-in `Availability` the row's
 * own offer does, so a row showing `notInstalled` (which requires
 * `Availability::Published`) cannot itself receive `AwaitingRelease` back
 * from the SAME running process. The arm still has to exist — Rust's own
 * `MariadbInstallResultDto` is eight-wide regardless of which paths a
 * particular UI reaches — and leaving it unhandled would be exactly the kind
 * of wildcard-shaped gap this codebase's exhaustiveness rule exists to
 * forbid.
 */
export function mariadbInstallResultNotice(result: MariadbInstallResultDto): Notice {
	switch (result.kind) {
		case 'installed':
			return result.detected
				? {
						tone: 'ok',
						title: `MariaDB ${result.version} installed`,
						body:
							"Downloaded from OpenVHost's own GitHub release, checked against OpenVHost's built-in " +
							"SHA-256, and unpacked into OpenVHost's own packages folder. Homebrew was never involved."
					}
				: {
						tone: 'warn',
						title: `MariaDB ${result.version} unpacked, but its programs were not found`,
						body:
							'The download and the checksum were fine, but mariadbd, mariadb or mariadb-admin is ' +
							'missing from the extracted files, so OpenVHost cannot run this version. Nothing else ' +
							'on this machine was changed.'
					};
		case 'alreadyInstalled':
			return {
				tone: 'ok',
				title: `MariaDB ${result.version} was already installed`,
				body: 'Nothing was downloaded — that exact version is already in the packages folder.'
			};
		case 'cancelled':
			return {
				tone: 'warn',
				title: 'Install cancelled',
				body:
					'Nothing was installed and no half-downloaded files were left behind. Your existing ' +
					'MariaDB, data directory and password are untouched.'
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
		case 'awaitingRelease':
			return engineAwaitingReleaseNotice(engineDescriptor('mariadb'), result.tag);
		case 'unavailable':
			return {
				tone: 'warn',
				title: `No verified MariaDB download for ${result.target}`,
				body: mariadbUnavailableBody(result.target)
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
 * A ledger row that could not be written — the MariaDB mirror of
 * `mysqlLedgerNotice`. `null` on the happy path: the package tree is the
 * inventory, so a missing row costs provenance, never the install.
 */
export function mariadbLedgerNotice(ledger: MariadbLedgerWriteDto): string | null {
	switch (ledger.kind) {
		case 'recorded':
			return null;
		case 'failed':
			return (
				`MariaDB is installed and usable, but OpenVHost could not record it in its own database: ` +
				`${ledger.reason}. The version is still detected from the packages folder.`
			);
		default: {
			const unreachable: never = ledger;
			return unreachable;
		}
	}
}
