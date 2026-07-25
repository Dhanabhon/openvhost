// SPDX-License-Identifier: GPL-3.0-or-later
// SPA shell for Tauri: no SSR. Every route under this layout prerenders to a
// static file in build/ — `/` (Sites, the landing page) to index.html and
// `/services` to services.html — and navigation between them is client-side.
export const ssr = false;
export const prerender = true;
