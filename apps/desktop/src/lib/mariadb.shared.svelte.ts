// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE MariadbStore, wired to the real IPC layer.
//
// Mirrors `databases.shared.svelte.ts`: a shared module singleton rather than
// one constructed locally inside `routes/databases/+page.svelte`, so this
// store's state can be seeded directly by a route-level SSR test — `onMount`
// never runs under `svelte/server`, so a store built ONLY inside the page's
// own `<script>` can never be put into an error/populated state before that
// render.
//
// Kept out of `mariadb.svelte.ts` on purpose, same reasoning as
// `databases.shared.svelte.ts`: that module stays a pure, api-injected store
// class (its unit tests hand it a fake `MariadbApi`), and this is the one
// place that binds it to the real Tauri commands.
import {
	cancelMariadbInstall,
	initializeMariadb,
	installMariadb,
	mariadbEnvironment,
	mariadbRootPassword,
	rescanMariadb,
	resetMariadbRootPassword,
	verifyMariadbConnection
} from './ipc';
import { MariadbStore } from './mariadb.svelte';

export const mariadbStore = new MariadbStore({
	mariadbEnvironment,
	rescanMariadb,
	installMariadb,
	cancelMariadbInstall,
	initializeMariadb,
	mariadbRootPassword,
	resetMariadbRootPassword,
	verifyMariadbConnection
});
