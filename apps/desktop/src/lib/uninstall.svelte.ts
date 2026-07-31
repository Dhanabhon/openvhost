// SPDX-License-Identifier: GPL-3.0-or-later
// State for uninstalling an installed package (package-uninstall design D6):
// which package the confirmation is open for, the plan Rust computed for it,
// the live `brew uninstall` output, and the one uninstall that may be running.
//
// ONE instance app-wide (`uninstall.shared.svelte.ts`), shared by the Languages
// and Databases pages rather than one per page. That is not tidiness: `brew
// install`, `brew uninstall` and the MySQL init all serialize behind a single
// `InstallLock` (design D1), so a PHP uninstall genuinely does block a MySQL
// one. A per-page store would leave the other page offering a button that
// could only sit on a mutex.
//
// Shape mirrors `languages.svelte.ts`/`databases.svelte.ts`: an injected api
// object (so tests never touch real IPC), plain `$state` fields, and
// re-entrancy guards that live HERE rather than only on a button's `disabled`
// attribute — deleting that attribute must leave a test failing, not just a UI
// regression nothing catches.
import { errorMessage } from './errors';
import { mayProceed, type PackageKind, type UninstallPlan } from './uninstall.derive';

export interface UninstallApi {
	/** PURE QUERY — spawns nothing and changes nothing. Safe to call every time
	 *  the user presses Uninstall, which is why the confirmation can show the
	 *  real inventory instead of a guess. */
	uninstallPlan(kind: PackageKind, major: string): Promise<UninstallPlan>;
	/** Runs `brew uninstall` and the cleanup, streaming its output on the same
	 *  channel `install_php` uses — see {@link UninstallStore.appendLog}. */
	uninstallPackage(kind: PackageKind, major: string): Promise<void>;
}

/** Same shape `LogPane.svelte` renders (`services.svelte.ts`'s `UiLog`),
 *  redeclared rather than imported — the same decoupling `languages.svelte.ts`
 *  and `databases.derive.ts` already chose: the only thing shared is the shape
 *  LogPane expects. */
export interface UiLog {
	id: string;
	tsMs: number;
	level: 'info' | 'warn' | 'error';
	line: string;
}

/** Upper bound on `log`'s length — same value and slice-when-over-cap
 *  technique as `languages.svelte.ts`'s `LOG_CAP`, for the same reason: a brew
 *  run's tail is exactly what a failure needs read back. */
const LOG_CAP = 200;

/** Which package the confirmation is open for. */
export interface UninstallTarget {
	kind: PackageKind;
	major: string;
}

export class UninstallStore {
	/** Non-null exactly while the confirmation is on screen. */
	target = $state<UninstallTarget | null>(null);
	/** What Rust says an uninstall would remove, keep and refuse. `null` while
	 *  the query is in flight, or after it failed — never treated as
	 *  "proceedable" in either case (see {@link canProceed}). */
	plan = $state<UninstallPlan | null>(null);
	planning = $state(false);
	/** '' when idle, otherwise the major currently being uninstalled. */
	uninstalling = $state('');
	error = $state('');
	log = $state<UiLog[]>([]);

	constructor(private api: UninstallApi) {}

	get isOpen(): boolean {
		return this.target !== null;
	}

	/** Design D3: blockers are refusals, so a plan carrying one is never
	 *  proceedable, and neither is a missing plan — the UI must not offer to
	 *  uninstall on the strength of nothing. */
	get canProceed(): boolean {
		return mayProceed(this.plan);
	}

	/**
	 * Open the confirmation for `kind`/`major` and fetch its plan.
	 *
	 * Refused outright while an uninstall is running: that operation holds the
	 * install lock, so a plan fetched now would describe a machine that is
	 * mid-change, and the dialog it opened would replace the live output of the
	 * thing actually happening.
	 *
	 * A plan that lands after the user asked for a DIFFERENT package (a fast
	 * double-click across two rows) is dropped — the identity check on
	 * `this.target` is what makes the last request the one that owns the
	 * dialog, rather than whichever round trip happened to finish last.
	 */
	async request(kind: PackageKind, major: string): Promise<void> {
		if (this.uninstalling !== '') return;
		const requested: UninstallTarget = { kind, major };
		this.target = requested;
		this.plan = null;
		this.error = '';
		this.log = [];
		this.planning = true;
		try {
			const plan = await this.api.uninstallPlan(kind, major);
			if (this.target !== requested) return;
			this.plan = plan;
		} catch (e) {
			if (this.target !== requested) return;
			this.error = errorMessage(e);
		} finally {
			if (this.target === requested) this.planning = false;
		}
	}

	/**
	 * Dismiss the confirmation. A no-op while an uninstall is running —
	 * deliberately unlike `QuitDialog`, which cancels even mid-quit because its
	 * window is about to be destroyed anyway. Here the live `brew uninstall`
	 * output is the only feedback there is, and dismissing it would leave the
	 * user staring at a page that says nothing while a package is removed.
	 */
	close(): void {
		if (this.uninstalling !== '') return;
		this.target = null;
		this.plan = null;
		this.error = '';
		this.log = [];
	}

	/** Record an outside failure (e.g. the page's log subscription could not be
	 *  registered) on the same channel this store's own calls use. */
	fail(e: unknown): void {
		this.error = errorMessage(e);
	}

	/**
	 * Run the uninstall. Returns whether it actually happened, so the caller
	 * can re-read its own environment only when something changed.
	 *
	 * Every refusal is checked HERE, not only on the confirm button:
	 * re-entrancy, a missing plan, a plan carrying a blocker, and a plan that
	 * does not describe the current target. That last one is defensive and
	 * cheap — the command is driven by `target` while the refusal decision is
	 * read off `plan`, and the two disagreeing would mean uninstalling one
	 * version on the strength of another version's blocker-free plan.
	 */
	async confirm(): Promise<boolean> {
		if (this.uninstalling !== '') return false;
		const target = this.target;
		const plan = this.plan;
		if (target === null || plan === null) return false;
		if (!mayProceed(plan)) return false;
		if (plan.kind !== target.kind || plan.major !== target.major) return false;

		this.error = '';
		this.log = [];
		this.uninstalling = target.major;
		try {
			await this.api.uninstallPackage(target.kind, target.major);
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.uninstalling = '';
		}
		// The row this dialog described is gone; leaving the dialog up would
		// invite a second confirm against a plan that no longer describes
		// anything. The supervisor row disappears on its own — `Unregistered`
		// reaches `ServicesStore.applyUnregistered` through the layout.
		this.target = null;
		this.plan = null;
		this.log = [];
		return true;
	}

	/**
	 * Append one line of `brew uninstall` output.
	 *
	 * Gated on an uninstall actually running for this major, because the event
	 * this is fed from is the INSTALL channel (design D1: same lock, same
	 * output surface). Without the gate, a `brew install`'s lines would pile up
	 * here and appear inside an uninstall dialog opened minutes later.
	 *
	 * `major` becomes the entry's `id`, matching `UiLog`'s shape without
	 * inventing a second one — the same convention `LanguagesStore.appendLog`
	 * follows. Capped at {@link LOG_CAP}: once over, slice to the tail, because
	 * the tail is what a failed run needs read back.
	 */
	appendLog(major: string, line: string): void {
		if (this.uninstalling === '' || this.uninstalling !== major) return;
		const next = [...this.log, { id: major, tsMs: Date.now(), level: 'info' as const, line }];
		this.log = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}
}
