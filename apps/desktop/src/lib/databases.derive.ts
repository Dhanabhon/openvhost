// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers for the Databases (MySQL) page. Mirrors `sites.derive.ts`'s
// `scaffoldNotice` exhaustiveness pattern (spec D6): the whole point of this
// page is rendering every lifecycle state honestly, so the state a row is in
// is computed here, once, as a plain discriminated union — never inferred ad
// hoc in a template.

import type {
	MariadbPackageOfferDto,
	MysqlDatadirStateDto,
	MysqlInitStepDto,
	MysqlInstallProgressDto,
	MysqlInstanceDto,
	MysqlPackageOfferDto,
	MysqlRuntimeSourceDto
} from './ipc';
import { mysqlSourceBadge, mysqlUninstallOffered, type Notice } from './mysql-install.derive';

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
 * Which install pipeline a row belongs to (P1 MariaDB UI design D1). Closed:
 * {@link engineDescriptor} switches over it with a `const _: never` arm, so a
 * third engine fails typecheck there rather than silently reusing MySQL's
 * copy and policies.
 */
export type EngineKind = 'mysql' | 'mariadb';

/**
 * The union of every engine's package-offer DTO — MySQL's own
 * `MysqlPackageOfferDto` (`available`/`unavailable`) plus MariaDB's
 * `MariadbPackageOfferDto`, which adds the third `awaitingRelease` state
 * (design D2). A real `MysqlPackageOfferDto` value satisfies this union
 * structurally (every one of its members matches a member here), so
 * `mysqlRowState` can keep handing it a plain `MysqlInstanceDto['offer']`
 * with no cast — this type only widens what {@link notInstalledRowState}
 * accepts, it does not change what MySQL ever actually produces.
 */
export type EngineOfferDto = MysqlPackageOfferDto | MariadbPackageOfferDto;

/** Where a runtime came from, in the shape {@link EngineDescriptor}'s
 *  `sourcePolicy`/`uninstallPolicy` need to answer "show a badge?"/"offer
 *  Uninstall?" — MySQL's own `MysqlRuntimeSourceDto` (`packaged`/`homebrew`).
 *  MariaDB has no source concept of its own to report (no Homebrew install
 *  path ever existed for it), so its policies below ignore this parameter
 *  entirely rather than pretending to interpret a shape that never varies. */
export type EngineSourceDto = MysqlRuntimeSourceDto;

/**
 * The generic shape the shared row/credentials render from — structurally
 * `MysqlInstanceDto` with `offer` widened to {@link EngineOfferDto}. A real
 * `MysqlInstanceDto` satisfies this type as-is (its narrower `offer` is a
 * subset), so every existing MySQL call site is unaffected; a future MariaDB
 * adapter (task 3) builds one of these from `MariadbEnvironmentDto` instead.
 */
export type EngineInstanceDto = Omit<MysqlInstanceDto, 'offer'> & { offer: EngineOfferDto };

/**
 * The static, per-engine facts the shared row/credentials need to paint
 * themselves without ever asking "which engine am I" in a template (design
 * D1: "no `{#if engine === …}` anywhere in a template"). Resolved ONCE, by
 * {@link engineDescriptor}; every value below is data, not a decision left
 * for the template to make.
 */
export interface EngineDescriptor {
	/** The word this engine's name reads as in a sentence — "MySQL"/"MariaDB". */
	label: string;
	/** The literal word old "mysql-row-…"/"mysql-credentials-…" test ids
	 *  hardcoded, now substituted from here — kept as `'mysql'` for MySQL so
	 *  every existing test id is byte-for-byte unchanged (design D1's own
	 *  stated gate for this refactor). Test ids that never carried an engine
	 *  word (`install-{major}`, `uninstall-{major}`, …) are untouched: their
	 *  uniqueness across engines is the CALLER's job (the identity value it
	 *  passes as `major`), not this prefix's. */
	idPrefix: string;
	/** The fixed port the credentials block shows when the caller does not
	 *  override it — 3306 for MySQL, 3307 for MariaDB (the two run side by
	 *  side, spec §10 point 7). */
	defaultPort: number;
	/** MySQL's own `mysqlPortConflictHint`, reused verbatim; MariaDB's own
	 *  equivalent, naming no Homebrew service — none exists to suggest
	 *  stopping. */
	portConflictHint: (stderrTail: readonly string[]) => string | null;
	/** The disclosure shown under a not-yet-ready row, explaining this
	 *  engine's datadir story. MySQL's is `SHARED_DATADIR_DISCLOSURE`
	 *  (datadirs are shared ACROSS its two install sources); MariaDB has no
	 *  second source to share one with, so it gets its own, simpler fact. */
	datadirDisclosure: string;
	/** The manual-recovery sentence for a stale stored credential, rendered by
	 *  BOTH the reset-`authFailed` and verify-`authFailed` states (fix wave
	 *  item 1). MySQL's is {@link STALE_CREDENTIAL_RECOVERY}, unchanged;
	 *  MariaDB's own equivalent names MariaDB's own `--skip-grant-tables`
	 *  recovery procedure, never MySQL's. Previously a bare module constant
	 *  `MysqlCredentials.svelte` rendered unconditionally for both engines —
	 *  a fourth instance of the "shared component says MySQL" bug this file's
	 *  own D1 discipline exists to prevent, missed by an earlier sweep because
	 *  it is a STRING, not a function call site. */
	staleCredentialRecovery: string;
	/** What provenance badge to show beside the version, or none. MySQL's own
	 *  `mysqlSourceBadge`, reused verbatim; MariaDB shows none — with exactly
	 *  one possible source, there is nothing to disambiguate. */
	sourcePolicy: (source: EngineSourceDto | null) => { label: string; title: string } | null;
	/** Whether an installed row may offer Uninstall. MySQL's own
	 *  `mysqlUninstallOffered` (`false` for a `packaged` source — no
	 *  `openvhost-pkg` uninstall counterpart existed when that shipped);
	 *  MariaDB's is `true` unconditionally — its uninstall already goes
	 *  through the shared `PackageKind::Mariadb` path. NOT cosmetic (design
	 *  D1): a shared row that inherited MySQL's default unchanged would
	 *  render `PACKAGED_UNINSTALL_UNAVAILABLE` on every installed MariaDB
	 *  row. */
	uninstallPolicy: (source: EngineSourceDto | null) => boolean;
	/** The "get started" invite `DatabasesEmpty.svelte` shows above an empty
	 *  rowlist — what pressing Install actually does, in this engine's own
	 *  words (task 3: "give `DatabasesEmpty` what it needs to speak for
	 *  either engine"). A plain string field, like {@link datadirDisclosure},
	 *  rather than an inline `{#if engine === …}` in that component's
	 *  template — the identical D1 discipline this file already applies to
	 *  every other per-engine fact. Not gated on availability: whether a
	 *  PARTICULAR host can install right now is a per-row fact this invite has
	 *  never concerned itself with, for either engine. */
	installInviteBody: string;
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
	/** Not installed, and this build publishes no checksum-verified download
	 *  for this host — an Intel Mac, today. An ABSENCE, not an error: the row
	 *  renders the explanation and no Install control at all. Replaces the old
	 *  `noBrew` state, which the move off Homebrew made false: installing MySQL
	 *  no longer involves brew, so a machine without it is no longer stuck. */
	| { kind: 'unavailable'; target: string }
	/** The ninth row state (P1 MariaDB UI design D2) — never reachable for
	 *  MySQL itself (`MysqlPackageOfferDto` has no such member), carried here
	 *  because the row that renders it is shared. A build exists and is
	 *  pinned, but the release that would serve it (`tag`) has not been
	 *  published, so the URL 404s. Deliberately NOT a flavour of
	 *  `unavailable`: that means "no build exists for this machine at all"; this
	 *  means "a build exists — nobody can have it yet". The next action
	 *  belongs to the maintainer, not the user, so this renders its own copy
	 *  and no Install control, same as `unavailable`. */
	| { kind: 'awaitingRelease'; tag: string }
	| { kind: 'notInstalled'; version: string }
	| {
			kind: 'installing';
			/** The last pipeline state seen, or `null` before the first event. */
			progress: MysqlInstallProgressDto | null;
			/** The length declared by the `started` event — the later
			 *  `downloaded` events do not repeat it. */
			total: number | null;
	  }
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
	instance: EngineInstanceDto;
	/** The major currently running `install_mysql`, or `''` when idle —
	 *  shared page-wide, the same single-flight rule `LanguagesStore.installing`
	 *  already follows (one `InstallLock`, spec D7). */
	installingMajor: string;
	/** The last install-pipeline state, page-wide (only one install can run).
	 *  Replaces the brew era's `installLog`: an install is no longer a child
	 *  process whose stdout can be tailed, it is five typed states. */
	installProgress: MysqlInstallProgressDto | null;
	/** The declared download length, carried forward from `started`. */
	installTotal: number | null;
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
 * 2. Not installed at all: `notInstalled` when this build has a
 *    checksum-verified download for this host, else `unavailable` — there is
 *    nothing this row's own Install button could do about a target we publish
 *    nothing for, so it renders no button rather than one that throws.
 *    (This used to read Homebrew's presence. It no longer can: installing MySQL
 *    is download → verify → extract, and a machine with no brew installs fine.)
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
		return {
			kind: 'installing',
			progress: inputs.installProgress,
			total: inputs.installTotal
		};
	}
	if (inputs.initializingMajor === instance.major) {
		return { kind: 'initializing', log: [...inputs.initLog] };
	}
	if (!instance.installed) {
		return notInstalledRowState(instance.offer);
	}
	return datadirRowState(instance.datadirState, instance.major, inputs.initFailure);
}

/**
 * A not-yet-installed row's state, from what this build can actually offer on
 * this host. Exhaustive over {@link EngineOfferDto} — a third offer state
 * must decide here rather than inherit "Install is fine" (this doc comment's
 * own long-standing promise; P1 MariaDB UI design D2 is what cashes it in).
 *
 * Widened beyond `MysqlInstanceDto['offer']` (`MysqlPackageOfferDto`, a closed
 * two-member union that can never actually carry `awaitingRelease`) so this
 * one function stays the single, shared, exhaustive home for BOTH engines'
 * offer unions — `mysqlRowState` below still only ever calls it with a real
 * MySQL offer, which structurally satisfies this wider parameter without a
 * cast. Exported so the `awaitingRelease` arm is directly testable with a
 * hand-built offer no real `MysqlInstanceDto` can produce.
 */
export function notInstalledRowState(offer: EngineOfferDto): MysqlRowState {
	switch (offer.kind) {
		case 'available':
			return { kind: 'notInstalled', version: offer.version };
		case 'awaitingRelease':
			return { kind: 'awaitingRelease', tag: offer.tag };
		case 'unavailable':
			return { kind: 'unavailable', target: offer.target };
		default: {
			const unreachable: never = offer;
			return unreachable;
		}
	}
}

/**
 * Whether this engine currently has anything to press Install on — the
 * question `DatabasesEmpty.svelte`'s own "get started" invite needs answered
 * before it claims an action exists (fix wave item 2): `awaitingRelease` and
 * `unavailable` both mean no Install control exists anywhere on the page,
 * even before anything is installed, so the invite must not describe an
 * install, or name a mechanism (Homebrew), that this engine is not actually
 * offering right now. Exhaustive over {@link EngineOfferDto} for the same
 * reason {@link notInstalledRowState} is: a third offer state must decide
 * here, not silently inherit "yes, installable".
 */
export function engineInstallOffered(offer: EngineOfferDto): boolean {
	switch (offer.kind) {
		case 'available':
			return true;
		case 'awaitingRelease':
		case 'unavailable':
			return false;
		default: {
			const unreachable: never = offer;
			return unreachable;
		}
	}
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
 * Copy for a MySQL major whose datadir is shared across install sources
 * (MySQL-from-tarball design D6). The datadir is keyed by MAJOR, not by where
 * the binaries came from, so a user who initialized 8.4 under the Homebrew era
 * keeps their databases after installing the packaged 8.4 — same major, same
 * MySQL, same on-disk format.
 *
 * Replaces the old `HOMEBREW_DATADIR_DISCLOSURE`, which described a fact that
 * stopped being true when installing stopped going through brew: OpenVHost no
 * longer runs `brew install mysql@8.4`, so brew's own formula no longer creates
 * anything as a side effect of pressing Install here.
 */
export const SHARED_DATADIR_DISCLOSURE =
	'Data directories are shared per version, not per install source — if you already initialized MySQL 8.4 through Homebrew, this keeps those databases rather than starting over.';

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

/** MariaDB's own port-conflict hint (P1 MariaDB UI design). Same
 *  "address already in use" match {@link mysqlPortConflictHint} uses above,
 *  but names no Homebrew service — MariaDB never had one to stop (design D2:
 *  "no Homebrew fallback… anywhere in this app").
 *
 *  Task 3 review: an earlier draft named "another MariaDB or MySQL server"
 *  as the likely occupant of port 3307. That was not an honest guess —
 *  OpenVHost's own MySQL always binds 3306, never 3307 (`MARIADB_ENDPOINT`'s
 *  own doc comment: the two engines' distinct ports are what let both run at
 *  once), and nothing else commonly defaults to 3307 the way MySQL's own
 *  Homebrew formula defaults to 3306. Naming a specific-but-unlikely product
 *  would read as a diagnosis; this names none, which is the honest version. */
function mariadbPortConflictHint(stderrTail: readonly string[]): string | null {
	const isPortConflict = stderrTail.some((line) => /address already in use/i.test(line));
	if (!isPortConflict) return null;
	return (
		'This looks like a port 3307 conflict. Nothing else in OpenVHost uses this port — check what ' +
		'else on this machine is bound to it, stop that, then try again.'
	);
}

/** MariaDB's datadir disclosure. This build ships exactly one series, so
 *  there is no cross-source sharing story to tell, unlike MySQL's
 *  {@link SHARED_DATADIR_DISCLOSURE} (which exists because Homebrew and
 *  OpenVHost's own installer can both produce a MySQL of the same major). */
const MARIADB_DATADIR_DISCLOSURE =
	"MariaDB's data directory is created the first time you initialize it below.";

/** MariaDB's own manual-recovery copy for a stale stored credential (fix wave
 *  item 1) — mirrors {@link STALE_CREDENTIAL_RECOVERY} exactly, substituting
 *  MariaDB's own `--skip-grant-tables` recovery procedure for MySQL's, the
 *  same substitution {@link mariadbPortConflictHint}/
 *  {@link MARIADB_DATADIR_DISCLOSURE} already make for their own facts. */
const MARIADB_STALE_CREDENTIAL_RECOVERY =
	"The stored password doesn't match this data directory — it may have been restored from a backup or changed outside OpenVHost. There is no in-app recovery yet: reset MariaDB's root password manually (MariaDB's own --skip-grant-tables recovery procedure), then use Reset here once you're back in.";

/** MySQL's "get started" invite body — moved here VERBATIM from
 *  `DatabasesEmpty.svelte`'s own markup (task 3), so its rendered text is
 *  byte-for-byte unchanged and that component's existing tests stay green. */
const MYSQL_INSTALL_INVITE_BODY =
	'OpenVHost downloads MySQL from Oracle, checks it against a checksum built into this app, and ' +
	'unpacks it into its own packages folder — no Homebrew required. It then initializes a data ' +
	'directory with a generated root password and runs it under the supervisor below.';

/** MariaDB's own "get started" invite body (task 3) — names its real source
 *  (OpenVHost's own GitHub release, never Oracle) and states plainly that
 *  Homebrew was never involved, rather than "not required" (which would
 *  wrongly imply an optional Homebrew path exists — design D2: "no Homebrew
 *  fallback for MariaDB anywhere in this app"). */
const MARIADB_INSTALL_INVITE_BODY =
	'OpenVHost downloads MariaDB from its own GitHub release, checks it against a checksum built ' +
	'into this app, and unpacks it into its own packages folder. MariaDB has never gone through ' +
	'Homebrew in this app. It then initializes a data directory with a generated root password and ' +
	'runs it under the supervisor below.';

const MYSQL_DESCRIPTOR: EngineDescriptor = {
	label: 'MySQL',
	idPrefix: 'mysql',
	defaultPort: 3306,
	portConflictHint: mysqlPortConflictHint,
	datadirDisclosure: SHARED_DATADIR_DISCLOSURE,
	staleCredentialRecovery: STALE_CREDENTIAL_RECOVERY,
	sourcePolicy: mysqlSourceBadge,
	uninstallPolicy: mysqlUninstallOffered,
	installInviteBody: MYSQL_INSTALL_INVITE_BODY
};

const MARIADB_DESCRIPTOR: EngineDescriptor = {
	label: 'MariaDB',
	idPrefix: 'mariadb',
	defaultPort: 3307,
	portConflictHint: mariadbPortConflictHint,
	datadirDisclosure: MARIADB_DATADIR_DISCLOSURE,
	staleCredentialRecovery: MARIADB_STALE_CREDENTIAL_RECOVERY,
	sourcePolicy: () => null,
	uninstallPolicy: () => true,
	installInviteBody: MARIADB_INSTALL_INVITE_BODY
};

/**
 * Resolved ONCE per row (design D1): a `switch` over the closed
 * {@link EngineKind}, `const _: never` at the end, so a third engine fails
 * typecheck here rather than silently falling back to MySQL's values.
 */
export function engineDescriptor(engine: EngineKind): EngineDescriptor {
	switch (engine) {
		case 'mysql':
			return MYSQL_DESCRIPTOR;
		case 'mariadb':
			return MARIADB_DESCRIPTOR;
		default: {
			const unreachable: never = engine;
			return unreachable;
		}
	}
}

/**
 * Copy for the ninth row state (design D2): a build exists and is pinned,
 * but the release that would serve it has not been published, so there is
 * nothing to install here until it is. Visibly distinct from the
 * `unavailable` state's "there is no build for this machine at all" — this
 * names a release tag and a maintainer action, not a permanent absence.
 */
export function engineAwaitingReleaseNotice(descriptor: EngineDescriptor, tag: string): Notice {
	return {
		tone: 'warn',
		title: `${descriptor.label} is not published yet`,
		body:
			`OpenVHost has a ${descriptor.label} build pinned and audited (release "${tag}"), but that ` +
			`release has not been published yet, so there is nothing to install here until it is. This ` +
			`is not something you can fix from here — check back once it ships.`
	};
}
