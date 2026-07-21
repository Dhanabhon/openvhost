<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		coreInfo,
		listServices,
		onServiceLog,
		onServiceState,
		serviceLogTail,
		startService,
		stopService,
		type CoreInfo,
		type IpcError
	} from '$lib/ipc';
	import { ServicesStore } from '$lib/services.svelte';

	const store = new ServicesStore({ listServices, serviceLogTail });
	let info = $state<CoreInfo | null>(null);
	let error = $state<IpcError | null>(null);

	onMount(() => {
		let unsubs: Array<() => void> = [];
		(async () => {
			try {
				await store.init();
				info = await coreInfo();
				unsubs = await Promise.all([
					onServiceState((ev) => store.applyState(ev)),
					onServiceLog((ev) => store.applyLog(ev))
				]);
			} catch (e) {
				error = e as IpcError;
			}
		})();
		return () => unsubs.forEach((u) => u());
	});

	async function act(fn: (id: string) => Promise<void>, id: string) {
		error = null;
		try {
			await fn(id);
		} catch (e) {
			error = e as IpcError;
		}
	}

	const levelClass = (level: string) =>
		level === 'error' ? 'text-red-700' : level === 'warn' ? 'text-amber-700' : 'text-neutral-500';
	const fmtTs = (t: number) => new Date(t).toLocaleTimeString(undefined, { hour12: false });
</script>

<main class="mx-auto flex h-screen max-w-3xl flex-col p-6 font-sans">
	<h1 class="text-xl font-semibold">Services</h1>

	{#if error}
		<div class="mt-3 rounded border border-red-400 bg-red-50 p-3 text-red-800" role="alert" data-testid="error-banner">
			<strong>Command failed ({error.kind})</strong>
			<span>{'message' in error ? error.message : ''}</span>
		</div>
	{/if}

	<section class="mt-4 divide-y rounded border" data-testid="services">
		{#each store.services as s (s.id)}
			<div class="flex items-center gap-4 p-3">
				<div class="min-w-0 flex-1">
					<div class="font-semibold">{s.displayName}</div>
					{#if s.endpoint}<div class="truncate font-mono text-xs text-neutral-500">{s.endpoint}</div>{/if}
				</div>
				<span
					class="rounded-full border px-2.5 py-0.5 text-xs font-semibold"
					class:text-emerald-700={s.state.kind === 'running'}
					class:text-amber-700={s.state.kind === 'starting'}
					class:text-red-700={s.state.kind === 'failed'}
					class:text-neutral-500={s.state.kind === 'stopped'}
					data-testid="pill-{s.id}"
				>
					● {s.state.kind}
				</span>
				{#if s.state.kind === 'stopped'}
					<button class="rounded border px-3 py-1 text-sm font-medium" onclick={() => act(startService, s.id)}>Start</button>
				{:else if s.state.kind === 'failed'}
					<button class="rounded border px-3 py-1 text-sm font-medium" onclick={() => act(startService, s.id)}>Retry</button>
				{:else}
					<button class="rounded border px-3 py-1 text-sm font-medium" onclick={() => act(stopService, s.id)}>Stop</button>
				{/if}
			</div>
			{#if s.state.kind === 'failed'}
				<div class="border-t bg-red-50 p-3 text-sm" data-testid="failed-{s.id}">
					<div class="font-semibold text-red-700">
						{s.displayName} failed{#if s.state.exit != null}&nbsp;(exit {s.state.exit}){/if}
					</div>
					<pre class="mt-2 overflow-x-auto rounded border bg-white p-2 font-mono text-xs">{s.state.stderrTail.join('\n')}</pre>
				</div>
			{/if}
		{/each}
	</section>

	<h2 class="mt-6 text-xs font-semibold tracking-wide text-neutral-500 uppercase">Log</h2>
	<div class="mt-2 flex-1 overflow-auto rounded border bg-neutral-50 p-2 font-mono text-xs leading-6" data-testid="log">
		{#each store.logs as l, i (i)}
			<div class="grid grid-cols-[70px_44px_1fr] gap-2">
				<span class="text-neutral-400 tabular-nums">{fmtTs(l.tsMs)}</span>
				<span class="font-bold {levelClass(l.level)}">{l.level}</span>
				<span class="whitespace-pre-wrap">{l.line}</span>
			</div>
		{/each}
	</div>

	{#if info}
		<p class="mt-3 text-xs text-neutral-500">
			OpenVHost {info.appVersion} · {info.os}/{info.arch} · <span class="font-mono">{info.openvhostHome}</span>
		</p>
	{/if}
</main>
