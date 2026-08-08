// SPDX-License-Identifier: GPL-3.0-or-later
// State for the Languages page: the PHP environment snapshot, the one install
// that may be running, its live log, and the outcome of the last attempt.
//
// Shape mirrors `apply.svelte.ts`: an injected API object (so tests never touch
// real IPC), plain `$state` fields, and a re-entrancy guard that lives HERE
// rather than only on a button's `disabled` attribute — deleting that attribute
// must leave a test failing, not just a UI regression nothing catches.
import { errorMessage } from './errors';
import type { PhpEnvironmentDto, PhpInstallOutcomeDto } from './ipc';

export interface LanguagesApi {
	phpEnvironment(): Promise<PhpEnvironmentDto>;
	rescanPhpRuntimes(): Promise<PhpEnvironmentDto>;
	installPhp(major: string): Promise<PhpInstallOutcomeDto>;
}

/** Same shape `LogPane.svelte` already renders (`services.svelte.ts`'s `UiLog`),
 *  redeclared here rather than imported so this store stays decoupled from the
 *  services store — the only thing shared is the shape LogPane expects. */
export interface UiLog {
	id: string;
	tsMs: number;
	level: 'info' | 'warn' | 'error';
	line: string;
}

/** Upper bound on `log`'s length. Same slice-when-over-cap technique as
 *  `services.svelte.ts`'s `LOG_CAP`, but a larger number: that one caps a
 *  live service tail, this one caps a single `brew install`'s full output,
 *  which routinely runs longer than 50 lines (formula resolution, fetch,
 *  build steps) and the tail is exactly what a failure needs. */
const LOG_CAP = 200;

export class LanguagesStore {
	env = $state<PhpEnvironmentDto | null>(null);
	/** '' when idle, otherwise the major currently installing. */
	installing = $state('');
	log = $state<UiLog[]>([]);
	error = $state('');
	/** The last install attempt's outcome, whichever route it took (off-Homebrew
	 *  slice 5C design D4). A TAGGED result, not a brew-shaped record: only
	 *  `result.kind === 'brew'` carries an `exitCode`, because only that route
	 *  runs a child process. */
	outcome = $state<PhpInstallOutcomeDto | null>(null);

	constructor(private api: LanguagesApi) {}

	get brewFound(): boolean {
		return this.env?.brewFound ?? false;
	}

	get anyInstalled(): boolean {
		return this.env?.runtimes.some((r) => r.installed) ?? false;
	}

	/**
	 * Cheap, spawn-free snapshot (spec: safe on mount and after every install).
	 *
	 * A failed re-read is not evidence the PHP environment vanished — it only
	 * means this one round trip didn't land. Overwriting `env` with `null`
	 * would hide the last known (possibly just-installed) state behind
	 * `+page.svelte`'s `{#if store.env}` gate along with it. So a failure here
	 * keeps whatever `env` already held and only sets `error`; `env` starts
	 * `null` and stays `null` on the very first load's failure, since there is
	 * nothing yet to keep.
	 */
	async refresh(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.phpEnvironment();
		} catch (e) {
			this.error = errorMessage(e);
		}
	}

	/**
	 * The "Check again" button: unlike {@link refresh}, this spawns a probe
	 * per candidate binary — explicit and user-initiated only.
	 *
	 * This is the only recovery path once a user has gone off to install
	 * Homebrew (or a PHP version) by hand and come back — it is either this or
	 * quitting and relaunching the app. A transient failure here is not
	 * evidence the PHP environment vanished, same reasoning as {@link refresh}:
	 * overwriting `env` with `null` would blank the empty-state guidance the
	 * user is actively following out from under them. So a failure keeps
	 * whatever `env` already held and only sets `error`; `env` starts `null`
	 * and stays `null` on a first-load failure, since there is nothing yet to
	 * keep.
	 */
	async rescan(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.rescanPhpRuntimes();
		} catch (e) {
			this.error = errorMessage(e);
		}
	}

	/** Record an outside failure (e.g. the page's install-log subscription
	 *  could not be registered) on the same channel this store's own calls use. */
	fail(e: unknown): void {
		this.error = errorMessage(e);
	}

	/** Append one line of `brew install` output. `PhpInstallLogEvent` carries a
	 *  stream (stdout/stderr), not a severity — brew's output has none for a
	 *  classifier to assign — so every line lands as `info`. `major` becomes the
	 *  entry's `id`, matching `UiLog`'s shape without inventing a second one —
	 *  and doubling as the attribution `logFor` filters on. Capped at
	 *  {@link LOG_CAP} the same way `services.svelte.ts`'s `applyLog` is: once
	 *  over, slice to the tail, because the tail is what a failed install needs
	 *  read back. */
	appendLog(major: string, line: string): void {
		const next = [...this.log, { id: major, tsMs: Date.now(), level: 'info' as const, line }];
		this.log = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}

	/** The current attempt's output, and only for the version it belongs to —
	 *  `+page.svelte` calls this instead of comparing its own page-local
	 *  "which row did I last attempt" marker against the whole, unfiltered
	 *  `log`. That comparison was the bug: `log` is never per-version on its
	 *  own, so a failed 8.4 install's lines stayed there and rendered under
	 *  the 8.3 row on the very next attempt. */
	logFor(major: string): UiLog[] {
		return this.log.filter((entry) => entry.id === major);
	}

	/**
	 * Install `major` — the route (OpenVHost's own package tree, or Homebrew) is
	 * decided server-side from the same table that fills a row's `offer`, so
	 * nothing here dispatches on it. Always re-reads the environment with
	 * {@link refresh} on success rather than assuming the row is now installed —
	 * `detected` exists precisely because brew can exit 0 without the version
	 * being found afterwards, and assuming would hide that.
	 *
	 * The re-entrancy guard lives here, not only on the Install button's
	 * `disabled` attribute: deleting that attribute must still leave a second
	 * concurrent call refused.
	 *
	 * Scopes `log` to this attempt's own major before doing anything else —
	 * symmetric with `error`, cleared the line below. Without this, a previous
	 * attempt's output (a different major entirely) stayed in `log` for the
	 * life of the page, and the page had no way to tell whose output was
	 * whose; see {@link logFor}.
	 */
	async install(major: string): Promise<boolean> {
		if (this.installing !== '') return false;
		this.installing = major;
		this.error = '';
		this.log = this.log.filter((entry) => entry.id === major);
		try {
			this.outcome = await this.api.installPhp(major);
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.installing = '';
		}
		await this.refresh();
		return true;
	}
}
