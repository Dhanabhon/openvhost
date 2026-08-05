<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { engineDescriptor, type EngineKind } from '$lib/databases.derive';

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
	 *
	 * `engine` (P1 MariaDB UI design D1) — defaults to `'mysql'` so the
	 * existing MySQL group's markup, test id and copy stay byte-for-byte
	 * unchanged. The MariaDB group renders its own instance of this same
	 * component with `engine="mariadb"` rather than a second, near-duplicate
	 * one — the row/credentials precedent this task follows.
	 *
	 * `installable` (fix wave item 2) IS gated on `awaitingRelease`/
	 * `unavailable`, unlike the paragraph above: the audit found this
	 * component pitching "Install {label} to get started" — and describing
	 * the download mechanism, Homebrew mention included — directly above a
	 * row that offers no Install control at all in either of those two
	 * states. Whether a *particular host* can install right now is still a
	 * per-row fact this component does not diagnose (it names no target, no
	 * release tag), but it must stop claiming an action, or a mechanism, that
	 * is not actually on offer. Defaults to `true` so every existing caller
	 * that never passes it — every MySQL call site, and every pre-existing
	 * test — renders byte-for-byte what it always did.
	 */
	let {
		engine = 'mysql',
		anyInstalled,
		installable = true
	}: {
		engine?: EngineKind;
		anyInstalled: boolean;
		installable?: boolean;
	} = $props();

	const descriptor = $derived(engineDescriptor(engine));
</script>

{#if !anyInstalled}
	<div class="empty invite" data-testid="databases-no-{descriptor.idPrefix}">
		{#if installable}
			<h3>Install {descriptor.label} to get started</h3>
			<p>{descriptor.installInviteBody}</p>
		{:else}
			<!-- No target, no release tag, no download mechanism named here —
			     the row below already carries that detail (its own
			     `awaitingRelease`/`unavailable` notice), and this invite must not
			     repeat, or drift from, whatever that says. -->
			<h3>{descriptor.label} cannot be installed here right now</h3>
			<p>See the explanation below.</p>
		{/if}
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
