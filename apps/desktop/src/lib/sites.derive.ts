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
