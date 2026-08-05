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
 *
 * `'mariadb'` (P1 MariaDB UI design D5) reuses this same shared uninstall
 * pipeline — MariaDB's own command surface has no `uninstall_mariadb` of its
 * own; removing a packaged install goes through `PackageKind::Mariadb` here,
 * same as PHP and MySQL.
 */
export type PackageKind = 'php' | 'mysql' | 'mariadb';

/** One thing an uninstall leaves alone, with its on-disk location when it has
 *  one. `path: null` is a kept thing that is not a file — the stored root
 *  credential, a site's recorded PHP version.
 *
 *  `headline` marks the ONE item {@link keptSentence} may name in prose. It is
 *  carried on the wire rather than inferred here because both gates flagged the
 *  same thing: selecting "the first kept item with a path" coupled a
 *  destructive dialog's central promise to the order of a Rust `Vec`, so
 *  re-ordering the inventory for an unrelated reason would silently move the
 *  sentence onto a different directory. Rust asserts exactly one per plan; this
 *  file still refuses to guess if that is ever violated (see
 *  {@link keptPathsClause}). */
export interface KeptItem {
	what: string;
	path: string | null;
	headline: boolean;
}

/**
 * Why an uninstall is refused (design D3). Every variant is a REFUSAL, not a
 * warning: there is no force path, so the UI must never offer to proceed past
 * one.
 */
export type Blocker =
	| { kind: 'serviceNotTerminal'; id: string; state: string }
	| { kind: 'sitesPinned'; domains: string[] }
	/** The keg `brew uninstall <formula>` would remove belongs to a DIFFERENT
	 *  formula — Homebrew has aliased this version's name onto another one. On
	 *  the owner's machine `php@8.5` is an alias of the unversioned `php`, whose
	 *  keg is `Cellar/php/8.5.9`, so the removal this app would ask for and the
	 *  removal brew would perform are not the same removal. */
	| { kind: 'foreignKeg'; formula: string; owner: string; keg: string }
	/** Nothing under any known Homebrew prefix resolved to a keg for this
	 *  formula, so what `brew uninstall <formula>` would remove is unknown —
	 *  which is not the same as safe. */
	| { kind: 'unknownKeg'; formula: string; searched: string[] };

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
		case 'mariadb':
			return 'MariaDB';
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
		case 'mariadb':
			// Same guarantee as MySQL's, in the same shape (P1 MariaDB UI spec
			// §10 point 4: "leaves the datadir and the credential row intact").
			return `Your databases are not touched${stay}, and your root password is kept, so reinstalling ${major} picks up where you left off.`;
		default: {
			const unreachable: never = kind;
			return unreachable;
		}
	}
}

/**
 * ` — they stay in <path>`, or `''` when no kept item is marked as the headline.
 *
 * Names ONE kept path rather than joining all of them, and that is a
 * correctness choice, not brevity. A MySQL plan keeps four things (the datadir,
 * the root password, `my.cnf` and the user's own overrides), so joining every
 * path turns "your databases … stay in X, Y and Z" into a claim that the
 * databases live in a config file. The dialog renders the complete list, paths
 * and all, directly below this sentence — nothing is hidden by naming one here.
 *
 * WHICH one is `KeptItem.headline`, not `keeps[0]`. This used to take the first
 * item carrying a path, which made the sentence a function of Rust's vector
 * ORDER: an inventory re-ordered for any unrelated reason would move the
 * app's central "your data is safe" promise onto a different directory, and
 * nothing about that change would look like it touched this dialog. Both gates
 * flagged it; the flag is now on the wire.
 *
 * FAIL SAFE, not fail loud: Rust asserts exactly one headline per plan, and if
 * that is ever violated — none, or two — this drops the clause entirely rather
 * than picking one. A confirmation that says "your databases are not touched"
 * is still true and still safe to act on; a confirmation that names the WRONG
 * directory is worse than one that names none, because the user reads it as a
 * promise about that path.
 *
 * Split out so both branches of {@link keptSentence} read the same data the
 * same way and neither can quietly stop consulting it.
 */
function keptPathsClause(keeps: readonly KeptItem[]): string {
	const headlines = keeps.filter((item) => item.headline && item.path !== null && item.path !== '');
	if (headlines.length !== 1) return '';
	const path = headlines[0].path;
	if (path === null) return '';
	return ` — they stay in ${path}`;
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
		case 'foreignKeg':
			// The refusal that protects something OUTSIDE OpenVHost. On the
			// owner's machine `php@8.5` is an alias of the unversioned `php`
			// formula, so `brew uninstall php@8.5` removes the linked `php` —
			// the interpreter every `php` command on the machine resolves to,
			// most of which have nothing to do with this app. Naming the OWNER
			// and the KEG is the whole message: "this is not the removal you
			// think you are asking for" is unarguable once the user can see
			// which directory would go.
			return {
				kind: 'foreignKeg',
				obstacle: `Homebrew treats ${blocker.formula} as an alias for its unversioned ${blocker.owner} formula — it resolves to ${blocker.keg}. Removing it would take your linked ${blocker.owner} with it, not just this version.`,
				action: `OpenVHost won't take your ${blocker.owner} down as a side effect of removing a version here. If that is what you mean, run \`brew uninstall ${blocker.owner}\` yourself — same command, consequence in front of you.`
			};
		case 'unknownKeg': {
			// Unknown is NOT safe. An absent or unreadable `opt` link is no
			// evidence that the name is harmless to hand to brew: brew resolves
			// its own aliases from its taps whether or not a link exists here,
			// so the `foreignKeg` danger is fully present in this case too —
			// just unprovable. Refusing fails visibly and leaves a manual path;
			// proceeding would fail quietly and take the user's `php` with it.
			const looked =
				blocker.searched.length === 0
					? 'no Homebrew prefix to look in'
					: `nothing under ${joinList(blocker.searched)}`;
			return {
				kind: 'unknownKeg',
				obstacle: `OpenVHost can't tell which Homebrew keg ${blocker.formula} refers to — there is ${looked}.`,
				action: `It won't hand a name it can't resolve to \`brew uninstall\`, because an alias can point at a formula you use elsewhere. Check with \`brew info ${blocker.formula}\` first; if the answer is what you expect, run the uninstall yourself.`
			};
		}
		default: {
			const unreachable: never = blocker;
			return unreachable;
		}
	}
}

/** The two facts a row needs before it may offer Uninstall at all. Structural,
 *  so a `PhpRuntimeDto` or a `MysqlInstanceDto` can be handed straight in. */
export interface UninstallOffer {
	installed: boolean;
	/** Whether THIS BUILD manages the version — `MysqlInstanceDto.cataloged`'s
	 *  meaning exactly. */
	cataloged: boolean;
}

/**
 * Whether a row may offer an Uninstall action.
 *
 * `installed` alone is not enough, and that gap shipped: `php_rows` lists an
 * installed-but-not-catalogued major (a hand-installed `php@7.4`, or one a
 * later catalogue drops) so it does not vanish from the page while it is still
 * serving sites — with `installed: true`. The row then offered Uninstall,
 * `Target::parse` refused the major it was never going to accept, and the user
 * read "This could not be checked, so nothing has been changed."
 *
 * The refusal is correct and stays: it is the catalogue gate that keeps
 * anything but a version this build offers out of `brew`'s argv. What was wrong
 * was offering the button. `MysqlRow.svelte` has had this guard since its own
 * slice; this is the same rule, in one place both rows can read.
 */
export function offersUninstall(row: UninstallOffer): boolean {
	return row.installed && row.cataloged;
}

/**
 * Why the action is absent, and what to do instead — because an affordance
 * that silently is not there teaches nothing (this page's own C2/C3 lesson).
 *
 * Names the formula rather than saying "use Homebrew": the version was
 * discovered under a Homebrew prefix, so `brew uninstall <formula>` is the
 * command that removes it, and a next action the user cannot type is not a next
 * action. Ends on "Check again" because design D5 makes that the convergence
 * point — a major removed behind the app's back is unregistered by the rescan,
 * leaving exactly the state an in-app uninstall would have.
 */
export function outOfCatalogueNote(kind: PackageKind, major: string): string {
	const formula = brewFormula(kind, major);
	const removal =
		formula === null
			? `OpenVHost won't uninstall it. Remove it yourself, then press Check again.`
			: `OpenVHost won't uninstall it. Remove it with Homebrew yourself — ` +
				`\`brew uninstall ${formula}\` — then press Check again.`;
	return `${packageLabel(kind)} ${major} was installed outside the versions this build manages, so ${removal}`;
}

/**
 * The Homebrew formula for a version, for COPY ONLY — `null` when this kind
 * has no Homebrew formula at all (P1 MariaDB UI design D5).
 *
 * Mirrors `openvhost_core::brew_formula` / `mysql_brew_formula` (`php@8.3`,
 * `mysql@8.4`) so the command in {@link outOfCatalogueNote} is the one that
 * actually works. Nothing composed here ever reaches a child process's argv:
 * every real `brew` invocation is composed in `openvhost-core` from a
 * catalogue-gated major, which is exactly why the out-of-catalogue case has no
 * in-app path in the first place. Exhaustive over `PackageKind` with the
 * never-typed arm, so a third kind cannot inherit PHP's naming by accident.
 *
 * MariaDB returns `null` rather than `''` or a plausible-looking `'mariadb'`:
 * a packaged MariaDB has no Homebrew origin and never will, so there is no
 * formula name to invent, silent or otherwise. Every caller decides — here,
 * {@link outOfCatalogueNote} drops the `brew uninstall` clause entirely
 * rather than print one naming a formula that does not exist.
 */
function brewFormula(kind: PackageKind, major: string): string | null {
	switch (kind) {
		case 'php':
			return `php@${major}`;
		case 'mysql':
			return `mysql@${major}`;
		case 'mariadb':
			return null;
		default: {
			const unreachable: never = kind;
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
