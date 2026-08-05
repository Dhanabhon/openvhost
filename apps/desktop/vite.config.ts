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
					// A jsdom project now exists below for anything that needs a live DOM —
					// user events, focus, measurement (the Sites row overflow menu is the
					// feature that triggered adding it; see
					// `docs/superpowers/specs/2026-08-05-sites-row-overflow-menu-design.md`
					// D4). `*.dom.test.ts` is that project's opt-in file suffix and is
					// EXCLUDED here so no file runs under both projects.
					//
					// `*.svelte.test.ts` (without the `.dom` infix) still is NOT excluded —
					// a rune-based `.svelte.ts` store (e.g. `sites.svelte.ts`) has no DOM
					// dependency and runs fine under `node`, and so does rendering a
					// `.svelte` component through `svelte/server` (see
					// `SiteDrawer.svelte.test.ts`), whose markup carries the `selected`/
					// `value` attributes a browser would apply.
					include: ['src/**/*.{test,spec}.{js,ts}'],
					exclude: ['src/**/*.dom.test.{js,ts}']
				}
			},
			{
				extends: './vite.config.ts',
				test: {
					name: 'dom',
					environment: 'jsdom',
					// Opt-in by filename suffix, not by directory — a DOM test sits right next
					// to the component it exercises, same as every `server`-project test
					// already does. `*.dom.test.ts` is excluded from `server` above for exactly
					// this pattern, so a file never runs under both projects.
					include: ['src/**/*.dom.test.{js,ts}']
				},
				resolve: {
					// Vitest transforms modules through Vite's SSR-style resolution
					// regardless of `environment`, so without this override `import { mount,
					// unmount } from 'svelte'` would still resolve svelte's "default" export
					// condition (`src/index-server.js`, whose `mount`/`unmount` are stubs
					// that throw `lifecycle_function_unavailable`) instead of the "browser"
					// condition (`src/index-client.js`) that actually mounts a component into
					// a real DOM — confirmed against the installed svelte package's own
					// `exports` map. It also steers `@sveltejs/vite-plugin-svelte` (which
					// compiles a `.svelte` file to `generate: 'server'` markup-only output or
					// `generate: 'client'` DOM-mounting output based on the same SSR signal)
					// toward the client build, which is the one this project's tests need.
					conditions: ['browser']
				}
			}
		]
	}
});
