// SPDX-License-Identifier: GPL-3.0-or-later
// SPA shell for Tauri: no SSR. Every route under this layout prerenders to a
// static file in build/ — `/` (Sites, the landing page) to index.html,
// `/services` to services.html, `/web-server` to web-server.html — and
// navigation between them is client-side.
//
// Because `ssr = false`, each of those files is a ~2 KB shell with no component
// markup in it. Their presence proves the route was picked up by prerendering,
// NOT that anything renders: the panels are asserted separately by the vitest
// suites, which call `render()` from `svelte/server` on components directly and
// so bypass this setting. Do not treat "the HTML file exists" as page coverage.
export const ssr = false;
export const prerender = true;
