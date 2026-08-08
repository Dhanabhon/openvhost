// SPDX-License-Identifier: GPL-3.0-or-later
// Pure copy + decisions for the chosen default PHP (default-PHP design D2/D6).
//
// Separate from `php-install.derive.ts` even though both serve the Languages
// page: that module is about getting a version ONTO the machine, this one is
// about which of the versions already there answers `localhost:8080`. Keeping
// them apart means a wording change to one cannot silently become the other —
// the mirror-image mistake this codebase shipped four times in one slice when a
// "generalized" component kept saying MySQL.
//
// The rule inherited verbatim from that file: NO WILDCARD ARM over
// `DefaultPhpDto`. Every `switch` ends in `const unreachable: never`, so a
// fifth resolution state fails TYPECHECK here rather than rendering as
// whichever branch happened to come last — or, worse, as nothing at all, which
// for this union means a user silently served a PHP they did not choose.

import type { DefaultPhpDto } from './ipc';

/**
 * Whether `major` is the version the user CHOSE — never merely the version
 * that happens to be served.
 *
 * `Unset` returns `false` for every major, and that is the load-bearing part.
 * On every machine that predates this slice the catch-all serves *something*,
 * and badging that row "Default" would assert a choice nobody made — the exact
 * conflation design D2 exists to forbid, and a visible change to every real
 * machine today besides.
 *
 * `PreferredMissing` returns `false` for its `serving` major too, for the same
 * reason: that major is a fallback, not a decision. What the user chose is
 * named by {@link defaultPhpNotice} instead, which can say so even when the
 * chosen major has no row left on the page.
 */
export function isChosenDefault(resolved: DefaultPhpDto, major: string): boolean {
	switch (resolved.kind) {
		case 'nothingInstalled':
			return false;
		case 'unset':
			return false;
		case 'preferred':
			return resolved.major === major;
		case 'preferredMissing':
			return false;
		default: {
			const unreachable: never = resolved;
			return unreachable;
		}
	}
}

/**
 * Whether a preference is stored at all — `Preferred` or `PreferredMissing`,
 * never `Unset`.
 *
 * The two "a choice was made" states, named once so
 * {@link offersDefaultChoice} does not have to restate the union.
 */
export function hasStoredDefault(resolved: DefaultPhpDto): boolean {
	switch (resolved.kind) {
		case 'nothingInstalled':
		case 'unset':
			return false;
		case 'preferred':
		case 'preferredMissing':
			return true;
		default: {
			const unreachable: never = resolved;
			return unreachable;
		}
	}
}

/**
 * Whether the "Make default" control appears on the page at all.
 *
 * **Design D6 leaves this to the implementer, and this is the call: the control
 * appears when the choice is real, or when a choice has already been made.**
 *
 *  * **Two or more installed majors** — a genuine choice exists, and it is the
 *    case the whole slice is for. Offered.
 *  * **One installed major, nothing chosen** — the spec's own words: "the
 *    answer is not in doubt". A button whose only possible effect is to store
 *    what already happens is the affordance-that-changes-nothing this page
 *    keeps having to delete. Not offered — which is also what keeps a
 *    one-PHP machine (the common case) pixel-identical to before this slice.
 *  * **Nothing installed** — nothing to choose between. Not offered.
 *  * **A preference already stored, whatever the count** — offered, and this
 *    clause is the one that matters. Uninstall down to a single major while a
 *    preference names a different one and the answer IS in doubt: without this,
 *    the user could neither see nor change a choice that is actively affecting
 *    what they are served. Gating purely on `installed >= 2` would strand
 *    exactly the state spec claim 4 exists to keep legible.
 *
 * Takes the installed COUNT rather than the rows, so the rule is stated in
 * terms of the only thing it depends on and a test does not have to build five
 * runtimes to ask a question about two.
 */
export function offersDefaultChoice(resolved: DefaultPhpDto, installedCount: number): boolean {
	return installedCount >= 2 || hasStoredDefault(resolved);
}

/**
 * The one sentence the page says about a default that cannot be honoured —
 * `null` in every other state.
 *
 * **Spec claim 4**: uninstalling the default must leave the state legible.
 * "Your default was 8.4, which is no longer installed", not a silent fallback
 * to 8.1. And legible is not enough on its own — the user has to be able to do
 * something, so both branches name the two ways out (put it back, or choose
 * one of the versions that is actually here).
 *
 * Page-level rather than per-row, deliberately, and this is the reason:
 * `requested` may have **no row at all**. Rows come from the catalogue plus
 * what is installed, so a preference for a hand-installed `php@7.4` that has
 * since been removed appears in neither list. A per-row note could not carry
 * the fact anywhere, and the fact is the whole point.
 *
 * Returns `null` for `unset` — every machine that has not chosen — which is
 * what keeps this slice invisible until someone does.
 */
export function defaultPhpNotice(resolved: DefaultPhpDto): string | null {
	switch (resolved.kind) {
		case 'nothingInstalled':
			return null;
		case 'unset':
			return null;
		case 'preferred':
			return null;
		case 'preferredMissing':
			return resolved.serving === null
				? `Your default PHP is ${resolved.requested}, which is not installed — and neither is ` +
						`any other version, so localhost:8080 serves no PHP at all. Install PHP ` +
						`${resolved.requested} to get it back, or install another version and make that ` +
						`the default.`
				: `Your default PHP is ${resolved.requested}, which is no longer installed. ` +
						`localhost:8080 is being served by PHP ${resolved.serving} instead. Install PHP ` +
						`${resolved.requested} to get it back, or make another installed version the ` +
						`default.`;
		default: {
			const unreachable: never = resolved;
			return unreachable;
		}
	}
}

/**
 * The title above {@link defaultPhpNotice}, so the banner leads with what
 * happened rather than with a paragraph.
 */
export const DEFAULT_PHP_MISSING_TITLE = 'Your default PHP is not installed';

/** The row control's label. Centralised here with the rest of this surface's
 *  copy so a future i18n extraction has one file to walk. */
export const MAKE_DEFAULT_LABEL = 'Make default';

/**
 * The button's text, which relabels while THIS row's write is in flight.
 *
 * The `settingDefault` marker is a string rather than a boolean precisely so a
 * row can tell "somebody else is busy" (disabled) from "it is me" (disabled AND
 * relabelled) — and the doc on that field said so while the button rendered a
 * constant, which is the second half of a claim this slice shipped without.
 * Mirrors `uninstallConfirmLabel`, which does the same for its own row.
 */
export function makeDefaultLabel(inFlight: boolean): string {
	return inFlight ? 'Setting…' : MAKE_DEFAULT_LABEL;
}

/** The badge on the row the user chose. */
export const DEFAULT_BADGE_LABEL = 'Default';

/** The badge's tooltip — what "default" actually means, since the word alone
 *  could be read as "the recommended one" (a badge that already exists two
 *  chips to the left). */
export const DEFAULT_BADGE_TITLE =
	'Serves localhost:8080 and any request that matches no site. Sites keep their own PHP version.';

/** The `title` on a Make default button, saying what pressing it will do —
 *  including that it does not take effect until Apply, which is the part a user
 *  would otherwise have to discover from the dialog that opens. */
export function makeDefaultTitle(major: string): string {
	return (
		`Make PHP ${major} serve localhost:8080. You will see the change as a diff and apply it ` +
		`yourself; your sites keep their own PHP version.`
	);
}
