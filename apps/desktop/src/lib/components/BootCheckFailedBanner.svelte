<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	// The half of the degraded-boot slice that must NOT copy the store slice.
	//
	// `store-status.svelte.ts` treats a failed ask as silence, and that is right
	// for a BANNER: speaking there would make a false claim about a healthy
	// machine. It is wrong here, because the two failure modes are opposite —
	// what `boot_status` gates is a full-window takeover, and blanking a working
	// app over one unanswered question is a worse failure than the one this
	// slice exists to fix. So a failed ask renders the app AND says so.
	//
	// Do not "fix" the inconsistency between the two modules. See
	// `bootRendering` in `boot-status.svelte.ts` for the full reasoning; this
	// component is the visible half of that decision.
	//
	// `error` is the failed ask itself, rendered through `errorMessage` — the
	// same discipline every other failure surface here applies: whatever the
	// transport said, verbatim, never parsed and never replaced by a friendlier
	// guess.
	import { errorMessage } from '$lib/errors';
	import type { IpcError } from '$lib/ipc';

	let { error }: { error: IpcError | null } = $props();

	/** Tauri's own refusal for a command whose state was never managed — the
	 *  sentence this entire line of work exists to stop showing a user. */
	const UNMANAGED_STATE = '.manage()';
	/** What to say instead. It claims strictly less than the string it replaces
	 *  and nothing that is not true of every way this could arise. */
	const UNMANAGED_STATE_SUBSTITUTE =
		'OpenVHost did not get far enough through startup to record how the launch went.';

	// The ONE place a message may be replaced rather than rendered verbatim, and
	// it is a floor rather than a paraphrase: every other transport error still
	// goes through untouched, which the tests pin from both sides.
	//
	// Unreachable today, and deliberately guarded anyway. `error` is whatever
	// `boot_status` threw, and the one path on which that could be Tauri's
	// unmanaged-state string is `BootState` itself going unmanaged — which
	// `lib.rs:406` makes impossible: the `app.manage(boot)` there is
	// unconditional, `bootstrap` returns no `Result` to bail out of, and the main
	// thread is blocked inside `setup` so no invoke can dispatch before it runs.
	// But that is a chain of three reasons, not a test; Tauri creates the window
	// and the webview BEFORE running setup, and bootstrap measures 270-390 ms. If
	// any link ever breaks, the failure mode is this banner rendering
	// *"…You must call `.manage()` before using this command"* beside a
	// fully-mounted app — the pre-branch bug plus a banner, in the slice built to
	// delete it. `boot.rs`'s `reveal_run_dir_target_never_names_a_rust_api_at_the_user`
	// pins the same property for that command's refusals; this is its other half.
	const detail = $derived.by(() => {
		const message = errorMessage(error);
		return message.includes(UNMANAGED_STATE) ? UNMANAGED_STATE_SUBSTITUTE : message;
	});
</script>

{#if error !== null}
	<!-- `.banner-error` + role="alert", the pairing ErrorBanner/ApplyErrorBanner/
	     StoreUnavailableBanner already use, rather than SiteReadinessBanner's
	     accent-tinted role="status": something HAS failed, and what it failed to
	     establish is whether the rest of this window can be trusted at all. That
	     is worth interrupting for. Structure, spacing and tokens are the shared
	     banner recipe — this introduces no new visual language. -->
	<div class="banner-error" role="alert" data-testid="boot-check-failed-banner">
		<strong>OpenVHost could not check how far this launch got</strong>
		<!-- The transport's own words first, for the same reason
		     StoreUnavailableBanner leads with its reason: it is the only line that
		     can point at a cause. -->
		<span class="detail" data-testid="boot-check-failed-reason">{detail}</span>
		<span class="line">
			The rest of this window is showing as usual, because hiding a working app over one unanswered
			question would be the worse mistake.
		</span>
		<span class="line">
			If pages refuse to load or report errors that name things you have never heard of, this is why
			OpenVHost cannot say so plainly. Reopening it is the thing to try.
		</span>
	</div>
{/if}

<style>
	/* The `.fail-detail` failure-surface recipe from docs/design/mock.css, reused
	   verbatim from ErrorBanner.svelte and StoreUnavailableBanner.svelte — same
	   margins, padding, radius and font size as every other banner in this
	   stack, so this reads as one more banner rather than a one-off. */
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
		/* A transport error can name a path with no spaces in it; without this the
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
