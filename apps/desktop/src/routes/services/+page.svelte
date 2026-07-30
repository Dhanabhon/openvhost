<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { coreInfo, onServiceLog, type CoreInfo, type IpcError } from '$lib/ipc';
	import AppShell from '$lib/components/AppShell.svelte';
	import LogPane from '$lib/components/LogPane.svelte';
	import ServicesPanel from '$lib/components/ServicesPanel.svelte';
	import { runningCount } from '$lib/services.derive';
	import { servicesStore as store } from '$lib/services.shared.svelte';

	let info = $state<CoreInfo | null>(null);
	const running = $derived(runningCount(store.services));

	// The service snapshot and the `service-state` subscription belong to
	// `routes/+layout.svelte` now — they feed the titlebar count on EVERY route, so
	// keeping them here would leave other pages announcing "0 running". What stays is
	// page-specific: the live log feed and the tail that seeds it (both exist only
	// because this page renders LogPane) plus the footer's one-shot CoreInfo.
	onMount(() => {
		let unlisten: (() => void) | null = null;
		let disposed = false;

		void (async () => {
			try {
				const stop = await onServiceLog((ev) => store.applyLog(ev));
				// This page CAN unmount now that Services is a route the user navigates away
				// from, and the `await` above may resolve after that: unsubscribe immediately
				// instead of registering a listener nothing will ever clean up.
				if (disposed) {
					stop();
					return;
				}
				unlisten = stop;
				// Waits for the layout's snapshot internally, so a page that mounts before
				// its layout still seeds from the right service.
				await store.loadLogTail();
				info = await coreInfo();
			} catch (e) {
				store.fail(e as IpcError);
			}
		})();

		return () => {
			disposed = true;
			unlisten?.();
			unlisten = null;
		};
	});
</script>

<AppShell runningCount={running} active="services">
	<h1 class="sr-only">OpenVHost — Services</h1>
	<!-- The error banner used to live here. AppShell renders it now, from the same shared
	     store, so start/stop and startup-load failures surface on every route instead of
	     only on this one. -->
	<ServicesPanel
		services={store.services}
		onStart={(id) => void store.start(id)}
		onStop={(id) => void store.stop(id)}
	/>
	<LogPane logs={store.logs} firstServiceId={store.services[0]?.id ?? null} />
	{#if info}
		<p class="coreinfo mono">
			OpenVHost {info.appVersion} · {info.os}/{info.arch} · {info.openvhostHome}
		</p>
	{/if}
</AppShell>

<style>
	/* .coreinfo adapts mock.css's `.statusline` (the caption-sized footer metadata strip used
	   under the log toolbar in the mockup's log-focused screen). */
	.coreinfo {
		padding: 6px var(--vh-space-6) var(--vh-space-4);
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
</style>
