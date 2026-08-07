// SPDX-License-Identifier: GPL-3.0-or-later
import type { NginxRuntimeSourceDto, ServiceStatus, SiteDto } from '$lib/ipc';

/**
 * The supervised state-kind a row shows, or `null` when the row has no
 * supervised service (Apache) or the snapshot has not arrived yet. Never falls
 * back to another row's state — a row showing a neighbour's status would be a
 * lie about what is running.
 *
 * Returns the KIND rather than the whole state object for two reasons:
 * `ServiceState` is not exported from `$lib/ipc`, and `StatusPill` takes
 * `kind: StateKind`. Indexing off the exported `ServiceStatus` keeps this in
 * step with the binding without widening the barrel.
 */
export function statusFor(
	// `readonly` so a component can keep its own `services` prop readonly (the shape
	// ServicesPanel.svelte already uses) and still call this — the lookup never
	// mutates. Accepts a mutable array too, so no caller changes.
	services: readonly ServiceStatus[],
	serviceId: string | null
): ServiceStatus['state']['kind'] | null {
	if (serviceId === null) return null;
	return services.find((s) => s.id === serviceId)?.state.kind ?? null;
}

export function hotReloadLabel(supportsHotReload: boolean): string {
	return supportsHotReload ? 'Supported' : 'Not supported';
}

/**
 * The small provenance badge beside nginx's name on the Web Server page —
 * which install put that binary there (nginx source design D1).
 *
 * Mirrors `mysqlSourceBadge` (`mysql-install.derive.ts`) exactly rather than
 * inventing a second visual language for the same idea one page over (design
 * D3): source is provenance, not health, and must not be styled as a second
 * status — a green "Packaged" badge next to a red "Failed" pill would read
 * as a contradiction rather than two facts. `null` covers BOTH "no nginx was
 * found" and the Apache row, which has no runtime at all — `server.supported`
 * already tells those two apart for any caller that cares, so this function
 * adds no second discriminator.
 *
 * A Homebrew badge shows **no version at all**. nginx has no `--version`
 * flag, only `-v`, and finding out means executing the binary — the exact
 * cost design D2 exists to remove from the packaged path, where the version
 * is read off the tree instead. The row's own "Version" fact already reports
 * whatever WAS learned; this badge only ever adds provenance, never a guess.
 */
export function nginxSourceBadge(
	source: NginxRuntimeSourceDto | null
): { label: string; title: string } | null {
	if (source === null) return null;
	switch (source.kind) {
		case 'packaged':
			return {
				label: `OpenVHost ${source.version}`,
				title:
					`Installed by OpenVHost from its own nginx build, checksum-verified. Exact version ` +
					`${source.version}, read from the package tree — never probed.`
			};
		case 'homebrew':
			return {
				label: 'Homebrew',
				title:
					'Installed by Homebrew, not by OpenVHost. nginx has no --version flag, so its exact ' +
					"patch release is only known by asking the binary itself, which this badge won't guess."
			};
		default: {
			const unreachable: never = source;
			return unreachable;
		}
	}
}

/** The reason a Start button is disabled. Spec §4 fixes this string; the form
 *  and its test both read it from here so they cannot drift apart. It names the
 *  next step rather than only the problem — "no config" alone leaves the user
 *  to guess that Apply is what produces one. */
export const NO_CONFIG_REASON = 'No config generated yet — apply your changes first.';

/** What the row's service control should be right now.
 *
 *  A discriminated union rather than a pile of booleans on the component: the
 *  choice is a decision, it is testable as a table here, and the component is
 *  left with nothing to decide. */
export type StartStopControl =
	| { kind: 'none' }
	| { kind: 'start'; disabled: boolean; reason: string }
	| { kind: 'retry' }
	| { kind: 'stop' };

/**
 * `statusKind === null` means the supervisor snapshot has NOT ARRIVED, which is
 * not the same as "stopped" — see the test. It renders nothing, the same rule
 * the status pill already follows (`{#if statusKind}` in WebServerRow.svelte).
 *
 * `configExists` only ever gates `start`. A `failed` service has already been
 * started once, and a `running` one is a live process; neither decision has
 * anything to do with a file being on disk right now.
 *
 * `configExists` is a TRI-STATE (`boolean | null`), matching the backend's
 * `config_exists`: a filesystem stat has three honest outcomes, and
 * `true`/`false` alone cannot carry "could not tell". `null` means the stat
 * itself failed — a permission error on a parent directory, a dangling
 * symlink from an interrupted atomic write, and so on — which is NOT the same
 * fact as "confirmed absent", and must not be treated as one.
 */
export function startStopFor(
	statusKind: ServiceStatus['state']['kind'] | null,
	configExists: boolean | null
): StartStopControl {
	if (statusKind === null) return { kind: 'none' };
	if (statusKind === 'failed') return { kind: 'retry' };
	if (statusKind === 'stopped') {
		if (configExists === false) {
			return { kind: 'start', disabled: true, reason: NO_CONFIG_REASON };
		}
		// `configExists === true` is the ordinary enabled case. `configExists ===
		// null` — existence UNKNOWN — is deliberately handled the SAME way:
		// Start enabled, with no reason shown. We could not determine whether the
		// config is there, so the honest move is not to guess a confident wrong
		// diagnosis ("no config generated yet — apply your changes first") when
		// the real cause might be a permission error that Apply cannot fix. If
		// the user presses Start and the config genuinely is missing (or
		// otherwise bad), nginx itself refuses to start and the service goes to
		// `failed` — Task 4 of this plan renders that failure's stderr tail on
		// the row, so the user ends up reading nginx's own words naming the
		// actual problem. That is strictly more useful, and strictly more
		// honest, than a confident sentence this function cannot back up.
		return { kind: 'start', disabled: false, reason: '' };
	}
	return { kind: 'stop' };
}

/**
 * The PHP majors an enabled site needs whose php-fpm pool is not running.
 *
 * A PHP site needs nginx AND a pool. Starting nginx alone leaves the site
 * answering 502 with nothing on screen connecting the two — the page names the
 * gap instead of letting the user find it in a browser.
 *
 * Only while nginx is RUNNING: with nginx stopped the user has not asked to
 * serve anything, and a pool warning would be noise about a problem they do not
 * have. Only ENABLED sites: a disabled site's pool is genuinely not needed, and
 * warning about it teaches the user to ignore this line.
 *
 * A major missing from the snapshot counts as not running. That is the
 * never-installed case, which is the one most likely to bite a new user, and
 * treating "absent" as "fine" would hide exactly that.
 *
 * `site.phpVersion` is already `major.minor` (e.g. `8.4`), the same selector
 * `PhpVersion::parse` validates in `crates/openvhost-core/src/site/model.rs`
 * and the same value `php_fpm_spec` (`apps/desktop/src-tauri/src/stack.rs:79`)
 * builds its `php-fpm-<major>` service id from — no extraction needed.
 */
export function stoppedPoolsFor(
	sites: readonly SiteDto[],
	services: readonly ServiceStatus[],
	nginxRunning: boolean
): string[] {
	if (!nginxRunning) return [];
	const needed = new Set(sites.filter((s) => s.enabled).map((s) => s.phpVersion));
	const down = [...needed].filter(
		(major) => services.find((s) => s.id === `php-fpm-${major}`)?.state.kind !== 'running'
	);
	return down.sort();
}
