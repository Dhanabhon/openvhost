<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	// The app-level half of the optional-state.db slice (design D5).
	//
	// Without it, DEGRADE is dishonest: `list_log_sources` comes back short and
	// reads as "you have no sites", a stored default PHP reads as "no
	// preference". Those are quiet wrong answers — the failure mode this project
	// keeps getting burned by — so the condition is stated once, out loud, for
	// every route at the same time.
	//
	// `reason` is the store's OWN failure text (`unable to open database file`,
	// `permission denied`), rendered verbatim and never parsed. Carrying it is
	// the point: the sentence this whole slice replaces was Tauri's "you must
	// call `.manage()` before using this command", and swapping one unusable
	// sentence for a generic one would not have been an improvement.
	//
	// `null` means "nothing to report" AND "we could not tell" — see
	// `store-status.svelte.ts`. Both must render as silence; only a confirmed
	// failure may speak.
	let { reason }: { reason: string | null } = $props();
</script>

{#if reason !== null}
	<!-- `.banner-error` + role="alert", the pairing ErrorBanner/ApplyErrorBanner
	     already use, rather than SiteReadinessBanner's accent-tinted role="status":
	     something HAS failed here, and it silently changes what every page can
	     answer. A user who is about to press Save on a site deserves to be told
	     before, not after. Structure, spacing and tokens are otherwise the shared
	     banner recipe — this introduces no new visual language. -->
	<div class="banner-error" role="alert" data-testid="store-unavailable-banner">
		<strong>OpenVHost can't open its data store</strong>
		<!-- The reason first, because it is the only actionable line: "permission
		     denied" tells a user what to fix, where the two sentences below only
		     tell them what to expect. pre-wrap/anywhere for the same reason
		     ApplyErrorBanner and ScaffoldNoticeBanner use them — an OS error can
		     carry a long unbreakable path. -->
		<span class="detail" data-testid="store-unavailable-reason">{reason}</span>
		<span class="line">
			Your sites, web server settings and database passwords are kept in it, so anything that reads
			or changes them refuses until it opens — and lists that draw on it, such as per-site logs, are
			short without saying so.
		</span>
		<span class="line">
			Starting and stopping services, installing versions, and the nginx and PHP logs are
			unaffected. Reopening OpenVHost tries again.
		</span>
	</div>
{/if}

<style>
	/* The `.fail-detail` failure-surface recipe from docs/design/mock.css, reused
	   verbatim from ErrorBanner.svelte — same margins, padding, radius and font
	   size as every other banner in this stack, so this reads as one more banner
	   rather than a one-off. */
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
	.banner-error .detail {
		display: block;
		white-space: pre-wrap;
		/* An OS error can name a path with no spaces in it; without this the
		   banner refuses to wrap and pushes the window's own layout wider. */
		overflow-wrap: anywhere;
		margin-bottom: var(--vh-space-2);
	}
	.banner-error .line {
		display: block;
	}
	.banner-error .line + .line {
		margin-top: var(--vh-space-1);
	}
</style>
