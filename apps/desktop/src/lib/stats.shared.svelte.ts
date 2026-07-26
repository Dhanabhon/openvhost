// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE StatsStore, wired to the real IPC layer.
//
// Shared for the same reason `services.shared.svelte.ts` is: the status bar is
// rendered by AppShell on every route, so a per-page instance would restart the
// timers on every navigation and throw away the home figure each time.
//
// Kept out of `stats.svelte.ts` so that module stays a pure, api-injected store
// whose tests hand it a fake.
import { homeDiskUsage, servicesMemory } from './ipc';
import { StatsStore } from './stats.svelte';

export const statsStore = new StatsStore({ servicesMemory, homeDiskUsage });
