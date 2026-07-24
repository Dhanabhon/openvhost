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
	import AppShell from '$lib/components/AppShell.svelte';
	import LogPane from '$lib/components/LogPane.svelte';
	import ServicesPanel from '$lib/components/ServicesPanel.svelte';
	import { runningCount } from '$lib/services.derive';
	import { ServicesStore } from '$lib/services.svelte';

	const store = new ServicesStore({ listServices, serviceLogTail });
	let info = $state<CoreInfo | null>(null);
	let error = $state<IpcError | null>(null);
	const running = $derived(runningCount(store.services));

	onMount(() => {
		let unsubs: Array<() => void> = [];
		(async () => {
			try {
				unsubs = await Promise.all([
					onServiceState((ev) => store.applyState(ev)),
					onServiceLog((ev) => store.applyLog(ev))
				]);
				await store.init();
				info = await coreInfo();
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
</script>

<AppShell runningCount={running}>
	<h1 class="sr-only">OpenVHost — Services</h1>
	{#if error}
		<div class="banner-error" role="alert" data-testid="error-banner">
			<strong>Command failed ({error.kind})</strong>
			<span>{'message' in error ? error.message : ''}</span>
		</div>
	{/if}
	<ServicesPanel
		services={store.services}
		onStart={(id) => act(startService, id)}
		onStop={(id) => act(stopService, id)}
	/>
	<LogPane logs={store.logs} />
	{#if info}
		<p class="coreinfo mono">
			OpenVHost {info.appVersion} · {info.os}/{info.arch} · {info.openvhostHome}
		</p>
	{/if}
</AppShell>

<style>
	/* .banner-error has no direct mock.css analog (the mockup never shows a page-level IPC
	   error banner) — it reuses the `.fail-detail` failure-surface recipe (fail-tinted
	   background/border/text) from docs/design/mock.css so it reads as the same "failure"
	   semantic used everywhere else in the product. */
	.banner-error {
		margin: var(--vh-space-3) var(--vh-space-6) 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		background: var(--vh-fail-tint);
		border-radius: var(--vh-radius-control);
		color: var(--vh-fail);
		font-size: var(--vh-text-table);
	}
	.banner-error strong {
		display: block;
		margin-bottom: 2px;
	}
	/* .coreinfo adapts mock.css's `.statusline` (the caption-sized footer metadata strip used
	   under the log toolbar in the mockup's log-focused screen). */
	.coreinfo {
		padding: 6px var(--vh-space-6) var(--vh-space-4);
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
</style>
