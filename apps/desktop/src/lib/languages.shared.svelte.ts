// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE LanguagesStore, wired to the real IPC layer.
//
// Mirrors `services.shared.svelte.ts`: a shared module singleton rather than
// one constructed locally inside `routes/languages/+page.svelte`, so this
// store's state can be seeded directly by a route-level SSR test the same way
// `servicesStore.services = [...]` is seeded in `routes/routes.test.ts` —
// `onMount` never runs under `svelte/server`, so a store built ONLY inside the
// page's own `<script>` can never be put into an error/populated state before
// that render. See branch-review-fix-report.md (C2/C3): the reviewer's point
// that "a single SSR test per route, rendering the store in each terminal
// state, would have caught three of the five findings" only works if the
// route has a store to seed from outside — this file is what makes that
// possible for the Languages page.
//
// Kept out of `languages.svelte.ts` on purpose, same reasoning as
// `services.shared.svelte.ts`: that module stays a pure, api-injected store
// class (its unit tests hand it a fake `LanguagesApi`), and this is the one
// place that binds it to the real Tauri commands.
import { installPhp, phpEnvironment, rescanPhpRuntimes } from './ipc';
import { LanguagesStore } from './languages.svelte';

export const languagesStore = new LanguagesStore({
	phpEnvironment,
	rescanPhpRuntimes,
	installPhp
});
