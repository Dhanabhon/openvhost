<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		blockerMessage,
		keptSentence,
		mayProceed,
		refusalHeadline,
		uninstallConfirmLabel,
		uninstallLead,
		uninstallTitle,
		type UninstallPlan
	} from '$lib/uninstall.derive';
	import type { UiLog } from '$lib/uninstall.svelte';
	import Button from './Button.svelte';
	import LogPane from './LogPane.svelte';

	let {
		plan,
		planning = false,
		uninstalling = false,
		error = '',
		log = [],
		onCancel,
		onConfirm
	}: {
		/** What Rust says this uninstall would do. `null` while the (pure, spawn-
		 *  free) plan query is in flight, and after it failed — this dialog never
		 *  offers to proceed on the strength of a plan it does not have. */
		plan: UninstallPlan | null;
		planning?: boolean;
		uninstalling?: boolean;
		error?: string;
		log?: UiLog[];
		onCancel: () => void;
		onConfirm: () => void;
	} = $props();

	/** Design D3: a blocker is a REFUSAL. When this is false the confirm button
	 *  is not disabled — it is not rendered at all, so there is nothing to
	 *  re-enable in a devtools console and no "force" affordance to find. */
	const proceedable = $derived(mayProceed(plan));
	const blocked = $derived(plan !== null && plan.blockers.length > 0);
	/** Every refusal, already turned into copy by the exhaustive
	 *  `blockerMessage` — the template deliberately does no branching over
	 *  `Blocker` itself, so a new variant cannot reach the user as a blank row. */
	const refusals = $derived((plan?.blockers ?? []).map(blockerMessage));

	let dialog = $state<HTMLElement | null>(null);

	onMount(() => {
		// Focus the container, not a button — the QuitDialog lesson: a stray
		// Enter or Space right after this opens must activate nothing, and
		// script-focusing a control leaves a focus ring the user never navigated
		// to (see `QuitDialog.svelte`'s own note for the full reasoning).
		dialog?.focus();
	});

	function onKeydown(e: KeyboardEvent): void {
		if (e.key === 'Escape') {
			e.preventDefault();
			// Cancel is refused by the store while the uninstall is running; the
			// key is still handled so the dialog never looks frozen.
			onCancel();
			return;
		}
		if (e.key !== 'Tab' || dialog === null) return;
		const els = Array.from(dialog.querySelectorAll<HTMLElement>('button:not(:disabled)'));
		if (els.length === 0) return;
		const first = els[0];
		const last = els[els.length - 1];
		if (e.shiftKey && (document.activeElement === dialog || document.activeElement === first)) {
			e.preventDefault();
			last.focus();
		} else if (!e.shiftKey && document.activeElement === last) {
			e.preventDefault();
			first.focus();
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<!-- `aria-hidden` + no click handler, the QuitDialog rule: this guards a
     destructive decision, and the same stray click that dismisses a drawer
     harmlessly would here dismiss a decision the user has not made yet. -->
<div class="scrim" aria-hidden="true"></div>

<div
	class="dialog"
	role="dialog"
	aria-modal="true"
	aria-labelledby="uninstall-title"
	bind:this={dialog}
	data-testid="uninstall-dialog"
	tabindex="-1"
>
	<h2 id="uninstall-title">
		{plan === null ? 'Uninstall' : uninstallTitle(plan.kind, plan.major)}
	</h2>

	{#if plan === null}
		<p class="body">
			{#if planning}
				Checking what this would remove…
			{:else if error === ''}
				Nothing to uninstall.
			{:else}
				This could not be checked, so nothing has been changed.
			{/if}
		</p>
	{:else if blocked}
		<!-- Design D3: names the obstacle and what to do about it, and offers no
		     way past it. There is no `--force`; a user who wants this gone does
		     the same work with the consequences visible. -->
		<p class="body refusal" data-testid="uninstall-refused">
			{refusalHeadline(plan.kind, plan.major)}
		</p>
		<ul class="refusals">
			{#each refusals as refusal, i (i)}
				<li data-blocker={refusal.kind}>
					<span class="obstacle">{refusal.obstacle}</span>
					<span class="action">{refusal.action}</span>
				</li>
			{/each}
		</ul>
	{:else}
		<p class="body">{uninstallLead(plan.kind, plan.major)}</p>
		<!-- The sentence that makes this safe to click. Rendered from the plan's
		     own `keeps`, so it cannot drift from what the executor actually
		     spares (design D2/D6). -->
		<p class="body kept" data-testid="uninstall-kept-sentence">
			{keptSentence(plan.kind, plan.major, plan.keeps)}
		</p>

		<div class="inventory">
			<section aria-labelledby="uninstall-removes">
				<h3 id="uninstall-removes">Removed</h3>
				<ul class="removes">
					{#each plan.removes as item, i (i)}
						<li>{item}</li>
					{/each}
				</ul>
			</section>
			<section aria-labelledby="uninstall-keeps">
				<h3 id="uninstall-keeps">Kept</h3>
				<ul class="keeps">
					{#each plan.keeps as item, i (i)}
						<li>
							{item.what}{#if item.path !== null}
								<span class="mono">{item.path}</span>{/if}
						</li>
					{/each}
				</ul>
			</section>
		</div>
	{/if}

	{#if log.length > 0}
		<!-- Not gated on `uninstalling`: a failed run resets that flag before this
		     re-renders, and the output is exactly what a failure needs read back
		     (the `LanguageRow.svelte` C1 lesson). -->
		<LogPane logs={log} />
	{/if}

	{#if error !== ''}
		<!-- `white-space: pre-wrap` inline as well as scoped: brew's stderr is
		     multi-line, and the SSR test harness never sees scoped styles. -->
		<p class="error" role="alert" data-testid="uninstall-error" style="white-space: pre-wrap">
			{error}
		</p>
	{/if}

	<div class="actions">
		<Button variant="quiet" testId="uninstall-cancel" disabled={uninstalling} onclick={onCancel}>
			{proceedable ? 'Cancel' : 'Close'}
		</Button>
		{#if proceedable}
			<Button
				variant="primary"
				testId="uninstall-confirm"
				ariaLabel={plan === null
					? undefined
					: `Uninstall ${plan.kind === 'php' ? 'PHP' : 'MySQL'} ${plan.major}`}
				disabled={uninstalling}
				onclick={onConfirm}
			>
				{uninstallConfirmLabel(uninstalling)}
			</Button>
		{/if}
	</div>
</div>

<style>
	/* Backdrop + centred card, the same recipe QuitDialog/ApplyDialog use, so
	   all three overlays read as one system. Sized between the two: this has
	   more to say than the quit confirmation but never a diff's worth, and it
	   still has to work in a 380px panel — hence the viewport-relative floor. */
	.scrim {
		position: fixed;
		inset: 0;
		background: var(--vh-scrim);
		backdrop-filter: blur(2px);
		z-index: var(--vh-z-dialog-backdrop);
	}
	.dialog {
		position: fixed;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		z-index: var(--vh-z-dialog);
		width: min(560px, calc(100vw - 2 * var(--vh-space-6)));
		max-height: min(80vh, 720px);
		overflow-y: auto;
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		box-shadow: var(--vh-shadow-overlay);
		padding: var(--vh-space-6);
	}
	/* The container takes focus on open and is `tabindex="-1"`, so the global
	   `:focus-visible` ring would draw around the whole card for a target no
	   keyboard user can reach — suppressed here and only here, exactly as
	   QuitDialog does. Every control inside keeps the global ring. */
	.dialog:focus,
	.dialog:focus-visible {
		outline: none;
	}
	.dialog h2 {
		font-size: var(--vh-text-section);
		font-weight: 600;
		margin: 0;
	}
	.dialog h3 {
		font-size: var(--vh-text-caption);
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--vh-text-2);
		margin: 0 0 var(--vh-space-2);
	}
	.body {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		line-height: 1.6;
		margin: var(--vh-space-2) 0 0;
	}
	/* The one sentence that has to be read, so it gets the page's primary text
	   colour and weight while the lead above it stays secondary — hierarchy by
	   contrast, not by colour alone (brand guidelines §4.3). */
	.body.kept {
		color: var(--vh-text);
		font-weight: 500;
	}
	/* A refusal is not a failure of the app, and it is not decoration either:
	   amber, the "needs attention, not broken" tone `MysqlRow.svelte`'s
	   `.note.warn` already vouches for at 4.68:1. */
	.body.refusal {
		color: var(--vh-start);
		font-weight: 500;
	}
	.refusals {
		list-style: none;
		margin: var(--vh-space-3) 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--vh-space-3);
	}
	.refusals li {
		border: 1px solid color-mix(in oklab, var(--vh-start) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		font-size: var(--vh-text-table);
		line-height: 1.6;
	}
	.refusals .obstacle {
		display: block;
		color: var(--vh-text);
	}
	.refusals .action {
		display: block;
		margin-top: 4px;
		color: var(--vh-text-2);
	}
	/* Two columns on a comfortable width, one on a narrow panel — the 380px
	   floor this app supports is well under the 420px break, so the two lists
	   stack there rather than shrinking into unreadable columns. */
	.inventory {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
		gap: var(--vh-space-4);
		margin-top: var(--vh-space-4);
	}
	.inventory ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
		font-size: var(--vh-text-table);
		line-height: 1.5;
	}
	.inventory li {
		padding-left: var(--vh-space-4);
		position: relative;
		color: var(--vh-text);
	}
	/* A leading glyph per row rather than a bullet, so "removed" and "kept" are
	   distinguishable without relying on colour (brand guidelines §4.3). */
	.removes li::before {
		content: '−';
		position: absolute;
		left: 0;
		color: var(--vh-fail);
	}
	.keeps li::before {
		content: '✓';
		position: absolute;
		left: 0;
		color: var(--vh-run);
	}
	.mono {
		display: block;
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-log);
		color: var(--vh-text-2);
		word-break: break-all;
	}
	.error {
		white-space: pre-wrap;
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: var(--vh-space-4) 0 0;
		font-size: var(--vh-text-table);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: var(--vh-space-2);
		margin-top: var(--vh-space-6);
	}
</style>
