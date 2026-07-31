<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Dispatches between the two spec-D7 read mechanisms (whole-branch review
  CRITICAL fix): a `"file"` source renders the poll-driven toolbar + body +
  status line (`LogToolbar`/`LogBody`/`LogStatusLine`, all UNCHANGED — this
  component only decides WHETHER they render, not what they do); a
  `serviceRing` source renders the existing `LogPane` live-output surface
  instead, fed `ringLogs` (populated via `service_log_tail` +
  `service-log`, never `readLogWindow` — see `logs.svelte.ts`'s
  `selectRingSource`/`applyRingLog`), plus its own copy of the spec D5
  privacy note (security audit L3: `LogPane` carries no toolbar of its own,
  so without this the note — otherwise supplied by `LogToolbar` on the file
  branch — was silently absent for ring sources, which are at least as
  likely to carry sensitive data as a file log).

  Extracted from `+page.svelte` specifically so this decision is testable
  via `svelte/server`: `LogsStore` is page-local and `onMount` never runs
  under SSR (see `logs-page.test.ts`'s header), so `+page.svelte` itself
  cannot be driven into a ring-selected state in a test. This component
  takes the same fields as plain props, so `LogSourceContent.svelte.test.ts`
  can prove "a ring deep link renders the live-output surface, not the
  error state" directly.

  `LogPane`'s own "Open in Logs" link is not offered here (`firstServiceId`
  is passed as `null`): that link exists to navigate someone AWAY from
  wherever they are TO `/logs` — offering it from inside `/logs` itself
  would be a pointless self-link.
-->
<script lang="ts">
	import type {
		IpcError,
		LogLevel,
		LogResetDto,
		LogRowDto,
		LogSourceDto,
		ServiceLogEvent
	} from '$lib/ipc';
	import { privacyNoteCopy } from '$lib/logs.copy';
	import LogPane from './LogPane.svelte';
	import LogToolbar from './LogToolbar.svelte';
	import LogBody from './LogBody.svelte';
	import LogStatusLine from './LogStatusLine.svelte';

	let {
		selected,
		ringLogs,
		requestedUnavailable,
		readError,
		exists,
		rows,
		filtered,
		reset,
		follow,
		newRowsWhilePaused,
		needle,
		caseSensitive,
		minLevel,
		sizeBytes,
		truncatedLines,
		scanBoundReached,
		onNeedle,
		onCaseSensitive,
		onMinLevel,
		onSetFollow,
		onJumpToLatest,
		onSelectStream,
		onRevealFolder,
		onScroll
	}: {
		selected: LogSourceDto | null;
		/** NOT `readonly`, matching `LogPane.svelte`'s own existing `logs:
		 *  UiLog[]` contract (`ServiceLogEvent` and `UiLog` are structurally
		 *  identical) — `LogsStore.ringLogs` is itself a plain, non-readonly
		 *  `$state<ServiceLogEvent[]>`, so this passes straight through with
		 *  no copy. */
		ringLogs: ServiceLogEvent[];
		requestedUnavailable: LogSourceDto | null;
		readError: IpcError | null;
		exists: boolean;
		rows: readonly LogRowDto[];
		filtered: boolean;
		reset: LogResetDto | null;
		follow: boolean;
		newRowsWhilePaused: boolean;
		needle: string;
		caseSensitive: boolean;
		minLevel: LogLevel | null;
		sizeBytes: number;
		truncatedLines: number;
		scanBoundReached: boolean;
		onNeedle: (v: string) => void;
		onCaseSensitive: (v: boolean) => void;
		onMinLevel: (v: LogLevel | null) => void;
		onSetFollow: (v: boolean) => void;
		onJumpToLatest: () => void;
		onSelectStream: (stream: 'access' | 'error') => void;
		onRevealFolder: () => void;
		onScroll: (nearBottom: boolean) => void;
	} = $props();

	const isRing = $derived(selected?.kind === 'serviceRing');
</script>

{#if isRing}
	<LogPane logs={ringLogs} firstServiceId={null} />
	<!-- Security audit L3: `LogPane` has no toolbar of its own, so the spec
	     D5 privacy note (otherwise rendered by `LogToolbar` for the file
	     branch below) would silently be absent for a ring source without
	     this — even though ring output (raw child stdout/stderr, e.g.
	     mysqld/php-fpm startup noise) is at least as likely to carry a
	     connection string as a file log. Duplicated here rather than hoisted
	     above the whole `{#if}`, so `LogToolbar`'s own existing contract and
	     tests (it renders this note as part of its documented job) stay
	     untouched and the file branch's visual order is unchanged. -->
	<p class="privacy-note" data-testid="log-privacy-note">{privacyNoteCopy()}</p>
{:else}
	<LogToolbar
		{needle}
		{caseSensitive}
		{minLevel}
		{follow}
		{newRowsWhilePaused}
		{selected}
		{onNeedle}
		{onCaseSensitive}
		{onMinLevel}
		{onSetFollow}
		{onJumpToLatest}
		{onSelectStream}
	/>
	<LogBody
		{selected}
		{requestedUnavailable}
		{readError}
		{exists}
		{rows}
		{filtered}
		{reset}
		{follow}
		{onRevealFolder}
		{onScroll}
	/>
	<LogStatusLine
		{selected}
		{requestedUnavailable}
		{sizeBytes}
		{truncatedLines}
		{scanBoundReached}
		{follow}
		{onRevealFolder}
	/>
{/if}

<style>
	/* Same recipe as `LogToolbar.svelte`'s own `.privacy-note` — kept as a
	   duplicate rule rather than a shared class/import because Svelte scopes
	   `<style>` per component; this file's copy backs the ring-branch `<p>`
	   above (security audit L3), `LogToolbar`'s copy backs its own. */
	.privacy-note {
		margin: 0 var(--vh-space-6) var(--vh-space-2);
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
</style>
