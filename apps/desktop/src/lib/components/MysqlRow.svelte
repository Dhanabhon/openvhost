<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type {
		MysqlConnectionProofDto,
		MysqlInstallOutcomeDto,
		MysqlInstanceDto,
		MysqlResetOutcomeDto,
		ServiceStatus
	} from '$lib/ipc';
	import {
		HOMEBREW_DATADIR_DISCLOSURE,
		mysqlInitStepLabel,
		mysqlPortConflictHint,
		mysqlRowState,
		unreachableMysqlRowState,
		type MysqlInitFailure,
		type UiLog
	} from '$lib/databases.derive';
	import Button from './Button.svelte';
	import LogPane from './LogPane.svelte';
	import MysqlCredentials from './MysqlCredentials.svelte';
	import StatusPill from './StatusPill.svelte';

	let {
		instance,
		brewFound,
		installingMajor,
		installLog,
		installOutcome,
		installError = '',
		initializingMajor,
		initLog,
		initFailure,
		initError = '',
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
		onInitialize,
		onStart,
		onStop,
		onReveal,
		onHide,
		onCopyPassword,
		onReset,
		onVerify
	}: {
		instance: MysqlInstanceDto;
		brewFound: boolean;
		/** The major installing anywhere on the page, '' when idle — shared
		 *  page-wide (one `InstallLock`), same as `LanguagesStore.installing`. */
		installingMajor: string;
		installLog: UiLog[];
		/** The last `install_mysql` outcome, whichever major it was for —
		 *  `MysqlInstallOutcomeDto` carries its own `major`, so this row only
		 *  renders it once it matches `instance.major` (mirrors
		 *  `LanguageRow.svelte`'s `rowOutcome`). */
		installOutcome: MysqlInstallOutcomeDto | null;
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
		onInitialize: (major: string) => void;
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

	const anyInstallOrInitRunning = $derived(installingMajor !== '' || initializingMajor !== '');

	const rowInstallOutcome = $derived(
		installOutcome !== null && installOutcome.major === instance.major ? installOutcome : null
	);
	/** Brew exited 0 but the version was not found afterwards — same case
	 *  `LanguageRow.svelte`'s `notFound` covers, for the identical reason. */
	const installNotFound = $derived(
		rowInstallOutcome !== null && rowInstallOutcome.exitCode === 0 && !rowInstallOutcome.detected
	);
	/** A non-zero exit (or `null`, killed by a signal) is an OUTCOME to render,
	 *  never a thrown `error` — same C1 audit finding `LanguageRow.svelte`
	 *  fixed for PHP. */
	const rowInstallFailed = $derived(rowInstallOutcome !== null && rowInstallOutcome.exitCode !== 0);

	const portConflictHint = $derived(
		serviceState?.kind === 'failed' ? mysqlPortConflictHint(serviceState.stderrTail) : null
	);

	// Named `rowState`, not `state`: svelte-check's TS layer (svelte2tsx) gets
	// confused between a plain local binding named `state` and the `$state`
	// rune in the SAME script block — it reported `$state` as "used before its
	// declaration" and tried to treat `state` as a Svelte store, neither of
	// which is real. Renaming the variable is the whole fix.
	const rowState = $derived(
		mysqlRowState({
			brewFound,
			instance,
			installingMajor,
			installLog,
			initializingMajor,
			initLog,
			initFailure
		})
	);
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
	<div class="row mysql-row" data-testid="mysql-row-{instance.major}">
		<div class="primary">
			<span class="version">MySQL {instance.major}</span>
			<span class="badge unmanaged">Not managed</span>
		</div>
		<div class="pill-cell">
			{#if serviceState}
				<StatusPill kind={serviceState.kind} testId="mysql-pill-{instance.major}" />
			{/if}
		</div>
		<div class="row-actions"></div>
	</div>
	<p class="note" data-testid="out-of-catalogue-{instance.major}">
		MySQL {instance.major} is installed, but this build only manages MySQL {catalogedMajorsList.join(
			', '
		)}. Shown for visibility only — no actions are offered here.
	</p>
{:else}
	<div class="row mysql-row" data-testid="mysql-row-{instance.major}">
		<div class="primary">
			<span class="version">MySQL {instance.major}</span>
		</div>
		<div class="pill-cell">
			{#if serviceState}
				<StatusPill kind={serviceState.kind} testId="mysql-pill-{instance.major}" />
			{/if}
		</div>
		<div class="row-actions">
			{#if rowState.kind === 'noBrew' || rowState.kind === 'datadirForeign'}
				<!-- Nothing to offer: no brew to install through, or a foreign
				     datadir this app will not touch. -->
			{:else if rowState.kind === 'notInstalled'}
				<Button
					variant="primary"
					size="sm"
					testId="install-{instance.major}"
					ariaLabel="Install MySQL {instance.major}"
					disabled={anyInstallOrInitRunning}
					onclick={() => onInstall(instance.major)}
				>
					Install
				</Button>
			{:else if rowState.kind === 'installing'}
				<Button variant="primary" size="sm" disabled onclick={() => {}}>Installing…</Button>
			{:else if rowState.kind === 'installedNotInitialized' || rowState.kind === 'initFailed'}
				<Button
					variant="primary"
					size="sm"
					testId={rowState.kind === 'initFailed'
						? `retry-init-${instance.major}`
						: `initialize-${instance.major}`}
					ariaLabel="Initialize MySQL {instance.major}"
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
							ariaLabel="Retry MySQL {instance.major}"
							onclick={() => onStart(instance.serviceId ?? '')}>Retry</Button
						>
					{:else if serviceState.kind === 'stopped'}
						<Button
							variant="quiet"
							size="sm"
							testId="start-{instance.serviceId}"
							ariaLabel="Start MySQL {instance.major}"
							onclick={() => onStart(instance.serviceId ?? '')}>Start</Button
						>
					{:else}
						<Button
							variant="quiet"
							size="sm"
							testId="stop-{instance.serviceId}"
							ariaLabel="Stop MySQL {instance.major}"
							onclick={() => onStop(instance.serviceId ?? '')}>Stop</Button
						>
					{/if}
				{/if}
			{:else}
				{unreachableMysqlRowState(rowState)}
			{/if}
		</div>
	</div>

	{#if rowState.kind === 'noBrew'}
		<p class="note" data-testid="mysql-no-brew-note-{instance.major}">
			Install Homebrew above to continue.
		</p>
	{:else if rowState.kind === 'notInstalled'}
		<p class="note" data-testid="disclosure-{instance.major}">{HOMEBREW_DATADIR_DISCLOSURE}</p>
		{#if installError !== ''}
			<p class="error" role="alert" style="white-space: pre-wrap">{installError}</p>
		{/if}
		{#if rowInstallFailed}
			<p class="error" role="alert">
				{#if rowInstallOutcome?.exitCode !== null && rowInstallOutcome?.exitCode !== undefined}
					brew exited with code {rowInstallOutcome.exitCode} while installing MySQL {instance.major}.
				{:else}
					brew was killed before installing MySQL {instance.major} finished.
				{/if}
				Check the log above for what brew actually did.
			</p>
		{:else if installNotFound}
			<p class="note warn" role="alert">
				Homebrew reported success installing MySQL {instance.major}, but the version was not found
				afterwards. Check the log above for what brew actually did.
			</p>
		{/if}
	{:else if rowState.kind === 'installing'}
		<p class="note" data-testid="disclosure-{instance.major}">{HOMEBREW_DATADIR_DISCLOSURE}</p>
		{#if rowState.log.length > 0}
			<LogPane logs={rowState.log} />
		{/if}
	{:else if rowState.kind === 'installedNotInitialized'}
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
			MySQL {instance.major}'s data directory already has unexpected content and OpenVHost will not
			touch it: <span class="mono">{rowState.detail}</span>. Once it looks like an empty MySQL {instance.major}
			data directory again, use Check again above.
		</p>
	{:else if rowState.kind === 'ready'}
		{#if serviceState?.kind === 'failed'}
			<p class="error" role="alert" data-testid="pool-failed-{instance.serviceId}">
				MySQL {instance.major} failed{#if serviceState.exit !== null}&nbsp;(exit {serviceState.exit}){/if}.
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
	.mysql-row {
		grid-template-columns: minmax(160px, 0.6fr) 120px auto;
	}
	.pill-cell {
		min-width: 0;
	}
	.primary {
		font-weight: 600;
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
	}
	.primary .version {
		white-space: nowrap;
	}
	.badge.unmanaged {
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
	.row-actions {
		display: flex;
		justify-content: flex-end;
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
