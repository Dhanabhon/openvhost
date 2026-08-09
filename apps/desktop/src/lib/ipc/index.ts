// SPDX-License-Identifier: GPL-3.0-or-later
// The ONLY module allowed to touch Tauri IPC (master plan §5).
import { commands, events } from './bindings';
// The one non-generated type import in this file, and it is load-bearing:
// `uninstall_plan`'s payload is what a DESTRUCTIVE confirmation renders, so the
// two uninstall wrappers below are typed against the shape that dialog reads
// (`uninstall.derive.ts`) rather than a generated alias. A Rust-side drift then
// fails to compile at the wrapper instead of silently rendering a blank where a
// kept datadir path belongs. `uninstall.derive.ts` imports nothing from here, so
// this introduces no cycle.
import type { PackageKind, UninstallPlan } from '../uninstall.derive';
import type {
	ApplyOutcomeDto,
	ApplyPlanDto,
	BootStatusDto,
	CoreInfo,
	CreateSiteResult,
	DefaultPhpDto,
	FileChangeDto,
	HomeUsageDto,
	IpcError,
	LogLevel,
	LogLine,
	LogResetDto,
	LogRowDto,
	LogSourceDto,
	LogSourceKindDto,
	LogSourceRowDto,
	LogWindowDto,
	LogWindowQuery,
	MariadbConnectionProofDto,
	MariadbDatadirStateDto,
	MariadbEnvironmentDto,
	MariadbInitLogEvent,
	MariadbInitOutcomeDto,
	MariadbInitStepDto,
	MariadbInstallLogEvent,
	MariadbInstallOutcomeDto,
	MariadbInstallProgressDto,
	MariadbInstallProgressEvent,
	MariadbInstallResultDto,
	MariadbLedgerWriteDto,
	MariadbPackageOfferDto,
	MariadbResetOutcomeDto,
	MysqlConnectionProofDto,
	MysqlDatadirStateDto,
	MysqlEnvironmentDto,
	MysqlInitLogEvent,
	MysqlInitOutcomeDto,
	MysqlInitStepDto,
	MysqlInstallLogEvent,
	MysqlInstallOutcomeDto,
	MysqlInstallProgressDto,
	MysqlInstallProgressEvent,
	MysqlInstallResultDto,
	MysqlInstanceDto,
	MysqlLedgerWriteDto,
	MysqlPackageOfferDto,
	MysqlResetOutcomeDto,
	MysqlRuntimeSourceDto,
	NginxRuntimeSourceDto,
	PendingInstallDto,
	PhpEnvironmentDto,
	PhpInstallLogEvent,
	PhpInstallOutcomeDto,
	PhpInstallProgressDto,
	PhpInstallProgressEvent,
	PhpInstallResultDto,
	PhpLedgerWriteDto,
	PhpPackageOfferDto,
	PhpRuntimeDto,
	PhpRuntimeSourceDto,
	ScaffoldOutcomeDto,
	ScaffoldStepDto,
	ServiceLogEvent,
	ServiceProblemDto,
	ServiceRegisteredEvent,
	ServicesMemoryDto,
	ServiceStateEvent,
	ServiceStatus,
	ServiceUnregisteredEvent,
	SiteDto,
	SiteInput,
	ValidationReportDto,
	WebServerDto,
	WebServerSettingsDto
} from './bindings';

export type {
	ApplyOutcomeDto,
	ApplyPlanDto,
	BootStatusDto,
	CoreInfo,
	CreateSiteResult,
	DefaultPhpDto,
	FileChangeDto,
	HomeUsageDto,
	IpcError,
	LogLevel,
	LogLine,
	LogResetDto,
	LogRowDto,
	LogSourceDto,
	LogSourceKindDto,
	LogSourceRowDto,
	LogWindowDto,
	LogWindowQuery,
	MariadbConnectionProofDto,
	MariadbDatadirStateDto,
	MariadbEnvironmentDto,
	MariadbInitLogEvent,
	MariadbInitOutcomeDto,
	MariadbInitStepDto,
	MariadbInstallLogEvent,
	MariadbInstallOutcomeDto,
	MariadbInstallProgressDto,
	MariadbInstallProgressEvent,
	MariadbInstallResultDto,
	MariadbLedgerWriteDto,
	MariadbPackageOfferDto,
	MariadbResetOutcomeDto,
	MysqlConnectionProofDto,
	MysqlDatadirStateDto,
	MysqlEnvironmentDto,
	MysqlInitLogEvent,
	MysqlInitOutcomeDto,
	MysqlInitStepDto,
	MysqlInstallLogEvent,
	MysqlInstallOutcomeDto,
	MysqlInstallProgressDto,
	MysqlInstallProgressEvent,
	MysqlInstallResultDto,
	MysqlInstanceDto,
	MysqlLedgerWriteDto,
	MysqlPackageOfferDto,
	MysqlResetOutcomeDto,
	MysqlRuntimeSourceDto,
	NginxRuntimeSourceDto,
	PendingInstallDto,
	PhpEnvironmentDto,
	PhpInstallLogEvent,
	PhpInstallOutcomeDto,
	PhpInstallProgressDto,
	PhpInstallProgressEvent,
	PhpInstallResultDto,
	PhpLedgerWriteDto,
	PhpPackageOfferDto,
	PhpRuntimeDto,
	PhpRuntimeSourceDto,
	ScaffoldOutcomeDto,
	ScaffoldStepDto,
	ServiceLogEvent,
	ServiceProblemDto,
	ServiceRegisteredEvent,
	ServicesMemoryDto,
	ServiceStateEvent,
	ServiceStatus,
	ServiceUnregisteredEvent,
	SiteDto,
	SiteInput,
	ValidationReportDto,
	WebServerDto,
	WebServerSettingsDto
};

function isIpcError(e: unknown): e is IpcError {
	return typeof e === 'object' && e !== null && typeof (e as { kind?: unknown }).kind === 'string';
}

/**
 * Normalize any thrown/rejected value into an `IpcError`. Real `IpcError`s
 * pass through unchanged; anything else escaping the invoke layer (raw
 * `Error` instances, plain-string rejections, other transport errors) is
 * wrapped as the `core` variant so callers can always rely on `.kind` and a
 * string `.message` — the UI never renders "undefined" or crashes on a
 * bare-primitive throw.
 */
function normalizeError(e: unknown): IpcError {
	if (isIpcError(e)) return e;
	return { kind: 'core', message: String(e) } satisfies IpcError;
}

/**
 * Fetch CoreInfo from the Rust core. Failures always throw an `IpcError`:
 * anything else escaping the invoke layer (transport errors, plain strings)
 * is normalized to the `core` variant so the UI never renders "undefined".
 */
export async function coreInfo(simulateError = false): Promise<CoreInfo> {
	try {
		const result = await commands.coreInfo(simulateError ? true : null);
		if (result.status === 'error') throw result.error;
		return result.data;
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Await a tauri-specta result, unwrapping to `.data` on success. Every error
 * path — a resolved `{ status: 'error' }` envelope AND a rejection that never
 * made it into that envelope (transport errors thrown before/instead of the
 * envelope) — is routed through {@link normalizeError}, the same
 * normalization `coreInfo` uses, so callers always see a proper `IpcError`.
 */
async function unwrap<T>(
	resultPromise: Promise<{ status: 'ok'; data: T } | { status: 'error'; error: unknown }>
): Promise<T> {
	try {
		const r = await resultPromise;
		if (r.status === 'error') throw r.error;
		return r.data;
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Why `state.db` is unavailable this run, or `null` when it opened fine.
 *
 * The one input to the app-level "data store unavailable" banner. The string is
 * the underlying failure — *unable to open database file*, *permission denied* —
 * and is rendered verbatim, never parsed: it is an OS/sqlite sentence nobody
 * agreed to keep stable, the same discipline `ApplyErrorBanner` applies to its
 * own `error` prop.
 */
export async function stateStoreStatus(): Promise<string | null> {
	return unwrap(commands.stateStoreStatus());
}

/**
 * How far this launch actually got (degraded-boot design D1).
 *
 * The one input to the takeover screen, and the one command that answers on
 * EVERY boot path: it extracts only `BootState`, which `setup` manages exactly
 * once at the top level, outside every bail arm. `stateStoreStatus` above does
 * NOT answer on a degraded arm — the `DbHandle` it reads is managed inside the
 * one arm that succeeded — so the banner it feeds cannot cover these states and
 * the takeover is what covers them instead.
 *
 * `home`, `path` and `reason` are rendered verbatim: `reason` is raw OS text
 * (*Permission denied (os error 13)*), and on `runDirUnusable` the path and the
 * errno ARE the payload — that is the whole of what makes it a user-fixable
 * permissions problem rather than a mystery.
 */
export async function bootStatus(): Promise<BootStatusDto> {
	return unwrap(commands.bootStatus());
}

/**
 * Show the unusable run directory in Finder (degraded-boot design D3).
 *
 * **Takes no path, and that is the point.** Rust reads the same `BootState`
 * `bootStatus` reports and reveals only `runDirUnusable`'s own directory; every
 * other state refuses. So this is not a "reveal any folder" primitive the
 * renderer gained — it can only ask for the folder it was already told about.
 * The same reason `openHomebrewSite` takes no URL.
 *
 * **Rejects, and the caller must render it.** The commonest `runDirUnusable`
 * is a `<home>/run` that could not be created, in which case there is nothing
 * on disk to reveal and this comes back *No such file or directory (os error
 * 2)*. The screen shows the path as copyable text regardless — that half never
 * fails.
 */
export async function revealRunDir(): Promise<void> {
	await unwrap(commands.revealRunDir());
}
export async function listServices(): Promise<ServiceStatus[]> {
	return unwrap(commands.listServices());
}
export async function startService(id: string): Promise<void> {
	await unwrap(commands.startService(id));
}
export async function stopService(id: string): Promise<void> {
	await unwrap(commands.stopService(id));
}
export async function serviceLogTail(id: string, n: number): Promise<LogLine[]> {
	return unwrap(commands.serviceLogTail(id, n));
}
/**
 * Subscribe to `service-state`. Rejects with an `IpcError`, like every command
 * above: `events.*.listen` reaches the transport directly (it is not routed
 * through {@link unwrap}), so a listener that cannot be registered would
 * otherwise reject with a raw `Error` — and the caller is now
 * `routes/+layout.svelte`, which renders the failure through the same
 * `.kind`/`.message` banner shape as everything else.
 */
export async function onServiceState(cb: (ev: ServiceStateEvent) => void): Promise<() => void> {
	try {
		return await events.serviceStateEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}
/** Subscribe to `service-log`. Same `IpcError` contract as {@link onServiceState}. */
export async function onServiceLog(cb: (ev: ServiceLogEvent) => void): Promise<() => void> {
	try {
		return await events.serviceLogEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Subscribe to `service-registered`, emitted when the supervisor adds a row
 * — including after startup (a PHP major installed at runtime, a freshly
 * initialized MySQL major). Carries the full {@link ServiceStatus}, not a
 * delta, since the id may be new to every caller. Same `IpcError` contract
 * as {@link onServiceState}.
 */
export async function onServiceRegistered(
	cb: (ev: ServiceRegisteredEvent) => void
): Promise<() => void> {
	try {
		return await events.serviceRegisteredEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Subscribe to `service-unregistered`, emitted when the supervisor FORGETS a
 * row — an in-app uninstall of a PHP or MySQL major (package-uninstall design
 * D4). Carries the id alone: the row is gone, so there is no status left to
 * describe. Same `IpcError` contract as {@link onServiceState}.
 */
export async function onServiceUnregistered(
	cb: (ev: ServiceUnregisteredEvent) => void
): Promise<() => void> {
	try {
		return await events.serviceUnregisteredEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

export async function listSites(): Promise<SiteDto[]> {
	return unwrap(commands.listSites());
}
/**
 * Create a site. `createFolder: true` also scaffolds the docroot folder and a
 * starter page on disk — see `CreateSiteResult.scaffold` (`null` means it was
 * not requested at all, never a fourth outcome alongside the scaffold enum's
 * own three).
 */
export async function createSite(
	input: SiteInput,
	createFolder: boolean
): Promise<CreateSiteResult> {
	return unwrap(commands.createSite(input, createFolder));
}
export async function updateSite(id: string, input: SiteInput): Promise<SiteDto> {
	return unwrap(commands.updateSite(id, input));
}
export async function deleteSite(id: string): Promise<boolean> {
	return unwrap(commands.deleteSite(id));
}

/**
 * Open a site in the default browser. Takes an id, NOT a URL — the URL is built in
 * Rust from the stored row, so this cannot be used to open an arbitrary address.
 */
export async function openSite(id: string): Promise<void> {
	await unwrap(commands.openSite(id));
}

/**
 * Open Homebrew's install page in the user's browser. Zero parameters, and the URL
 * is a Rust literal (`commands::open_homebrew_site`) — the webview never supplies one.
 *
 * This exists because a plain `<a target="_blank">` is inert in this webview: Tauri
 * only handles a new-window request when the app registers `on_new_window`, which it
 * does not, so the click would otherwise silently do nothing.
 */
export async function openHomebrewSite(): Promise<void> {
	await unwrap(commands.openHomebrewSite());
}

/**
 * What Apply would change across the whole generated config — the sites AND
 * the editable nginx settings, which are one config set and therefore one
 * plan. Read-only and process-free: safe to call after every site mutation and
 * every settings save for a pending-changes banner.
 */
export async function planConfigApply(): Promise<ApplyPlanDto> {
	return unwrap(commands.planConfigApply());
}

/**
 * Write the generated config, then restart whichever affected services were
 * running. Services that were not running are reported in `notStarted` instead
 * of being started as a side effect. A service that was running but could not be
 * cleanly stopped-and-restarted lands in `needsAttention` instead of
 * `restarted` — the UI must not present that outcome as a success.
 */
export async function applyConfig(): Promise<ApplyOutcomeDto> {
	return unwrap(commands.applyConfig());
}

/**
 * The stored nginx settings, or the documented defaults when nothing has been
 * saved yet. Reading never writes a row.
 */
export async function webServerSettings(): Promise<WebServerSettingsDto> {
	return unwrap(commands.webServerSettings());
}

/**
 * Validate and store the nginx settings. Does NOT apply them: the live config
 * changes only through {@link applyConfig}, which shows a diff first. A
 * rejected field throws an `IpcError` of kind `validation` naming that field
 * (snake_case, like the site editor's), and nothing is written — the values
 * already stored are left exactly as they were.
 */
export async function saveWebServerSettings(input: WebServerSettingsDto): Promise<void> {
	await unwrap(commands.saveWebServerSettings(input));
}

export async function listWebServers(): Promise<WebServerDto[]> {
	return unwrap(commands.listWebServers());
}
export async function readWebServerConfig(id: string): Promise<string> {
	return unwrap(commands.readWebServerConfig(id));
}
export async function validateWebServerConfig(id: string): Promise<ValidationReportDto> {
	return unwrap(commands.validateWebServerConfig(id));
}

/**
 * Stop every running service and tear the window down. Resolves only if the
 * quit did NOT happen — a successful quit destroys the window mid-call, so the
 * caller must treat "still here after awaiting" as a failure worth showing
 * rather than as success.
 */
export async function confirmQuit(): Promise<void> {
	await unwrap(commands.confirmQuit());
}

/**
 * Tell the Rust side that {@link onQuitRequested}'s listener is registered.
 *
 * Until this lands, a window close is NOT intercepted — so calling it is what
 * turns the confirmation on, and failing to call it degrades to the old
 * close-immediately behaviour rather than to an unquittable window.
 */
export async function quitDialogReady(): Promise<void> {
	await unwrap(commands.quitDialogReady());
}

/**
 * Subscribe to `quit-requested`, emitted when a close was intercepted. Same
 * `IpcError` contract as {@link onServiceState}.
 */
export async function onQuitRequested(cb: () => void): Promise<() => void> {
	try {
		return await events.quitRequestedEvent.listen(() => cb());
	} catch (e) {
		throw normalizeError(e);
	}
}

/** Resident memory of the supervised services, plus how many pids answered. */
export async function servicesMemory(): Promise<ServicesMemoryDto> {
	return unwrap(commands.servicesMemory());
}

/** Total bytes under the OpenVHost home. */
export async function homeDiskUsage(): Promise<HomeUsageDto> {
	return unwrap(commands.homeDiskUsage());
}

/**
 * Read-only PHP environment summary for the Languages page: whether Homebrew
 * was found, where it looked, and one row per catalogue/installed version.
 * Spawns nothing — safe to call on page mount and after every install.
 */
export async function phpEnvironment(): Promise<PhpEnvironmentDto> {
	return unwrap(commands.phpEnvironment());
}

/**
 * Explicit, user-initiated re-probe behind the "Check again" button. Unlike
 * {@link phpEnvironment}, this spawns a version probe per candidate binary.
 */
export async function rescanPhpRuntimes(): Promise<PhpEnvironmentDto> {
	return unwrap(commands.rescanPhpRuntimes());
}

/**
 * Install a PHP major — from OpenVHost's own package tree when this build
 * publishes one for that major on this host, and via Homebrew otherwise
 * (off-Homebrew slice 5C design D4). **The route is decided server-side**, from
 * the same table that fills a row's `offer`; nothing here chooses a pipeline.
 *
 * Resolves with a TAGGED outcome rather than throwing on a package-pipeline
 * failure — a verification failure, a stall, an unpublished release and an
 * unsupported host are all states of `result`, because throwing them would
 * discard the distinction `PhpPackageOfferDto`'s three states exist to make.
 *
 * `result.kind === 'brew'` is the Homebrew route, and it is the ONLY arm with an
 * `exitCode`: it is the only one with a child process. A packaged install
 * spawns nothing, so testing `exitCode !== 0` against any other arm would read a
 * success as "brew was killed". Its `detected` can be `false` even when
 * `exitCode` is `0` — that is the silent-failure case it exists for.
 *
 * Streams brew's output through {@link onPhpInstallLog} on the Homebrew route,
 * and typed pipeline states through {@link onPhpInstallProgress} on the packaged
 * one. Shares the one install lock with every other package operation.
 */
export async function installPhp(major: string): Promise<PhpInstallOutcomeDto> {
	return unwrap(commands.installPhp(major));
}

/**
 * Choose which PHP major the catch-all (`localhost:8080`) serves, or clear the
 * choice with `null` so the historical first-discovered rule applies again.
 *
 * **Stores only — it does not apply.** The generated config changes on the next
 * explicit {@link applyConfig}, which shows a diff first, validates with
 * `nginx -t` and rolls back if that fails. This is the same two-step
 * {@link saveWebServerSettings} follows, and for the same reason: a second way
 * for the live config to change would be a change nobody saw a diff for.
 *
 * A major that is not installed is refused with an `IpcError` of kind
 * `validation` naming `default_major`, and nothing is written. "Your default is
 * no longer installed" is a state you ARRIVE at by uninstalling it — never one
 * you can choose — so it is always the report of something that really
 * happened.
 */
export async function setDefaultPhp(major: string | null): Promise<void> {
	await unwrap(commands.setDefaultPhp(major));
}

/**
 * Cancel an in-flight PHP install — the mirror of {@link cancelMariadbInstall},
 * and the identical reasoning: neither a download nor a `brew install` has a
 * wall-clock bound, and the install permit is process-wide, so an install nobody
 * can stop starves every later one.
 *
 * Cancels whichever route the install took, and resolves `false` when the slot
 * holds something else (a MySQL install, an uninstall) or nothing at all.
 */
export async function cancelPhpInstall(): Promise<boolean> {
	return unwrap(commands.cancelPhpInstall());
}

/**
 * What uninstalling `major` would remove, what it would keep, and what — if
 * anything — refuses it (package-uninstall design D2/D3). A PURE QUERY: it
 * spawns no process and changes nothing, which is what makes it safe to call
 * every time the user presses Uninstall, so the confirmation shows the real
 * inventory instead of a guess.
 *
 * Typed against `uninstall.derive.ts`'s own `PackageKind`/`UninstallPlan`
 * rather than a generated alias, deliberately: that is the shape the
 * confirmation actually renders, so a Rust-side change that stopped matching it
 * fails to compile HERE instead of surfacing as a blank line in a destructive
 * dialog. See `uninstall.shared.svelte.ts` for the other half of that seam.
 */
export async function uninstallPlan(kind: PackageKind, major: string): Promise<UninstallPlan> {
	return unwrap(commands.uninstallPlan(kind, major));
}

/**
 * Uninstall `major` — `brew uninstall` plus the generated-config cleanup,
 * through the same install lock and the same live-output channel
 * {@link installPhp} uses (design D1). The datadir, the stored credentials and
 * the log directories are never touched on any path (design D2).
 *
 * Every refusal is decided server-side before anything is spawned, so calling
 * this without checking {@link uninstallPlan} first is refused rather than
 * forced.
 */
export async function uninstallPackage(kind: PackageKind, major: string): Promise<void> {
	await unwrap(commands.uninstallPackage(kind, major));
}

/** Subscribe to `php-install-log` — brew's own output, so the HOMEBREW route
 *  only. Same `IpcError` contract as {@link onServiceState}. */
export async function onPhpInstallLog(cb: (ev: PhpInstallLogEvent) => void): Promise<() => void> {
	try {
		return await events.phpInstallLogEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/** Subscribe to `php-install-progress` — one typed state per pipeline step on
 *  the PACKAGED route, the mirror of {@link onMysqlInstallProgress}. Carries
 *  `major`, unlike MariaDB's, because several PHP majors sit side by side and a
 *  progress bar has to know which row it belongs to. Same `IpcError` contract as
 *  {@link onServiceState}. */
export async function onPhpInstallProgress(
	cb: (ev: PhpInstallProgressEvent) => void
): Promise<() => void> {
	try {
		return await events.phpInstallProgressEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Whatever is currently installing or initializing, if anything — PHP or
 * MySQL alike (review fix wave, Important 1). Read by the quit dialog: a
 * build/init in progress is invisible to the services list (it is not a
 * supervised service), so without this a quit would silently discard it.
 * `null` only when nothing is running. Replaces the old PHP-only
 * `pendingPhpInstall`, which returned `null` for a MySQL occupant too,
 * leaving the quit dialog blind to it entirely.
 */
export async function pendingInstall(): Promise<PendingInstallDto | null> {
	return unwrap(commands.pendingInstall());
}

/**
 * Read-only MySQL environment summary for the Databases page: whether
 * Homebrew was found, where it looked, and one row per catalogue/installed
 * major with its on-disk datadir state. Spawns nothing — safe to call on page
 * mount and after every install/initialize.
 */
export async function mysqlEnvironment(): Promise<MysqlEnvironmentDto> {
	return unwrap(commands.mysqlEnvironment());
}

/**
 * Explicit, user-initiated re-probe behind the Databases page's rescan
 * affordance. Unlike {@link mysqlEnvironment}, this spawns a version probe per
 * candidate binary and sweeps abandoned staging directories left by a crashed
 * or force-quit init attempt.
 */
export async function rescanMysql(): Promise<MysqlEnvironmentDto> {
	return unwrap(commands.rescanMysql());
}

/**
 * Install a MySQL major from the pinned upstream tarball — download, SHA-256
 * verify, extract. **No Homebrew is involved**, so this works on a machine that
 * has never had brew installed.
 *
 * Streams typed pipeline states through {@link onMysqlInstallProgress} while it
 * runs, then resolves with an OUTCOME rather than throwing: a verification
 * failure, a stalled transfer, a cancel and an unavailable target are all
 * distinct, renderable results (see `MysqlInstallResultDto`). Only a rejected
 * major or a busy install lock throws.
 *
 * Shares the same install lock as {@link installPhp}: one of
 * `installPhp`/`installMysql`/`initializeMysql`/`uninstallPackage` at a time.
 */
export async function installMysql(major: string): Promise<MysqlInstallOutcomeDto> {
	return unwrap(commands.installMysql(major));
}

/**
 * Cancel an in-flight MySQL install, resolving to whether anything was actually
 * stopped (`false` when it had already finished, or when the shared lock is
 * held by a different run).
 *
 * Not a nicety. Nothing bounds the download by wall clock — only a 30-second
 * idle window — and the package pipeline's install permit is process-wide and
 * taken before staging, so an install nobody can stop starves every later one.
 * Cancelling drops the install future, which removes the staging directory and
 * releases that permit.
 */
export async function cancelMysqlInstall(): Promise<boolean> {
	return unwrap(commands.cancelMysqlInstall());
}

/** Subscribe to `mysql-install-log` — since the tarball slice this carries a
 *  MySQL **uninstall**'s brew output only; an install reports through
 *  {@link onMysqlInstallProgress}. Same `IpcError` contract as
 *  {@link onServiceState}. */
export async function onMysqlInstallLog(
	cb: (ev: MysqlInstallLogEvent) => void
): Promise<() => void> {
	try {
		return await events.mysqlInstallLogEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/** Subscribe to `mysql-install-progress` — one typed state per pipeline step
 *  (`started`/`downloaded`/`verified`/`extracted`/`linked`). Same `IpcError`
 *  contract as {@link onServiceState}. */
export async function onMysqlInstallProgress(
	cb: (ev: MysqlInstallProgressEvent) => void
): Promise<() => void> {
	try {
		return await events.mysqlInstallProgressEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Initialize a MySQL major's datadir: render + validate `my.cnf`, run the
 * staged init sequence with a generated root password, and — on success —
 * register the service so it appears in the Services panel. Streams progress
 * live through {@link onMysqlInitLog}. `alreadyInitialized`/`foreign` are
 * expected, non-error outcomes (a foreign datadir is reported, never
 * touched); `failed` names the step and reason. Shares the install lock —
 * see {@link installMysql}.
 */
export async function initializeMysql(major: string): Promise<MysqlInitOutcomeDto> {
	return unwrap(commands.initializeMysql(major));
}

/** Subscribe to `mysql-init-log`. Same `IpcError` contract as {@link onServiceState}. */
export async function onMysqlInitLog(cb: (ev: MysqlInitLogEvent) => void): Promise<() => void> {
	try {
		return await events.mysqlInitLogEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Reveal the stored root password for `major`, for the masked password
 * field's Reveal/Copy affordance. Throws if `major` has never been
 * initialized.
 */
export async function mysqlRootPassword(major: string): Promise<string> {
	return unwrap(commands.mysqlRootPassword(major));
}

/**
 * Regenerate `major`'s root password (reset-by-regenerate — there is no
 * user-chosen password this slice). `authFailed` is a distinct, renderable
 * outcome (never a thrown error): the stored password may be stale, e.g.
 * after restoring a datadir from an old backup outside the app.
 */
export async function resetMysqlRootPassword(major: string): Promise<MysqlResetOutcomeDto> {
	return unwrap(commands.resetMysqlRootPassword(major));
}

/**
 * `SELECT VERSION(), @@port` through the running server, authenticating with
 * the stored credential — the Databases page's "Verify connection"
 * affordance. Every failure mode (`authFailed`/`failed`) is a renderable
 * outcome, never a thrown error, so the button always has something to show.
 */
export async function verifyMysqlConnection(major: string): Promise<MysqlConnectionProofDto> {
	return unwrap(commands.verifyMysqlConnection(major));
}

/**
 * Read-only MariaDB environment summary for the Databases page — the
 * single-instance mirror of {@link mysqlEnvironment} (P1 MariaDB UI design
 * D6/D7): whether the one series this build ships is installed, and its
 * on-disk datadir state. Spawns nothing — safe to call on page mount and
 * after every install/initialize.
 */
export async function mariadbEnvironment(): Promise<MariadbEnvironmentDto> {
	return unwrap(commands.mariadbEnvironment());
}

/**
 * Explicit, user-initiated re-probe behind the MariaDB group's rescan
 * affordance — the mirror of {@link rescanMysql}.
 */
export async function rescanMariadb(): Promise<MariadbEnvironmentDto> {
	return unwrap(commands.rescanMariadb());
}

/**
 * Install the pinned MariaDB series from OpenVHost's own GitHub release —
 * download, SHA-256 verify, extract. **No Homebrew is, or ever was,
 * involved.** Takes no major: unlike {@link installMysql}, there is no
 * series to name — this build ships exactly one (design D7).
 *
 * Streams typed pipeline states through {@link onMariadbInstallProgress}
 * while it runs, then resolves with an OUTCOME rather than throwing — same
 * discipline as {@link installMysql}. Shares the same install lock as every
 * other package operation: one of `installPhp`/`installMysql`/
 * `initializeMysql`/`installMariadb`/`initializeMariadb`/`uninstallPackage`
 * at a time.
 */
export async function installMariadb(): Promise<MariadbInstallOutcomeDto> {
	return unwrap(commands.installMariadb());
}

/**
 * Cancel an in-flight MariaDB install — the mirror of
 * {@link cancelMysqlInstall}, and the identical reasoning: the download has
 * no wall-clock bound and the install permit is process-wide, so an install
 * nobody can stop starves every later one.
 */
export async function cancelMariadbInstall(): Promise<boolean> {
	return unwrap(commands.cancelMariadbInstall());
}

/** Subscribe to `mariadb-install-log` — same dual-purpose channel design D3
 *  gives MySQL's own `mysql-install-log`: MariaDB's install progress reports
 *  through {@link onMariadbInstallProgress}, so this carries an uninstall's
 *  output only. Same `IpcError` contract as {@link onServiceState}. */
export async function onMariadbInstallLog(
	cb: (ev: MariadbInstallLogEvent) => void
): Promise<() => void> {
	try {
		return await events.mariadbInstallLogEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/** Subscribe to `mariadb-install-progress` — one typed state per pipeline
 *  step, the mirror of {@link onMysqlInstallProgress}. Same `IpcError`
 *  contract as {@link onServiceState}. */
export async function onMariadbInstallProgress(
	cb: (ev: MariadbInstallProgressEvent) => void
): Promise<() => void> {
	try {
		return await events.mariadbInstallProgressEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Initialize MariaDB's datadir: the staged init sequence with a generated
 * root password, and — on success — register the service so it appears in
 * the Services panel. **No live per-line log**, unlike
 * {@link initializeMysql}: the core function reports nothing intermediate,
 * only a single terminal outcome, so there is nothing to stream while a run
 * is in progress. A `failed` outcome's `reason` already carries the
 * diagnostic in full, relayed post-hoc through {@link onMariadbInitLog} once
 * the run ends. Shares the install lock — see {@link installMariadb}.
 */
export async function initializeMariadb(): Promise<MariadbInitOutcomeDto> {
	return unwrap(commands.initializeMariadb());
}

/** Subscribe to `mariadb-init-log`. POST-HOC ONLY (see
 *  {@link initializeMariadb}) — this fires once, after a failed run ends,
 *  never while `initializeMariadb`'s promise is still pending. Same
 *  `IpcError` contract as {@link onServiceState}. */
export async function onMariadbInitLog(cb: (ev: MariadbInitLogEvent) => void): Promise<() => void> {
	try {
		return await events.mariadbInitLogEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}

/**
 * Reveal the stored root password for the MariaDB instance, for the masked
 * password field's Reveal/Copy affordance — the mirror of
 * {@link mysqlRootPassword}, minus the major: MariaDB ships exactly one
 * series. Throws if it has never been initialized.
 */
export async function mariadbRootPassword(): Promise<string> {
	return unwrap(commands.mariadbRootPassword());
}

/**
 * Regenerate the MariaDB instance's root password (reset-by-regenerate) —
 * the mirror of {@link resetMysqlRootPassword}. `authFailed` is a distinct,
 * renderable outcome (never a thrown error): the stored password may be
 * stale, e.g. after restoring a datadir from an old backup outside the app.
 */
export async function resetMariadbRootPassword(): Promise<MariadbResetOutcomeDto> {
	return unwrap(commands.resetMariadbRootPassword());
}

/**
 * `SELECT VERSION(), @@port` through the running server, authenticating with
 * the stored credential — the MariaDB group's "Verify connection"
 * affordance, mirroring {@link verifyMysqlConnection}. Every failure mode is
 * a renderable outcome, never a thrown error, so the button always has
 * something to show.
 */
export async function verifyMariadbConnection(): Promise<MariadbConnectionProofDto> {
	return unwrap(commands.verifyMariadbConnection());
}

/**
 * The full log-source catalogue: nginx's two globals, one row per installed
 * PHP major, an access/error pair per site, and one `"ring"` row per
 * supervised service. `"ring"` rows are read through {@link serviceLogTail}/
 * {@link onServiceLog}, never through {@link readLogWindow} — two
 * mechanisms, deliberately (spec D7).
 */
export async function listLogSources(): Promise<LogSourceRowDto[]> {
	return unwrap(commands.listLogSources());
}

/**
 * A bounded, filtered window of one `"file"` log source. `query.cursor` is
 * opaque — pass back exactly what a previous call returned in
 * `LogWindowDto.cursor` to resume, or `null` for a fresh tail. Never call
 * this with a `"ring"` source (see {@link listLogSources}); it is rejected
 * server-side.
 */
export async function readLogWindow(query: LogWindowQuery): Promise<LogWindowDto> {
	return unwrap(commands.readLogWindow(query));
}

/**
 * Open the folder containing a log source's file(s) in the OS file manager
 * — the user's one recourse against unbounded on-disk growth this slice
 * ships without rotation. The path is derived entirely server-side; this
 * only ever names a source.
 */
export async function revealLogFolder(source: LogSourceDto): Promise<void> {
	await unwrap(commands.revealLogFolder(source));
}
