// SPDX-License-Identifier: GPL-3.0-or-later
// The ONLY module allowed to touch Tauri IPC (master plan §5).
import { commands, events } from './bindings';
import type {
	CoreInfo,
	IpcError,
	LogLine,
	ServiceLogEvent,
	ServiceStateEvent,
	ServiceStatus,
	SiteDto,
	SiteInput,
	ValidationReportDto,
	WebServerDto
} from './bindings';

export type {
	CoreInfo,
	IpcError,
	LogLine,
	ServiceLogEvent,
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

export async function listWebServers(): Promise<WebServerDto[]> {
	return unwrap(commands.listWebServers());
}
export async function readWebServerConfig(id: string): Promise<string> {
	return unwrap(commands.readWebServerConfig(id));
}
export async function validateWebServerConfig(id: string): Promise<ValidationReportDto> {
	return unwrap(commands.validateWebServerConfig(id));
}
