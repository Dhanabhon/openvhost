// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers for the Databases (MySQL) page. Mirrors `sites.derive.ts`'s
// `scaffoldNotice` exhaustiveness pattern (spec D6): the whole point of this
// page is rendering every lifecycle state honestly, so the state a row is in
// is computed here, once, as a plain discriminated union — never inferred ad
// hoc in a template.

import type { MysqlDatadirStateDto, MysqlInitStepDto, MysqlInstanceDto } from './ipc';

/** Same shape `LogPane.svelte` renders (`services.svelte.ts`'s `UiLog`),
 *  redeclared here rather than imported — same reasoning as
 *  `languages.svelte.ts`'s own `UiLog`: this module stays decoupled from
 *  both sibling stores, and the only thing shared is the shape LogPane
 *  expects. */
export interface UiLog {
	id: string;
	tsMs: number;
	level: 'info' | 'warn' | 'error';
	line: string;
}

/**
 * One MySQL major's lifecycle, exhaustively — spec D6's eight named states,
 * verbatim. Drives `MysqlRow.svelte`'s ENTIRE render: which controls exist,
 * not just which copy shows. `MysqlRow.svelte`'s template ends its
 * `{#if}/{:else if}` chain with a call to {@link unreachableMysqlRowState}, so
 * a ninth variant added here without a matching template branch fails
 * typecheck instead of silently rendering nothing — the same guarantee
 * `scaffoldNotice` (`sites.derive.ts`) gives its own banner.
 */
export type MysqlRowState =
	| { kind: 'noBrew' }
	| { kind: 'notInstalled' }
	| { kind: 'installing'; log: UiLog[] }
	| { kind: 'installedNotInitialized' }
	| { kind: 'initializing'; log: UiLog[] }
	| { kind: 'initFailed'; step: MysqlInitStepDto; reason: string }
	| { kind: 'datadirForeign'; detail: string }
	| { kind: 'ready' };

/**
 * The last `initialize_mysql` attempt that ended in `Failed`, tagged with the
 * major it belongs to. Unlike `MysqlInstallOutcomeDto` (which carries its
 * own `major`), `MysqlInitOutcomeDto` does not — see
 * `DatabasesStore.initOutcome`'s doc comment — so the store wraps it before
 * handing it to {@link mysqlRowState}, the same "tag it with what it's for"
 * fix `ScaffoldNoticeBanner`'s `siteName`/`docroot` props apply to a DTO that
 * does not carry its own subject either.
 */
export interface MysqlInitFailure {
	major: string;
	step: MysqlInitStepDto;
	reason: string;
}

export interface MysqlRowInputs {
	/** `MysqlEnvironmentDto.brewFound` — a single, environment-wide fact, not
	 *  per-row, but still a real input to a not-yet-installed row's own state
	 *  (spec D1: an installed row is never told to go find brew, no matter
	 *  what `brewFound` says — only a row that still NEEDS brew to move
	 *  forward reads it at all). */
	brewFound: boolean;
	instance: MysqlInstanceDto;
	/** The major currently running `install_mysql`, or `''` when idle —
	 *  shared page-wide, the same single-flight rule `LanguagesStore.installing`
	 *  already follows (one `InstallLock`, spec D7). */
	installingMajor: string;
	installLog: readonly UiLog[];
	/** The major currently running `initialize_mysql`, or `''` when idle. */
	initializingMajor: string;
	initLog: readonly UiLog[];
	/** `null` once no failed attempt is remembered for this major, or once a
	 *  fresher disk read (a rescan, a successful retry) has superseded it. */
	initFailure: MysqlInitFailure | null;
}

/**
 * One MySQL row's current lifecycle state (spec D6). Precedence, highest
 * first:
 *
 * 1. An install or init IN FLIGHT for this exact major — both are
 *    single-flight page-wide (one `InstallLock`), so at most one of these two
 *    can ever be true for at most one row at a time; `installing` is checked
 *    first only to keep this function deterministic on the (unreachable in
 *    practice) case where both would somehow name the same major.
 * 2. Not installed at all: `noBrew` when Homebrew itself cannot be found —
 *    there is nothing this row's own Install button could do about that —
 *    else `notInstalled`.
 * 3. Installed: the on-DISK datadir state is authoritative and always wins
 *    over a remembered outcome, because it is read fresh on every
 *    environment load/rescan and a stale memory of a past failure must never
 *    outrank what is actually there right now (spec D2: datadir
 *    classification is "read from disk, never a stored boolean"). `foreign`
 *    renders as `datadirForeign` (reported, never touched); `initialized`
 *    renders as `ready`.
 * 4. Installed, datadir genuinely `notInitialized`: a remembered `initFailed`
 *    for this exact major, or plain `installedNotInitialized` otherwise.
 */
export function mysqlRowState(inputs: MysqlRowInputs): MysqlRowState {
	const { instance } = inputs;

	if (inputs.installingMajor === instance.major) {
		return { kind: 'installing', log: [...inputs.installLog] };
	}
	if (inputs.initializingMajor === instance.major) {
		return { kind: 'initializing', log: [...inputs.initLog] };
	}
	if (!instance.installed) {
		return inputs.brewFound ? { kind: 'notInstalled' } : { kind: 'noBrew' };
	}
	return datadirRowState(instance.datadirState, instance.major, inputs.initFailure);
}

function datadirRowState(
	state: MysqlDatadirStateDto,
	major: string,
	initFailure: MysqlInitFailure | null
): MysqlRowState {
	switch (state.kind) {
		case 'foreign':
			return { kind: 'datadirForeign', detail: state.detail };
		case 'initialized':
			return { kind: 'ready' };
		case 'notInitialized':
			if (initFailure !== null && initFailure.major === major) {
				return { kind: 'initFailed', step: initFailure.step, reason: initFailure.reason };
			}
			return { kind: 'installedNotInitialized' };
		default: {
			// `MysqlDatadirStateDto` is a closed, three-member union — same
			// exhaustiveness idiom as `scaffoldNotice` (sites.derive.ts): a
			// fourth variant fails THIS assignment at compile time.
			const unreachable: never = state;
			return unreachable;
		}
	}
}

/**
 * Exhaustiveness guard for `MysqlRowState`, called from `MysqlRow.svelte`'s
 * own final `{:else}` branch — the `scaffoldNotice` pattern (sites.derive.ts),
 * realized in a template's `{#if}/{:else if}` chain instead of a `switch`:
 * once every named `kind` above has its own branch, `state` narrows to
 * `never` at the call site, so a ninth variant added later fails typecheck
 * there instead of silently rendering nothing.
 */
export function unreachableMysqlRowState(state: never): never {
	throw new Error(`unhandled MySQL row state: ${JSON.stringify(state)}`);
}

/**
 * A short, dev-plain label for one step of the staged-init sequence (spec
 * D2), for an `initFailed` row's "failed while <label>" sentence.
 * `MysqlInitStepDto` is a stable discriminator, never parsed English (the
 * `ScaffoldStep` precedent) — so the mapping lives here, once, rather than at
 * each call site.
 */
export function mysqlInitStepLabel(step: MysqlInitStepDto): string {
	switch (step) {
		case 'render':
			return 'writing the configuration';
		case 'validate':
			return 'validating the configuration';
		case 'initialize':
			return 'creating the data directory';
		case 'startTempServer':
			return 'starting the temporary server';
		case 'setPassword':
			return 'setting the root password';
		case 'shutdown':
			return 'stopping the temporary server';
		case 'finalize':
			return 'finishing up';
		default: {
			// `MysqlInitStepDto` is a closed string-literal union mirrored 1:1
			// from Rust's `MysqlInitStep` — same exhaustiveness idiom as above.
			const unreachable: never = step;
			return unreachable;
		}
	}
}

/**
 * Copy for the Databases page's install-flow disclosure (spec Owner caveat
 * 1): Homebrew's OWN `mysql@8.4` post-install creates a separate datadir that
 * OpenVHost neither adopts nor deletes. Centralized so the page and its tests
 * share one string.
 */
export const HOMEBREW_DATADIR_DISCLOSURE =
	"Homebrew's own mysql@8.4 formula also creates a separate data directory the first time it's installed, with no root password. OpenVHost never touches or removes it.";

/**
 * The manual-recovery copy for a stale stored credential (spec Deferred:
 * "desync between state.db and a restored datadir" — no in-app recovery flow
 * ships this slice). Shared by the reset-`authFailed` and verify-`authFailed`
 * renderings, which are the same underlying fact told from two different
 * actions.
 */
export const STALE_CREDENTIAL_RECOVERY =
	"The stored password doesn't match this data directory — it may have been restored from a backup or changed outside OpenVHost. There is no in-app recovery yet: reset MySQL's root password manually (MySQL's own --skip-grant-tables recovery procedure), then use Reset here once you're back in.";

/**
 * The `brew services stop mysql@8.4` hint for a port-3306 conflict (spec
 * Owner caveat 1 / D4): Homebrew's own `mysql@8.4` `brew services` unit binds
 * the same fixed port this app's own instance does, and "Address already in
 * use" is the literal stderr substring `mysql_spec`'s failure path carries
 * (spec D4, same wording nginx/php-fpm already use) — matched
 * case-insensitively, since child stderr casing is not a contract.
 */
export function mysqlPortConflictHint(stderrTail: readonly string[]): string | null {
	const isPortConflict = stderrTail.some((line) => /address already in use/i.test(line));
	if (!isPortConflict) return null;
	return (
		'This looks like a port 3306 conflict. If a Homebrew-managed mysql@8.4 ' +
		'service is already running, stop it first: brew services stop mysql@8.4'
	);
}

/**
 * Fixed-width placeholder for the masked root-password field — NEVER sized
 * to the real password's length (32 hex chars, spec D3), which would leak
 * it. `MysqlCredentials.svelte` renders this whenever the password has not
 * been revealed; the real value only ever reaches this component's props
 * once `DatabasesStore.reveal()`/`copyPassword()` have actually fetched it.
 */
export const MASKED_PASSWORD_PLACEHOLDER = '••••••••••••••••••••••••';

/**
 * Whether any known MySQL instance — cataloged or not — is actually
 * installed. Mirrors `LanguagesStore.anyInstalled`'s role exactly (spec D6:
 * keeps the rowlist visible even with Homebrew missing, so an
 * already-installed instance is never hidden behind a "no brew" guide it no
 * longer needs), pulled out as a pure function so it is independently
 * testable rather than only reachable through the store.
 */
export function anyMysqlInstalled(instances: readonly MysqlInstanceDto[]): boolean {
	return instances.some((i) => i.installed);
}

/**
 * The catalogue majors this build actually offers to manage — read from the
 * environment's own rows rather than hardcoded, so a future catalogue change
 * (spec Deferred: "8.0 catalogue entry") needs no frontend edit. Used by an
 * out-of-catalogue row's one-line explanation.
 */
export function catalogedMajors(instances: readonly MysqlInstanceDto[]): string[] {
	return instances.filter((i) => i.cataloged).map((i) => i.major);
}
