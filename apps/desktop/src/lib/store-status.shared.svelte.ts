// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE StoreStatusStore, wired to the real IPC layer.
//
// Shared for the same reason `services.shared.svelte.ts` is, and for one more:
// the answer is fixed for the life of the process (the handle is managed once,
// at startup, and never reopened), so `routes/+layout.svelte` asks exactly once
// and every route reads the same value. A per-page instance would re-ask on
// every navigation and flash the banner in again each time.
//
// Kept out of `store-status.svelte.ts` on purpose, same reasoning as
// `services.shared.svelte.ts`: that module stays a pure, api-injected store
// class, and this is the one place that binds it to the real Tauri command.
import { stateStoreStatus } from './ipc';
import { StoreStatusStore } from './store-status.svelte';

export const storeStatusStore = new StoreStatusStore({ stateStoreStatus });
