// SPDX-License-Identifier: GPL-3.0-or-later
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
	// Pinned so this app never squats Vite's default 5173, which any other
	// Vite/SvelteKit project on this machine will also try to take.
	//
	// `strictPort` is the load-bearing half. Without it Vite silently falls forward
	// to the next free port when its own is busy — but `tauri.conf.json`'s `devUrl`
	// is a HARDCODED string, so it keeps loading the port it was told about. The
	// result is a Tauri window that either renders blank or, worse, attaches to a
	// DIFFERENT project's dev server that happens to hold that port. Failing to
	// start with "Port 5183 is already in use" is far better than either.
	//
	// Keep this in sync with `src-tauri/tauri.conf.json`'s `devUrl`. The two are not
	// derived from each other, so changing one alone reintroduces exactly the
	// mismatch described above.
	server: {
		port: 5183,
		strictPort: true
	},
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},

			// Single prerendered route → plain static output in build/ for Tauri.
			// (SvelteKit >= 2.62 reads plugin config passed here directly and ignores
			// svelte.config.js when it does — see @sveltejs/kit/src/exports/vite/index.js.)
			adapter: adapter()
		})
	],
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					// No separate browser/jsdom project exists yet, so `*.svelte.test.ts`
					// is NOT excluded here (unlike the sveltekit default template) — a
					// rune-based `.svelte.ts` store (e.g. `sites.svelte.ts`) has no DOM
					// dependency and runs fine under `node`, and so does rendering a
					// `.svelte` component through `svelte/server` (see
					// `SiteDrawer.svelte.test.ts`), whose markup carries the `selected`/
					// `value` attributes a browser would apply. A test that needs a live
					// DOM — user events, focus, measurement — would fail loudly here (no
					// `document`); that is the right time to add a browser/jsdom project.
					include: ['src/**/*.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
