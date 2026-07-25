// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE ServicesStore, wired to the real IPC layer.
//
// Why a shared instance instead of one per page: the titlebar shows "N running"
// on every route, but only the Services page used to subscribe to the
// supervisor — so the Sites page passed a hardcoded `runningCount={0}` and the
// titlebar lied at launch. `routes/+layout.svelte` snapshots and subscribes once
// here, and every page reads this same live state.
//
// Kept out of `services.svelte.ts` on purpose: that module stays a pure,
// api-injected store class (its unit tests hand it a fake `ServicesApi`), and
// this is the one place that binds it to the real Tauri commands.
import { listServices, serviceLogTail, startService, stopService } from './ipc';
import { ServicesStore } from './services.svelte';

export const servicesStore = new ServicesStore({
	listServices,
	serviceLogTail,
	startService,
	stopService
});
