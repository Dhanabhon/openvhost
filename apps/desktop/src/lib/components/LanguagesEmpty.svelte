<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { phpInstallInvite } from '../php-install.derive';
	import Button from './Button.svelte';

	/**
	 * The states a brand-new machine actually lands in (Task 7).
	 *
	 * `noRouteToAnyPhp` is checked FIRST and takes priority over `anyInstalled`,
	 * because "no PHP, press Install" is a dead end one level further up on a
	 * machine that cannot install anything at all. That reasoning is unchanged.
	 *
	 * What changed (off-Homebrew slice 5C design D2) is what answers it. It used
	 * to be `!brewFound`, and after slices 5A/5B that is a different question: a
	 * machine with no Homebrew and a packaged PHP was being told it could not
	 * install PHP, on a page that was simultaneously not listing the PHP it
	 * already had. The decision now comes from `noRouteToAnyPhp` in
	 * `php-install.derive.ts`, which is defined as "no row on this page has an
	 * Install button and nothing is already installed" — so this screen and the
	 * rows can never disagree.
	 *
	 * The copy below is NOT softened, on purpose. A user with no route to any
	 * PHP needs telling plainly, with the paths we actually searched. What
	 * changed is *when* it appears, not *what* it says.
	 *
	 * Three states, chosen in this order:
	 *   1. no route at all -> `languages-no-brew`
	 *   2. a route, no PHP -> `languages-no-php`
	 *   3. a PHP installed -> renders nothing; the caller's own rowlist is the UI
	 */
	let {
		brewFound,
		noRouteToAnyPhp,
		anyInstalled,
		brewSearched = [],
		installing = '',
		onRescan,
		onOpenBrewSite
	}: {
		/** Still exactly what it always meant: we looked for Homebrew and did or
		 *  did not find it (design D5). What it is no longer is this page's first
		 *  and highest-priority state — that job moved to `noRouteToAnyPhp`. All
		 *  it decides here is which install route the invitation names. */
		brewFound: boolean;
		/** Nothing installed AND nothing installable by any route. Computed by
		 *  the caller from the whole environment, never re-derived here: the
		 *  answer depends on every row's `offer`, which this component has no
		 *  business holding. */
		noRouteToAnyPhp: boolean;
		anyInstalled: boolean;
		/** Every path `find_brew()` actually checked (spec §6.1) — rendered
		 *  verbatim rather than a hardcoded guess, because an Intel Mac's
		 *  `/usr/local` and an Apple Silicon Mac's `/opt/homebrew` are different
		 *  paths, and a wrong guess sends the user checking a location brew was
		 *  never going to be at. Empty on an older backend, or a failure before
		 *  the search itself ran, so that case gets its own sentence rather than
		 *  a blank list. */
		brewSearched?: string[];
		/** The major installing anywhere on the page, '' when idle — same prop
		 *  `LanguageRow` reads to disable its own Install button. A1 audit
		 *  finding: `rescan_php_runtimes` now takes `InstallLock` with
		 *  `.lock().await` (H1's fix, so a rescan can never overwrite a
		 *  just-completed install with a stale set), so pressing this button
		 *  during an install blocks for the whole build — twenty minutes for a
		 *  source formula — with no feedback, and repeated presses queue
		 *  unbounded waiters on the mutex. Disabling the control while
		 *  `installing !== ''` is the fix; the label is left alone (unlike
		 *  `LanguageRow`'s Install button, this one has no "Installing…" major
		 *  of its own to name). */
		installing?: string;
		/** Wired to `LanguagesStore.rescan()` by the caller — this component
		 *  has no IPC import of its own, so it stays testable with a plain fake. */
		onRescan: () => void;
		/** Wired to `ipc.openHomebrewSite()` by the caller, same reasoning as
		 *  `onRescan`. A plain `<a target="_blank">` is inert in this webview:
		 *  Tauri only handles a new-window request when the app registers
		 *  `on_new_window`, which it does not, so WebKit is told not to create a
		 *  window and the click would silently do nothing. This has to be a real
		 *  control wired to a Rust-side opener command instead. */
		onOpenBrewSite: () => void;
	} = $props();

	/**
	 * The official Homebrew install command (https://brew.sh), shown as
	 * selectable text and NEVER as a button that runs it:
	 *
	 * 1. It is a `curl | bash` that requests `sudo` and changes the system
	 *    broadly. That is the machine owner's decision to make in their own
	 *    terminal, not one this app makes for them on first launch.
	 * 2. It would fail here regardless — the process this app spawns has no
	 *    tty to answer a sudo prompt, so "helpfully" running it would only
	 *    hand the user a more confusing error than the one they started with.
	 *
	 * Do not add an "Install Homebrew" button. If a future revision wants one,
	 * it needs a real answer to both points above first.
	 */
	const BREW_INSTALL_CMD =
		'/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"';
</script>

{#if noRouteToAnyPhp}
	<div class="empty" data-testid="languages-no-brew">
		<h3>Homebrew is required to install PHP</h3>
		<p>
			OpenVHost installs PHP through Homebrew, and it was not found
			{#if brewSearched.length > 0}
				at:
			{:else}
				— no install paths could be checked.
			{/if}
		</p>
		{#if brewSearched.length > 0}
			<ul class="paths">
				{#each brewSearched as path (path)}
					<li class="mono">{path}</li>
				{/each}
			</ul>
		{/if}
		<p>Install it from a terminal, then come back and check again:</p>
		<pre class="cmd"><code>{BREW_INSTALL_CMD}</code></pre>
		<p class="hint">
			<button
				type="button"
				class="link-button"
				data-testid="open-brew-site"
				onclick={onOpenBrewSite}
			>
				brew.sh
			</button> has the full instructions.
		</p>
		<Button size="sm" testId="languages-check-again" disabled={installing !== ''} onclick={onRescan}
			>Check again</Button
		>
	</div>
{:else if !anyInstalled}
	<div class="empty invite" data-testid="languages-no-php">
		<h3>Install PHP to get started</h3>
		<!-- The Homebrew sentence is verbatim what this page has always shown and
		     is still exactly right wherever Homebrew is present — every real
		     machine today, which is what keeps spec §8.6 true. It is only wrong on
		     the machine D2 exists for: no Homebrew, but a packaged version there
		     for the taking. Naming Homebrew there would be the same page-wide
		     claim about a per-major fact that D2 removes one branch up. -->
		<p>{phpInstallInvite(brewFound)}</p>
	</div>
{/if}

<style>
	/* Same recipe as +page.svelte's own `.empty` (padding-8/6, centred text,
	   --vh-text-2 body copy) — duplicated here rather than shared because this
	   component owns its own scoped styles, and the no-brew state additionally
	   needs a left-aligned path list and a monospace command block that a
	   purely centred block cannot hold well. */
	.empty {
		padding: var(--vh-space-8) var(--vh-space-6);
		text-align: center;
		color: var(--vh-text-2);
	}
	.empty h3 {
		margin: 0 0 var(--vh-space-2);
		color: var(--vh-text);
		font-size: var(--vh-text-section);
		font-weight: 600;
	}
	.empty p {
		margin: var(--vh-space-2) auto 0;
		max-width: 52ch;
	}
	.invite {
		/* The centred invitation Task 7 calls for: one heading, one line of
		   copy, no per-version enumeration — the rowlist right below already
		   lists every version and its own Install button, so repeating that
		   here would turn one clear call to action into a second inventory. */
		max-width: 60ch;
		margin: 0 auto;
	}
	.paths {
		list-style: none;
		margin: var(--vh-space-2) auto 0;
		padding: 0;
		display: inline-flex;
		flex-direction: column;
		gap: 2px;
		text-align: left;
	}
	.mono {
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
	}
	.cmd {
		margin: var(--vh-space-2) auto 0;
		max-width: 60ch;
		padding: var(--vh-space-3);
		background: var(--vh-surface-2);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
		white-space: pre-wrap;
		word-break: break-all;
		text-align: left;
		user-select: text;
	}
	.hint {
		font-size: var(--vh-text-table);
	}
	/* Looks and reads like the inline link it replaces (Task 7 review finding):
	   a plain `<a target="_blank">` is inert in this webview, so this is a real
	   `<button>` wired to `onOpenBrewSite`, styled to match `.hint a` exactly. */
	.link-button {
		display: inline;
		margin: 0;
		padding: 0;
		border: none;
		background: none;
		font: inherit;
		color: var(--vh-link);
		text-decoration: underline;
		cursor: pointer;
	}
	/* `.empty` centres text, and `.btn` is inline-flex, so plain `text-align:
	   center` on the parent already centres it — only the top gap is needed. */
	.empty :global(.btn) {
		margin-top: var(--vh-space-4);
	}
</style>
