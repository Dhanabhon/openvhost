// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE UninstallStore, wired to the real IPC layer.
//
// Shared rather than one per page, for a reason that is not tidiness: `brew
// install`, `brew uninstall` and the MySQL staged init all serialize behind a
// single `InstallLock` (package-uninstall design D1). A PHP uninstall genuinely
// blocks a MySQL one, so both pages must read the SAME "something is being
// uninstalled" state — two stores would leave the other page offering a button
// that could only sit on a mutex.
//
// Also what lets a route-level SSR test seed the dialog directly (`onMount`
// never runs under `svelte/server`), the same trick `languages.shared.svelte.ts`
// exists for.
//
// THE CONTRACT SEAM: the assignment below is where this frontend's idea of an
// uninstall plan meets the generated Tauri bindings. `UninstallApi` is written
// in terms of `uninstall.derive.ts`'s own `PackageKind`/`UninstallPlan`, so if
// the Rust DTO ever stops matching what the confirmation renders, THIS line
// fails to compile — rather than the dialog rendering `undefined` where a kept
// datadir path should be.
import { uninstallPackage, uninstallPlan } from './ipc';
import { UninstallStore, type UninstallApi } from './uninstall.svelte';

const api: UninstallApi = { uninstallPlan, uninstallPackage };

export const uninstallStore = new UninstallStore(api);
