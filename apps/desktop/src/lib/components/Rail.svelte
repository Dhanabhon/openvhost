<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { resolve } from '$app/paths';

	// 'sites', 'services', 'web-server', 'languages', 'databases' and now 'logs' are real
	// destinations. The union stays narrow on purpose — Settings joins it once its own slice
	// lands.
	//
	// Defaults to 'sites' because Sites is what `/` renders: the app's landing page needs no
	// `active` of its own, and an unset prop can only ever highlight the destination the user
	// is most likely on. AppShell.svelte defaults the same way — change both together.
	let {
		active = 'sites'
	}: {
		active?: 'services' | 'sites' | 'web-server' | 'languages' | 'databases' | 'logs';
	} = $props();
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
			<circle cx="40" cy="32" r="8" fill="var(--vh-run-dot)" />
		</svg>
		<span class="name">OpenVHost</span>
	</div>

	<!-- Sites: the landing page — `/`, not `/sites` (owner decision: the app opens on Sites). -->
	<a class="nav-item" href={resolve('/')} aria-current={active === 'sites' ? 'page' : undefined}>
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
	</a>

	<!-- Services: the other live destination, at its own `/services` route. -->
	<a
		class="nav-item"
		href={resolve('/services')}
		aria-current={active === 'services' ? 'page' : undefined}
	>
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

	<!-- Web server: read-only facts about each brand OpenVHost knows — its own `/web-server`
	     route, after Services because it answers "what would run", which only matters once you
	     know what IS running. Label is sentence case per brand §5, and matches the site
	     editor's own "Web server" field label. -->
	<a
		class="nav-item"
		href={resolve('/web-server')}
		aria-current={active === 'web-server' ? 'page' : undefined}
	>
		<!-- Hand-drawn to match the other rail glyphs (18px, currentColor stroke, 1.6 width):
		     two stacked units with a status lamp each. The real nginx/Apache brand marks are
		     NOT used here — the rail names a section, not a brand. -->
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
			<rect x="2.5" y="3" width="13" height="5" rx="1.5" />
			<rect x="2.5" y="10" width="13" height="5" rx="1.5" />
			<circle cx="5.2" cy="5.5" r="0.2" />
			<circle cx="5.2" cy="12.5" r="0.2" />
		</svg>
		Web server
	</a>

	<!-- Languages: PHP versions today, other runtimes later — its own `/languages` route, after
	     Web server so the two "what could run" destinations (web server brands, language
	     runtimes) sit together ahead of the Logs/Settings placeholders. -->
	<a
		class="nav-item"
		href={resolve('/languages')}
		aria-current={active === 'languages' ? 'page' : undefined}
	>
		<!-- Hand-drawn (18px, currentColor stroke, 1.6 width) rather than a real PHP elephant mark
		     — same reasoning as the web-server icon above: the rail names a section, not a brand.
		     A version-tag glyph (angle brackets + a tick) reads as "runtime version" generically. -->
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
			<path d="M6.5 5 3 9l3.5 4" />
			<path d="M11.5 5 15 9l-3.5 4" />
		</svg>
		Languages
	</a>

	<!-- Databases: MySQL lifecycle today, MariaDB later (spec Deferred) — its own `/databases`
	     route, after Languages: task brief order (both "what runtimes/services are managed"
	     destinations sit together, ahead of the Logs/Settings placeholders). -->
	<a
		class="nav-item"
		href={resolve('/databases')}
		aria-current={active === 'databases' ? 'page' : undefined}
	>
		<!-- Hand-drawn (18px, currentColor stroke, 1.6 width) rather than a real MySQL dolphin
		     mark — same reasoning as the other rail glyphs: the rail names a section, not a
		     brand. A stacked-cylinder "database" glyph reads as "data storage" generically. -->
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
			<ellipse cx="9" cy="4.5" rx="6" ry="2" />
			<path d="M3 4.5v9c0 1.1 2.7 2 6 2s6-.9 6-2v-9" />
			<path d="M3 9c0 1.1 2.7 2 6 2s6-.9 6-2" />
		</svg>
		Databases
	</a>

	<!-- Logs: its own `/logs` route (P1 live-log-viewer slice), after Databases — task brief
	     order (every "what runtimes/services are managed" destination sits ahead of it; Logs
	     answers "why did any of that fail", which only matters once something above it exists
	     to investigate). -->
	<a class="nav-item" href={resolve('/logs')} aria-current={active === 'logs' ? 'page' : undefined}>
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
	</a>

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
	</div>
</nav>

<style>
	/* Ported from docs/design/mock.css (.rail, .rail-brand, .nav-item, .rail-foot). One
	   addition beyond the mockup: an [aria-disabled='true'] muted variant for the Sites/Logs/
	   Settings placeholders. (This file used to also carry a permanently-disabled "Stop all"
	   button + its own .stop-all reset here — removed once the P1 tray design shipped a real,
	   working bulk-control equivalent in the menu-bar tray; see apps/desktop/src-tauri/src/tray/.) */
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
</style>
