// SPDX-License-Identifier: GPL-3.0-or-later
// State for the Languages page: the PHP environment snapshot, the one install
// that may be running, its live log, and the outcome of the last attempt.
//
// Shape mirrors `apply.svelte.ts`: an injected API object (so tests never touch
// real IPC), plain `$state` fields, and a re-entrancy guard that lives HERE
// rather than only on a button's `disabled` attribute — deleting that attribute
// must leave a test failing, not just a UI regression nothing catches.
import { errorMessage } from './errors';
import type {
	DefaultPhpDto,
	PhpEnvironmentDto,
	PhpInstallOutcomeDto,
	PhpInstallProgressDto
} from './ipc';
import { offersDefaultChoice } from './php-default.derive';
import { noRouteToAnyPhp, phpInstallDeclaredTotal } from './php-install.derive';

export interface LanguagesApi {
	phpEnvironment(): Promise<PhpEnvironmentDto>;
	rescanPhpRuntimes(): Promise<PhpEnvironmentDto>;
	installPhp(major: string): Promise<PhpInstallOutcomeDto>;
	/**
	 * Store which major the catch-all serves, or clear the choice with `null`.
	 *
	 * **Storing only.** The generated config changes on the user's next Apply —
	 * this page does not open a diff, and an earlier version of this comment
	 * claimed it did. Same shape as `save_web_server_settings`: validate on
	 * save (there, a candidate render through `nginx -t`; here, refusing a
	 * major that is not installed), then let Apply do the rewrite.
	 */
	setDefaultPhp(major: string | null): Promise<void>;
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
	/**
	 * The last pipeline state of a PACKAGED install in flight, tagged with the
	 * major it belongs to — `null` before the first event of a run, and after
	 * every run this store starts.
	 *
	 * **Tagged, unlike `DatabasesStore.installProgress`, and for the reason
	 * {@link logFor} exists**: several PHP majors sit side by side on this page,
	 * and this store has already shipped the untagged version of this bug once —
	 * a failed 8.4 install's output rendering under the 8.3 row. The event
	 * carries `major` precisely so a progress bar knows whose it is; throwing
	 * that away here and re-deriving it from `installing` would put the
	 * attribution back in the page.
	 *
	 * **It stays `null` for the whole of a Homebrew install** (spec §8.6), and
	 * that is not a coincidence to rely on loosely: `php-install-progress` is
	 * emitted only by `run_package_install`, and brew's route streams
	 * `php-install-log` instead. So on a machine with Homebrew and no package
	 * tree nothing ever writes here, and every consumer of it renders nothing.
	 */
	installProgress = $state<{ major: string; progress: PhpInstallProgressDto } | null>(null);
	/** The download length the server declared, captured from the `started`
	 *  event because no later event repeats it. `null` when it declared none —
	 *  a real case the UI must render as "so far" rather than as a percentage of
	 *  a number it invented. Untagged, unlike {@link installProgress}: it is only
	 *  ever read alongside a progress state for the same run, and {@link install}
	 *  clears both together. */
	installTotal = $state<number | null>(null);
	/**
	 * '' when idle, otherwise the major whose "Make default" is in flight.
	 *
	 * A state, not a boolean, for the reason `installing` is one and the reason
	 * this UI has now shipped the boolean-collapse bug five times: a row has to
	 * tell "somebody else is busy" (disabled) from "it is me" (disabled AND
	 * labelled), and a boolean can express only one of those.
	 */
	settingDefault = $state('');

	constructor(private api: LanguagesApi) {}

	get brewFound(): boolean {
		return this.env?.brewFound ?? false;
	}

	get anyInstalled(): boolean {
		return this.env?.runtimes.some((r) => r.installed) ?? false;
	}

	/**
	 * How the catch-all's PHP was resolved — which major serves `localhost:8080`
	 * AND why (design D2).
	 *
	 * `null` until the first environment read settles, which the page already
	 * gates on. Deliberately NOT defaulted to `{ kind: 'unset' }`: `unset` is a
	 * claim about this machine ("nobody has chosen"), and asserting it before we
	 * have looked would let a page painted mid-load contradict a stored
	 * preference for one frame — the same reason `noRouteToAnyPhp` refuses to
	 * guess before `env` arrives.
	 */
	get defaultPhp(): DefaultPhpDto | null {
		return this.env?.defaultPhp ?? null;
	}

	/**
	 * Whether the page offers a way to choose the default at all — see
	 * {@link offersDefaultChoice} for the rule and why it is not simply
	 * "more than one installed".
	 *
	 * `false` before `env` arrives: the affordance is a claim about what this
	 * machine can do, and we have not looked yet.
	 */
	get offersDefaultChoice(): boolean {
		if (this.env === null) return false;
		return offersDefaultChoice(
			this.env.defaultPhp,
			this.env.runtimes.filter((r) => r.installed).length
		);
	}

	/**
	 * Whether this machine has no route to any PHP at all — the ONE case the
	 * page-level "Homebrew is required" dead end is for (off-Homebrew slice 5C
	 * design D2). Nothing installed, and nothing installable by any route.
	 *
	 * `false` while `env` is still `null`, and that is not a default so much as
	 * the only honest answer: we have not looked yet, and a dead end is a claim
	 * about what this machine cannot do. The page does not render this branch
	 * before `env` arrives anyway (`{#if store.env}`), so the value is never
	 * painted — but a getter that guessed "yes" would be one refactor away from
	 * flashing the bluntest screen in the app on every page load.
	 */
	get noRouteToAnyPhp(): boolean {
		return this.env === null ? false : noRouteToAnyPhp(this.env);
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
	 * Record one packaged-install pipeline state as it arrives.
	 *
	 * `started` is also where the declared total is captured — no later event
	 * repeats it, so losing it here leaves every subsequent `downloaded` reading
	 * with no denominator and the bar undrawable.
	 */
	applyInstallProgress(major: string, progress: PhpInstallProgressDto): void {
		this.installProgress = { major, progress };
		const declared = phpInstallDeclaredTotal(progress);
		if (declared !== null) this.installTotal = declared;
	}

	/** This attempt's pipeline state, and only for the version it belongs to —
	 *  the progress twin of {@link logFor}, and there for the same reason: a row
	 *  must never paint another row's install. */
	progressFor(major: string): PhpInstallProgressDto | null {
		return this.installProgress?.major === major ? this.installProgress.progress : null;
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
	 *
	 * Progress is dropped as the run STARTS, not when it ends — the same rule
	 * `DatabasesStore.install` follows, and for the same reason: a stale
	 * "Checksum verified" from the previous attempt sitting above this attempt's
	 * first byte is exactly the confusion it prevents. It is also what keeps the
	 * Homebrew route silent forever: cleared here, never written by anything on
	 * that route.
	 */
	/**
	 * Choose `major` as the default the catch-all serves — or clear the choice
	 * with `null`.
	 *
	 * **Stores; does not apply.** Resolving `true` means the preference is in
	 * `state.db` and the caller should now show the user the diff: the page
	 * re-plans and opens the same `ApplyDialog` the Sites and Web server pages
	 * use, so the generated config still changes only through
	 * `plan_config_apply` → `apply_config` — diff preview, `nginx -t`, rollback
	 * (spec claim 6). Nothing here writes a config file, and nothing here
	 * restarts a service.
	 *
	 * Re-reads the environment on success rather than assuming the new value
	 * took, for the same reason {@link install} does: what the page renders must
	 * be what was stored, and the resolution is computed server-side against the
	 * installed set — a preference for a major that vanished between the click
	 * and the write comes back as `preferredMissing`, which the page then
	 * renders honestly instead of showing a badge on a lie.
	 *
	 * The re-entrancy guard lives here, not only on the button's `disabled`
	 * attribute: deleting that attribute must still leave a second concurrent
	 * call refused.
	 */
	async setDefault(major: string | null): Promise<boolean> {
		if (this.settingDefault !== '') return false;
		// `null` clears; the marker still has to be non-empty for the guard
		// above to hold, so it names the operation rather than a major.
		this.settingDefault = major ?? 'clear';
		this.error = '';
		try {
			await this.api.setDefaultPhp(major);
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.settingDefault = '';
		}
		await this.refresh();
		return true;
	}

	async install(major: string): Promise<boolean> {
		if (this.installing !== '') return false;
		this.installing = major;
		this.error = '';
		this.installProgress = null;
		this.installTotal = null;
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
