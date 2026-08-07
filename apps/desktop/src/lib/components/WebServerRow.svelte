<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { ServiceStatus, ValidationReportDto, WebServerDto } from '$lib/ipc';
	import { WEB_SERVERS, type WebServerKind } from '$lib/sites.derive';
	import { hotReloadLabel, nginxSourceBadge, startStopFor } from '$lib/webservers.derive';
	import Button from './Button.svelte';
	import StatusPill from './StatusPill.svelte';
	import WebServerIcon from './WebServerIcon.svelte';

	// One brand's row. Everything arrives as this row's OWN slice of the store —
	// `WebServerPanel` indexes the per-id maps once, so a row can neither read a
	// neighbour's config text nor a neighbour's error.
	let {
		server,
		serviceState,
		configText,
		configError,
		report,
		validating,
		onShowConfig,
		onValidate,
		onStart,
		onStop
	}: {
		server: WebServerDto;
		/** The whole supervised state, not just its kind: `failed` carries the
		 *  `stderrTail` this row renders, and a kind alone cannot express it.
		 *  `null` when the brand is unsupervised OR the snapshot has not
		 *  arrived — both render no control (see `startStopFor`). */
		serviceState: ServiceStatus['state'] | null;
		configText?: string;
		configError: string;
		report: ValidationReportDto | null;
		validating: boolean;
		onShowConfig: (id: string) => void;
		onValidate: (id: string) => void;
		onStart: (serviceId: string) => void;
		onStop: (serviceId: string) => void;
	} = $props();

	// Derived locally so the existing pill (`{#if statusKind}`) and the
	// `startStopFor` call below keep the same meaning after `serviceState`
	// replaced the bare kind: only this line needed to change.
	const statusKind = $derived(serviceState?.kind ?? null);

	// `server.serviceId` is null for a brand OpenVHost does not supervise
	// (Apache), which is a different "no control" from "state not yet known" —
	// both render nothing, but only one of them can ever change.
	const control = $derived(
		server.serviceId === null
			? { kind: 'none' as const }
			: startStopFor(statusKind, server.configExists)
	);

	/** Which install put this binary here (nginx source design D1) — `null`
	 *  for the Apache row and whenever no nginx was found. */
	const sourceBadge = $derived(nginxSourceBadge(server.source));

	/** Shown for any fact the backend could not fill in — never an empty gap the
	 * reader cannot interpret. Reachable for `version` whenever the probe fails;
	 * reachable for the paths only if a supported brand ever has none resolved,
	 * which today's `StackPaths` always does supply. */
	const UNKNOWN = 'Unknown';

	// Local view state, not store state: whether this row's config is revealed is
	// nobody else's business. It only ever records a DELIBERATE collapse — the
	// presence of `configText` is what reveals the block — which is also what lets
	// the SSR test assert the revealed markup without driving a click.
	let collapsed = $state(false);

	// The brand mark is drawn only for a brand we actually have artwork for; an id
	// outside the known list renders no mark rather than the wrong one.
	const brand = $derived<WebServerKind | null>(
		(WEB_SERVERS as readonly string[]).includes(server.id) ? (server.id as WebServerKind) : null
	);

	// PRECEDENCE: a read error hides the config text. The store clears
	// `configError[id]` when a read starts but never clears `configText[id]`, so a
	// row that read its config once and then failed a RE-read holds stale text and
	// a fresh error at the same time. Rendering both would put a `<pre>` claiming
	// to be the contents of a file directly beside a message saying that file could
	// not be read — a straight contradiction, and the `<pre>` is the half that is
	// wrong. The report is deliberately NOT suppressed the same way (see below): a
	// failed read does not invalidate a validator run that completed.
	const showConfig = $derived(configText !== undefined && configError === '' && !collapsed);
	const configId = $derived(`ws-config-${server.id}`);

	function toggleConfig(): void {
		if (showConfig) {
			collapsed = true;
			return;
		}
		// Re-reads instead of un-collapsing cached text: the file can change on disk
		// between two looks and this page exists to show what is there NOW. It is
		// also the retry path after a failed read, since the store clears this row's
		// error when the next read starts.
		collapsed = false;
		onShowConfig(server.id);
	}
</script>

<div class="row ws-row" class:muted={!server.supported} data-testid="ws-{server.id}">
	<div class="ws-head">
		<h3 class="primary">
			{#if brand}<WebServerIcon server={brand} />{/if}
			{server.displayName}
			<!-- WHICH INSTALL this binary came from (design D1). Absent for
			     Apache and whenever no nginx was found. Kept inside the same
			     heading MySQL's row uses for its own badge (design D3: provenance
			     lives with the name, never beside the status pill, so it cannot
			     read as a second status). -->
			{#if sourceBadge}
				<span
					class="badge source source-{server.source?.kind}"
					title={sourceBadge.title}
					data-testid="ws-source-{server.id}">{sourceBadge.label}</span
				>
			{/if}
		</h3>
		{#if statusKind}
			<StatusPill kind={statusKind} testId="ws-pill-{server.id}" />
		{/if}
		<div class="grow"></div>
		{#if server.supported}
			<div class="row-actions">
				{#if control.kind === 'start'}
					<Button
						variant="quiet"
						size="sm"
						testId="ws-start-{server.id}"
						ariaLabel="Start {server.displayName}"
						disabled={control.disabled}
						onclick={() => onStart(server.serviceId ?? '')}>Start</Button
					>
				{:else if control.kind === 'retry'}
					<Button
						variant="quiet"
						size="sm"
						testId="ws-retry-{server.id}"
						ariaLabel="Retry {server.displayName}"
						onclick={() => onStart(server.serviceId ?? '')}>Retry</Button
					>
				{:else if control.kind === 'stop'}
					<Button
						variant="quiet"
						size="sm"
						testId="ws-stop-{server.id}"
						ariaLabel="Stop {server.displayName}"
						onclick={() => onStop(server.serviceId ?? '')}>Stop</Button
					>
				{/if}
				<Button
					variant="quiet"
					size="sm"
					testId="show-config-{server.id}"
					expanded={showConfig}
					controls={showConfig ? configId : undefined}
					ariaLabel="{showConfig ? 'Hide' : 'Show'} {server.displayName} config"
					onclick={toggleConfig}
				>
					{showConfig ? 'Hide config' : 'Show config'}
				</Button>
				<Button
					variant="quiet"
					size="sm"
					testId="validate-{server.id}"
					disabled={validating}
					ariaLabel="Validate {server.displayName} config"
					onclick={() => onValidate(server.id)}
				>
					{validating ? 'Validating…' : 'Validate'}
				</Button>
			</div>
		{/if}
	</div>

	{#if control.kind === 'start' && control.disabled}
		<!-- The disabled button alone is a dead end: it says "not now" without
		     saying when. This names the action that produces a config. -->
		<p class="unavailable" data-testid="ws-start-reason-{server.id}">{control.reason}</p>
	{/if}

	{#if server.supported}
		<dl class="facts">
			<dt>Version</dt>
			<dd class="mono num">{server.version ?? UNKNOWN}</dd>
			<dt>Hot reload</dt>
			<dd>{hotReloadLabel(server.supportsHotReload)}</dd>
			<dt>Binary</dt>
			<dd class="mono path">{server.binaryPath ?? UNKNOWN}</dd>
			<dt>Config</dt>
			<dd class="mono path">{server.configPath ?? UNKNOWN}</dd>
		</dl>
	{:else}
		<!-- Same sentence as the site editor's web-server hint (SiteDrawer.svelte,
		     `#f-server-hint`) so the product says ONE thing about Apache in both
		     places — change the two together. A capability statement about OpenVHost,
		     not a guess about the user's machine: there is an NginxAdapter and nginx
		     templates, and no Apache counterpart. -->
		<p class="unavailable">
			OpenVHost cannot serve {server.displayName} sites yet — it only generates nginx config.
		</p>
	{/if}

	<!-- Per-row failure: a read that failed, or a validator that could not even be
	     launched (the store routes both here). `role="alert"` because it appears in
	     response to the user's click and would otherwise be silent. Verbatim — the
	     backend message names the file or the binary it tried. -->
	{#if configError !== ''}
		<p class="field-error" role="alert" data-testid="config-error-{server.id}">{configError}</p>
	{/if}

	{#if showConfig}
		<pre id={configId} class="config" data-testid="config-{server.id}">{configText}</pre>
	{/if}

	<!-- A completed validator run. Kept visible even beside a READ error: those are
	     different operations and both statements can be true at once. It is not kept
	     beside a VALIDATE error — that would be two statements about the same
	     operation, one of them stale — but nothing is suppressed here for it: the
	     store drops this row's verdict when a validate run starts, so a validate
	     error and a report can never arrive together. -->
	{#if report}
		<div
			class="report {report.ok ? 'report-ok' : 'report-fail'}"
			role="status"
			data-testid="report-{server.id}"
		>
			<p class="headline">
				{#if report.ok}
					<svg
						width="14"
						height="14"
						viewBox="0 0 14 14"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"
						aria-hidden="true"
					>
						<path d="M2.5 7.5l3 3 6-7" />
					</svg>
				{/if}
				{report.ok ? 'Config is valid' : 'Config is not valid'}
			</p>
			<!-- The validator's own stderr, VERBATIM: that diagnostic is the useful
			     part, so it is neither summarized nor truncated. Rendered only when it
			     has content, so a silent validator leaves no empty box. -->
			{#if report.stderr.trim() !== ''}
				<pre>{report.stderr}</pre>
			{/if}
			{#if !report.ok}
				<!-- Says the rewrite out loud. `site::apply` (openvhost-core's
				     `site/apply/mod.rs`) regenerates `<home>/config/generated/nginx/nginx.conf`
				     from the user's sites AND the stored web-server settings every time Apply
				     runs — so a hand edit here holds only
				     until the next Apply and then vanishes with no notice. This sentence is the
				     only place the product tells anyone to edit that file, so it is the place
				     that has to be honest about it. It no longer says "this page is read-only"
				     either — the settings form below this panel edits how that file is
				     generated, so the claim now belongs to this VIEW, not to the page. and it names the escape hatch
				     (`config/custom/`) rather than leaving the reader with nowhere safe to put a
				     customisation. -->
				<p class="next">
					Edit the file on disk, then validate again — this view shows it, it does not edit it.
					Apply regenerates this file from your sites and the settings below, so hand edits are lost
					on the next Apply. Add custom directives under <code>config/custom/</code> instead.
				</p>
			{/if}
		</div>
	{/if}

	{#if serviceState?.kind === 'failed'}
		<!-- Same recipe as ServiceRow.svelte's `fail-detail`, and for the same
		     reason: the supervisor's captured stderr is the only thing that
		     explains why a start did not take. Verbatim — an nginx [emerg] line
		     names the file and line number, and summarising it would throw away
		     the part that fixes the problem. -->
		<div class="report report-fail" role="status" data-testid="ws-failed-{server.id}">
			<p class="headline">
				{server.displayName} failed{#if serviceState.exit !== null}&nbsp;(exit {serviceState.exit}){/if}
			</p>
			{#if serviceState.stderrTail.length > 0}
				<pre>{serviceState.stderrTail.join('\n')}</pre>
			{/if}
		</div>
	{/if}
</div>

<style>
	/* Ported from docs/design/mock.css: `.row` (padding/border/transition), `.row .primary`,
	   `.row-actions`, `.grow`, `.validation` (the ok report), `.fail-detail` +
	   `.fail-detail .headline`/`pre` (the failed report), `.field-error` as SiteDrawer.svelte
	   already renders it, and `.diff`'s `max-height` for the scroll cap on config text.

	   Four deliberate deviations from the mock, all because this is a fact BLOCK rather than a
	   one-line table row:

	   1. `.row` is a vertical flex stack here, not the mock's centered grid. A brand carries a
	      head line, a four-fact list and up to three disclosure blocks; a single grid line
	      cannot hold that, and it must stay usable in a 380px panel.
	   2. No `.row:hover` highlight. In the mock that tint reads as "this row is selectable";
	      these rows are not selectable, they are containers for their own controls, so a tint
	      that follows the pointer down a tall block is noise. The transition is kept for the
	      class-driven state changes.
	   3. Facts are a `<dl>` (label/value pairs are exactly what a description list is for),
	      styled as a two-column grid whose value column can wrap — long paths must wrap rather
	      than overflow at 380px.
	   4. Config and stderr text use `--vh-text-log` (13px), the token brand §5 defines for
	      "logs/configs", rather than the mock's 12px `--vh-text-caption` on `.fail-detail pre`. */
	.row {
		display: flex;
		flex-direction: column;
		gap: var(--vh-space-3);
		padding: var(--vh-space-4);
		border-bottom: 1px solid var(--vh-border);
		transition: background var(--vh-dur-fast) var(--vh-ease-out);
	}
	.row:last-child {
		border-bottom: 0;
	}
	/* Muted, not faded: a brand OpenVHost cannot serve yet still has to be readable, and
	   dropping opacity on the whole row would dim a third-party brand mark too. */
	.muted .primary {
		color: var(--vh-text-2);
	}
	.ws-head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--vh-space-3);
	}
	.ws-head .primary {
		display: inline-flex;
		/* Wraps since the source badge landed: at a narrow panel this heading can
		   hold an icon, "nginx" and "OpenVHost 1.30.4" together, which on one
		   nowrap line would push the row's status pill and actions off-screen —
		   the same failure MysqlRow.svelte's identical comment documents fixing
		   once already. */
		flex-wrap: wrap;
		align-items: center;
		gap: 8px;
		font-size: var(--vh-text-body);
		font-weight: 600;
		min-width: 0;
	}
	/* Provenance, not status (design D3): a quiet outline chip that reads as
	   metadata beside the name rather than competing with the StatusPill in
	   this row's head. Same recipe as MysqlRow.svelte's identical badge —
	   matched deliberately rather than reinvented one page over. */
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
	.badge.source {
		font-weight: 500;
		letter-spacing: 0.01em;
	}
	/* The packaged chip borrows the link accent to say "this one is ours" —
	   `--vh-link` is brand-700, the same token MysqlRow.svelte's identical
	   chip uses. Homebrew keeps the neutral `.badge` base above. */
	.badge.source-packaged {
		color: var(--vh-link);
		border-color: color-mix(in oklab, var(--vh-link) 35%, transparent);
		background: color-mix(in oklab, var(--vh-link) 8%, transparent);
	}
	.grow {
		flex: 1;
	}
	.row-actions {
		display: flex;
		gap: 4px;
		justify-content: flex-end;
		opacity: 0.85;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content minmax(0, 1fr);
		gap: 2px var(--vh-space-4);
		margin: 0;
		font-size: var(--vh-text-table);
	}
	.facts dt {
		color: var(--vh-text-2);
	}
	.facts dd {
		margin: 0;
	}
	/* A resolved binary or config path is long and has no spaces, so it needs an explicit
	   break opportunity to wrap instead of forcing the panel wider than its column. */
	.facts .path {
		overflow-wrap: anywhere;
	}
	.unavailable {
		margin: 0;
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
	}
	.field-error {
		margin: 0;
		color: var(--vh-fail);
		font-size: var(--vh-text-caption);
	}
	.config,
	.report pre {
		margin: 0;
		padding: var(--vh-space-2) var(--vh-space-3);
		background: var(--vh-log-bg);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		color: var(--vh-text);
		font-size: var(--vh-text-log);
		line-height: 1.6;
		overflow: auto;
		/* Same cap mock.css puts on `.diff`: a few-hundred-line nginx.conf must not push the
		   next brand's row off the screen. The block scrolls; the page does not grow. */
		max-height: 320px;
	}
	.report {
		display: flex;
		flex-direction: column;
		gap: 6px;
		border: 1px solid;
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		font-size: var(--vh-text-table);
	}
	.report .headline {
		display: flex;
		align-items: center;
		gap: 8px;
		margin: 0;
		font-weight: 600;
	}
	.report-ok {
		border-color: color-mix(in oklab, var(--vh-run) 40%, transparent);
		background: color-mix(in oklab, var(--vh-run-dot) 8%, var(--vh-surface));
		color: var(--vh-run);
	}
	.report-fail {
		border-color: color-mix(in oklab, var(--vh-fail) 35%, transparent);
		background: var(--vh-fail-tint);
		color: var(--vh-fail);
	}
	.report .next {
		margin: 0;
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
</style>
