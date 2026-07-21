// SPDX-License-Identifier: GPL-3.0-or-later
// The ONLY module allowed to touch Tauri IPC (master plan §5).
import { commands } from './bindings';
import type { CoreInfo, IpcError } from './bindings';

export type { CoreInfo, IpcError };

/** Fetch CoreInfo from the Rust core. Throws IpcError on failure. */
export async function coreInfo(simulateError = false): Promise<CoreInfo> {
	const result = await commands.coreInfo(simulateError ? true : null);
	if (result.status === 'error') throw result.error;
	return result.data;
}
