<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type {
		MariadbInstallResultDto,
		MysqlConnectionProofDto,
		MysqlInstallOutcomeDto,
		MysqlInstallProgressDto,
		MysqlResetOutcomeDto,
		ServiceStatus
	} from '$lib/ipc';
	import {
		engineAwaitingReleaseNotice,
		engineDescriptor,
		mysqlInitStepLabel,
		mysqlRowState,
		unreachableMysqlRowState,
		type EngineInstanceDto,
		type EngineKind,
		type MysqlInitFailure,
		type UiLog
	} from '$lib/databases.derive';
	import {
		PACKAGED_UNINSTALL_UNAVAILABLE,
		mysqlCancelLabel,
		mysqlInstallProgressLabel,
		mysqlInstallProgressPercent
	} from '$lib/mysql-install.derive';
	import {
		engineLedgerNotice,
		engineOfferNotice,
		engineOutcomeNotice
	} from '$lib/mysql-row.derive';
	import { uninstallActionDisabled, uninstallConfirmLabel } from '$lib/uninstall.derive';
	import Button from './Button.svelte';
	import LogPane from './LogPane.svelte';
	import MysqlCredentials from './MysqlCredentials.svelte';
	import StatusPill from './StatusPill.svelte';

	let {
		engine = 'mysql',
		instance,
		installingMajor,
		installProgress,
		installTotal,
		cancellingInstall = false,
		installOutcome,
		mariadbInstallOutcome = null,
		installError = '',
		initializingMajor,
		initLog,
		initFailure,
		initError = '',
		uninstallingMajor = '',
		catalogedMajorsList,
		serviceState,
		password,
		revealed,
		revealing,
		passwordError,
		resetting,
		resetOutcome,
		resetError,
		verifying,
		verifyResult,
		verifyError,
		onInstall,
		onCancelInstall,
		onInitialize,
		onUninstall,
		onStart,
		onStop,
		onReveal,
		onHide,
		onCopyPassword,
		onReset,
		onVerify
	}: {
		/** Which engine this row paints (P1 MariaDB UI design D1) — defaults to
		 *  `'mysql'` so every existing caller/test is unaffected. Resolved ONCE
		 *  into {@link descriptor} below; nothing else in this file branches on
		 *  it again ("no `{#if engine === …}` anywhere in a template"). */
		engine?: EngineKind;
		instance: EngineInstanceDto;
		/** The major installing anywhere on the page, '' when idle — shared
		 *  page-wide (one `InstallLock`), same as `LanguagesStore.installing`. */
		installingMajor: string;
		/** The last install-pipeline state, page-wide. Replaces the brew era's
		 *  streamed stdout: an install is five typed states now, and `verified`
		 *  vs `extracted` is the distinction the whole checksum guarantee rests
		 *  on being visible. */
		installProgress: MysqlInstallProgressDto | null;
		/** The declared download length, from the `started` event. */
		installTotal: number | null;
		/** Whether a cancel has been asked for and not yet settled. */
		cancellingInstall?: boolean;
		/** The last `install_mysql` outcome, whichever major it was for —
		 *  `MysqlInstallOutcomeDto` carries its own `major`, so this row only
		 *  renders it once it matches `instance.major` (mirrors
		 *  `LanguageRow.svelte`'s `rowOutcome`). */
		installOutcome: MysqlInstallOutcomeDto | null;
		/** MariaDB's own settled `install_mariadb` outcome (P1 MariaDB UI
		 *  design), read ONLY when `engine === 'mariadb'` — a SEPARATE prop
		 *  rather than a widened {@link installOutcome}, because
		 *  `MariadbInstallResultDto` is not a subtype of `MysqlInstallResultDto`
		 *  (it adds `awaitingRelease`, design D2/D5): reading each engine's own
		 *  correctly-typed prop is what lets {@link outcomeNotice}/
		 *  {@link ledgerNotice} call the right notice function with no cast,
		 *  where a single widened prop could not. `MariadbInstallOutcomeDto`
		 *  itself carries no major (a field nothing can vary is overhead, the
		 *  same reasoning `MariadbInstance` gives for leaving `major` off its
		 *  own struct), so the tag is added at the boundary that builds this
		 *  prop — mirrors `DatabasesStore.initOutcome`'s identical "tag it with
		 *  what it's for" fix for a DTO that does not carry its own subject. */
		mariadbInstallOutcome?: { major: string; result: MariadbInstallResultDto } | null;
		/** A thrown install error, PRE-SCOPED by the page to this row (mirrors
		 *  `+page.svelte`'s `lastAttempted` convention on the Languages page —
		 *  `error` itself carries no major). Empty when this is not the row the
		 *  last install attempt belonged to. */
		installError?: string;
		initializingMajor: string;
		initLog: UiLog[];
		/** The remembered failed-init outcome for THIS major, or `null` — see
		 *  `DatabasesStore.initFailureFor`. */
		initFailure: MysqlInitFailure | null;
		/** A thrown initialize error, PRE-SCOPED by the page to this row — same
		 *  convention as `installError`, and distinct from `initFailure`: a
		 *  settled `Failed` outcome names a step and a reason, but
		 *  `initialize_mysql` can also THROW outright (e.g. "an install is
		 *  already running"), which leaves the row's disk-truth state at
		 *  `installedNotInitialized` with nothing else to say why the click did
		 *  not work. */
		initError?: string;
		/** The major being UNINSTALLED anywhere in the app, '' when idle — a
		 *  state rather than a boolean for the same reason `installingMajor` is
		 *  one: this row must tell "somebody else is busy" (disabled) from "it
		 *  is me" (disabled AND labelled "Uninstalling…"). */
		uninstallingMajor?: string;
		/** Every major THIS build offers to manage — for the out-of-catalogue
		 *  row's one-line explanation (spec D1). */
		catalogedMajorsList: string[];
		/** The whole supervised state, not just whether it is running — `failed`
		 *  carries the `stderrTail` this row renders. `null` means the snapshot
		 *  has not arrived yet, OR this row has no service at all (never
		 *  installed, or installed but not yet Initialized — `instance.serviceId`
		 *  is `null` in both cases, spec D6). */
		serviceState: ServiceStatus['state'] | null;
		/** The cached value, once fetched. See `MysqlCredentials.svelte`'s
		 *  identical prop — threaded straight through, never read or branched
		 *  on here. NOT the display gate; see `revealed`. */
		password?: string;
		/** The DISPLAY gate — see `MysqlCredentials.svelte`'s identical prop
		 *  (review fix: separate from `password`'s mere cache presence, so a
		 *  Copy click can never silently un-mask the field). Threaded straight
		 *  through, never read or branched on here. */
		revealed: boolean;
		revealing: boolean;
		passwordError: string;
		resetting: boolean;
		resetOutcome?: MysqlResetOutcomeDto;
		resetError: string;
		verifying: boolean;
		verifyResult?: MysqlConnectionProofDto;
		verifyError: string;
		onInstall: (major: string) => void;
		/** Aborts the install in flight. MANDATORY affordance: the download has
		 *  no wall-clock bound and the package pipeline's install permit is
		 *  process-wide, so an install nobody can stop starves every later one. */
		onCancelInstall: () => void;
		onInitialize: (major: string) => void;
		/** Opens the uninstall confirmation (package-uninstall design D6).
		 *  Uninstalls nothing on its own: the plan behind the dialog is a pure
		 *  query, and `brew uninstall` only runs on the dialog's own confirm. */
		onUninstall: (major: string) => void;
		onStart: (serviceId: string) => void;
		onStop: (serviceId: string) => void;
		onReveal: (major: string) => void;
		onHide: (major: string) => void;
		onCopyPassword: (major: string) => void;
		onReset: (major: string) => void;
		onVerify: (major: string) => void;
	} = $props();

	/**
	 * Local UI state for the reset confirm step — one `MysqlRow` instance per
	 * major, so (unlike `SiteListRow.svelte`'s delete confirm, which stays
	 * local for a store-refetch reason that DOES apply there) this could
	 * equally be lifted to a prop. It stays local anyway: nothing outside this
	 * row ever needs to read or seed it, and `MysqlCredentials.svelte` — the
	 * component that actually renders the confirm copy — takes it as a prop
	 * precisely so ITS OWN tests can drive every state directly (see that
	 * file's test header).
	 */
	let confirmingReset = $state(false);

	/** The static, per-engine facts (P1 MariaDB UI design D1) — resolved ONCE,
	 *  here, from the closed {@link EngineKind}. Everything below reads from
	 *  this; nothing re-decides "which engine am I" anywhere else. */
	const descriptor = $derived(engineDescriptor(engine));

	const anyInstallOrInitRunning = $derived(installingMajor !== '' || initializingMajor !== '');
	/** Page-wide, not per-row: `brew install`, `brew uninstall` and the staged
	 *  init all serialize behind one `InstallLock` (design D1), so a second
	 *  action would only queue on a mutex. Includes this row's own uninstall,
	 *  so a double-click cannot reach the command twice. */
	const uninstallDisabled = $derived(
		uninstallActionDisabled({
			installingMajor,
			initializingMajor,
			uninstallingMajor
		})
	);

	const rowInstallOutcome = $derived(
		installOutcome !== null && installOutcome.major === instance.major ? installOutcome : null
	);
	/** MariaDB's own settled outcome, tagged and matched the same way as
	 *  {@link rowInstallOutcome} above — `null` whenever this row is not the
	 *  one {@link mariadbInstallOutcome} names, or nothing has settled yet. */
	const mariadbRowOutcome = $derived(
		mariadbInstallOutcome !== null &&
			mariadbInstallOutcome !== undefined &&
			mariadbInstallOutcome.major === instance.major
			? mariadbInstallOutcome
			: null
	);
	const portConflictHint = $derived(
		serviceState?.kind === 'failed' ? descriptor.portConflictHint(serviceState.stderrTail) : null
	);

	// Named `rowState`, not `state`: svelte-check's TS layer (svelte2tsx) gets
	// confused between a plain local binding named `state` and the `$state`
	// rune in the SAME script block — it reported `$state` as "used before its
	// declaration" and tried to treat `state` as a Svelte store, neither of
	// which is real. Renaming the variable is the whole fix.
	const rowState = $derived(
		mysqlRowState({
			instance,
			installingMajor,
			installProgress,
			installTotal,
			initializingMajor,
			initLog,
			initFailure
		})
	);

	/** Which install put these binaries here — `null` when nothing is installed
	 *  (design D3: two sources coexist during the migration, and "which mysqld
	 *  am I actually running" must not be a guess). */
	const sourceBadge = $derived(descriptor.sourcePolicy(instance.source));
	/** MANDATORY absence: `openvhost-pkg` has no uninstall counterpart at all
	 *  yet, so the brew-driven dialog could only fail on a packaged runtime.
	 *  Descriptor-driven (design D1), NOT `mysqlUninstallOffered` inlined
	 *  directly: a naively shared row that inherited MySQL's "packaged means
	 *  no Uninstall" unchanged would render `PACKAGED_UNINSTALL_UNAVAILABLE`
	 *  on every installed MariaDB row, whose packaged Uninstall genuinely
	 *  works. */
	const canUninstall = $derived(descriptor.uninstallPolicy(instance.source));
	/** The settled-install banner, the ledger-write warning, and the
	 *  not-yet-installed explanation — all three dispatch on `engine` the SAME
	 *  way (design D1 follow-through, task 3 finding: EVERY engine used to
	 *  render MySQL's own hardcoded copy until this dispatch existed). Pulled
	 *  into `mysql-row.derive.ts` as a pure module (fix wave item 3): the
	 *  `switch`-with-`never`-arm dispatch itself is unchanged, only where it
	 *  lives — see that file for the full reasoning each of the three used to
	 *  carry here inline. */
	const outcomeNotice = $derived(engineOutcomeNotice(engine, rowInstallOutcome, mariadbRowOutcome));
	const ledgerNotice = $derived(engineLedgerNotice(engine, rowInstallOutcome, mariadbRowOutcome));
	const offerNotice = $derived(engineOfferNotice(engine, rowState));
</script>

{#if !instance.cataloged}
	<!-- Out-of-catalogue row (spec D1): an installed major this build does not
	     manage. Deliberately does NOT go through `mysqlRowState` at all — that
	     function models a MANAGED major's lifecycle, and every action command
	     rejects an out-of-catalogue major server-side regardless of what the UI
	     does, so this branch renders NO action of any kind: no Install, no
	     Initialize, no Start/Stop/Retry, no credentials block. The pill is the
	     one exception — it is informational (what IS running), not an action —
	     rendered only if a supervisor row happens to exist for it. -->
	<div class="row engine-row" data-testid="{descriptor.idPrefix}-row-{instance.major}">
		<div class="primary">
			<span class="version">{descriptor.label} {instance.major}</span>
			<span class="badge unmanaged">Not managed</span>
			<!-- Informational, like the pill beside it: WHERE this unmanaged
			     runtime came from is exactly the question a migration raises, and
			     answering it offers no action. -->
			{#if sourceBadge}
				<span
					class="badge source source-{instance.source?.kind}"
					title={sourceBadge.title}
					data-testid="{descriptor.idPrefix}-source-{instance.major}">{sourceBadge.label}</span
				>
			{/if}
		</div>
		<div class="pill-cell">
			{#if serviceState}
				<StatusPill kind={serviceState.kind} testId="{descriptor.idPrefix}-pill-{instance.major}" />
			{/if}
		</div>
		<div class="row-actions"></div>
	</div>
	<p class="note" data-testid="out-of-catalogue-{instance.major}">
		{descriptor.label}
		{instance.major} is installed, but this build only manages {descriptor.label}
		{catalogedMajorsList.join(', ')}. Shown for visibility only — no actions are offered here.
	</p>
{:else}
	<div class="row engine-row" data-testid="{descriptor.idPrefix}-row-{instance.major}">
		<div class="primary">
			<span class="version">{descriptor.label} {instance.major}</span>
			<!-- WHICH INSTALL these binaries came from (design D3). Absent when
			     nothing is installed. The Homebrew badge deliberately carries no
			     version: brew's exact patch release is only knowable by executing
			     a 55 MB mysqld, and printing the major where a full version
			     belongs would be a lie nobody could detect. -->
			{#if sourceBadge}
				<span
					class="badge source source-{instance.source?.kind}"
					title={sourceBadge.title}
					data-testid="{descriptor.idPrefix}-source-{instance.major}">{sourceBadge.label}</span
				>
			{/if}
		</div>
		<div class="pill-cell">
			{#if serviceState}
				<StatusPill kind={serviceState.kind} testId="{descriptor.idPrefix}-pill-{instance.major}" />
			{/if}
		</div>
		<div class="row-actions">
			{#if rowState.kind === 'unavailable' || rowState.kind === 'awaitingRelease' || rowState.kind === 'datadirForeign'}
				<!-- Nothing to offer: no checksum-verified download for this host, a
				     build that is pinned but not yet published, or a foreign datadir
				     this app will not touch. The absence is explained below the row,
				     never rendered as a button that throws. -->
			{:else if rowState.kind === 'notInstalled'}
				<Button
					variant="primary"
					size="sm"
					testId="install-{instance.major}"
					ariaLabel="Install {descriptor.label} {rowState.version}"
					disabled={anyInstallOrInitRunning}
					onclick={() => onInstall(instance.major)}
				>
					Install
				</Button>
			{:else if rowState.kind === 'installing'}
				<Button variant="primary" size="sm" disabled onclick={() => {}}>Installing…</Button>
				<!-- MANDATORY. Nothing bounds the download by wall clock — only a
				     30 s idle window — and the package pipeline's install permit is
				     process-wide and taken BEFORE staging, so a server dribbling one
				     byte every 29 s would hold it forever and starve every later
				     install. This button is the only way to get it back. -->
				<Button
					variant="quiet"
					size="sm"
					testId="cancel-install-{instance.major}"
					ariaLabel="Cancel installing {descriptor.label} {instance.major}"
					disabled={cancellingInstall}
					onclick={() => onCancelInstall()}
				>
					{mysqlCancelLabel(cancellingInstall)}
				</Button>
			{:else if rowState.kind === 'installedNotInitialized' || rowState.kind === 'initFailed'}
				<Button
					variant="primary"
					size="sm"
					testId={rowState.kind === 'initFailed'
						? `retry-init-${instance.major}`
						: `initialize-${instance.major}`}
					ariaLabel="Initialize {descriptor.label} {instance.major}"
					disabled={anyInstallOrInitRunning}
					onclick={() => onInitialize(instance.major)}
				>
					{rowState.kind === 'initFailed' ? 'Retry' : 'Initialize'}
				</Button>
			{:else if rowState.kind === 'initializing'}
				<Button variant="primary" size="sm" disabled onclick={() => {}}>Initializing…</Button>
			{:else if rowState.kind === 'ready'}
				{#if instance.serviceId && serviceState !== null}
					{#if serviceState.kind === 'failed'}
						<Button
							variant="quiet"
							size="sm"
							testId="retry-{instance.serviceId}"
							ariaLabel="Retry {descriptor.label} {instance.major}"
							onclick={() => onStart(instance.serviceId ?? '')}>Retry</Button
						>
					{:else if serviceState.kind === 'stopped'}
						<Button
							variant="quiet"
							size="sm"
							testId="start-{instance.serviceId}"
							ariaLabel="Start {descriptor.label} {instance.major}"
							onclick={() => onStart(instance.serviceId ?? '')}>Start</Button
						>
					{:else}
						<Button
							variant="quiet"
							size="sm"
							testId="stop-{instance.serviceId}"
							ariaLabel="Stop {descriptor.label} {instance.major}"
							onclick={() => onStop(instance.serviceId ?? '')}>Stop</Button
						>
					{/if}
				{/if}
			{:else}
				{unreachableMysqlRowState(rowState)}
			{/if}
			{#if instance.installed && canUninstall}
				<!-- Last in the row, after whatever the lifecycle offers: the rare,
				     destructive control. Present for every installed HOMEBREW major,
				     including one whose datadir is foreign (design D6/D2 — removing
				     the engine never touches a datadir, so a datadir this app
				     refuses to adopt is no reason to trap its binaries either) and
				     one that was never initialized. Opens a confirmation; it
				     uninstalls nothing by itself.

				     ABSENT for a PACKAGED MySQL runtime, deliberately: the uninstall
				     slice drives `brew uninstall`, and `openvhost-pkg` has no
				     uninstall counterpart at all yet. An affordance that is present
				     and fails is worse than one that is absent. A packaged MariaDB
				     runtime is NOT excluded here — its `uninstallPolicy` says so
				     (design D1). -->
				<Button
					variant="quiet"
					size="sm"
					testId="uninstall-{instance.major}"
					ariaLabel="Uninstall {descriptor.label} {instance.major}"
					disabled={uninstallDisabled}
					onclick={() => onUninstall(instance.major)}
				>
					{uninstallConfirmLabel(uninstallingMajor === instance.major)}
				</Button>
			{/if}
		</div>
	</div>

	{#if rowState.kind === 'unavailable'}
		<!-- An honest ABSENCE, not an error (design D2): Oracle publishes an
		     x86_64 build, but its bytes never went through the signature check
		     the catalogue's pin rests on, so this build offers nothing for it and
		     says exactly that — with the route that does still work. `offerNotice`
		     is resolved once in the script above, per engine. -->
		<p class="note warn" data-testid="{descriptor.idPrefix}-unavailable-{instance.major}">
			<strong>{offerNotice?.title}.</strong>
			{offerNotice?.body}
		</p>
	{:else if rowState.kind === 'awaitingRelease'}
		<!-- The ninth row state (design D2): a build exists and is pinned, but
		     the release that would serve it has not been published, so the next
		     action belongs to the maintainer, not the user. Its own copy, its
		     own test id — never folded into `unavailable`'s "no build at all". -->
		{@const notice = engineAwaitingReleaseNotice(descriptor, rowState.tag)}
		<p class="note warn" data-testid="{descriptor.idPrefix}-awaiting-release-{instance.major}">
			<strong>{notice.title}.</strong>
			{notice.body}
		</p>
	{:else if rowState.kind === 'notInstalled'}
		<p class="note" data-testid="offer-{instance.major}">
			<strong>{offerNotice?.title}.</strong>
			{offerNotice?.body}
		</p>
		<p class="note" data-testid="disclosure-{instance.major}">{descriptor.datadirDisclosure}</p>
		{#if installError !== ''}
			<p class="error" role="alert" style="white-space: pre-wrap">{installError}</p>
		{/if}
		{#if outcomeNotice}
			<p
				class={outcomeNotice.tone === 'error' ? 'error' : `note ${outcomeNotice.tone}`}
				role={outcomeNotice.tone === 'ok' ? undefined : 'alert'}
				data-testid="install-outcome-{instance.major}"
			>
				<strong>{outcomeNotice.title}.</strong>
				{outcomeNotice.body}
			</p>
		{/if}
	{:else if rowState.kind === 'installing'}
		<!-- The pipeline the user watches. `verified` and `extracted` are
		     SEPARATE sentences on purpose: a download that was checked against
		     the built-in SHA-256 and one that merely arrived must never look
		     identical, which is the whole of what golden rule 6 buys. -->
		<p class="note progress" data-testid="install-progress-{instance.major}">
			{rowState.progress === null
				? 'Preparing the download…'
				: mysqlInstallProgressLabel(rowState.progress, rowState.total)}
		</p>
		{#if rowState.progress !== null && mysqlInstallProgressPercent(rowState.progress, rowState.total) !== null}
			<div
				class="bar"
				role="progressbar"
				aria-label="Downloading {descriptor.label} {instance.major}"
				aria-valuemin={0}
				aria-valuemax={100}
				aria-valuenow={mysqlInstallProgressPercent(rowState.progress, rowState.total)}
				data-testid="install-bar-{instance.major}"
			>
				<span
					class="fill"
					style="width: {mysqlInstallProgressPercent(rowState.progress, rowState.total)}%"
				></span>
			</div>
		{/if}
	{:else if rowState.kind === 'installedNotInitialized'}
		<!-- The state a successful install lands in, so this is where its own
		     outcome is read back: the row stopped being `notInstalled` the moment
		     the rescan saw the new tree. -->
		{#if outcomeNotice}
			<p
				class={outcomeNotice.tone === 'error' ? 'error' : `note ${outcomeNotice.tone}`}
				role={outcomeNotice.tone === 'ok' ? undefined : 'alert'}
				data-testid="install-outcome-{instance.major}"
			>
				<strong>{outcomeNotice.title}.</strong>
				{outcomeNotice.body}
			</p>
		{/if}
		{#if ledgerNotice}
			<!-- Provenance lost, never the install: the packages tree IS the
			     inventory, so a missing row costs the recorded install date and
			     nothing else. Calling a demonstrably installed MySQL a failure
			     would be the bigger lie. -->
			<p class="note warn" data-testid="ledger-warning-{instance.major}">{ledgerNotice}</p>
		{/if}
		<p class="note" data-testid="disclosure-{instance.major}">{descriptor.datadirDisclosure}</p>
		{#if initError !== ''}
			<p class="error" role="alert" style="white-space: pre-wrap">{initError}</p>
		{/if}
	{:else if rowState.kind === 'initializing'}
		{#if rowState.log.length > 0}
			<LogPane logs={rowState.log} />
		{/if}
	{:else if rowState.kind === 'initFailed'}
		<p class="error" role="alert" data-testid="init-failed-{instance.major}">
			Initialization failed while {mysqlInitStepLabel(rowState.step)}: {rowState.reason}
		</p>
		{#if initError !== ''}
			<!-- A retry that THROWS outright (e.g. "an install is already
			     running") rather than settling with a new outcome — the
			     remembered `initFailure` above is from the PREVIOUS attempt and
			     stays on screen (only a settled retry supersedes it), so this is
			     additional, not a replacement. -->
			<p class="error" role="alert" style="white-space: pre-wrap">{initError}</p>
		{/if}
	{:else if rowState.kind === 'datadirForeign'}
		<!-- Reported, never touched, never even suggested to be cleaned up by
		     this app (spec click-list item 7: "no destructive offer") — the copy
		     states the fact and the one non-destructive next step (rescan once
		     it looks like an empty MySQL {instance.major} datadir again), and
		     stops there. What to do with the foreign content itself is the
		     user's call, made outside this app. -->
		<p class="note warn" data-testid="datadir-foreign-{instance.major}">
			{descriptor.label}
			{instance.major}'s data directory already has unexpected content and OpenVHost will not touch
			it: <span class="mono">{rowState.detail}</span>. Once it looks like an empty
			{descriptor.label}
			{instance.major} data directory again, use Check again above.
		</p>
	{:else if rowState.kind === 'ready'}
		{#if serviceState?.kind === 'failed'}
			<p class="error" role="alert" data-testid="pool-failed-{instance.serviceId}">
				{descriptor.label}
				{instance.major} failed{#if serviceState.exit !== null}&nbsp;(exit {serviceState.exit}){/if}.
			</p>
			{#if serviceState.stderrTail.length > 0}
				<pre class="pool-stderr">{serviceState.stderrTail.join('\n')}</pre>
			{/if}
			{#if portConflictHint !== null}
				<p class="note warn" data-testid="port-conflict-hint-{instance.major}">
					{portConflictHint}
				</p>
			{/if}
		{/if}
		<MysqlCredentials
			{engine}
			major={instance.major}
			socketPath={instance.socketPath ?? ''}
			{password}
			{revealed}
			{revealing}
			{passwordError}
			{confirmingReset}
			{resetting}
			{resetOutcome}
			{resetError}
			{verifying}
			{verifyResult}
			{verifyError}
			onReveal={() => onReveal(instance.major)}
			onHide={() => onHide(instance.major)}
			onCopyPassword={() => onCopyPassword(instance.major)}
			onRequestReset={() => (confirmingReset = true)}
			onCancelReset={() => (confirmingReset = false)}
			onConfirmReset={() => {
				confirmingReset = false;
				onReset(instance.major);
			}}
			onVerify={() => onVerify(instance.major)}
		/>
	{:else}
		{unreachableMysqlRowState(rowState)}
	{/if}

	{#if instance.installed && !canUninstall}
		<!-- Rendered once, after whatever the lifecycle had to say, so the
		     MISSING Uninstall control reads as a known limit rather than an
		     oversight. `openvhost-pkg` has no uninstall counterpart at all yet —
		     that is its own slice. -->
		<p class="note" data-testid="no-uninstall-{instance.major}">{PACKAGED_UNINSTALL_UNAVAILABLE}</p>
	{/if}
{/if}

<style>
	/* Same recipe as ServiceRow.svelte's `.row`/`.row-actions` — three columns
	   (name / pill / action) rather than that row's four: MySQL has no
	   generically-useful "endpoint" column before it is Ready (nothing to show
	   yet), and once Ready the socket path lives in the credentials block
	   below instead, which needs more room than a table cell gives it. */
	.row {
		display: grid;
		align-items: center;
		gap: var(--vh-space-4);
		padding: 10px var(--vh-space-4);
		border-bottom: 1px solid var(--vh-border);
	}
	.row:last-child {
		border-bottom: 0;
	}
	.row:hover {
		background: color-mix(in oklab, var(--vh-text) 3%, var(--vh-surface));
	}
	/* Was `.mysql-row`: renamed engine-neutral now that this row is shared
	   (design D1). Purely a styling hook — no test asserts this class name,
	   only `data-testid`s, which keep their own `mysql-`/`mariadb-` prefix via
	   `descriptor.idPrefix` instead. */
	.engine-row {
		grid-template-columns: minmax(160px, 0.6fr) 120px auto;
	}
	.pill-cell {
		min-width: 0;
	}
	/* Wraps since the source badge landed: at a 380px panel this cell can hold
	   "MySQL 9.7" + "Not managed" + "OpenVHost 8.4.11", which on one nowrap line
	   would push the row's action column off-screen — the exact failure the
	   status-bar slice had to fix once already. */
	.primary {
		font-weight: 600;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px 8px;
		min-width: 0;
	}
	.primary .version {
		white-space: nowrap;
	}
	/* Shared chip base. `.unmanaged` used to carry all of this alone; the
	   source badge needs the identical box, so it is a base plus two modifiers
	   rather than a second copy that can drift. */
	.badge {
		display: inline-flex;
		align-items: center;
		flex-shrink: 0;
		white-space: nowrap;
		padding: 1px 8px;
		border-radius: var(--vh-radius-pill);
		font-size: var(--vh-text-caption);
		font-weight: 600;
		color: var(--vh-text-2);
		background: var(--vh-surface-2);
		border: 1px solid var(--vh-border);
	}
	/* `gap` since the uninstall slice: an installed row can now hold two
	   controls (the lifecycle action and Uninstall) rather than one. */
	.row-actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--vh-space-2);
	}
	/* Plain secondary text, not a tinted box — this is a fact, not an alarm
	   (the Homebrew disclosure, the out-of-catalogue explanation, "install
	   Homebrew above"). `--vh-text-2` on `--vh-surface` is the same pairing
	   `.field .hint` and `.empty p` already use everywhere in this app. */
	.note {
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
	}
	.mono {
		font-family: var(--vh-font-mono);
	}
	/* Provenance, not status: a quiet outline chip rather than a filled pill,
	   so it reads as metadata beside the version and never competes with the
	   StatusPill in the next column. The packaged variant borrows the link
	   accent to say "this one is ours"; Homebrew keeps the neutral `.badge`
	   treatment `.unmanaged` already uses. */
	/* Provenance, not status: a quieter weight than `.unmanaged` so it reads as
	   metadata beside the version rather than competing with the StatusPill in
	   the next column. */
	.badge.source {
		font-weight: 500;
		letter-spacing: 0.01em;
	}
	/* The packaged chip borrows the link accent to say "this one is ours".
	   `--vh-link` is brand-700, the same token `.link-button` already uses on
	   this surface; Homebrew keeps the neutral base. */
	.badge.source-packaged {
		color: var(--vh-link);
		border-color: color-mix(in oklab, var(--vh-link) 35%, transparent);
		background: color-mix(in oklab, var(--vh-link) 8%, transparent);
	}
	/* The live pipeline line. Tabular numerals so a byte counter ticking up
	   does not shuffle the words after it left and right on every event. */
	.note.progress {
		font-variant-numeric: tabular-nums;
	}
	.bar {
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		height: 4px;
		border-radius: 2px;
		background: var(--vh-surface-2);
		overflow: hidden;
	}
	.bar .fill {
		display: block;
		height: 100%;
		background: var(--vh-link);
		/* Width only — a transform-based fill would need a wrapper to avoid
		   scaling the rounded ends, and 4px of bar is not worth that. The
		   transition is short enough to read as "it moved", not as animation. */
		transition: width var(--vh-dur-fast) linear;
	}
	/* Amber "needs attention, not fixed" tone — the vouched pairing
	   `ScaffoldNoticeBanner.svelte` uses (`--vh-start` directly on plain
	   `--vh-surface`, no tint), which measures 4.68:1. Reused verbatim rather
	   than a new tint, per this task's own instruction to check the numbers
	   rather than reach for the sibling-banner tint recipe (see
	   `MysqlCredentials.svelte`'s `.error`/`.ok` comment for the full
	   contrast-checking record). */
	.note.warn {
		color: var(--vh-start);
		border: 1px solid color-mix(in oklab, var(--vh-start) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
	}
	.error {
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		font-size: var(--vh-text-table);
	}
	.pool-stderr {
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		padding: var(--vh-space-2) var(--vh-space-3);
		background: var(--vh-log-bg);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		color: var(--vh-text);
		font-size: var(--vh-text-log);
		line-height: 1.6;
		overflow: auto;
		max-height: 320px;
		white-space: pre-wrap;
	}
</style>
