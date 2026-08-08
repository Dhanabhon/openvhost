<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type {
		PhpInstallOutcomeDto,
		PhpInstallProgressDto,
		PhpRuntimeDto,
		ServiceStatus
	} from '../ipc';
	import type { UiLog } from '../languages.svelte';
	import {
		phpInstallOffered,
		phpInstallProgressLabel,
		phpInstallProgressPercent,
		phpNoRouteNote,
		phpOutcomeRender,
		phpSourceBadge
	} from '../php-install.derive';
	import {
		DEFAULT_BADGE_LABEL,
		DEFAULT_BADGE_TITLE,
		makeDefaultLabel,
		makeDefaultTitle
	} from '../php-default.derive';
	import {
		offersUninstall,
		outOfCatalogueNote,
		uninstallActionDisabled,
		uninstallConfirmLabel
	} from '../uninstall.derive';
	import Button from './Button.svelte';
	import LogPane from './LogPane.svelte';
	import StatusPill from './StatusPill.svelte';

	let {
		row,
		cataloged,
		brewFound,
		serviceState,
		installing = '',
		uninstalling = '',
		log = [],
		error = '',
		outcome = null,
		installProgress = null,
		installTotal = null,
		isDefault = false,
		offersDefault = false,
		settingDefault = '',
		onInstall,
		onUninstall,
		onMakeDefault,
		onStart,
		onStop
	}: {
		row: PhpRuntimeDto;
		/** Whether THIS BUILD manages the version — the same fact
		 *  `MysqlInstanceDto.cataloged` carries for a MySQL row.
		 *
		 *  A separate prop rather than a field read off `row` inside the markup,
		 *  for one reason: it is the input to a DECISION (`offersUninstall`), and
		 *  keeping it explicit means a row rendered without it fails to compile
		 *  rather than defaulting to "managed" — which is how the dead Uninstall
		 *  button reached a user in the first place.
		 *
		 *  `false` happens for an installed major outside the catalogue: a
		 *  hand-installed `php@7.4`, or one a later catalogue drops. Such a row
		 *  is deliberately still LISTED (spec §6.1: it may still be serving
		 *  sites) and still gets its Start/Stop control, because the pool is real
		 *  — it just gets no Uninstall, because `Target::parse` would refuse the
		 *  major and the dialog would open only to say so. */
		cataloged: boolean;
		/** Whether Homebrew is on this machine (off-Homebrew slice 5C design
		 *  D2/D5). A required prop for the same reason `cataloged` is one: it is
		 *  an input to a DECISION — whether this row's Install button can work at
		 *  all — so a row rendered without it must fail to compile rather than
		 *  default to "yes, brew is there".
		 *
		 *  It does NOT decide alone. `phpInstallOffered` pairs it with this row's
		 *  own `offer`, because the question is per-major: an `Available` 8.4
		 *  needs no Homebrew, while an `Unavailable` 8.1 needs it permanently.
		 *  Answering that with one machine-wide bool is precisely what D2
		 *  removes from the page above. */
		brewFound: boolean;
		/** The whole supervised state, not just whether it is running: `failed`
		 *  carries the `stderrTail` this row renders, and a boolean cannot express
		 *  it. Read from the shared services store by the caller, never tracked a
		 *  second time here.
		 *
		 *  `null` means the snapshot has not arrived yet, OR this row has no pool
		 *  at all. Both render no service control — but the not-installed row is
		 *  caught earlier by the `!row.installed` branch, which renders Install. */
		serviceState: ServiceStatus['state'] | null;
		/** The major installing anywhere on the page, '' when idle. Disables
		 *  every install button, not just this row's — only one install can run
		 *  at a time (`LanguagesStore.install`'s own re-entrancy guard). */
		installing?: string;
		/** The major being UNINSTALLED anywhere in the app, '' when idle — a
		 *  state, not a boolean, for the same reason `installing` is one: this
		 *  row has to tell "somebody else is busy" (disabled) from "it is me"
		 *  (disabled AND labelled "Uninstalling…"), and a boolean can express
		 *  only one of those. */
		uninstalling?: string;
		log?: UiLog[];
		error?: string;
		/** The last install attempt's outcome, whichever major it was for — this
		 *  row only renders it once `outcome.major` matches its own. A TAGGED
		 *  result since design D4: only the `brew` arm carries an `exitCode`,
		 *  because only that route runs a child process. */
		outcome?: PhpInstallOutcomeDto | null;
		/** This row's own live pipeline state on the PACKAGED route, already
		 *  scoped to this major by `LanguagesStore.progressFor`.
		 *
		 *  `null` is the only value a Homebrew machine can ever see (spec §8.6):
		 *  `php-install-progress` is emitted solely by `run_package_install`, and
		 *  `run_brew_install` streams `php-install-log` instead. So the block this
		 *  prop gates renders nothing at all on every real machine today — which
		 *  is why it is a display prop with a truthful `null` default rather than
		 *  a required one like `brewFound`: nothing DECIDES on it, and forgetting
		 *  to pass it degrades to exactly the behaviour that shipped before it
		 *  existed. */
		installProgress?: PhpInstallProgressDto | null;
		/** The length the server declared for this download, `null` when it
		 *  declared none. Read only alongside {@link installProgress}. */
		installTotal?: number | null;
		/** Whether THIS major is the one the user chose. Computed page-side by
		 *  `isChosenDefault`, never derived here from `row`: the answer depends on
		 *  a resolution the row cannot see, and a row that guessed would badge the
		 *  major that merely sorted first — the accident this whole slice exists
		 *  to stop presenting as a decision. */
		isDefault?: boolean;
		/** Whether the page offers a way to choose at all (`offersDefaultChoice`).
		 *  Passed in rather than inferred from `isDefault`, because "no badge" and
		 *  "no choice on this machine" are different facts: a two-PHP machine with
		 *  nothing chosen shows no badge on any row and must still offer the
		 *  control on both. */
		offersDefault?: boolean;
		/** The major whose choice is in flight, or `''`. Same shape as
		 *  `installing`/`uninstalling` above, so one row's pending write disables
		 *  the control on every row rather than only its own. */
		settingDefault?: string;
		onInstall: (major: string) => void;
		/** Opens the uninstall confirmation (package-uninstall design D6). Never
		 *  uninstalls anything on its own — nothing is spawned until the dialog's
		 *  own confirm, and the plan it shows is a pure query. */
		onUninstall: (major: string) => void;
		onMakeDefault: (major: string) => void;
		onStart: (serviceId: string) => void;
		onStop: (serviceId: string) => void;
	} = $props();

	const isInstalling = $derived(installing === row.major);
	/** Installed AND managed by this build. The second half is the fix for a
	 *  button that opened a dialog only to report a refusal — see
	 *  `offersUninstall`'s own doc comment. */
	const canUninstall = $derived(offersUninstall({ installed: row.installed, cataloged }));
	/** Page-wide, not per-row: one `InstallLock` serializes brew installs and
	 *  brew uninstalls alike, so a second action would only queue on a mutex.
	 *  Includes this row's own uninstall, so a double-click cannot fire twice. */
	const uninstallDisabled = $derived(
		uninstallActionDisabled({
			installingMajor: installing,
			initializingMajor: '',
			uninstallingMajor: uninstalling
		})
	);
	const rowOutcome = $derived(outcome && outcome.major === row.major ? outcome : null);
	/** Every arm of the settled result, classified once (design D4). NOT a
	 *  `result.kind === 'brew'` test: that would leave the eight packaged arms
	 *  rendering nothing at all — no error, no warning, no success — which is
	 *  the C1 defect this row already fixed once for brew's own non-zero exit.
	 *  `phpOutcomeRender` ends in `const unreachable: never`, so a ninth arm
	 *  fails to compile there instead of silently rendering as silence here. */
	const settled = $derived(
		rowOutcome === null ? null : phpOutcomeRender(rowOutcome.result, row.major)
	);
	/** The failure line. For the brew route this is still exactly what it was —
	 *  a non-zero exit, or `null` for a signal kill, is an OUTCOME to render and
	 *  not a thrown error, so `error` above never carries it. */
	const installFailed = $derived(settled?.alert ?? null);
	/** It reported success and the runtime still is not there. Silence here is
	 *  the failure it prevents: without this message the user just presses
	 *  Install again with nothing explaining why nothing happened. */
	const notFound = $derived(settled?.warning ?? null);
	/** Paired with the row's own `installed`, so a claim of success is never made
	 *  about a row the environment re-read did not confirm. */
	const justInstalled = $derived((settled?.succeeded ?? false) && row.installed);
	/** Provenance, not status (design D3). `null` for a Homebrew keg, which is
	 *  why nothing new appears on a machine that has only ever used brew. */
	const sourceBadge = $derived(phpSourceBadge(row.source));
	/** Whether Install could actually work for THIS major on THIS machine — the
	 *  row's own offer paired with `brewFound`, never one or the other alone. */
	const installOffered = $derived(phpInstallOffered(row.offer, brewFound));
	/** …and why not, when it could not. Absent affordance, present explanation. */
	const noRouteNote = $derived(phpNoRouteNote(row.offer, row.major, brewFound));
	/** `null` whenever there is nothing honest to draw — before the first byte,
	 *  with no declared length, and for the three steps after the download, which
	 *  are moments rather than durations. */
	const installPercent = $derived(
		installProgress === null ? null : phpInstallProgressPercent(installProgress, installTotal)
	);
</script>

<div class="row lang-row" data-testid="lang-row-{row.major}">
	<!-- The label is its own element rather than a bare text node so it can carry
	     `white-space: nowrap`. As a bare node it was the only flexible thing in
	     this cell, so the badge's width came straight out of it and "PHP 8.5"
	     broke across two lines — on the recommended row only, which is why every
	     other row looked fine. -->
	<div class="primary">
		<span class="version">PHP {row.major}</span>
		{#if row.recommended}
			<span class="badge recommended">Recommended</span>
		{/if}
		<!-- WHICH INSTALL put these binaries here (design D3). Absent when nothing
		     is installed, and absent for a Homebrew keg — unlike MysqlRow's and
		     WebServerRow's otherwise identical chips, which do label their brewed
		     runtimes. See `phpSourceBadge` for why this one does not: a chip on all
		     five rows of a brew-only machine would say the same thing five times,
		     and it would be a visible change to every real machine today, which
		     spec §8.6 forbids. Kept beside the version, never beside the status
		     pill in the next cell, so it cannot read as a second status. -->
		{#if sourceBadge}
			<span
				class="badge source source-{row.source?.kind}"
				title={sourceBadge.title}
				data-testid="php-source-{row.major}">{sourceBadge.label}</span
			>
		{/if}
		<!-- WHICH major the catch-all serves, and only when it was CHOSEN. Never
		     rendered for `unset`'s serving major or for what a missing preference
		     falls back to — see `isChosenDefault`, which is where that rule lives
		     and is matched exhaustively. Beside the version like the source chip,
		     never beside the status pill, so it cannot read as a second status. -->
		{#if isDefault}
			<span class="badge default" title={DEFAULT_BADGE_TITLE} data-testid="php-default-{row.major}"
				>{DEFAULT_BADGE_LABEL}</span
			>
		{/if}
	</div>

	<!-- Renders nothing when the state is unknown, the same `{#if}` guard
	     WebServerRow uses: an absent snapshot is not a state to name. -->
	<div class="pill-cell">
		{#if serviceState}
			<StatusPill kind={serviceState.kind} testId="lang-pill-{row.major}" />
		{/if}
	</div>

	<!-- `title` so the full value stays reachable when the cell ellipsizes, same
	     reasoning as ServiceRow.svelte's endpoint column. -->
	<div class="meta mono path" title={row.path ?? undefined}>{row.path ?? '—'}</div>

	<!-- The socket is the first thing anyone needs when a site 502s — plain
	     selectable text rather than a copy-button-only affordance, so it works
	     even before any "copy" UI exists. -->
	<div class="meta mono socket" title={row.socketPath ?? undefined}>{row.socketPath ?? '—'}</div>

	<div class="row-actions">
		{#if !row.installed}
			<!-- Offered only where it could actually work (design D2/D4). With no
			     Homebrew, `install_php` on an `AwaitingRelease` or `Unavailable`
			     offer fails at `find_brew()` before anything is spawned, so the
			     button's only possible outcome would be "Homebrew was not found" —
			     the affordance-that-can-only-fail this codebase keeps deleting. The
			     note below the row says what is missing instead. An `Available`
			     offer keeps its button with or without brew: that row is ours. -->
			{#if installOffered}
				<Button
					variant="primary"
					size="sm"
					testId="install-{row.major}"
					ariaLabel="Install PHP {row.major}"
					disabled={installing !== ''}
					onclick={() => onInstall(row.major)}
				>
					{isInstalling ? 'Installing…' : 'Install'}
				</Button>
			{/if}
		{:else}
			{#if row.serviceId && serviceState !== null}
				{#if serviceState.kind === 'failed'}
					<Button
						variant="quiet"
						size="sm"
						testId="retry-{row.serviceId}"
						ariaLabel="Retry PHP {row.major}"
						onclick={() => onStart(row.serviceId ?? '')}>Retry</Button
					>
				{:else if serviceState.kind === 'stopped'}
					<Button
						variant="quiet"
						size="sm"
						testId="start-{row.serviceId}"
						ariaLabel="Start PHP {row.major}"
						onclick={() => onStart(row.serviceId ?? '')}>Start</Button
					>
				{:else}
					<Button
						variant="quiet"
						size="sm"
						testId="stop-{row.serviceId}"
						ariaLabel="Stop PHP {row.major}"
						onclick={() => onStop(row.serviceId ?? '')}>Stop</Button
					>
				{/if}
			{/if}
			{#if canUninstall}
				<!-- Last in the row, after the daily Start/Stop control: this is the
				     rare, destructive one, and it opens a confirmation rather than
				     doing anything (design D6). Offered for every installed major
				     THIS BUILD MANAGES, including one whose pool has no supervisor
				     row yet — that state is exactly when a user is most likely to
				     want it gone. An out-of-catalogue major gets the note below
				     instead of a button that could only report a refusal. -->
				<Button
					variant="quiet"
					size="sm"
					testId="uninstall-{row.major}"
					ariaLabel="Uninstall PHP {row.major}"
					disabled={uninstallDisabled}
					onclick={() => onUninstall(row.major)}
				>
					{uninstallConfirmLabel(uninstalling === row.major)}
				</Button>
			{/if}
			<!-- Offered only on an installed row that is not already the default,
			     and only when the page offers the choice at all. The row that IS
			     the default carries the badge instead: a control whose only effect
			     is to store what is already stored is the affordance-that-changes-
			     nothing this page keeps having to delete. -->
			{#if offersDefault && !isDefault}
				<Button
					variant="quiet"
					size="sm"
					testId="make-default-{row.major}"
					ariaLabel={makeDefaultTitle(row.major)}
					disabled={settingDefault !== ''}
					onclick={() => onMakeDefault(row.major)}
				>
					{makeDefaultLabel(settingDefault === row.major)}
				</Button>
			{/if}
		{/if}
	</div>
</div>

{#if !row.installed && noRouteNote !== null}
	<!-- Why this row has no Install button. PER ROW, never page-wide (design
	     D2): on a machine with a packaged 8.4 and no Homebrew, 8.4 installs and
	     8.1/8.3/8.5 do not, and one sentence at the top of the page cannot be
	     true of both. Same neutral secondary-text treatment as the
	     out-of-catalogue note below and for the same reason — `Unavailable` is
	     the ordinary state of four majors out of five, and every major on Intel.
	     Nothing here is broken; something is simply not installed. -->
	<p class="note" data-testid="php-no-route-{row.major}">{noRouteNote}</p>
{/if}

{#if row.installed && !cataloged}
	<!-- Why the Uninstall button is not there. An absent affordance with no
	     explanation is this page's own recurring failure (C2/C3): the user is
	     left pressing nothing and learning nothing. `MysqlRow.svelte` says the
	     equivalent for an unmanaged MySQL instance; unlike that row, this one
	     keeps its Start/Stop control, because the pool IS real and supervised —
	     only the removal has no in-app path. -->
	<p class="note" data-testid="php-out-of-catalogue-{row.major}">
		{outOfCatalogueNote('php', row.major)}
	</p>
{/if}

{#if isInstalling && installProgress !== null}
	<!-- The packaged route's pipeline, the only progress feedback that route has:
	     it spawns no child process, so `LogPane` below stays empty for the whole
	     of it — brew's output is a different channel entirely. Without this the
	     day a release is published a user presses Install and watches nothing
	     happen for the length of a download.

	     `verified` and `extracted` are SEPARATE sentences on purpose (see
	     `phpInstallProgressLabel`): a download that was checked against the
	     built-in SHA-256 and one that merely arrived must never look identical,
	     which is the whole of what golden rule 6 buys.

	     BOTH HALVES OF THIS CONDITION ARE LOAD-BEARING, for different reasons,
	     and it is written once rather than split into a named derived precisely
	     so neither can go quietly redundant:

	      * `installProgress !== null` is what keeps spec §8.6 true. A brew
	        install sets `installing` to this major and never emits a single
	        progress event — `php-install-progress` has one emitter,
	        `run_package_install` — so gating on `isInstalling` alone would put a
	        download line under every Install press on every real machine today.
	      * `isInstalling` is what makes the line go away when the run settles.
	        `install()`'s `finally` clears `installing` before the settled outcome
	        renders, so without it a finished "Installed —…" would sit above the
	        success message for the rest of the page's life. -->
	<p class="note progress" data-testid="php-install-progress-{row.major}">
		{phpInstallProgressLabel(installProgress, installTotal)}
	</p>
	{#if installPercent !== null}
		<div
			class="bar"
			role="progressbar"
			aria-label="Downloading PHP {row.major}"
			aria-valuemin={0}
			aria-valuemax={100}
			aria-valuenow={installPercent}
			data-testid="php-install-bar-{row.major}"
		>
			<span class="fill" style="width: {installPercent}%"></span>
		</div>
	{/if}
{/if}

{#if log.length > 0}
	<!-- C1 fix: no longer gated on `isInstalling`. `install()`'s `finally` resets
	     `installing` to '' BEFORE this row re-renders with the settled outcome, so
	     gating on it made the log vanish at the exact moment — a failed or killed
	     install — a user most needs to read it. `log` is already scoped to this
	     row's own major (`store.logFor`) and only cleared at the START of this
	     row's NEXT attempt, so it safely survives here on its own. -->
	<LogPane logs={log} />
{/if}

{#if error !== ''}
	<!-- `white-space: pre-wrap` ALSO set inline, duplicating the scoped `.error`
	     rule below — same reason as ApplyDialog.svelte: the SSR test harness
	     (`svelte/server`) never sees scoped styles, only this inline copy. -->
	<p class="error" role="alert" style="white-space: pre-wrap">{error}</p>
{/if}

{#if installFailed !== null}
	<!-- C1 fix: brew's own non-zero exit (or a signal kill, `exitCode === null`)
	     used to render NOTHING — no error (nothing threw), no `notFound` (that
	     branch requires `exitCode === 0`), and by then the log had already been
	     hidden by the `isInstalling` gate above. Spec §6 calls this a "Failed" row
	     state; this is it. Since design D4 the wording lives in
	     `phpOutcomeRender`, which answers for the packaged arms too so that none
	     of them can reintroduce the same silence. -->
	<p class="error" role="alert">{installFailed}</p>
{:else if notFound !== null}
	<p class="warn" role="alert">{notFound}</p>
{/if}

{#if serviceState?.kind === 'failed'}
	<!-- The supervisor's captured stderr is the only thing that explains why a
	     start did not take. Verbatim — a php-fpm startup error names the pool
	     file and the directive that broke, and summarising it would throw away
	     the part that fixes the problem. Same treatment WebServerRow gives a
	     failed nginx. -->
	<p class="error" role="alert" data-testid="pool-failed-{row.serviceId}">
		PHP {row.major}'s pool failed{#if serviceState.exit !== null}&nbsp;(exit {serviceState.exit}){/if}.
	</p>
	{#if serviceState.stderrTail.length > 0}
		<pre class="pool-stderr">{serviceState.stderrTail.join('\n')}</pre>
	{/if}
{/if}

{#if justInstalled}
	<p class="ok" role="status">
		Installed PHP {row.fullVersion ?? row.major}. Apply your sites to start its pool.
	</p>
{/if}

<style>
	/* Same recipe as ServiceRow.svelte's `.row`/`.row-actions` — four data columns
	   (version / path / socket) plus a fixed action column, instead of that row's
	   three, because an installed PHP row has more to show than a running service
	   does. */
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
	.lang-row {
		/* First column min raised 120px -> 190px: it has to hold "PHP 8.5", an
		   8px gap and the ~106px Recommended badge without either wrapping. At
		   120px the badge's width was taken out of the label. The path and
		   socket columns already ellipsize, so the space comes from there. */
		grid-template-columns:
			minmax(190px, 0.6fr) 120px minmax(180px, 1.4fr) minmax(180px, 1.4fr)
			auto;
	}
	.pill-cell {
		min-width: 0;
	}
	.primary {
		font-weight: 600;
		display: flex;
		/* Wraps since the source badge landed, exactly as MysqlRow.svelte's
		   identical comment records doing once already: this cell can now hold
		   "PHP 8.4" + "Recommended" + "OpenVHost 8.4.24", which on one nowrap
		   line would push the row's action column off-screen — the failure the
		   status-bar slice and the responsive slice have each had to fix. Inert
		   until a packaged row exists: with the two chips this cell held before,
		   the content still fits the 190px track and nothing wraps. */
		flex-wrap: wrap;
		align-items: center;
		gap: 6px 8px;
		min-width: 0;
	}
	.primary .version {
		white-space: nowrap;
	}
	/* Shared chip base, transcribed line-for-line from MysqlRow.svelte's own
	   `.badge` rather than approximated — the same discipline 4C's review
	   applied to WebServerRow's copy of it. Svelte scopes styles per component,
	   so a third literal copy is the only way three components can share one
	   look; extracting a shared component would change the class attribute (and
	   therefore the markup) of two already-shipped, already-tested rows.
	   `.recommended` below overrides the palette at a higher specificity, so
	   its rendering is byte-for-byte what it was before the base existed. */
	.badge {
		display: inline-flex;
		align-items: center;
		/* Never absorb the shortfall by squashing: if this cell is ever too
		   narrow the badge keeps its size and the row scrolls, rather than the
		   badge silently collapsing into an unreadable sliver. */
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
	.badge.recommended {
		color: var(--vh-accent);
		background: var(--vh-selected);
		border: 1px solid color-mix(in oklab, var(--vh-accent) 35%, transparent);
	}
	/* Provenance, not status: a quieter weight than `.recommended` so it reads
	   as metadata beside the version rather than competing with the StatusPill
	   in the next column. */
	.badge.source {
		font-weight: 500;
		letter-spacing: 0.01em;
	}
	/* The packaged chip borrows the link accent to say "this one is ours".
	   `--vh-link` is brand-700, the same token MysqlRow.svelte's and
	   WebServerRow.svelte's identical chips use. Disjoint from every colour
	   StatusPill can paint (`--vh-run`/`--vh-start`/`--vh-fail`/`--vh-stop`),
	   and it carries no status dot, so it cannot read as a status beside one. */
	.badge.source-packaged {
		color: var(--vh-link);
		border-color: color-mix(in oklab, var(--vh-link) 35%, transparent);
		background: color-mix(in oklab, var(--vh-link) 8%, transparent);
	}
	/* A CHOICE the user made, so it borrows `--vh-accent` — the same token
	   `.recommended` uses, because both say "this one is singled out" — but at
	   the quieter `.badge.source` weight, since it sits beside the version as
	   metadata rather than announcing itself. Deliberately NOT `--vh-link`: the
	   source chip means "we built this one" and can appear on the same row, so
	   two chips sharing a colour would blur two unrelated facts. Disjoint from
	   every colour StatusPill paints (`--vh-run`/`--vh-start`/`--vh-fail`/
	   `--vh-stop`) and carries no dot, so it cannot read as a status. */
	.badge.default {
		font-weight: 500;
		letter-spacing: 0.01em;
		color: var(--vh-accent);
		border: 1px solid color-mix(in oklab, var(--vh-accent) 35%, transparent);
		background: color-mix(in oklab, var(--vh-accent) 8%, transparent);
	}
	.meta {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* `gap` since the uninstall slice: an installed row can now hold two
	   controls (Start/Stop and Uninstall), and without it they would touch. */
	.row-actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--vh-space-2);
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
	/* Same secondary-text note `MysqlRow.svelte` uses for its own unmanaged-row
	   explanation, so the two pages explain the same situation the same way.
	   Deliberately NOT the amber `.note.warn` treatment: nothing is wrong here
	   — an out-of-catalogue version keeps serving perfectly well; it just has
	   no in-app removal. */
	.note {
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
	}
	/* The live pipeline line. Tabular numerals so a byte counter ticking up does
	   not shuffle the words after it left and right on every event. Transcribed
	   line-for-line from `MysqlRow.svelte`'s own `.note.progress`/`.bar`/`.fill`
	   rather than approximated — the same discipline the source badge above
	   followed, and for the same reason: Svelte scopes styles per component, so a
	   literal copy is the only way two components can share one look without
	   changing the markup of an already-shipped, already-tested row. */
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
	.warn {
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		font-size: var(--vh-text-table);
	}
	.ok {
		color: var(--vh-run);
		background: var(--vh-add-tint);
		border: 1px solid color-mix(in oklab, var(--vh-run) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 var(--vh-space-4) var(--vh-space-3);
		font-size: var(--vh-text-table);
	}

	/* The one-line row costs 940px: the four track floors 190 + 120 + 180 + 180 = 670, four
	   16px gaps and the row's own 2x16px padding make 766, and the widest the action column
	   ever gets is 174. px tracks do not shrink, so below that the grid overflowed into
	   `.panel`'s `overflow: hidden`. Measured at the app's own smallest legal window (960,
	   leaving a row 694px): 191px of overflow, with Uninstall's LEFT edge already past the
	   panel — not clipped, gone. On a row that is not installed yet the same is true of
	   Install, so a small window could not install PHP at all.

	   174 is the widest action column, not the resting one, and the difference matters: it is
	   `Installing…` alone, which beats Stop + `Uninstalling…`. A threshold derived from
	   resting labels would have been 136 and wrong by 38px at exactly the moment a user is
	   watching an install.

	   The threshold is 970, not 940. Labels are text and this row's swing between resting and
	   busy is already 60px on one button, so the sum is only as stable as today's wording.
	   Wrapping 30px early is invisible; wrapping late puts a control off-screen. If you change
	   a track or a label above, change this with it, and keep the slack.

	   Flex rather than a second set of tracks: every cell keeps its exact width as a flex
	   basis, so the version, the pill and both paths still line up down the list, and the
	   wrap follows DOM order — no `order`, so focus order is untouched. */
	@container langlist (width < 970px) {
		.lang-row {
			display: flex;
			flex-wrap: wrap;
			/* Tighter between a row's own lines than between cells on one line, so the pair
			   still reads as one row against the 1px border that separates rows. */
			row-gap: var(--vh-space-2);
		}
		.lang-row > .primary {
			flex: 1 1 190px;
		}
		.lang-row > .pill-cell {
			flex: 0 0 120px;
		}
		/* Both keep `flex-shrink`, so a long path still ellipsizes — `.meta` already sets
		   `min-width: 0` and the ellipsis — rather than forcing another line. */
		.lang-row > .path,
		.lang-row > .socket {
			flex: 1 1 180px;
		}
		/* `flex-shrink: 0` is the load-bearing half: button labels are text and cannot be
		   squeezed, so the group claims its natural width and wraps instead. `flex-grow` then
		   hands it the rest of its line, which `justify-content: flex-end` keeps right-aligned
		   under the row's right edge exactly as on one line. */
		.lang-row > .row-actions {
			flex: 1 0 auto;
		}
	}
</style>
