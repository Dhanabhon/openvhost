// SPDX-License-Identifier: GPL-3.0-or-later
// State for the MariaDB group on the Databases page (P1 MariaDB UI design
// D1): the single-instance environment snapshot, the one install/init that
// may be running, its live install progress and post-hoc init log, the
// cached root password (fetched ONLY on demand — spec D3/D6's MANDATORY rule
// carries unchanged from MySQL's slice), and the last reset/verify outcome.
//
// SCALARS, NOT DICTIONARIES — the whole reason this is its OWN store rather
// than a wider `DatabasesStore`. MariaDB ships exactly one series
// (`MARIADB_SERIES`), and `MariadbInstanceRepo` "binds MARIADB_SERIES itself;
// none takes a series from a caller", so `DatabasesStore`'s ten per-major
// `Record<string, T>` maps would invent a namespace and a concurrency that
// cannot exist here. One password, one revealed flag, one verify result.
//
// Shape otherwise mirrors `databases.svelte.ts`: an injected API object (so
// tests never touch real IPC), plain `$state` fields, and re-entrancy guards
// that live HERE rather than only on a button's `disabled` attribute.
import { errorMessage } from './errors';
import type { EngineInstanceDto, MysqlInitFailure } from './databases.derive';
import type {
	MariadbConnectionProofDto,
	MariadbEnvironmentDto,
	MariadbInitOutcomeDto,
	MariadbInstallOutcomeDto,
	MariadbInstallProgressDto,
	MariadbResetOutcomeDto
} from './ipc';

export interface MariadbApi {
	mariadbEnvironment(): Promise<MariadbEnvironmentDto>;
	rescanMariadb(): Promise<MariadbEnvironmentDto>;
	installMariadb(): Promise<MariadbInstallOutcomeDto>;
	/** Aborts an in-flight `installMariadb`, resolving to whether anything was
	 *  actually stopped. See {@link MariadbStore.cancelInstall}. */
	cancelMariadbInstall(): Promise<boolean>;
	initializeMariadb(): Promise<MariadbInitOutcomeDto>;
	mariadbRootPassword(): Promise<string>;
	resetMariadbRootPassword(): Promise<MariadbResetOutcomeDto>;
	verifyMariadbConnection(): Promise<MariadbConnectionProofDto>;
}

/** The one series this build ships (`openvhost_core::MARIADB_SERIES` on the
 *  Rust side, `"11.4"`). MariaDB's own DTOs carry no wire field for this
 *  (design: "a field nothing can vary is overhead" — the same reasoning
 *  `MariadbInstance` gives for leaving `major` off its own struct), so this
 *  is the ONE place the frontend invents an identity value for the row this
 *  store describes. Exported so `+page.svelte` and this module's own tests
 *  share the identical literal rather than two copies that could drift. */
export const MARIADB_SERIES = '11.4';

/** Same shape `LogPane.svelte` renders (`services.svelte.ts`'s `UiLog`),
 *  redeclared here rather than imported — the same decoupling
 *  `databases.derive.ts`'s own `UiLog` doc comment gives, now applied a
 *  third time: this module stays independent of the MySQL-facing derive
 *  layer, and the only thing genuinely shared is the shape LogPane expects. */
export interface UiLog {
	id: string;
	tsMs: number;
	level: 'info' | 'warn' | 'error';
	line: string;
}

/** Upper bound on `installLog`/`initLog`'s length — same value and
 *  slice-when-over-cap technique as `DatabasesStore`'s `LOG_CAP`. */
const LOG_CAP = 200;

/**
 * Adapts the single-instance `MariadbEnvironmentDto` into the shape the
 * SHARED `MysqlRow`/`MysqlCredentials` components render from
 * (`EngineInstanceDto`, design D1). MariaDB's own DTO carries neither
 * `major` nor `cataloged` — `MariadbInstanceRepo` "refuses a series
 * argument", and this build manages exactly the one series it ships — so
 * this is the one place that invents the row's identity value
 * ({@link MARIADB_SERIES}) and its `cataloged` flag (always `true`: unlike
 * MySQL's multi-major catalogue, there is no "installed but unmanaged"
 * concept here to represent — a MariaDB install can only ever be the one
 * series this build manages). `source` is always `null`: MariaDB has no
 * provenance to disambiguate, the same fact `MARIADB_DESCRIPTOR.sourcePolicy`
 * encodes by ignoring its argument unconditionally.
 */
export function mariadbInstance(env: MariadbEnvironmentDto): EngineInstanceDto {
	return {
		major: MARIADB_SERIES,
		cataloged: true,
		installed: env.installed,
		path: env.path,
		socketPath: env.socketPath,
		serviceId: env.serviceId,
		datadirState: env.datadirState,
		source: null,
		offer: env.offer
	};
}

export class MariadbStore {
	env = $state<MariadbEnvironmentDto | null>(null);
	/** Page-level failure: the environment could not be loaded/rescanned at
	 *  all, or an outside failure (see {@link fail}). */
	error = $state('');

	/** Whether `install_mariadb` is currently running. A boolean, not a major
	 *  string like `DatabasesStore.installing`: this build ships exactly one
	 *  series, so there is no "which" left to record, only "is it happening". */
	installing = $state(false);
	installLog = $state<UiLog[]>([]);
	/** The last pipeline state of the install in flight, or `null` before the
	 *  first event arrives (and after a fresh install starts). */
	installProgress = $state<MariadbInstallProgressDto | null>(null);
	/** The download length the server declared, captured from the `started`
	 *  event because no later event repeats it. */
	installTotal = $state<number | null>(null);
	/** True between {@link cancelInstall} being pressed and the install
	 *  settling — see `DatabasesStore.cancellingInstall`'s identical doc
	 *  comment for why this is a state and not a disabled attribute. */
	cancellingInstall = $state(false);
	/** The last `install_mariadb` outcome. `MariadbInstallOutcomeDto` carries
	 *  no major of its own (design: a field nothing can vary is overhead), so
	 *  this stays a bare scalar — the row's own tagged-prop contract is
	 *  satisfied at the ONE place that actually feeds it
	 *  (`routes/databases/+page.svelte`), not baked in here. */
	installOutcome = $state<MariadbInstallOutcomeDto | null>(null);

	/** Whether `initialize_mariadb` is currently running. */
	initializing = $state(false);
	/** POST-HOC ONLY. `initialize_mariadb` — unlike `initialize_mysql` —
	 *  reports no intermediate output; `mariadb-init-log-event` fires once,
	 *  after the run ends, relaying a FAILED run's reason split into lines.
	 *  Never populated while {@link initializing} is true, so a consumer that
	 *  rendered this live (the way `MysqlRow`'s `initializing` state renders
	 *  `DatabasesStore.initLog`) would show nothing for the whole run and then
	 *  dump everything at once — which reads as a frozen app. This store does
	 *  not pretend otherwise: {@link initFailure}'s `reason` already carries
	 *  the same text in full, so this log is kept for parity with the
	 *  registered channel (and any future consumer) rather than rendered as a
	 *  second copy of words the row already shows. */
	initLog = $state<UiLog[]>([]);
	initOutcome = $state<MariadbInitOutcomeDto | null>(null);

	/** The cached root password, once fetched. MANDATORY (spec D3/D6): never
	 *  populated except by an explicit {@link reveal}/{@link copyPassword}
	 *  call — never on mount, never as a side effect of
	 *  `refresh`/`rescan`/`install`/`initialize`. NOT the display gate — see
	 *  {@link revealed}. */
	password = $state<string | undefined>(undefined);
	passwordError = $state('');
	revealing = $state(false);
	/** The DISPLAY gate — true ONLY after an explicit {@link reveal} call,
	 *  cleared by {@link forgetPassword} (Hide). Kept deliberately separate
	 *  from {@link password} so a cache hit alone (a prior
	 *  {@link copyPassword}) can never flip the UI to plaintext — the same
	 *  review fix `DatabasesStore.revealed`'s own doc comment records for
	 *  MySQL, carried here unchanged. */
	revealed = $state(false);

	resetting = $state(false);
	resetOutcome = $state<MariadbResetOutcomeDto | undefined>(undefined);
	resetError = $state('');

	verifying = $state(false);
	verifyResult = $state<MariadbConnectionProofDto | undefined>(undefined);
	verifyError = $state('');

	constructor(private api: MariadbApi) {}

	get anyInstalled(): boolean {
		return this.env !== null && this.env.installed;
	}

	/** `''` when idle, otherwise {@link MARIADB_SERIES} — the shape the
	 *  SHARED row's `installingMajor` prop expects (`MysqlRowState`'s own
	 *  contract), computed here once rather than re-derived at every call
	 *  site that needs it. */
	get installingMajor(): string {
		return this.installing ? MARIADB_SERIES : '';
	}

	get initializingMajor(): string {
		return this.initializing ? MARIADB_SERIES : '';
	}

	/** The remembered `failed` outcome, in the shape `mysqlRowState` takes as
	 *  `initFailure` (`MysqlInitFailure`, reused verbatim rather than a
	 *  parallel MariaDB type: `MariadbInitStepDto` is a strict SUBSET of
	 *  `MysqlInitStepDto` — design: "no `Validate` variant, MariaDB has no
	 *  `--validate-config`" — so every MariaDB step is already a valid
	 *  `MysqlInitStepDto` with no cast). `null` once no failed attempt is
	 *  remembered, or once a fresher disk read has superseded it — same
	 *  precedence `mysqlRowState`'s own doc comment states for MySQL. */
	get initFailure(): MysqlInitFailure | null {
		if (this.initOutcome === null || this.initOutcome.kind !== 'failed') return null;
		return { major: MARIADB_SERIES, step: this.initOutcome.step, reason: this.initOutcome.reason };
	}

	/** Cheap, spawn-free snapshot — mirrors `DatabasesStore.refresh` exactly,
	 *  including keeping the last known environment on a failed re-read. */
	async refresh(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.mariadbEnvironment();
		} catch (e) {
			this.error = errorMessage(e);
		}
	}

	/** The "Check again" button — mirrors `DatabasesStore.rescan` exactly. */
	async rescan(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.rescanMariadb();
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

	appendInstallLog(line: string): void {
		const next = [
			...this.installLog,
			{ id: MARIADB_SERIES, tsMs: Date.now(), level: 'info' as const, line }
		];
		this.installLog = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}

	appendInitLog(line: string): void {
		const next = [
			...this.initLog,
			{ id: MARIADB_SERIES, tsMs: Date.now(), level: 'info' as const, line }
		];
		this.initLog = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}

	/** Record one install-pipeline state as it arrives — `started` is also
	 *  where the declared total is captured, since no later event repeats it. */
	applyInstallProgress(progress: MariadbInstallProgressDto): void {
		this.installProgress = progress;
		if (progress.kind === 'started') this.installTotal = progress.total;
	}

	/**
	 * Install the pinned MariaDB series — download, verify, extract.
	 * Mirrors `DatabasesStore.install`'s discipline exactly (the re-entrancy
	 * guard lives here, progress is cleared as the run STARTS not when it
	 * ends, a settled call always re-reads the environment), minus the major
	 * parameter `install_mariadb` never takes (design D7: "none takes a
	 * series argument").
	 */
	async install(): Promise<boolean> {
		if (this.installing) return false;
		this.installing = true;
		this.installProgress = null;
		this.installTotal = null;
		this.cancellingInstall = false;
		this.installLog = [];
		try {
			this.installOutcome = await this.api.installMariadb();
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.installing = false;
			this.cancellingInstall = false;
		}
		await this.refresh();
		return true;
	}

	/** Ask the backend to abort the install in flight — see
	 *  `DatabasesStore.cancelInstall`'s doc comment for the full reasoning
	 *  (identical here: one shared `InstallLock`, no wall-clock bound on the
	 *  download). */
	async cancelInstall(): Promise<void> {
		if (!this.installing || this.cancellingInstall) return;
		this.cancellingInstall = true;
		try {
			const stopped = await this.api.cancelMariadbInstall();
			if (!stopped) this.cancellingInstall = false;
		} catch (e) {
			this.error = errorMessage(e);
			this.cancellingInstall = false;
		}
	}

	/**
	 * Run MariaDB's staged init. Same shape as {@link install}: re-entrancy
	 * guard here, log scoped up front, environment re-read on settle
	 * regardless of outcome. `initOutcome` is recorded for every settled
	 * outcome, including `failed`, which is what lets {@link initFailure}
	 * attribute it to this row.
	 */
	async initialize(): Promise<boolean> {
		if (this.initializing) return false;
		this.initializing = true;
		this.initLog = [];
		let outcome: MariadbInitOutcomeDto;
		try {
			outcome = await this.api.initializeMariadb();
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.initializing = false;
		}
		this.initOutcome = outcome;
		await this.refresh();
		return true;
	}

	/** Fetch-if-needed the root password — the ONE place either
	 *  {@link reveal} or {@link copyPassword} reaches the real value (spec
	 *  D3/D6 MANDATORY: never fetched eagerly). Idempotent, so both callers
	 *  share one IPC round trip once the value is already cached. Deliberately
	 *  does NOT touch {@link revealed}: that is the caller's decision. */
	private async ensurePassword(): Promise<string | undefined> {
		if (this.password !== undefined) return this.password;
		this.passwordError = '';
		this.revealing = true;
		try {
			const password = await this.api.mariadbRootPassword();
			this.password = password;
			return password;
		} catch (e) {
			this.passwordError = errorMessage(e);
			return undefined;
		} finally {
			this.revealing = false;
		}
	}

	/** The Reveal button: fetch-if-needed via {@link ensurePassword}, then
	 *  turn the display gate ON — the only method that ever does. */
	async reveal(): Promise<void> {
		const password = await this.ensurePassword();
		if (password !== undefined) this.revealed = true;
	}

	/** The Copy button: fetch-if-needed via the SAME {@link ensurePassword}
	 *  cache Reveal uses. MANDATORY: deliberately does NOT touch
	 *  {@link revealed} — Copy must never un-mask the on-screen field (a
	 *  screen-share is exactly the scenario this protects). */
	async copyPassword(): Promise<string | undefined> {
		return this.ensurePassword();
	}

	/** Drops the cached password AND turns the display gate off — the "Hide"
	 *  control's dismiss/clear action, never an IPC call. Also used by
	 *  {@link resetPassword} to invalidate a now-stale cached value the
	 *  instant a reset starts. */
	forgetPassword(): void {
		this.password = undefined;
		this.revealed = false;
	}

	/** Regenerate the root password (spec D3: reset-by-regenerate). Whatever
	 *  was cached is dropped as the call STARTS, not only on success — same
	 *  "drop the previous verdict as the run starts" rule
	 *  `webservers.svelte.ts`'s `validate()` follows. */
	async resetPassword(): Promise<void> {
		this.forgetPassword();
		this.resetError = '';
		this.resetOutcome = undefined;
		this.resetting = true;
		try {
			this.resetOutcome = await this.api.resetMariadbRootPassword();
		} catch (e) {
			this.resetError = errorMessage(e);
		} finally {
			this.resetting = false;
		}
	}

	/** `SELECT VERSION(), @@port` through the running server — the "it works"
	 *  moment (spec D7). Drops the previous verdict as the call starts, same
	 *  reasoning as {@link resetPassword}. */
	async verifyConnection(): Promise<void> {
		this.verifyError = '';
		this.verifyResult = undefined;
		this.verifying = true;
		try {
			this.verifyResult = await this.api.verifyMariadbConnection();
		} catch (e) {
			this.verifyError = errorMessage(e);
		} finally {
			this.verifying = false;
		}
	}
}
