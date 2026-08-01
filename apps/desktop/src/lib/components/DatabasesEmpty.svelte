<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	/**
	 * The one state a brand-new machine lands in that the rowlist cannot say
	 * for itself: nothing is installed yet, so the rows below are all empty and
	 * the page needs a sentence explaining what pressing Install does.
	 *
	 * **The no-Homebrew guide is gone**, and its absence is the point of the
	 * MySQL-from-tarball slice: installing MySQL is now download → SHA-256
	 * verify → extract, so a machine that has never had Homebrew installs
	 * perfectly well. Telling that user to go install brew first would have been
	 * a dead end invented by this component rather than by the product. Whether
	 * a *particular host* has a verified download is a per-row fact
	 * (`MysqlInstanceDto.offer`), rendered by `MysqlRow` where the row's own
	 * Install control is decided — not here.
	 */
	let {
		anyInstalled
	}: {
		anyInstalled: boolean;
	} = $props();
</script>

{#if !anyInstalled}
	<div class="empty invite" data-testid="databases-no-mysql">
		<h3>Install MySQL to get started</h3>
		<p>
			OpenVHost downloads MySQL from Oracle, checks it against a checksum built into this app, and
			unpacks it into its own packages folder — no Homebrew required. It then initializes a data
			directory with a generated root password and runs it under the supervisor below.
		</p>
	</div>
{/if}

<style>
	/* Same recipe as `LanguagesEmpty.svelte`'s own `<style>` block, minus the
	   path list and command box the no-brew guide needed — this component has
	   no no-brew guide any more (see the script header). */
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
</style>
