<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { pillClass, type StateKind } from '../services.derive';

	let { kind, testId }: { kind: StateKind; testId?: string } = $props();
</script>

<span class="pill {pillClass(kind)}" data-testid={testId}><span class="dot"></span>{kind}</span>

<style>
	/* Ported from docs/design/mock.css (.pill, .pill .dot, .pill-*, @keyframes vh-pulse). */
	.pill {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 2px 10px 2px 7px;
		border-radius: var(--vh-radius-pill);
		font-size: var(--vh-text-caption);
		font-weight: 600;
		border: 1px solid var(--vh-border);
		background: var(--vh-surface);
		white-space: nowrap;
	}
	.pill .dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		flex: none;
	}
	.pill-running {
		color: var(--vh-run);
	}
	.pill-running .dot {
		background: var(--vh-run-dot);
	}
	.pill-starting {
		color: var(--vh-start);
	}
	.pill-starting .dot {
		background: var(--vh-start-dot);
		animation: vh-pulse 1.2s var(--vh-ease-out) infinite;
	}
	.pill-failed {
		color: var(--vh-fail);
		border-color: color-mix(in oklab, var(--vh-fail) 40%, transparent);
	}
	.pill-failed .dot {
		background: var(--vh-fail-dot);
	}
	.pill-stopped {
		color: var(--vh-stop);
	}
	.pill-stopped .dot {
		background: var(--vh-stop-dot);
	}
	@keyframes vh-pulse {
		50% {
			opacity: 0.35;
		}
	}
</style>
