<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { IpcError } from '$lib/ipc';

	// Moved out of routes/services/+page.svelte, which used to be the ONLY renderer of
	// the shared services store's error. That was survivable while Services was `/`;
	// once Sites became the landing page it meant a failed startup `listServices` left
	// the first screen showing "0 running" with no explanation anywhere — a false
	// statement about the user's system, silently. AppShell renders this instead, so
	// the failure surfaces on whatever route the user is on.
	let { error }: { error: IpcError | null } = $props();
</script>

{#if error}
	<div class="banner-error" role="alert" data-testid="error-banner">
		<strong>Command failed ({error.kind})</strong>
		<span>{'message' in error ? error.message : ''}</span>
	</div>
{/if}

<style>
	/* No direct mock.css analog (the mockup never shows a page-level IPC error banner) —
	   this reuses the `.fail-detail` failure-surface recipe (fail-tinted
	   background/border/text) from docs/design/mock.css so it reads as the same
	   "failure" semantic used everywhere else in the product. */
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
</style>
