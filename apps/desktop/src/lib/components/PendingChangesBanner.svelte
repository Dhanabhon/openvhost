<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import Button from './Button.svelte';

	let { count, onReview }: { count: number; onReview: () => void } = $props();
</script>

{#if count > 0}
	<div class="banner" role="status" data-testid="pending-changes">
		<span>
			{count}
			{count === 1 ? 'change' : 'changes'} not applied yet
		</span>
		<div class="grow"></div>
		<Button variant="primary" onclick={onReview}>Review and apply</Button>
	</div>
{/if}

<style>
	/* Same surface/border vocabulary as SitesPanel's `.panel`, but tinted with the
	   brand accent rather than plain `--vh-surface` — this banner is an action
	   prompt, not a data panel, and should not read as just another list. */
	.banner {
		display: flex;
		align-items: center;
		gap: var(--vh-space-4);
		margin: var(--vh-space-3) var(--vh-space-6) 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		border: 1px solid color-mix(in oklab, var(--vh-accent) 35%, transparent);
		background: color-mix(in oklab, var(--vh-accent) 8%, var(--vh-surface));
		border-radius: var(--vh-radius-card);
		font-size: var(--vh-text-table);
	}
	.grow {
		flex: 1;
	}
</style>
