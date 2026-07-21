<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { coreInfo, type CoreInfo, type IpcError } from '$lib/ipc';

	let info = $state<CoreInfo | null>(null);
	let error = $state<IpcError | null>(null);
	let loading = $state(false);

	async function load(simulate = false) {
		loading = true;
		error = null;
		try {
			info = await coreInfo(simulate);
		} catch (e) {
			info = null;
			error = e as IpcError;
		} finally {
			loading = false;
		}
	}
</script>

<main class="mx-auto max-w-xl p-8 font-sans">
	<h1 class="text-2xl font-semibold">OpenServ — dev shell</h1>
	<p class="mt-1 text-sm opacity-70">Phase 0 slice: one typed IPC command.</p>

	<div class="mt-6 flex gap-3">
		<button
			class="rounded bg-emerald-700 px-4 py-2 text-white disabled:opacity-50"
			onclick={() => load(false)}
			disabled={loading}
			data-testid="load-btn"
		>
			{loading ? 'Loading…' : 'Load core info'}
		</button>
		{#if import.meta.env.DEV}
			<button
				class="rounded border border-red-600 px-4 py-2 text-red-600 disabled:opacity-50"
				onclick={() => load(true)}
				disabled={loading}
				data-testid="simulate-btn"
			>
				Simulate failure (dev)
			</button>
		{/if}
	</div>

	{#if error}
		<div
			class="mt-6 rounded border border-red-400 bg-red-50 p-4 text-red-800"
			role="alert"
			data-testid="error-banner"
		>
			<strong class="block">Command failed ({error.kind})</strong>
			<span>{'message' in error ? error.message : 'Simulated failure (dev only)'}</span>
		</div>
	{:else if info}
		<dl class="mt-6 grid grid-cols-2 gap-2 rounded border p-4" data-testid="core-info">
			<dt class="font-medium">App version</dt>
			<dd>{info.appVersion}</dd>
			<dt class="font-medium">OS</dt>
			<dd>{info.os}</dd>
			<dt class="font-medium">Arch</dt>
			<dd>{info.arch}</dd>
			<dt class="font-medium">OpenServ home</dt>
			<dd class="break-all">{info.openservHome}</dd>
		</dl>
	{/if}
</main>
