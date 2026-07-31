<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!--
  Dispatches between the two spec-D7 read mechanisms (whole-branch review
  CRITICAL fix): a `"file"` source renders the poll-driven toolbar + body +
  status line (`LogToolbar`/`LogBody`/`LogStatusLine`, all UNCHANGED — this
  component only decides WHETHER they render, not what they do); a
  `serviceRing` source renders the existing `LogPane` live-output surface
  instead, fed `ringLogs` (populated via `service_log_tail` +
  `service-log`, never `readLogWindow` — see `logs.svelte.ts`'s
  `selectRingSource`/`applyRingLog`).

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
