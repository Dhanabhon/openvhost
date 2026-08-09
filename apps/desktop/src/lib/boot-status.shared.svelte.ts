// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE BootStatusStore, wired to the real IPC layer.
//
// Shared for the same reasons `store-status.shared.svelte.ts` is: the answer is
// fixed for the life of the process (`BootState` is managed once, in `setup`,
// and never replaced), so `routes/+layout.svelte` asks exactly once and the
// whole app reads the same value. A per-page instance would re-ask on every
// navigation — and on a degraded boot there are no pages to navigate between,
// which is exactly why the reader is the layout and not a page.
//
// Kept out of `boot-status.svelte.ts` on purpose, same reasoning as
// `store-status.shared.svelte.ts`: that module stays a pure, api-injected store
// class, and this is the one place that binds it to the real Tauri command.
import { bootStatus } from './ipc';
import { BootStatusStore } from './boot-status.svelte';

export const bootStatusStore = new BootStatusStore({ bootStatus });
