// SPDX-License-Identifier: GPL-3.0-or-later
// State for the Databases page: the MySQL environment snapshot, the one
// install/init that may be running (they share one InstallLock, spec D7, so
// at most one of `installing`/`initializing` is ever non-'' at a time), their
// live logs, revealed root passwords (fetched ONLY on demand — spec D3/D6
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
import { errorMessage } from './errors';
import { anyMysqlInstalled, type MysqlInitFailure, type UiLog } from './databases.derive';
import type {
	MysqlConnectionProofDto,
	MysqlEnvironmentDto,
	MysqlInitOutcomeDto,
	MysqlInstallOutcomeDto,
	MysqlResetOutcomeDto
} from './ipc';

export interface DatabasesApi {
	mysqlEnvironment(): Promise<MysqlEnvironmentDto>;
	rescanMysql(): Promise<MysqlEnvironmentDto>;
	installMysql(major: string): Promise<MysqlInstallOutcomeDto>;
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

	/** Revealed root passwords, keyed by major. MANDATORY (spec D3/D6): never
	 *  populated except by an explicit {@link reveal} call — never on mount,
	 *  never as a side effect of `refresh`/`rescan`/`install`/`initialize`. */
	passwords = $state<Record<string, string>>({});
	passwordError = $state<Record<string, string>>({});
	revealing = $state<Record<string, boolean>>({});

	resetting = $state<Record<string, boolean>>({});
	resetOutcome = $state<Record<string, MysqlResetOutcomeDto>>({});
	resetError = $state<Record<string, string>>({});

	verifying = $state<Record<string, boolean>>({});
	verifyResult = $state<Record<string, MysqlConnectionProofDto>>({});
	verifyError = $state<Record<string, string>>({});

	constructor(private api: DatabasesApi) {}

	get brewFound(): boolean {
		return this.env?.brewFound ?? false;
	}

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
	 * Install `major` via Homebrew. Mirrors `LanguagesStore.install` exactly:
	 * the re-entrancy guard lives here (not only on a button's `disabled`),
	 * `installLog` is scoped to this attempt's own major before anything else
	 * happens, and a settled call always re-reads the environment via
	 * {@link refresh} rather than assuming the row is now installed.
	 */
	async install(major: string): Promise<boolean> {
		if (this.installing !== '') return false;
		this.installing = major;
		this.installLog = this.installLog.filter((entry) => entry.id === major);
		try {
			this.installOutcome = await this.api.installMysql(major);
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.installing = '';
		}
		await this.refresh();
		return true;
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
	 * the Reveal/Copy affordance's one and only path to the real value (spec
	 * D3/D6 MANDATORY: never fetched eagerly). Idempotent by design, so both
	 * Reveal and Copy can call this without doubling the IPC round trip once
	 * the value is already on screen.
	 */
	async reveal(major: string): Promise<void> {
		if (this.passwords[major] !== undefined) return;
		this.passwordError = { ...this.passwordError, [major]: '' };
		this.revealing = { ...this.revealing, [major]: true };
		try {
			const password = await this.api.mysqlRootPassword(major);
			this.passwords = { ...this.passwords, [major]: password };
		} catch (e) {
			this.passwordError = { ...this.passwordError, [major]: errorMessage(e) };
		} finally {
			this.revealing = { ...this.revealing, [major]: false };
		}
	}

	/**
	 * Drops a cached password from memory — the "Hide" control's dismiss/clear
	 * action, never an IPC call. Also used by {@link resetPassword} to
	 * invalidate a now-stale cached value the instant a reset starts. A no-op,
	 * not an error, when nothing was cached.
	 */
	forgetPassword(major: string): void {
		if (this.passwords[major] === undefined) return;
		this.passwords = withoutKey(this.passwords, major);
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
