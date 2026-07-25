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
	{#if store.error}
		<div class="banner-error" role="alert" data-testid="error-banner">
			<strong>Command failed ({store.error.kind})</strong>
			<span>{'message' in store.error ? store.error.message : ''}</span>
		</div>
	{/if}
	<ServicesPanel
		services={store.services}
		onStart={(id) => void store.start(id)}
		onStop={(id) => void store.stop(id)}
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
