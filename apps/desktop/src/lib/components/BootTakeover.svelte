<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import TitleBar from './TitleBar.svelte';
	import Button from './Button.svelte';
	import { takeoverCopy } from '$lib/boot-takeover.derive';
	import type { DegradedBoot } from '$lib/boot-status.svelte';

	// A TAKEOVER, not a banner (design D2). The store slice could put a banner
	// over a running app because a broken `state.db` is a partial failure of a
	// working app. Here nothing works: a per-command "unavailable" value would be
	// indistinguishable from a legitimate empty state — an empty service list
	// looks exactly like a machine with nothing installed — which trades a
	// frightening developer string for a plausible lie.
	//
	// The copy arrives already decided (`boot-takeover.derive.ts`): which
	// sentences, which verbatim details, and which of the two screens this is.
	// This component picks no copy, for the same reason `SiteReadinessBanner`
	// does not.
	let {
		boot,
		quitting = false,
		quitError = '',
		revealError = '',
		onReveal,
		onQuit
	}: {
		boot: DegradedBoot;
		quitting?: boolean;
		quitError?: string;
		/** Why the last Reveal in Finder did not open anything, or `''`. Its own
		 *  slot rather than sharing `quitError`: two actions that can both fail on
		 *  one screen need the message to say which one did. */
		revealError?: string;
		onReveal: () => void;
		onQuit: () => void;
	} = $props();

	const copy = $derived(takeoverCopy(boot));
</script>

<!-- `TitleBar`, NOT a hand-rolled <div> (design D6). The window is
     `titleBarStyle: "Overlay"` with `hiddenTitle`, so macOS draws only the
     traffic lights and NOTHING here is draggable unless the webview says so:
     this strip carries both the `env(titlebar-area-x, 72px)` inset that keeps
     content clear of the traffic lights and the `data-tauri-drag-region="deep"`
     that makes the window movable at all. A degraded window the user can
     neither move nor reach the close button on is worse than the bug this
     screen exists to fix.

     `runningCount={null}` renders no pill: this process has no supervisor, and
     on the `alreadyRunning` screen the services ARE up — in the other instance.
     See TitleBar.svelte's own prop comment.

     NOT role="alert" (unlike StoreUnavailableBanner, and deliberately). An
     assertive live region announces something that ARRIVES while the user is
     reading something else; this is the document itself, and the layout renders
     nothing before it, so there is no "before" for it to interrupt. A <main>
     with an <h1> is what a screen reader is already going to read on load, and
     `aria-live` on a whole page is a well-known way to make it read twice. -->
<div class="boot-window" data-testid="boot-takeover">
	<TitleBar runningCount={null} />
	<main class="boot-main">
		<div class="boot-card" data-testid={copy.testId}>
			<h1>{copy.title}</h1>

			<!-- The verbatim block, FIRST — above the explanation, for the same
			     reason StoreUnavailableBanner leads with its reason: the path and
			     the OS error are the only lines a user can act on, and the prose
			     below only tells them what to expect. -->
			<dl class="facts">
				{#each copy.details as detail (detail.testId)}
					<dt>{detail.label}</dt>
					<dd data-testid={detail.testId}>{detail.value}</dd>
				{/each}
			</dl>

			{#each copy.lines as line, i (i)}
				<p class="line">{line}</p>
			{/each}

			{#if quitError !== ''}
				<!-- role="alert" HERE and not on the screen: this one really does
				     arrive after the user pressed a button, and a Quit that silently
				     did nothing is the failure mode to avoid. -->
				<p class="action-error" role="alert" data-testid="boot-quit-error">{quitError}</p>
			{/if}

			{#if revealError !== ''}
				<!-- The same reasoning as the quit error, and it matters MORE here:
				     the commonest reason this screen exists is a run directory that
				     was never created, and revealing a folder that is not there fails.
				     A user already on an error screen pressing a button that does
				     nothing visible is the worst outcome available. The `Folder` fact
				     above stays selectable text so there is always a way through. -->
				<p class="action-error" role="alert" data-testid="boot-reveal-error">{revealError}</p>
			{/if}

			<div class="actions">
				<!-- Quit stays FIRST, and keeps `primary`. Reveal is a convenience on
				     one of the two screens; quitting is the one thing every screen
				     offers, so it must not move position between them. -->
				<Button variant="primary" disabled={quitting} testId="boot-quit" onclick={onQuit}>
					{quitting ? 'Stopping…' : 'Quit OpenVHost'}
				</Button>
				{#if copy.revealsRunDir}
					<!-- Only where a folder was actually named (design D3), and that is
					     `boot-takeover.derive.ts`'s call, not this template's — a
					     `{#if boot.kind === 'runDirUnusable'}` here would let a fifth
					     state inherit the button without failing to compile. -->
					<Button variant="quiet" testId="boot-reveal" onclick={onReveal}>Reveal in Finder</Button>
				{/if}
			</div>
		</div>
	</main>
</div>

<style>
	/* Mirrors AppShell's `.window`: `auto 1fr` over the full height, so the
	   titlebar keeps its strip and the message region takes the rest. Same
	   `height: 100%` chain through routes/layout.css's `html, body`. */
	.boot-window {
		display: grid;
		grid-template-rows: auto 1fr;
		height: 100%;
		width: 100%;
		background: var(--vh-bg);
	}
	/* Centred rather than top-left: there is no navigation, no second thing to
	   look at, and one decision to make. `min-height: 0` + `overflow: auto` for
	   the same reason `.shell`/`.content` carry them — a long OS error must
	   scroll, not push the titlebar off the window. */
	.boot-main {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 0;
		overflow: auto;
		padding: var(--vh-space-6);
	}
	/* The app's card recipe (surface, 1px border, `--vh-radius-card`), not the
	   dialog's: this is the window's content, so it casts no overlay shadow and
	   sits on no scrim. */
	.boot-card {
		width: min(520px, 100%);
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		padding: var(--vh-space-6);
	}
	/* `--vh-text-page`, not `--vh-text-display`: the page-title level this app
	   already uses for "Sites"/"Services". Display size would shout, and the
	   `alreadyRunning` screen is explicitly telling the user nothing is wrong. */
	.boot-card h1 {
		font-family: var(--vh-font-display);
		font-size: var(--vh-text-page);
		font-weight: 600;
		margin: 0;
		color: var(--vh-text);
	}
	.facts {
		margin: var(--vh-space-4) 0 0;
		padding: var(--vh-space-3) var(--vh-space-4);
		background: var(--vh-surface-2);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
	}
	.facts dt {
		font-size: var(--vh-text-caption);
		color: var(--vh-text-2);
	}
	.facts dt + dd {
		margin: 2px 0 0;
	}
	.facts dd + dt {
		margin-top: var(--vh-space-3);
	}
	/* Mono, selectable, and wrapping anywhere. A path has no spaces to break at
	   and an OS error can carry one — without `overflow-wrap: anywhere` the card
	   refuses to wrap and pushes the window's own layout wider, the same fix
	   ApplyErrorBanner and StoreUnavailableBanner already apply. `user-select`
	   is stated explicitly because the titlebar above sets `user-select: none`
	   and this is the one text on the screen worth copying. */
	.facts dd {
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-log);
		color: var(--vh-text);
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		user-select: text;
	}
	.line {
		margin: var(--vh-space-3) 0 0;
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		line-height: 1.6;
	}
	/* Shared by both action failures — a quit that did not complete and a reveal
	   that opened nothing. One rule, two `data-testid`s: which action failed is
	   carried by the message, not by a second colour. */
	.action-error {
		margin: var(--vh-space-3) 0 0;
		color: var(--vh-fail);
		font-size: var(--vh-text-table);
	}
	/* Left-aligned, unlike the dialog's right-aligned pair: this is the start of
	   the reading column rather than a corner to park controls in — and on the
	   run-dir screen the second button follows the first here, in reading order,
	   instead of being pushed away from it. */
	.actions {
		display: flex;
		gap: var(--vh-space-2);
		margin-top: var(--vh-space-6);
	}
</style>
