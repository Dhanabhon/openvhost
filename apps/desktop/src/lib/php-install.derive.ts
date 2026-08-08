// SPDX-License-Identifier: GPL-3.0-or-later
// Pure copy + classification for the Languages page, once the page stopped
// requiring Homebrew (off-Homebrew slice 5C, design D2/D3/D4). Every
// user-visible string on the PHP install path lives here so the components and
// their tests share one wording and a future i18n extraction has a single file
// to walk — the same job `mysql-install.derive.ts` does for the Databases page.
//
// The rule this module exists to keep, copied from that file because it earned
// its place there: NO WILDCARD ARM over any union it consumes. Every `switch`
// ends in `const unreachable: never = x`, so a fourth offer state or a tenth
// install result fails TYPECHECK here rather than silently rendering as
// whichever branch happened to come last — or, worse, as nothing at all.

import { formatBytes } from './logs.derive';
import type {
	PhpInstallProgressDto,
	PhpInstallResultDto,
	PhpPackageOfferDto,
	PhpRuntimeSourceDto
} from './ipc';

/**
 * One line describing the pipeline step the user is currently watching on the
 * PACKAGED route — the mirror of `mysqlInstallProgressLabel`.
 *
 * `total` is the length the server declared, carried forward from the `started`
 * event because no later event repeats it. `null` when the server declared
 * none, which is a real case and renders as an honest "so far" rather than a
 * fabricated percentage.
 *
 * MANDATORY, inherited verbatim from MySQL's own copy because the reason is
 * identical: `verified` and `extracted` must never render the same sentence.
 * They are the difference between a download that was checked against the
 * compiled-in SHA-256 and one that merely arrived, which is precisely what
 * golden rule 6 buys — and a guarantee nobody can see is a guarantee nobody
 * has. `php-install.derive.test.ts` asserts every pair of these five is
 * distinct, not merely that each is non-empty.
 *
 * A SEPARATE function from `mysqlInstallProgressLabel` rather than a reuse of
 * it, for the reason `mariadb-install.derive.ts`'s header already states about
 * its own copies: the two DTOs are structurally identical today, so TypeScript
 * would happily accept one function for both — and that is the trap, not the
 * saving. A wording change made for MySQL's Oracle download would silently
 * become PHP's, and this codebase has already shipped the mirror-image bug four
 * times in one slice (a "generalized" component still saying MySQL).
 */
export function phpInstallProgressLabel(
	progress: PhpInstallProgressDto,
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
			return "Installed — the files are in place in OpenVHost's own packages folder";
		default: {
			const unreachable: never = progress;
			return unreachable;
		}
	}
}

/**
 * How far along the transfer is, 0–100, or `null` when there is nothing honest
 * to draw: before the first byte, when the server declared no length, and for
 * every step after the download (which are moments, not durations, and would
 * otherwise animate a bar that is really just waiting).
 */
export function phpInstallProgressPercent(
	progress: PhpInstallProgressDto,
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
 *  every later `downloaded` event needs it as a denominator and none of them
 *  repeats it. */
export function phpInstallDeclaredTotal(progress: PhpInstallProgressDto): number | null {
	return progress.kind === 'started' ? progress.total : null;
}

/**
 * The small provenance badge beside an installed row's version — which install
 * put those binaries there (design D3).
 *
 * **A Homebrew row gets NO badge, and that is the one deliberate difference
 * from `mysqlSourceBadge`/`nginxSourceBadge`**, which both label their own
 * Homebrew runtimes. Two reasons, and the second is the binding one:
 *
 *  1. Those two pages badge Homebrew because a packaged and a brewed runtime
 *     coexist there *today*, so an unlabelled row would be ambiguous. On the
 *     Languages page nothing is packaged yet — every installed PHP on every
 *     real machine is a keg — so a "Homebrew" chip on all five rows would be
 *     noise that says the same thing five times.
 *  2. Spec §5 says it outright ("`Packaged` rows carry a source badge,
 *     Homebrew rows carry none"), and spec §8.6 makes it testable: this slice
 *     must change nothing on a machine with Homebrew and no package tree. A
 *     badge on every brewed row would be a visible change to every real
 *     machine today, which is exactly what §8.6 forbids.
 *
 * A packaged badge names the exact patch level, and costs **nothing** to
 * produce: `packaged` carries the version because OpenVHost's own install
 * wrote it down as a directory name (`packages/php/8.4/8.4.24/`). Nothing is
 * executed to learn it — that asymmetry is what slice 5B built and what this
 * badge finally spends.
 */
export function phpSourceBadge(
	source: PhpRuntimeSourceDto | null
): { label: string; title: string } | null {
	if (source === null) return null;
	switch (source.kind) {
		case 'packaged':
			return {
				label: `OpenVHost ${source.version}`,
				title:
					`Installed by OpenVHost from its own PHP build, checksum-verified. Exact version ` +
					`${source.version}, read from the package tree — never probed.`
			};
		case 'homebrew':
			return null;
		default: {
			const unreachable: never = source;
			return unreachable;
		}
	}
}

/**
 * Whether this row can offer an Install button that could actually work.
 *
 * `brewFound` is an INPUT to a per-row answer here, not a page-level gate —
 * that is the whole of design D2. Homebrew genuinely is required for most
 * rows, permanently: only 8.4 is pinned, and on Intel nothing is packaged at
 * all, so `Unavailable` is the ordinary path and not a failure path. What was
 * wrong was answering a per-major question with one machine-wide bool.
 *
 * The routing table mirrored here is `php_pkg::route_for`, and it is mirrored
 * rather than re-derived: an `Available` offer goes to the package pipeline,
 * and BOTH other states go to Homebrew. So with no Homebrew on the machine,
 * `install_php` on either of those two states fails at
 * `find_brew().ok_or_else(...)` with "Homebrew was not found" before anything
 * is spawned. A button whose only outcome is that error is the affordance this
 * codebase keeps having to delete; {@link phpNoRouteNote} is what replaces it.
 */
export function phpInstallOffered(offer: PhpPackageOfferDto, brewFound: boolean): boolean {
	switch (offer.kind) {
		case 'available':
			// Our own bytes, for this host, verified. Needs no Homebrew at all —
			// the row this whole programme exists for.
			return true;
		case 'awaitingRelease':
			// `route_for` sends this to Homebrew: the build is finished and
			// audited, but no release serves it yet, so there is nothing to
			// download. Today this is what 8.4 reports on Apple Silicon, and its
			// Homebrew Install button still works — withholding it would remove a
			// working control.
			return brewFound;
		case 'unavailable':
			// No package for this major on this host. Four of five majors today,
			// and every major on Intel. Homebrew is the supported route here, not
			// a fallback to apologise for.
			return brewFound;
		default: {
			const unreachable: never = offer;
			return unreachable;
		}
	}
}

/**
 * Why this row has no Install button, in its own words — `null` whenever it
 * has one.
 *
 * Absent affordance, present explanation. This page has shipped the other
 * shape twice (C2/C3) and both times the user was left pressing nothing and
 * learning nothing. Per row, never page-wide: on a machine with a packaged 8.4
 * and no Homebrew, 8.4 is installable and 8.1/8.3/8.5 are not, and one
 * sentence cannot be true of both.
 *
 * Returns `null` on every state a machine with Homebrew can reach, which is
 * what keeps spec §8.6 true: with `brewFound`, no row anywhere on the page
 * gains a line of text.
 */
export function phpNoRouteNote(
	offer: PhpPackageOfferDto,
	major: string,
	brewFound: boolean
): string | null {
	switch (offer.kind) {
		case 'available':
			return null;
		case 'awaitingRelease':
			return brewFound
				? null
				: `Installing PHP ${major} needs Homebrew, which was not found. OpenVHost's own ` +
						`PHP ${major} build is finished, but the release that would serve it ` +
						`(${offer.tag}) has not been published yet — that step belongs to OpenVHost's ` +
						`maintainers, not to you.`;
		case 'unavailable':
			return brewFound
				? null
				: `Installing PHP ${major} needs Homebrew, which was not found. OpenVHost ` +
						`publishes no PHP ${major} package for ${offer.target}.`;
		default: {
			const unreachable: never = offer;
			return unreachable;
		}
	}
}

/** The shape {@link noRouteToAnyPhp} reads off a row — structurally a
 *  `PhpRuntimeDto`, narrowed to the two fields the answer depends on so a test
 *  fixture does not have to build a whole runtime to ask the question. */
export interface PhpRouteRow {
	installed: boolean;
	offer: PhpPackageOfferDto;
}

/**
 * Whether this machine has **no route to any PHP at all** — the one case the
 * page-level "Homebrew is required to install PHP" screen was actually written
 * for (design D2).
 *
 * Nothing installed, and nothing installable by any route. Deliberately
 * defined in terms of {@link phpInstallOffered} rather than restating the
 * rule, so the screen and the rows can never disagree: the dead end appears
 * exactly when no row on the page has an Install button and no PHP is already
 * there. A row-level change cannot leave this stale.
 *
 * `brewFound` short-circuits first, and it has to: an empty `runtimes` list on
 * a machine that HAS Homebrew is a page still loading its catalogue, not a
 * dead end.
 */
export function noRouteToAnyPhp(env: {
	brewFound: boolean;
	runtimes: readonly PhpRouteRow[];
}): boolean {
	if (env.brewFound) return false;
	if (env.runtimes.some((r) => r.installed)) return false;
	return !env.runtimes.some((r) => phpInstallOffered(r.offer, env.brewFound));
}

/**
 * What the "install PHP to get started" invitation says about HOW it will be
 * installed.
 *
 * The Homebrew sentence is verbatim what this page has always shown and is
 * still exactly right on a machine with Homebrew — which is every real machine
 * today, so §8.6 holds. It is only wrong on the machine D2 exists for: no
 * Homebrew, but a packaged version there for the taking. Saying "OpenVHost
 * installs it through Homebrew" there is the same page-wide claim about a
 * per-major fact that D2 removes one paragraph up.
 */
export function phpInstallInvite(brewFound: boolean): string {
	return brewFound
		? 'Choose a version below — OpenVHost installs it through Homebrew and serves your sites with it.'
		: 'Choose a version below — OpenVHost installs it from its own verified package and serves your sites with it.';
}

/** The three slots a settled install fills on its row. Separate fields rather
 *  than one union because the row renders them in three different places, with
 *  three different roles (`alert`, `alert`, `status`) — and because `alert` and
 *  `succeeded` can genuinely both be set: brew can exit non-zero having already
 *  created the formula directory. Collapsing them would be the
 *  state-into-a-boolean shape this UI has shipped five times. */
export interface PhpOutcomeRender {
	/** The failure line, rendered `.error` / `role="alert"`. */
	alert: string | null;
	/** The "it says it worked, but…" line, rendered `.warn` / `role="alert"`.
	 *  Only ever set when {@link alert} is null. */
	warning: string | null;
	/** Whether a runtime is now on disk because of this attempt. Combined with
	 *  the row's own `installed` before it becomes the success message, so a
	 *  claim of success is never made about a row the re-read did not confirm. */
	succeeded: boolean;
}

/**
 * Classify one settled `install_php` result into what its row shows.
 *
 * **Every arm, exhaustively.** `PhpInstallResultDto` gained eight arms beyond
 * `Brew` in this slice (design D4), and a `kind === 'brew'` test would leave
 * all eight rendering nothing at all — no error, no warning, no success. That
 * is the C1 defect this file already fixed once for brew's own non-zero exit,
 * and re-introducing it on the packaged path would be the same bug in a new
 * costume: a failed install that looks exactly like an install nobody started.
 *
 * The packaged arms are **unreachable today** — every offer this build can make
 * is `AwaitingRelease` or `Unavailable`, both of which `route_for` sends to
 * Homebrew — so their copy ships unproven by construction, exactly as spec §6
 * says the packaged path does. What is proven is that none of them renders
 * silence.
 *
 * The `brew` arm's three answers are unchanged from before this slice, down to
 * the wording, because that path is the one every real machine still takes.
 */
export function phpOutcomeRender(result: PhpInstallResultDto, major: string): PhpOutcomeRender {
	switch (result.kind) {
		case 'brew': {
			// `exitCode !== 0` covers both a real non-zero code and `null` (killed
			// by a signal): both are "not a clean exit". Right HERE and wrong
			// anywhere else — the packaged arms have no exit code because they
			// spawn no child, which is precisely why they are separate arms.
			const alert =
				result.exitCode !== 0
					? result.exitCode === null
						? `brew was killed before installing PHP ${major} finished. ` +
							`Check the log above for what brew actually did.`
						: `brew exited with code ${result.exitCode} while installing PHP ${major}. ` +
							`Check the log above for what brew actually did.`
					: null;
			return {
				alert,
				// Brew exited 0 and the formula directory still is not there.
				// Silence here is the failure this answer prevents: without it the
				// user just presses Install again with nothing explaining why
				// nothing happened.
				warning:
					alert === null && !result.detected
						? `Homebrew reported success installing PHP ${major}, but the version was not ` +
							`found afterwards. Check the log above for what brew actually did.`
						: null,
				succeeded: result.detected
			};
		}
		case 'installed':
			return {
				alert: null,
				// The packaged mirror of brew's silent failure: the tree is on disk
				// and `current` points at it, but no php-fpm was found under it.
				warning: result.detected
					? null
					: `OpenVHost installed PHP ${result.version}, but its php-fpm was not found ` +
						`afterwards.`,
				succeeded: result.detected
			};
		case 'alreadyInstalled':
			// Not a failure and not news: the tree was already there at the pinned
			// version. The row's re-read shows it as installed, which is the whole
			// answer.
			return { alert: null, warning: null, succeeded: true };
		case 'cancelled':
			// "Nothing happened", not a failure to explain away — staging unwound
			// with the dropped future. The only ways to reach it today are an
			// explicit cancel and `perform_quit`, and neither wants an alert.
			return { alert: null, warning: null, succeeded: false };
		case 'verificationFailed':
			// Golden rule 6's whole point, and the one failure that must never
			// read as a transient glitch: the bytes were not the bytes we pinned.
			return {
				alert:
					`The PHP ${major} download did not match its expected checksum, so nothing was ` +
					`installed. Expected ${result.expected}, got ${result.actual}.`,
				warning: null,
				succeeded: false
			};
		case 'stalled':
			return {
				alert:
					`The PHP ${major} download stalled and was abandoned, so nothing was installed. ` +
					`${result.detail}`,
				warning: null,
				succeeded: false
			};
		case 'awaitingRelease':
			// `route_for` sends an `AwaitingRelease` OFFER to Homebrew, so reaching
			// this means the catalogue changed under a run already in flight.
			return {
				alert:
					`OpenVHost's PHP ${major} package has not been published yet (release ` +
					`${result.tag}), so nothing was installed. That step belongs to OpenVHost's ` +
					`maintainers, not to you.`,
				warning: null,
				succeeded: false
			};
		case 'unavailable':
			return {
				alert:
					`OpenVHost publishes no PHP ${major} package for ${result.target}, so nothing ` +
					`was installed.`,
				warning: null,
				succeeded: false
			};
		case 'failed':
			return {
				alert: `Installing PHP ${major} failed. ${result.reason}`,
				warning: null,
				succeeded: false
			};
		default: {
			const unreachable: never = result;
			return unreachable;
		}
	}
}
