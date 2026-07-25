// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers for the Sites UI. The `.localhost` suffix is fixed: such
// domains resolve without touching the hosts file, which is why this slice
// needs no privileged helper. Custom TLDs are a later slice.

const LOCALHOST_SUFFIX = '.localhost';

/** Compose the stored domain from the subdomain the user types. */
export function composeDomain(subdomain: string): string {
	return `${subdomain}${LOCALHOST_SUFFIX}`;
}

/**
 * Strip exactly one trailing `.localhost` for editing. A stored domain without
 * that suffix is only reachable by hand-editing `state.db`; it is shown as-is
 * rather than adding a second domain-entry mode.
 */
export function splitDomain(domain: string): string {
	return domain.endsWith(LOCALHOST_SUFFIX) ? domain.slice(0, -LOCALHOST_SUFFIX.length) : domain;
}

/** Row pill for a site's stored `enabled` flag (reuses the shared pill classes). */
export function enabledPill(enabled: boolean): { label: string; cls: string } {
	return enabled
		? { label: 'enabled', cls: 'pill-running' }
		: { label: 'disabled', cls: 'pill-stopped' };
}

/**
 * Selectable PHP versions. Fixed for this slice — annotating which are
 * installed needs the package IPC (its own slice).
 */
export const PHP_VERSIONS = ['8.4', '8.3', '8.2', '8.1'] as const;

/**
 * Web servers a site can run under.
 *
 * The IPC boundary types `webServer` as a bare `string` (specta exports the Rust
 * `WebServer` enum's wire form, not a TS union), so this is the frontend's own
 * narrowing of that. Kept in lockstep with `WebServer::parse` in
 * `crates/openvhost-core/src/site/model.rs`, which is the authority — the server
 * rejects anything else regardless of what the UI offers.
 */
export const WEB_SERVERS = ['nginx', 'apache'] as const;
export type WebServerKind = (typeof WEB_SERVERS)[number];

/**
 * Options for the site editor's PHP-version `<select>`, always including the
 * site's stored version.
 *
 * `PHP_VERSIONS` above is a CLOSED list, but state.db can hold any version an
 * older build — or a later edit of that list — allowed. A `<select>` with no
 * `<option>` matching its bound value renders blank and the binding silently
 * takes the browser's own pick instead, so saving would rewrite the site's PHP
 * version to something the user never chose. Prepending the stored value keeps
 * it visible, selected, and reversible until the user deliberately changes it.
 *
 * The annotation follows docs/design/site-editor.html's `8.1 — install first`
 * convention, but says "not available" rather than "install first": all this
 * knows is that OpenVHost does not offer the version, which is not the same
 * claim as it being absent from disk.
 */
export function phpVersionOptions(current?: string | null): { value: string; label: string }[] {
	const offered = PHP_VERSIONS.map((v) => ({ value: v, label: v }));
	if (!current || (PHP_VERSIONS as readonly string[]).includes(current)) return offered;
	return [{ value: current, label: `${current} — not available` }, ...offered];
}
