<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import Button from './Button.svelte';

	/**
	 * The states a brand-new machine actually lands in — mirrors
	 * `LanguagesEmpty.svelte` exactly (spec D6: "NoBrew guide mirrors
	 * Languages"). `brewFound` is checked FIRST and outranks `anyInstalled`,
	 * because "no MySQL, press Install" is a dead end one level further up on
	 * a machine that cannot install anything at all.
	 *
	 * Three states, chosen in this order:
	 *   1. no brew          -> `databases-no-brew`
	 *   2. brew, no MySQL   -> `databases-no-mysql`
	 *   3. brew + a MySQL   -> renders nothing; the caller's own rowlist is the UI
	 */
	let {
		brewFound,
		anyInstalled,
		brewSearched = [],
		installing = '',
		onRescan,
		onOpenBrewSite
	}: {
		brewFound: boolean;
		anyInstalled: boolean;
		/** Every path `find_brew()` actually checked — rendered verbatim rather
		 *  than a hardcoded guess, same reasoning as `LanguagesEmpty`. */
		brewSearched?: string[];
		/** The major currently installing anywhere on the page, '' when idle —
		 *  same prop `MysqlRow` reads to disable its own Install button. */
		installing?: string;
		/** Wired to `DatabasesStore.rescan()` by the caller — this component has
		 *  no IPC import of its own, so it stays testable with a plain fake. */
		onRescan: () => void;
		/** Wired to `ipc.openHomebrewSite()` by the caller, same reasoning as
		 *  `LanguagesEmpty`'s identical prop: a plain `<a target="_blank">` is
		 *  inert in this webview. */
		onOpenBrewSite: () => void;
	} = $props();

	/**
	 * The official Homebrew install command (https://brew.sh), shown as
	 * selectable text and NEVER as a button that runs it — same reasoning as
	 * `LanguagesEmpty`'s identical constant: it is a `curl | bash` that
	 * requests `sudo`, and the process this app spawns has no tty to answer
	 * that prompt anyway.
	 */
	const BREW_INSTALL_CMD =
		'/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"';
</script>

{#if !brewFound}
	<div class="empty" data-testid="databases-no-brew">
		<h3>Homebrew is required to install MySQL</h3>
		<p>
			OpenVHost installs MySQL through Homebrew, and it was not found
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
		<Button size="sm" testId="databases-check-again" disabled={installing !== ''} onclick={onRescan}
			>Check again</Button
		>
	</div>
{:else if !anyInstalled}
	<div class="empty invite" data-testid="databases-no-mysql">
		<h3>Install MySQL to get started</h3>
		<p>
			OpenVHost installs MySQL 8.4 through Homebrew, initializes its data directory with a generated
			root password, and runs it under the supervisor below.
		</p>
	</div>
{/if}

<style>
	/* Byte-identical recipe to `LanguagesEmpty.svelte`'s own `<style>` block —
	   duplicated rather than shared for the same reason that file gives: this
	   component owns its own scoped styles, and the no-brew state needs a
	   left-aligned path list and a monospace command block a purely centred
	   block cannot hold well. */
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
	.empty :global(.btn) {
		margin-top: var(--vh-space-4);
	}
</style>
