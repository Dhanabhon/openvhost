// SPDX-License-Identifier: GPL-3.0-or-later
// What the landing page says a site actually needs (first-run readiness design
// D1–D3).
//
// Sites is the landing route, so it is the first screen a new user sees. It has
// announced a missing PHP since an earlier slice and said NOTHING about nginx —
// and serving a site needs both. On a machine with PHP but no nginx the page
// showed no banner at all, invited the user to add a site, and the site did not
// serve. This module owns the rule so the page has nothing left to decide, the
// same way `php-install.derive.ts` and `php-default.derive.ts` own theirs.
//
// The rule inherited from those files: NO WILDCARD ARM over `ReadinessCheck`.
// The one `switch` here ends in `const unreachable: never`, so a fourth read
// outcome fails TYPECHECK rather than silently falling into whichever branch
// happened to come last — which for this union means either a false claim about
// the user's machine or a silent page, and this UI has shipped both.

import type { WebServerDto } from './ipc';

/**
 * What a requirement read has told us so far.
 *
 * Three states, not a boolean, because a read has three honest outcomes and
 * this page has already shipped the two-state version of this bug twice:
 *
 *  * `unknown` — we have not looked yet, **or** the look failed. Either way the
 *    page must say nothing about this requirement. `phpEnvKnown` exists on the
 *    Sites page for exactly the first half (a banner claiming "nothing
 *    installed" before the read returns would flash a false claim on every load
 *    of every machine that does have PHP); the I2 audit finding is the second
 *    half (a FAILED read used to render the same "nothing installed" claim as a
 *    genuinely empty one — a false claim about the machine, stated as fact).
 *    Both collapse to `unknown` HERE because the readiness banner's answer to
 *    both is identical: don't claim. The two are still distinguished on the
 *    page, which renders a separate error banner for the failed read.
 *  * `present` — confirmed installed.
 *  * `absent` — confirmed missing. The only state that may produce a claim.
 */
export type ReadinessCheck = 'unknown' | 'present' | 'absent';

/** The two things serving a site needs. Databases are deliberately not here
 *  (design D4): a site serves without one, and listing it would make this
 *  banner cry wolf on a machine that is fine. */
export type RequirementId = 'php' | 'nginx';

/** Where the remedy lives. A route id rather than a resolved href so the call
 *  site keeps its own visible `resolve(...)` — the convention `logs.derive.ts`
 *  documents, and what `svelte/no-navigation-without-resolve` inspects. */
export type RemedyRoute = '/languages' | '/web-server';

interface Requirement {
	readonly id: RequirementId;
	/** The banner's title when this is the ONLY thing missing, where there is
	 *  room to lead with the specific fact rather than a summary. */
	readonly headline: string;
	/** The fact as a sentence, for the list rendered when more than one thing is
	 *  missing and the title has to summarise instead. */
	readonly lead: string;
	/** Why it matters — the line shown under {@link headline} when this is the
	 *  only thing missing, where the title has already stated the fact. */
	readonly why: string;
	readonly route: RemedyRoute;
	readonly linkText: string;
}

/** One rendered line: a sentence, and the one place to go about it. */
export interface ReadinessLine {
	readonly id: RequirementId;
	/** Already resolved against how many things are missing, so the component
	 *  has no copy decision left to make — and cannot disagree with the title
	 *  that {@link siteReadiness} chose alongside it. */
	readonly text: string;
	readonly route: RemedyRoute;
	readonly linkText: string;
}

export interface ReadinessNotice {
	readonly title: string;
	/** Never empty — a `ReadinessNotice` exists only because something is
	 *  missing. `siteReadiness` returns `null` rather than an empty notice. */
	readonly lines: readonly ReadinessLine[];
}

/**
 * PHP's remedy is the Languages page, which owns installing a version with
 * live output and its own error states. The wording is carried over VERBATIM
 * from the banner this replaces — the existing copy is good and is not the
 * thing being fixed (design §7.2).
 */
const PHP: Requirement = {
	id: 'php',
	headline: 'No PHP version is installed yet',
	lead: 'No PHP version is installed.',
	why: 'Sites need one to run.',
	route: '/languages',
	linkText: 'Install a version on the Languages page'
};

/**
 * nginx's remedy is the Web server page.
 *
 * Note what it deliberately does NOT say: "install nginx here". There is no
 * `install_nginx` command — the app installs PHP, MySQL and MariaDB and finds
 * nginx, it does not fetch it (see `list_web_servers`' own comment: "it stops
 * being fine the day nginx gains an install or rescan flow"). Design D4 rules
 * out growing a second install path on this page, so the honest link is to the
 * surface that owns the fact, and the copy promises nothing more than that.
 */
const NGINX: Requirement = {
	id: 'nginx',
	headline: 'nginx is not installed',
	lead: 'nginx is not installed.',
	why: 'Sites are served by nginx.',
	route: '/web-server',
	linkText: 'Check the Web server page'
};

/**
 * The title when more than one requirement is missing, where no single
 * headline can carry the fact. Plain and forward-looking per the brand voice
 * (§6.1: state what happened, offer the next action) — the list underneath
 * supplies both facts and both remedies.
 */
export const READINESS_MULTI_TITLE = "Sites can't run yet";

/**
 * Whether a check may be stated as an absence.
 *
 * `unknown` returns `false`, and that clause is the whole point of the type:
 * "we have not looked" and "the look failed" are not evidence of absence, and
 * treating either as one is the exact defect the `phpEnvKnown` gate and the I2
 * fix each exist to prevent.
 *
 * Exhaustive, with no wildcard arm — a fourth `ReadinessCheck` must be decided
 * about here rather than silently inheriting "not missing".
 */
function claimsAbsence(check: ReadinessCheck): boolean {
	switch (check) {
		case 'unknown':
			return false;
		case 'present':
			return false;
		case 'absent':
			return true;
		default: {
			const unreachable: never = check;
			return unreachable;
		}
	}
}

/**
 * The one readiness banner, or `null` when there is nothing honest to say.
 *
 * **One banner, never two stacked (design D1).** Two info banners on a first
 * run is noise, and the user would have to read both to answer the single
 * question they actually have — *can I serve a site yet?*
 *
 * `null` covers three different situations on purpose, because the page's
 * behaviour in all three is identical: both requirements present (every
 * developed machine today), neither read has returned yet, and a read that
 * failed. Only a CONFIRMED absence produces a notice.
 *
 * Order is fixed — PHP then nginx — rather than derived from which read
 * happened to settle first, so the banner does not reorder itself under the
 * user as the second response arrives.
 */
export function siteReadiness(php: ReadinessCheck, nginx: ReadinessCheck): ReadinessNotice | null {
	const missing: Requirement[] = [];
	if (claimsAbsence(php)) missing.push(PHP);
	if (claimsAbsence(nginx)) missing.push(NGINX);

	if (missing.length === 0) return null;
	// One missing requirement leads with its own headline and the line says why,
	// so a PHP-only machine reads exactly as it did before this slice. Two lose
	// that room: the title summarises and each line states its own fact instead.
	const only = missing.length === 1;
	return {
		title: only ? missing[0].headline : READINESS_MULTI_TITLE,
		lines: missing.map((r) => ({
			id: r.id,
			text: only ? r.why : r.lead,
			route: r.route,
			linkText: r.linkText
		}))
	};
}

/**
 * The PHP side of {@link siteReadiness}, from the majors the Sites page already
 * derives.
 *
 * `null` means unknown, and the page passes `phpEnvKnown ? installed : null` —
 * the SAME expression it already hands `SitesPanel`'s `installed` prop, which
 * is a tri-state for this exact reason. An empty array is NOT unknown: it is
 * the confirmed-empty environment the banner exists for. Collapsing the two is
 * the bug (`installedPhpVersions` reads `[]` while loading, after a failure,
 * and when genuinely empty).
 */
export function phpCheck(installedMajors: readonly string[] | null): ReadinessCheck {
	if (installedMajors === null) return 'unknown';
	return installedMajors.length === 0 ? 'absent' : 'present';
}

/** The row id `list_web_servers` gives nginx (`commands.rs`, `web_server_rows`). */
const NGINX_ROW_ID = 'nginx';

/**
 * The nginx side of {@link siteReadiness}, read off the web-server list.
 *
 * `null` means the read has not settled or it failed — same contract as
 * {@link phpCheck}, and the page passes `webServersKnown ? servers : null`.
 *
 * A list with **no nginx row at all** is `unknown`, not `absent`. Every list
 * `list_web_servers` builds contains one, so a list without one is a shape this
 * function does not recognise — and the rule this whole module is built on is
 * that we never claim absence from something we do not understand. Saying
 * "install nginx" because a DTO changed shape would be a false claim about the
 * machine, which is the failure mode, not a missed warning.
 *
 * `binaryPath` is the discriminator, not `source`: `source` is `null` both when
 * no nginx was found AND for the Apache row, and its own doc comment says it
 * "adds no second discriminator for 'is there a server here'". `binary_path` is
 * `p.nginx_bin.as_ref().map(...)` — `None` exactly when nginx was not found,
 * never an empty string, since `fallback_brew()`'s invented path was deleted in
 * slice 4B.
 */
export function nginxCheck(servers: readonly WebServerDto[] | null): ReadinessCheck {
	if (servers === null) return 'unknown';
	const row = servers.find((s) => s.id === NGINX_ROW_ID);
	if (row === undefined) return 'unknown';
	return row.binaryPath === null ? 'absent' : 'present';
}
