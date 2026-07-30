// SPDX-License-Identifier: GPL-3.0-or-later
// Copy-to-clipboard, for the Databases page's connection block and root
// password (spec D6). Deliberately the plain Web Clipboard API rather than a
// new Tauri command/plugin: writing a string the renderer already holds has
// no business logic for a Rust command to own, and Tauri's WebView (WKWebView
// on macOS) supports `navigator.clipboard.writeText` directly from a
// user-gesture handler with no extra capability grant.
//
// UNTESTABLE HERE: this project's vitest tests run under `svelte/server`
// (see vite.config.ts), which has no `navigator` global and never invokes a
// click handler's body in the first place (SSR renders markup only). Manual
// click-list item.
export async function copyToClipboard(text: string): Promise<void> {
	await navigator.clipboard.writeText(text);
}
