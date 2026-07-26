<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import type { ApplyOutcomeDto, FileChangeDto } from '$lib/ipc';
	import Button from './Button.svelte';

	let {
		changes,
		applying = false,
		error = '',
		outcome = null,
		onApply,
		onClose
	}: {
		changes: readonly FileChangeDto[];
		applying?: boolean;
		error?: string;
		outcome?: ApplyOutcomeDto | null;
		onApply: () => void;
		onClose: () => void;
	} = $props();

	/**
	 * `needsAttention` did not exist when this dialog's shape was first sketched:
	 * a later security/reliability review found that Apply could stop a service
	 * and fail to bring it back while `apply_sites` still resolved successfully.
	 * A non-empty list means the apply did NOT fully succeed, so it is rendered
	 * INSTEAD of the plain success message, never alongside it — presenting both
	 * would bury the thing the user has to act on under a headline that says
	 * "Applied.".
	 */
	const needsAttention = $derived(outcome?.needsAttention ?? []);
	const hasNeedsAttention = $derived(needsAttention.length > 0);

	let dialog = $state<HTMLElement | null>(null);

	/**
	 * Focus/Tab-trap machinery, deliberately mirroring `QuitDialog.svelte`'s
	 * (see that component's deviation note) rather than a fresh implementation —
	 * this is the second modal in the app, and the pattern already exists.
	 * Diffs from QuitDialog: this dialog's file list scrolls internally, so
	 * `querySelectorAll('button:not(:disabled)')` would also pick up any
	 * future interactive element inside `.files`; there is none today (the
	 * diff view is read-only), so the two buttons in `footer` remain the
	 * entire focusable set.
	 */
	onMount(() => {
		// Focus Close, not Apply: same reasoning as QuitDialog focusing Cancel —
		// a stray Enter/Space right after the dialog opens must not fire the
		// mutating action.
		dialog?.querySelector<HTMLElement>('button')?.focus();
	});

	function onKeydown(e: KeyboardEvent): void {
		if (e.key === 'Escape') {
			e.preventDefault();
			onClose();
			return;
		}
		if (e.key !== 'Tab' || dialog === null) return;
		const els = Array.from(dialog.querySelectorAll<HTMLElement>('button:not(:disabled)'));
		if (els.length === 0) return;
		const first = els[0];
		const last = els[els.length - 1];
		if (e.shiftKey && document.activeElement === first) {
			e.preventDefault();
			last.focus();
		} else if (!e.shiftKey && document.activeElement === last) {
			e.preventDefault();
			first.focus();
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<div class="scrim">
	<div class="dialog" role="dialog" aria-modal="true" aria-label="Apply changes" bind:this={dialog}>
		<header>
			<h2>Apply changes</h2>
			<p class="sub">{changes.length} {changes.length === 1 ? 'file' : 'files'}</p>
		</header>

		<div class="files">
			{#each changes as c (c.path)}
				<article class="file" data-kind={c.kind}>
					<div class="path">
						<span class="badge" data-kind={c.kind}>{c.kind}</span>
						<span class="mono">{c.path}</span>
					</div>
					<!-- Split per line so an added/removed line can carry its own colour; a
					     single <pre> could only be coloured as a whole. Every value here goes
					     through Svelte's `{...}` text interpolation, which escapes it — the
					     diff carries generated config and user-controlled docroot paths, so
					     `{@html}` is not used anywhere in this file (see the IPC-surface
					     security review that flagged this). -->
					<pre class="diff">{#each c.diff.split('\n') as line, i (i)}<span
								class="line"
								data-line={line.startsWith('+') ? 'add' : line.startsWith('-') ? 'del' : 'ctx'}
								>{line}</span
							>{/each}</pre>
				</article>
			{/each}
		</div>

		{#if error !== ''}
			<!-- pre-wrap: nginx's stderr is multi-line and ran off-screen when it was
			     rendered as a single line (the ServiceRow lesson). This also carries a
			     failed `plan_site_apply` (MissingRuntime / NotAPlainFile): that error
			     arrives with `changes` empty, and without this the dialog would show an
			     empty file list and nothing explaining why. -->
			<!-- `white-space: pre-wrap` is ALSO set inline here, duplicating the scoped
			     `.error` rule below: this project's SSR test harness (`svelte/server`
			     `render()`) returns component markup only — scoped `<style>` is
			     extracted to a stylesheet by the bundler and is invisible to that
			     harness — so the inline copy is what `ApplyDialog.svelte.test.ts`
			     actually asserts on, while the scoped rule is what the browser applies. -->
			<p class="error" role="alert" data-testid="apply-error" style="white-space: pre-wrap">
				{error}
			</p>
		{/if}

		{#if hasNeedsAttention}
			<!-- Unconditional on `changes`/`error`: a service the pipeline could not
			     restart still needs the user's eyes even if new, unapplied changes
			     have appeared since this outcome was produced. Rendered INSTEAD of
			     `.ok` below, never alongside it — see the `needsAttention` doc
			     comment above. -->
			<div class="warn" data-testid="needs-attention" role="alert">
				<strong>Needs your attention</strong>
				<ul>
					{#each needsAttention as problem (problem.id)}
						<li><span class="mono">{problem.id}</span>: {problem.reason}</li>
					{/each}
				</ul>
			</div>
		{:else if outcome && changes.length === 0 && error === ''}
			<!-- `outcome` describes the LAST apply, not the dialog's current state —
			     it goes stale the instant there is something pending again (the user
			     closed the dialog, edited an unrelated site, and reopened it) or the
			     automatic re-plan inside `run()` threw after a successful apply. Either
			     case would otherwise show "Applied." next to a file that is not live,
			     or alongside the error explaining why the re-plan failed. -->
			<p class="ok" data-testid="apply-success" role="status">
				Applied.
				{#if outcome.restarted.length > 0}Restarted {outcome.restarted.join(', ')}.{/if}
				{#if outcome.notStarted.length > 0}
					{outcome.notStarted.join(', ')} was not running — the new config applies next time it starts.
				{/if}
			</p>
		{/if}

		<footer>
			<Button onclick={onClose}>Close</Button>
			<Button variant="primary" disabled={applying || changes.length === 0} onclick={onApply}>
				{applying ? 'Applying…' : 'Apply'}
			</Button>
		</footer>
	</div>
</div>

<style>
	/* Backdrop + centred card, same recipe as QuitDialog.svelte (z-dialog-backdrop/
	   z-dialog, centred via fixed + translate). This dialog can outgrow the quit
	   dialog's fixed width — a diff can be long — so it sizes off the viewport
	   instead of a fixed px width. */
	.scrim {
		position: fixed;
		inset: 0;
		background: var(--vh-scrim);
		backdrop-filter: blur(2px);
		z-index: var(--vh-z-dialog-backdrop);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: var(--vh-space-6);
	}
	.dialog {
		position: relative;
		z-index: var(--vh-z-dialog);
		width: min(720px, 100%);
		max-height: min(80vh, 720px);
		display: flex;
		flex-direction: column;
		background: var(--vh-surface);
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-card);
		box-shadow: var(--vh-shadow-overlay);
		padding: var(--vh-space-6);
	}
	.dialog header h2 {
		font-size: var(--vh-text-section);
		font-weight: 600;
		margin: 0;
	}
	.dialog .sub {
		color: var(--vh-text-2);
		font-size: var(--vh-text-table);
		margin: 2px 0 0;
	}
	.files {
		flex: 1;
		overflow-y: auto;
		margin: var(--vh-space-4) 0;
		display: flex;
		flex-direction: column;
		gap: var(--vh-space-3);
	}
	.file {
		border: 1px solid var(--vh-border);
		border-radius: var(--vh-radius-control);
		overflow: hidden;
	}
	.file .path {
		display: flex;
		align-items: center;
		gap: var(--vh-space-2);
		padding: var(--vh-space-2) var(--vh-space-3);
		background: var(--vh-surface-2);
		border-bottom: 1px solid var(--vh-border);
		font-size: var(--vh-text-table);
	}
	.badge {
		display: inline-flex;
		align-items: center;
		padding: 1px 8px;
		border-radius: var(--vh-radius-pill);
		font-size: var(--vh-text-caption);
		font-weight: 600;
		border: 1px solid var(--vh-border);
		text-transform: capitalize;
	}
	.badge[data-kind='added'] {
		color: var(--vh-diff-add-text);
		background: var(--vh-diff-add-bg);
		border-color: color-mix(in oklab, var(--vh-diff-add-text) 35%, transparent);
	}
	.badge[data-kind='removed'] {
		color: var(--vh-diff-del-text);
		background: var(--vh-diff-del-bg);
		border-color: color-mix(in oklab, var(--vh-diff-del-text) 35%, transparent);
	}
	.badge[data-kind='modified'] {
		color: var(--vh-text-2);
		background: var(--vh-surface);
	}
	.diff {
		margin: 0;
		padding: var(--vh-space-2) 0;
		white-space: pre;
		overflow-x: auto;
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-log);
		line-height: 1.5;
	}
	.diff .line {
		display: block;
		padding: 0 var(--vh-space-3);
	}
	.diff .line[data-line='add'] {
		background: var(--vh-diff-add-bg);
		color: var(--vh-diff-add-text);
	}
	.diff .line[data-line='del'] {
		background: var(--vh-diff-del-bg);
		color: var(--vh-diff-del-text);
	}
	.error {
		white-space: pre-wrap;
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 0 var(--vh-space-4);
		font-size: var(--vh-text-table);
	}
	.ok {
		color: var(--vh-run);
		background: var(--vh-add-tint);
		border: 1px solid color-mix(in oklab, var(--vh-run) 35%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 0 var(--vh-space-4);
		font-size: var(--vh-text-table);
	}
	/* Never `.ok`'s green — a needsAttention outcome is not a success, and
	   colour must not be the only carrier of that (brand guidelines §4.2), so
	   it also gets a distinct heading and `role="alert"` rather than
	   `role="status"`. */
	.warn {
		color: var(--vh-fail);
		background: var(--vh-fail-tint);
		border: 1px solid color-mix(in oklab, var(--vh-fail) 45%, transparent);
		border-radius: var(--vh-radius-control);
		padding: var(--vh-space-3);
		margin: 0 0 var(--vh-space-4);
		font-size: var(--vh-text-table);
	}
	.warn strong {
		display: block;
		margin-bottom: 4px;
	}
	.warn ul {
		margin: 0;
		padding-left: var(--vh-space-4);
	}
	footer {
		display: flex;
		justify-content: flex-end;
		gap: var(--vh-space-2);
	}
</style>
