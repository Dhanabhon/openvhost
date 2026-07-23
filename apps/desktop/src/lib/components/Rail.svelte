<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';

	// Only 'services' exists as a real destination today (Phase 1 slice A). The union stays
	// narrow on purpose — Sites/Logs/Settings join it once their own slices land.
	let { active = 'services' }: { active?: 'services' } = $props();
</script>

<nav class="rail" aria-label="Main">
	<div class="rail-brand">
		<svg width="24" height="24" viewBox="0 0 64 64" fill="none" aria-hidden="true">
			<path
				d="M42 10 H26 A16 16 0 0 0 10 26 V38 A16 16 0 0 0 26 54 H42"
				stroke="var(--vh-accent)"
				stroke-width="9"
				stroke-linecap="round"
			/>
			<circle cx="40" cy="32" r="8" fill="#3FB950" />
		</svg>
		<span class="name">OpenVHost</span>
	</div>

	<!-- Sites: inert placeholder — no site backend yet (own future slice). Not a link/button,
	     so it is never in the tab order and never fakes a click action. -->
	<span class="nav-item" aria-disabled="true">
		<svg
			width="18"
			height="18"
			viewBox="0 0 18 18"
			fill="none"
			stroke="currentColor"
			stroke-width="1.6"
			stroke-linecap="round"
			aria-hidden="true"
		>
			<rect x="2.5" y="3.5" width="13" height="11" rx="2" />
			<path d="M2.5 7h13" />
			<circle cx="5" cy="5.2" r="0.2" />
		</svg>
		Sites
	</span>

	<!-- Services: the only live destination this slice ships. -->
	<a class="nav-item" href={resolve('/')} aria-current={active === 'services' ? 'page' : undefined}>
		<svg
			width="18"
			height="18"
			viewBox="0 0 18 18"
			fill="none"
			stroke="currentColor"
			stroke-width="1.6"
			stroke-linecap="round"
			stroke-linejoin="round"
			aria-hidden="true"
		>
			<path d="M2 9.5h3l2-5 3 9 2-4h4" />
		</svg>
		Services
	</a>

	<!-- Logs: inert placeholder — full log-viewer redesign is slice B. -->
	<span class="nav-item" aria-disabled="true">
		<svg
			width="18"
			height="18"
			viewBox="0 0 18 18"
			fill="none"
			stroke="currentColor"
			stroke-width="1.6"
			stroke-linecap="round"
			aria-hidden="true"
		>
			<path d="M3 4.5h12M3 9h12M3 13.5h7" />
		</svg>
		Logs
	</span>

	<!-- Settings: inert placeholder — no settings content yet. -->
	<span class="nav-item" aria-disabled="true">
		<svg
			width="18"
			height="18"
			viewBox="0 0 18 18"
			fill="none"
			stroke="currentColor"
			stroke-width="1.6"
			stroke-linecap="round"
			aria-hidden="true"
		>
			<path d="M3 5.5h12M3 12.5h12" />
			<circle cx="7" cy="5.5" r="1.8" />
			<circle cx="11" cy="12.5" r="1.8" />
		</svg>
		Settings
	</span>

	<div class="rail-foot">
		<span class="num">v0.1.0</span>
		<!-- No wired action yet — a real, natively-disabled control rather than a fake href="#" link. -->
		<button type="button" class="stop-all" disabled>Stop all</button>
	</div>
</nav>

<style>
	/* Ported from docs/design/mock.css (.rail, .rail-brand, .nav-item, .rail-foot). Two
	   additions beyond the mockup: an [aria-disabled='true'] muted variant for the Sites/Logs/
	   Settings placeholders, and a .stop-all button reset (the mockup's "Stop all" is a plain
	   `.link` anchor; here it is a real disabled <button>, so it needs its own un-anchor styling
	   rather than the `.link` rule from mock.css). */
	.rail {
		background: var(--vh-surface-2);
		border-right: 1px solid var(--vh-border);
		display: flex;
		flex-direction: column;
		padding: var(--vh-space-4) var(--vh-space-3);
		gap: var(--vh-space-1);
	}
	.rail-brand {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: var(--vh-space-2) var(--vh-space-2) var(--vh-space-4);
	}
	.rail-brand svg {
		flex: none;
	}
	.rail-brand .name {
		font-family: var(--vh-font-display);
		font-weight: 500;
		font-size: var(--vh-text-section);
		letter-spacing: -0.01em;
	}
	.nav-item {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 8px 10px;
		border-radius: var(--vh-radius-control);
		color: var(--vh-text-2);
		text-decoration: none;
		font-weight: 500;
		transition:
			background var(--vh-dur-fast) var(--vh-ease-out),
			color var(--vh-dur-fast) var(--vh-ease-out);
	}
	.nav-item:hover {
		background: color-mix(in oklab, var(--vh-text) 6%, transparent);
		color: var(--vh-text);
	}
	.nav-item[aria-current='page'] {
		background: var(--vh-selected);
		color: var(--vh-accent);
	}
	.nav-item svg {
		flex: none;
	}
	.nav-item[aria-disabled='true'] {
		color: var(--vh-text-disabled);
		cursor: default;
	}
	.nav-item[aria-disabled='true']:hover {
		background: transparent;
		color: var(--vh-text-disabled);
	}
	.rail-foot {
		margin-top: auto;
		padding: var(--vh-space-2);
		display: flex;
		align-items: center;
		justify-content: space-between;
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
	}
	.stop-all {
		font-family: inherit;
		font-size: var(--vh-text-caption);
		font-weight: 500;
		color: var(--vh-link);
		background: none;
		border: 0;
		padding: 0;
		cursor: pointer;
	}
	.stop-all:hover {
		text-decoration: underline;
	}
	.stop-all:disabled {
		color: var(--vh-text-disabled);
		cursor: not-allowed;
		text-decoration: none;
	}
</style>
