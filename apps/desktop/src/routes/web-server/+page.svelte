<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import ErrorBanner from '$lib/components/ErrorBanner.svelte';
	import WebServerPanel from '$lib/components/WebServerPanel.svelte';
	import { runningCount } from '$lib/services.derive';
	import { servicesStore } from '$lib/services.shared.svelte';
	import { webServersStore as store } from '$lib/webservers.svelte';

	// The titlebar's "N running" belongs to every route and comes from the shared
	// supervisor state the layout subscribes to — never a literal.
	const running = $derived(runningCount(servicesStore.services));

	onMount(() => {
		// Fire-and-forget: the shell and the page head paint immediately and the rows
		// appear when the list resolves. That matters here because `list_web_servers`
		// probes the version, which SPAWNS `nginx -v` server-side — awaiting it before
		// rendering would show the user an empty window for the length of a process
		// launch. Failures land on `store.error` and render in the banner below.
		void store.load();
	});
</script>

<AppShell runningCount={running} active="web-server">
	<h1 class="sr-only">OpenVHost — Web server</h1>
	<!-- AppShell renders the SERVICES store's banner (supervisor failures, on every
	     route). This one is the web-server list's own page-level failure — the whole
	     `list_web_servers` call failing — which nothing else would surface. Per-row
	     failures are NOT here: they render on their own row so one brand's problem
	     cannot blank the page. -->
	<ErrorBanner error={store.error} />
	<WebServerPanel
		servers={store.servers}
		services={servicesStore.services}
		configText={store.configText}
		configError={store.configError}
		reports={store.reports}
		validating={store.validating}
		onShowConfig={(id) => void store.showConfig(id)}
		onValidate={(id) => void store.validate(id)}
	/>
</AppShell>
