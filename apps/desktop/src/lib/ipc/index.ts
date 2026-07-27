// SPDX-License-Identifier: GPL-3.0-or-later
// The ONLY module allowed to touch Tauri IPC (master plan §5).
import { commands, events } from './bindings';
import type {
	ApplyOutcomeDto,
	ApplyPlanDto,
	CoreInfo,
	FileChangeDto,
	HomeUsageDto,
	InstallOutcomeDto,
	IpcError,
	LogLine,
	PhpEnvironmentDto,
	PhpInstallLogEvent,
	PhpRuntimeDto,
	ServiceLogEvent,
	ServiceProblemDto,
	ServicesMemoryDto,
	ServiceStateEvent,
	ServiceStatus,
	SiteDto,
	SiteInput,
	ValidationReportDto,
	WebServerDto
} from './bindings';

export type {
	ApplyOutcomeDto,
	ApplyPlanDto,
	CoreInfo,
	FileChangeDto,
	HomeUsageDto,
	InstallOutcomeDto,
	IpcError,
	LogLine,
	PhpEnvironmentDto,
	PhpInstallLogEvent,
	PhpRuntimeDto,
	ServiceLogEvent,
	ServiceProblemDto,
	ServicesMemoryDto,
	ServiceStateEvent,
	ServiceStatus,
	SiteDto,
	SiteInput,
	ValidationReportDto,
	WebServerDto
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

export async function listSites(): Promise<SiteDto[]> {
	return unwrap(commands.listSites());
}
export async function createSite(input: SiteInput): Promise<SiteDto> {
	return unwrap(commands.createSite(input));
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
 * What Apply would change against the current sites. Read-only and
 * process-free — safe to call after every site mutation for a pending-changes
 * banner.
 */
export async function planSiteApply(): Promise<ApplyPlanDto> {
	return unwrap(commands.planSiteApply());
}

/**
 * Apply the sites, then restart whichever affected services were running.
 * Services that were not running are reported in `notStarted` instead of
 * being started as a side effect. A service that was running but could not be
 * cleanly stopped-and-restarted lands in `needsAttention` instead of
 * `restarted` — the UI must not present that outcome as a success.
 */
export async function applySites(): Promise<ApplyOutcomeDto> {
	return unwrap(commands.applySites());
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
 * Install a PHP major via Homebrew. Streams its output live through
 * {@link onPhpInstallLog} while it runs, then resolves with the outcome —
 * including `detected`, which can be `false` even when `exitCode` is `0`.
 */
export async function installPhp(major: string): Promise<InstallOutcomeDto> {
	return unwrap(commands.installPhp(major));
}

/** Subscribe to `php-install-log`. Same `IpcError` contract as {@link onServiceState}. */
export async function onPhpInstallLog(cb: (ev: PhpInstallLogEvent) => void): Promise<() => void> {
	try {
		return await events.phpInstallLogEvent.listen((e) => cb(e.payload));
	} catch (e) {
		throw normalizeError(e);
	}
}
