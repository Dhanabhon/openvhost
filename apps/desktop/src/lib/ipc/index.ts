// SPDX-License-Identifier: GPL-3.0-or-later
// The ONLY module allowed to touch Tauri IPC (master plan §5).
import { commands, events } from './bindings';
import type { CoreInfo, IpcError, LogLine, ServiceLogEvent, ServiceStateEvent, ServiceStatus } from './bindings';

export type { CoreInfo, IpcError, LogLine, ServiceLogEvent, ServiceStateEvent, ServiceStatus };

function isIpcError(e: unknown): e is IpcError {
	return typeof e === 'object' && e !== null && typeof (e as { kind?: unknown }).kind === 'string';
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
		if (isIpcError(e)) throw e;
		throw { kind: 'core', message: String(e) } satisfies IpcError;
	}
}

function unwrap<T>(r: { status: 'ok'; data: T } | { status: 'error'; error: unknown }): T {
	if (r.status === 'error') throw r.error;
	return r.data;
}

export async function listServices(): Promise<ServiceStatus[]> {
	return unwrap(await commands.listServices());
}
export async function startService(id: string): Promise<void> {
	unwrap(await commands.startService(id));
}
export async function stopService(id: string): Promise<void> {
	unwrap(await commands.stopService(id));
}
export async function serviceLogTail(id: string, n: number): Promise<LogLine[]> {
	return unwrap(await commands.serviceLogTail(id, n));
}
export function onServiceState(cb: (ev: ServiceStateEvent) => void): Promise<() => void> {
	return events.serviceStateEvent.listen((e) => cb(e.payload));
}
export function onServiceLog(cb: (ev: ServiceLogEvent) => void): Promise<() => void> {
	return events.serviceLogEvent.listen((e) => cb(e.payload));
}
