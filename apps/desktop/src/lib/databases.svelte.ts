// SPDX-License-Identifier: GPL-3.0-or-later
// State for the Databases page: the MySQL environment snapshot, the one
// install/init that may be running (they share one InstallLock, spec D7, so
// at most one of `installing`/`initializing` is ever non-'' at a time), their
// live logs, cached root passwords (fetched ONLY on demand — spec D3/D6
// MANDATORY: never eagerly), and the per-major outcome of the last
// reset/verify attempt.
//
// Shape mirrors `languages.svelte.ts`: an injected API object (so tests never
// touch real IPC), plain `$state` fields, and re-entrancy guards that live
// HERE rather than only on a button's `disabled` attribute. The per-major
// dictionaries (`passwords`, `resetOutcome`, `verifyResult`, …) mirror
// `webservers.svelte.ts`'s `Record<string, T>` convention, including its
// "drop the previous verdict as the run STARTS" discipline — a fresh red
// beside a stale green from an earlier click is the exact bug that
// discipline exists to prevent.
//
// Review fix: `passwords` (cache presence) and `revealed` (the on-screen
// display gate) are deliberately TWO separate dictionaries. `reveal()` sets
// both; `copyPassword()` — Copy — only ever touches the cache, never the
// gate, so a Copy click can never silently un-mask the field (see
// `revealed`'s own doc comment for the screen-share scenario this fixes).
import { errorMessage } from './errors';
import { anyMysqlInstalled, type MysqlInitFailure, type UiLog } from './databases.derive';
import { mysqlInstallDeclaredTotal } from './mysql-install.derive';
import type {
	MysqlConnectionProofDto,
	MysqlEnvironmentDto,
	MysqlInitOutcomeDto,
	MysqlInstallOutcomeDto,
	MysqlInstallProgressDto,
	MysqlResetOutcomeDto
} from './ipc';

export interface DatabasesApi {
	mysqlEnvironment(): Promise<MysqlEnvironmentDto>;
	rescanMysql(): Promise<MysqlEnvironmentDto>;
	installMysql(major: string): Promise<MysqlInstallOutcomeDto>;
	/** Aborts an in-flight `installMysql`, resolving to whether anything was
	 *  actually stopped. See {@link DatabasesStore.cancelInstall}. */
	cancelMysqlInstall(): Promise<boolean>;
	initializeMysql(major: string): Promise<MysqlInitOutcomeDto>;
	mysqlRootPassword(major: string): Promise<string>;
	resetMysqlRootPassword(major: string): Promise<MysqlResetOutcomeDto>;
	verifyMysqlConnection(major: string): Promise<MysqlConnectionProofDto>;
}

/** Upper bound on `installLog`/`initLog`'s length — same slice-when-over-cap
 *  technique as `languages.svelte.ts`'s `LOG_CAP`, and the same value: a
 *  `brew install`/staged-init run routinely produces more than a screenful,
 *  and the tail is exactly what a failure needs read back. */
const LOG_CAP = 200;

/** Drops `key` from a `Record`, immutably — the one bit of dict bookkeeping
 *  used often enough here to name, rather than inlining
 *  `Object.fromEntries(Object.entries(...).filter(...))` at every call site. */
function withoutKey<T>(record: Record<string, T>, key: string): Record<string, T> {
	return Object.fromEntries(Object.entries(record).filter(([k]) => k !== key));
}

export class DatabasesStore {
	env = $state<MysqlEnvironmentDto | null>(null);
	/** Page-level failure: the environment could not be loaded/rescanned at
	 *  all, or an outside failure (see {@link fail}). */
	error = $state('');

	/** '' when idle, otherwise the major currently running `install_mysql`. */
	installing = $state('');
	installLog = $state<UiLog[]>([]);
	/**
	 * The last pipeline state of the install in flight, or `null` before the
	 * first event arrives (and after a fresh install starts).
	 *
	 * Page-wide rather than per-major, matching {@link installing}: one
	 * `InstallLock` means one install, so a per-major dictionary would model a
	 * concurrency that cannot happen and would then need reconciling.
	 */
	installProgress = $state<MysqlInstallProgressDto | null>(null);
	/** The download length the server declared, captured from the `started`
	 *  event because no later event repeats it. `null` when it declared none —
	 *  which is a real case, and one the UI must render as "so far" rather than
	 *  as a percentage of a number it made up. */
	installTotal = $state<number | null>(null);
	/** True between {@link cancelInstall} being pressed and the install
	 *  settling. A state, not a disabled attribute: the Cancel button must read
	 *  "Cancelling…" while the staging directory unwinds, or it invites a
	 *  second press at the worst moment. */
	cancellingInstall = $state(false);
	/** The last `install_mysql` outcome, whichever major it was for —
	 *  `MysqlInstallOutcomeDto` carries its own `major`, same as PHP's
	 *  `InstallOutcomeDto`. */
	installOutcome = $state<MysqlInstallOutcomeDto | null>(null);

	/** '' when idle, otherwise the major currently running `initialize_mysql`. */
	initializing = $state('');
	initLog = $state<UiLog[]>([]);
	/** The last `initialize_mysql` outcome, tagged with the major it belongs
	 *  to — unlike `MysqlInstallOutcomeDto`, `MysqlInitOutcomeDto` carries no
	 *  `major` of its own (spec D7), so this store wraps it before handing it
	 *  to `mysqlRowState` via {@link initFailureFor}. */
	initOutcome = $state<{ major: string; outcome: MysqlInitOutcomeDto } | null>(null);

	/** Cached root passwords, keyed by major. MANDATORY (spec D3/D6): never
	 *  populated except by an explicit {@link reveal}/{@link copyPassword}
	 *  call — never on mount, never as a side effect of
	 *  `refresh`/`rescan`/`install`/`initialize`. NOT the display gate — see
	 *  {@link revealed}. A defined `passwords[major]` means "fetched and
	 *  cached", nothing about whether it should currently be shown in
	 *  plaintext. */
	passwords = $state<Record<string, string>>({});
	passwordError = $state<Record<string, string>>({});
	revealing = $state<Record<string, boolean>>({});
	/**
	 * The DISPLAY gate, keyed by major — true ONLY after an explicit
	 * {@link reveal} call, cleared by {@link forgetPassword} (Hide). Review
	 * fix: this used to not exist at all, and `MysqlCredentials.svelte` masked
	 * purely on `passwords[major] !== undefined` — so `copyPassword()`
	 * fetching into the SAME cache silently un-masked the field on a Copy
	 * click with no Reveal ever pressed (a screen-share hazard). Kept
	 * deliberately separate from `passwords` so a cache hit alone can never
	 * flip the UI to plaintext.
	 */
	revealed = $state<Record<string, boolean>>({});

	resetting = $state<Record<string, boolean>>({});
	resetOutcome = $state<Record<string, MysqlResetOutcomeDto>>({});
	resetError = $state<Record<string, string>>({});

	verifying = $state<Record<string, boolean>>({});
	verifyResult = $state<Record<string, MysqlConnectionProofDto>>({});
	verifyError = $state<Record<string, string>>({});

	constructor(private api: DatabasesApi) {}

	get anyInstalled(): boolean {
		return this.env !== null && anyMysqlInstalled(this.env.instances);
	}

	/**
	 * Cheap, spawn-free snapshot — mirrors `LanguagesStore.refresh` exactly,
	 * including keeping the last known environment on a failed re-read: a
	 * failed re-read is not evidence MySQL vanished, only that this one round
	 * trip did not land.
	 */
	async refresh(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.mysqlEnvironment();
		} catch (e) {
			this.error = errorMessage(e);
		}
	}

	/** The "Check again" button — mirrors `LanguagesStore.rescan` exactly. */
	async rescan(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.rescanMysql();
		} catch (e) {
			this.error = errorMessage(e);
		}
	}

	/** Record an outside failure (e.g. the page's log-event subscriptions
	 *  could not be registered) on the same channel this store's own calls
	 *  use. */
	fail(e: unknown): void {
		this.error = errorMessage(e);
	}

	appendInstallLog(major: string, line: string): void {
		const next = [
			...this.installLog,
			{ id: major, tsMs: Date.now(), level: 'info' as const, line }
		];
		this.installLog = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}

	installLogFor(major: string): UiLog[] {
		return this.installLog.filter((entry) => entry.id === major);
	}

	appendInitLog(major: string, line: string): void {
		const next = [...this.initLog, { id: major, tsMs: Date.now(), level: 'info' as const, line }];
		this.initLog = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}

	initLogFor(major: string): UiLog[] {
		return this.initLog.filter((entry) => entry.id === major);
	}

	/**
	 * Record one install-pipeline state as it arrives.
	 *
	 * `started` is also where the declared total is captured — no later event
	 * repeats it, so losing it here means every subsequent `downloaded` reading
	 * has no denominator.
	 */
	applyInstallProgress(progress: MysqlInstallProgressDto): void {
		this.installProgress = progress;
		const declared = mysqlInstallDeclaredTotal(progress);
		if (declared !== null) this.installTotal = declared;
	}

	/**
	 * Install `major` from the pinned upstream tarball — download, verify,
	 * extract. **No Homebrew.** Mirrors `LanguagesStore.install`'s discipline:
	 * the re-entrancy guard lives here (not only on a button's `disabled`), and
	 * a settled call always re-reads the environment via {@link refresh} rather
	 * than assuming the row is now installed.
	 *
	 * Progress state is cleared as the run STARTS, not when it ends — the same
	 * "drop the previous verdict as the run starts" rule
	 * `webservers.svelte.ts`'s `validate()` follows. A stale "Checksum
	 * verified" from the previous attempt sitting above this attempt's first
	 * byte is exactly the confusion that rule exists to prevent.
	 *
	 * Only a rejected major or a busy install lock THROWS; every pipeline
	 * failure — a checksum mismatch, a stall, an unavailable target, a cancel —
	 * comes back as a settled outcome to render.
	 */
	async install(major: string): Promise<boolean> {
		if (this.installing !== '') return false;
		this.installing = major;
		this.installProgress = null;
		this.installTotal = null;
		this.cancellingInstall = false;
		this.installLog = this.installLog.filter((entry) => entry.id === major);
		try {
			this.installOutcome = await this.api.installMysql(major);
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.installing = '';
			this.cancellingInstall = false;
		}
		await this.refresh();
		return true;
	}

	/**
	 * Ask the backend to abort the install in flight.
	 *
	 * MANDATORY, not a convenience: nothing bounds the download by wall clock —
	 * only a 30-second idle window — and `openvhost-pkg`'s install permit is
	 * process-wide and taken BEFORE staging, so a server dribbling one byte
	 * every 29 seconds would hold it effectively forever and starve every later
	 * install. This is the only way a user can get that permit back.
	 *
	 * Does not clear {@link installing} itself: the in-flight `install()` call
	 * settles with a `cancelled` outcome and clears it there, so there is
	 * exactly one place the row leaves its installing state.
	 *
	 * A `false` reply means nothing was stopped — the run had already finished,
	 * or the shared lock was held by a different one — so the button is put back
	 * rather than left reading "Cancelling…" over a run that is still going. The
	 * window is small but real: this store sets `installing` synchronously,
	 * before the backend has recorded the run's abort handle.
	 */
	async cancelInstall(): Promise<void> {
		if (this.installing === '' || this.cancellingInstall) return;
		this.cancellingInstall = true;
		try {
			const stopped = await this.api.cancelMysqlInstall();
			if (!stopped) this.cancellingInstall = false;
		} catch (e) {
			this.error = errorMessage(e);
			this.cancellingInstall = false;
		}
	}

	/**
	 * Run the staged-init sequence for `major` (spec D2). Same shape as
	 * {@link install}: re-entrancy guard here, log scoped up front,
	 * environment re-read on settle regardless of outcome (registration on
	 * success, a swept staging dir on failure — either way the on-disk state
	 * may have changed). `initOutcome` is recorded for every settled outcome,
	 * including `failed`, which is what lets {@link initFailureFor} attribute
	 * it to this row.
	 */
	async initialize(major: string): Promise<boolean> {
		if (this.initializing !== '') return false;
		this.initializing = major;
		this.initLog = this.initLog.filter((entry) => entry.id === major);
		let outcome: MysqlInitOutcomeDto;
		try {
			outcome = await this.api.initializeMysql(major);
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.initializing = '';
		}
		this.initOutcome = { major, outcome };
		await this.refresh();
		return true;
	}

	/**
	 * The remembered `failed` outcome for `major`, or `null` — the shape
	 * `mysqlRowState` takes as `initFailure`. Superseded by any later attempt
	 * on ANY major (this store overwrites `initOutcome` wholesale) and, at
	 * the row itself, by a fresh disk read that shows Ready/Foreign (see
	 * `mysqlRowState`'s own precedence — a stale memory of failure must never
	 * outrank what is actually on disk right now).
	 */
	initFailureFor(major: string): MysqlInitFailure | null {
		if (this.initOutcome === null) return null;
		if (this.initOutcome.major !== major) return null;
		if (this.initOutcome.outcome.kind !== 'failed') return null;
		return {
			major,
			step: this.initOutcome.outcome.step,
			reason: this.initOutcome.outcome.reason
		};
	}

	/**
	 * Fetch and cache `major`'s root password, UNLESS it is already cached —
	 * the ONE place either {@link reveal} or {@link copyPassword} reaches the
	 * real value (spec D3/D6 MANDATORY: never fetched eagerly). Idempotent by
	 * design, so both callers share one IPC round trip once the value is
	 * already cached. Deliberately does NOT touch {@link revealed}: that is
	 * the caller's decision, not this fetch's — see the two public methods
	 * below.
	 */
	private async ensurePassword(major: string): Promise<string | undefined> {
		if (this.passwords[major] !== undefined) return this.passwords[major];
		this.passwordError = { ...this.passwordError, [major]: '' };
		this.revealing = { ...this.revealing, [major]: true };
		try {
			const password = await this.api.mysqlRootPassword(major);
			this.passwords = { ...this.passwords, [major]: password };
			return password;
		} catch (e) {
			this.passwordError = { ...this.passwordError, [major]: errorMessage(e) };
			return undefined;
		} finally {
			this.revealing = { ...this.revealing, [major]: false };
		}
	}

	/**
	 * The Reveal button: fetch-if-needed via {@link ensurePassword}, then turn
	 * the display gate ON — the only method that ever does. Never flips the
	 * gate on a failed fetch (nothing to reveal).
	 */
	async reveal(major: string): Promise<void> {
		const password = await this.ensurePassword(major);
		if (password !== undefined) {
			this.revealed = { ...this.revealed, [major]: true };
		}
	}

	/**
	 * The Copy button: fetch-if-needed via the SAME {@link ensurePassword}
	 * cache Reveal uses, returning the value for the caller to write to the
	 * clipboard. Review fix (MANDATORY): deliberately does NOT touch
	 * {@link revealed} — Copy must never un-mask the on-screen field (a
	 * screen-share is exactly the scenario this protects), even though it
	 * fetches and caches the identical secret Reveal does.
	 */
	async copyPassword(major: string): Promise<string | undefined> {
		return this.ensurePassword(major);
	}

	/**
	 * Drops a cached password AND turns the display gate off — the "Hide"
	 * control's dismiss/clear action, never an IPC call. Also used by
	 * {@link resetPassword} to invalidate a now-stale cached value (and
	 * un-reveal it) the instant a reset starts. A no-op, not an error, when
	 * nothing was cached or revealed.
	 */
	forgetPassword(major: string): void {
		if (this.passwords[major] !== undefined) {
			this.passwords = withoutKey(this.passwords, major);
		}
		if (this.revealed[major]) {
			this.revealed = withoutKey(this.revealed, major);
		}
	}

	/**
	 * Regenerate `major`'s root password (spec D3: reset-by-regenerate).
	 * Whatever was cached under {@link passwords} is dropped as the call
	 * STARTS, not only on success — showing the OLD value after a reset
	 * attempt, even a failed one, would be actively misleading rather than
	 * merely stale: a failed reset already means "this password no longer
	 * provably matches". Drops the previous verdict the same instant, mirroring
	 * `webservers.svelte.ts`'s `validate()` — a stale green result must never
	 * sit beside this run's own fresh red one.
	 */
	async resetPassword(major: string): Promise<void> {
		this.forgetPassword(major);
		this.resetError = { ...this.resetError, [major]: '' };
		this.resetOutcome = withoutKey(this.resetOutcome, major);
		this.resetting = { ...this.resetting, [major]: true };
		try {
			const outcome = await this.api.resetMysqlRootPassword(major);
			this.resetOutcome = { ...this.resetOutcome, [major]: outcome };
		} catch (e) {
			this.resetError = { ...this.resetError, [major]: errorMessage(e) };
		} finally {
			this.resetting = { ...this.resetting, [major]: false };
		}
	}

	/**
	 * `SELECT VERSION(), @@port` through the running server — the "it works"
	 * moment (spec D7). Drops the previous verdict as the call starts, same
	 * reasoning as {@link resetPassword}.
	 */
	async verifyConnection(major: string): Promise<void> {
		this.verifyError = { ...this.verifyError, [major]: '' };
		this.verifyResult = withoutKey(this.verifyResult, major);
		this.verifying = { ...this.verifying, [major]: true };
		try {
			const result = await this.api.verifyMysqlConnection(major);
			this.verifyResult = { ...this.verifyResult, [major]: result };
		} catch (e) {
			this.verifyError = { ...this.verifyError, [major]: errorMessage(e) };
		} finally {
			this.verifying = { ...this.verifying, [major]: false };
		}
	}
}
