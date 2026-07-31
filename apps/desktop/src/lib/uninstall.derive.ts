// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers for uninstalling an installed package (package-uninstall design
// D6). Every user-visible word of the confirmation is computed here so
// `UninstallDialog.svelte` can stay a renderer: the dialog decides layout, this
// file decides what it says, and the tests assert the words rather than the
// markup — the same split `sites.derive.ts`/`databases.derive.ts` already use.
//
// Two rules shape this file:
//
//  1. **The paths come from the plan, never from here.** D6's whole point is
//     that the user learns their data is safe from a sentence naming the real
//     directory. A path hardcoded in this file would keep saying "safe" after
//     the executor started removing something else. {@link keptSentence}
//     therefore reads its paths out of `keeps` and nothing else.
//  2. **No wildcard arms.** `PackageKind` and `Blocker` are matched
//     exhaustively with the never-typed default arm this codebase already uses
//     (`scaffoldNotice`, `mysqlRowState`, `resetNoticeCopy`). A new package
//     kind or a new refusal reason must fail to compile here rather than
//     silently rendering nothing — this UI has shipped a
//     state-collapsed-into-one-rendering bug four times.

/**
 * Which package an uninstall targets. Structurally identical to the generated
 * binding Task 2 emits — the assignment in `uninstall.shared.svelte.ts` is
 * what pins the two together, so a drift in the Rust contract fails typecheck
 * at that seam instead of being discovered at runtime.
 */
export type PackageKind = 'php' | 'mysql';

/** One thing an uninstall leaves alone, with its on-disk location when it has
 *  one. `path: null` is a kept thing that is not a file — the stored root
 *  credential, a site's recorded PHP version. */
export interface KeptItem {
	what: string;
	path: string | null;
}

/**
 * Why an uninstall is refused (design D3). Both variants are REFUSALS, not
 * warnings: there is no force path, so the UI must never offer to proceed past
 * one.
 */
export type Blocker =
	| { kind: 'serviceNotTerminal'; id: string; state: string }
	| { kind: 'sitesPinned'; domains: string[] };

/**
 * What an uninstall would do, computed by Rust before anything is spawned.
 * `blockers` empty means it may proceed.
 */
export interface UninstallPlan {
	kind: PackageKind;
	major: string;
	/** Human-readable, ordered — rendered verbatim, never re-worded here. */
	removes: string[];
	keeps: KeptItem[];
	blockers: Blocker[];
}

/** How this app writes each package's name. Exhaustive over `PackageKind`,
 *  deliberately NO `default:` arm — a third kind fails typecheck here. */
export function packageLabel(kind: PackageKind): string {
	switch (kind) {
		case 'php':
			return 'PHP';
		case 'mysql':
			return 'MySQL';
		default: {
			const unreachable: never = kind;
			return unreachable;
		}
	}
}

/** The confirmation's question, in the user's words (D6). */
export function uninstallTitle(kind: PackageKind, major: string): string {
	return `Uninstall ${packageLabel(kind)} ${major}?`;
}

/** The one plain sentence naming what goes (D6). The itemised list of removals
 *  is rendered from `UninstallPlan.removes` alongside this — this sentence is
 *  the headline, not the inventory. */
export function uninstallLead(kind: PackageKind, major: string): string {
	return `This removes the ${packageLabel(kind)} ${major} program files.`;
}

/**
 * The sentence that makes this dialog safe to click: what SURVIVES, named with
 * the real paths out of `keeps` (design D2/D6).
 *
 * MySQL's wording is the owner's own, verbatim from D6 — "Your databases are
 * not touched … so reinstalling 8.4 picks up where you left off" — because
 * keeping the data and throwing away the root password would be the same as
 * destroying it, and this sentence is the only place the user is told
 * otherwise. PHP's is the same shape for the same reason, plus the honest
 * consequence D3 chose deliberately: a site pinned to this major keeps that
 * setting, and the apply pipeline will reject it until the user changes it or
 * reinstalls.
 *
 * The path is read out of `keeps` rather than templated in, so this sentence
 * cannot drift from what the executor actually spares — see
 * {@link keptPathsClause} for which path it names and why only one. A plan
 * whose kept items carry no path at all (nothing on disk survives, only stored
 * state) drops the clause instead of rendering an empty one.
 */
export function keptSentence(kind: PackageKind, major: string, keeps: readonly KeptItem[]): string {
	const stay = keptPathsClause(keeps);
	switch (kind) {
		case 'mysql':
			return `Your databases are not touched${stay}, and your root password is kept, so reinstalling ${major} picks up where you left off.`;
		case 'php':
			return `Your logs are not touched${stay}, and any site still set to PHP ${major} keeps that setting until you change it.`;
		default: {
			const unreachable: never = kind;
			return unreachable;
		}
	}
}

/**
 * ` — they stay in <path>`, or `''` when nothing kept has a path.
 *
 * Names the HEADLINE kept path — the first item carrying one — rather than
 * joining all of them, and that is a correctness choice, not brevity. A MySQL
 * plan keeps four things (the datadir, the root password, `my.cnf` and the
 * user's own overrides), so joining every path turns "your databases … stay in
 * X, Y and Z" into a claim that the databases live in a config file. The Rust
 * inventory orders each kind's keeps headline-first for exactly this reason,
 * and the dialog renders the complete list, paths and all, directly below this
 * sentence — nothing is hidden by naming one here.
 *
 * Split out so both branches of {@link keptSentence} read the same data the
 * same way and neither can quietly stop consulting it.
 */
function keptPathsClause(keeps: readonly KeptItem[]): string {
	const headline = keeps.find((item) => item.path !== null && item.path !== '');
	if (headline === undefined || headline.path === null) return '';
	return ` — they stay in ${headline.path}`;
}

/** `a`, `a and b`, `a, b and c` — Oxford-comma-free, matching
 *  `services.derive.ts`'s `formatNameList` convention for user-facing lists. */
function joinList(items: readonly string[]): string {
	if (items.length <= 1) return items.join('');
	return `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`;
}

/** The headline over a refused uninstall — a refusal, not a warning with a way
 *  past it (design D3: there is no `--force`). */
export function refusalHeadline(kind: PackageKind, major: string): string {
	return `${packageLabel(kind)} ${major} can't be uninstalled yet.`;
}

/** One refusal, split into what is in the way and what the user can do about
 *  it. `kind` is carried through so a list can key on it without re-deriving
 *  it from the prose. */
export interface BlockerMessage {
	kind: Blocker['kind'];
	/** Names its own subject — the service id and its state, or the domains. */
	obstacle: string;
	/** The next action, in the user's terms. Never "try again later". */
	action: string;
}

/**
 * Copy for one refusal. Exhaustive over `Blocker` with the never-typed default
 * arm — a third refusal reason must fail to compile here rather than reach the
 * user as a blank line.
 *
 * Every variant says something structurally different, on purpose: this
 * codebase's recurring bug is two distinct states rendering as one message,
 * and `uninstall.derive.test.ts` asserts the pair is distinct rather than
 * merely non-empty.
 */
export function blockerMessage(blocker: Blocker): BlockerMessage {
	switch (blocker.kind) {
		case 'serviceNotTerminal':
			return {
				kind: 'serviceNotTerminal',
				// The state word is rendered verbatim rather than mapped: it comes
				// from the supervisor, and inventing our own word for it here is how
				// a UI ends up claiming "running" for a service that is `starting`.
				obstacle: `${blocker.id} is ${blocker.state}.`,
				action: `Stop it first — Services page, menu-bar menu, or \`openvhost stop ${blocker.id}\` — then try again. OpenVHost won't stop it for you: a database stopped mid-write is not something a menu click should cause.`
			};
		case 'sitesPinned': {
			const count = blocker.domains.length;
			const plural = count === 1 ? '1 site still uses' : `${count} sites still use`;
			const them = count === 1 ? 'it' : 'them';
			const sites = count === 1 ? 'the site' : 'those sites';
			return {
				kind: 'sitesPinned',
				obstacle: `${plural} this version: ${joinList(blocker.domains)}.`,
				action: `Point ${them} at another PHP version (or delete ${sites}) first. OpenVHost never repoints a site for you: the next version may not run its code.`
			};
		}
		default: {
			const unreachable: never = blocker;
			return unreachable;
		}
	}
}

/** What a page must know to decide whether an Uninstall action is live. Every
 *  field is "'' when idle, otherwise the major it is busy with" — the same
 *  convention `LanguagesStore.installing`/`DatabasesStore.initializing`
 *  already use. */
export interface UninstallBusy {
	installingMajor: string;
	initializingMajor: string;
	uninstallingMajor: string;
}

/**
 * Whether every Uninstall action on the page is disabled.
 *
 * Page-wide rather than per-row, and that is deliberate: `brew install`,
 * `brew uninstall` and the MySQL init all serialize behind one `InstallLock`
 * (design D1), so a second action would not run concurrently — it would sit on
 * a mutex with no feedback. Includes the uninstall's OWN major, so a
 * double-click cannot reach the command twice.
 *
 * The store carries the same guard independently ({@link
 * UninstallStore.confirm}), so deleting a `disabled` attribute must still
 * leave a second call refused.
 */
export function uninstallActionDisabled(busy: UninstallBusy): boolean {
	return (
		busy.installingMajor !== '' || busy.initializingMajor !== '' || busy.uninstallingMajor !== ''
	);
}

/** The confirm button's label. `Uninstalling…` for the same reason
 *  `Installing…` exists on the row buttons: a `brew uninstall` takes seconds,
 *  and a button that looks idle invites a second click. */
export function uninstallConfirmLabel(uninstalling: boolean): string {
	return uninstalling ? 'Uninstalling…' : 'Uninstall';
}

/** Whether a fetched plan may proceed at all (design D3: blockers are refusals).
 *  A `null` plan — not fetched yet, or the fetch failed — is never proceedable:
 *  the UI must not offer to uninstall on the strength of nothing. */
export function mayProceed(plan: UninstallPlan | null): boolean {
	return plan !== null && plan.blockers.length === 0;
}
