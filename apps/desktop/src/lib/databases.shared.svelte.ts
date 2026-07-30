// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE DatabasesStore, wired to the real IPC layer.
//
// Mirrors `languages.shared.svelte.ts`: a shared module singleton rather than
// one constructed locally inside `routes/databases/+page.svelte`, so this
// store's state can be seeded directly by a route-level SSR test — `onMount`
// never runs under `svelte/server`, so a store built ONLY inside the page's
// own `<script>` can never be put into an error/populated state before that
// render.
//
// Kept out of `databases.svelte.ts` on purpose, same reasoning as
// `languages.shared.svelte.ts`: that module stays a pure, api-injected store
// class (its unit tests hand it a fake `DatabasesApi`), and this is the one
// place that binds it to the real Tauri commands.
import {
	initializeMysql,
	installMysql,
	mysqlEnvironment,
	mysqlRootPassword,
	rescanMysql,
	resetMysqlRootPassword,
	verifyMysqlConnection
} from './ipc';
import { DatabasesStore } from './databases.svelte';

export const databasesStore = new DatabasesStore({
	mysqlEnvironment,
	rescanMysql,
	installMysql,
	initializeMysql,
	mysqlRootPassword,
	resetMysqlRootPassword,
	verifyMysqlConnection
});
