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
 * `installed` is THIS machine's actually-installed PHP majors — `phpEnvironment()`'s
 * runtimes filtered to `installed` — not a hardcoded list. A closed list unrelated to
 * the machine was the exact trap this replaces: on a machine with only 8.5 installed,
 * a dropdown of `8.4`/`8.3`/`8.2`/`8.1` let every option lead to an Apply the backend
 * refused (no runtime at that version), and nothing on screen said why.
 *
 * Even so, state.db can hold a version `installed` does not contain — an older build's
 * choice, or a runtime since uninstalled. A `<select>`-like control with no option
 * matching its bound value renders blank and the binding silently takes whatever the
 * browser (or, here, `Select.svelte`) picks instead, so saving would rewrite the site's
 * PHP version to something the user never chose. Prepending the stored value keeps it
 * visible, selected, and reversible until the user deliberately changes it.
 *
 * The annotation follows docs/design/site-editor.html's `8.1 — install first`
 * convention, but says "not available" rather than "install first": all this knows is
 * that it is not installed, which is not the same claim as "install it and this would
 * work" (the version might not exist, or might be unreachable for other reasons).
 */
export function phpVersionOptions(
	current: string | undefined | null,
	installed: readonly string[]
): { value: string; label: string }[] {
	const offered = installed.map((v) => ({ value: v, label: v }));
	if (!current || installed.includes(current)) return offered;
	return [{ value: current, label: `${current} — not available` }, ...offered];
}

/**
 * The PHP version a brand-new site should start with: the newest installed major, or
 * `undefined` when nothing is installed at all.
 *
 * A new site used to default to a hardcoded `PHP_VERSIONS[0]` — a version that may not
 * exist on this machine, so the site was born broken before the user touched anything
 * (the second of the three mistakes behind this task). `undefined` here is a real,
 * checked case: `SiteDrawer.svelte` uses it to refuse to offer a doomed-to-fail Add
 * form instead of silently falling back to an empty string.
 *
 * Versions are `major.minor` strings, and comparing them as strings breaks the day a
 * two-digit component ships: `"8.9" > "8.10"` lexically, which would pick the OLDER
 * release as "newest". Both components are compared numerically instead.
 */
export function defaultPhpVersion(installed: readonly string[]): string | undefined {
	if (installed.length === 0) return undefined;
	return installed.reduce((newest, candidate) =>
		compareVersions(candidate, newest) > 0 ? candidate : newest
	);
}

/** Numeric `major.minor` comparison — see `defaultPhpVersion` for why a lexical
 *  string comparison is not safe here. */
function compareVersions(a: string, b: string): number {
	const [aMajor, aMinor] = a.split('.').map(Number);
	const [bMajor, bMinor] = b.split('.').map(Number);
	return aMajor !== bMajor ? aMajor - bMajor : aMinor - bMinor;
}
