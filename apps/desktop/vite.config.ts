// SPDX-License-Identifier: GPL-3.0-or-later
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
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
					// dependency and runs fine under `node`. A future test that renders an
					// actual `.svelte` component would fail loudly here (no `document`),
					// which is the right time to add a dedicated browser/jsdom project.
					include: ['src/**/*.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
