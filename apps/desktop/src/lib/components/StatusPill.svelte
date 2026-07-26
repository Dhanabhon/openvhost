<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { pillClass, type StateKind } from '../services.derive';

	let { kind, testId }: { kind: StateKind; testId?: string } = $props();
</script>

<span class="pill {pillClass(kind)}" data-testid={testId}><span class="dot"></span>{kind}</span>

<style>
	/* Ported from docs/design/mock.css (.pill, .pill .dot, .pill-*, @keyframes vh-pulse). */
	/* `justify-content` + symmetric padding: the pill is a GRID ITEM in a fixed
	   120px track, and a grid item's default `justify-self: stretch` widens this
	   inline-flex box to the whole track. Without centring, the dot and label stay
	   packed at the flex start with all the slack piled up on the right, which is
	   what the badge looked like before. The track stays fixed on purpose (a
	   content-sized one would shift the action buttons every time the state text
	   changed width), so the content has to centre inside it rather than the box
	   shrinking to fit.

	   The padding is symmetric here where the mock's is `2px 10px 2px 7px`. That
	   asymmetry is right for a pill that HUGS its content — it tightens the visually
	   lighter dot side — but under centring it just offsets the centred group 1.5px
	   off true centre. TitleBar's pill still hugs (it is a flex child, not a grid
	   item) and keeps the mock's asymmetry. */
	.pill {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		padding: 2px 10px;
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
