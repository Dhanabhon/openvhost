// SPDX-License-Identifier: GPL-3.0-or-later
// How far this launch got, and what the layout should therefore render.
//
// The frontend half of the degraded-boot slice (design D2). `bootstrap` returns
// a `BootState` on every path and `setup` manages it exactly once, outside every
// bail arm, so `boot_status` answers even on a launch where almost nothing else
// does — which is the whole reason the takeover can be driven from a command at
// all.
//
// FOUR states, not two, and none of them is a boolean. This project has been
// bitten five times by a boolean standing where a state belongs, and here the
// collapse would be actively harmful: "we have not asked yet" and "the ask
// failed" have OPPOSITE right answers, and folding either into "not ready"
// would blank a healthy app.
//
// DOM-free and api-injected, the same shape as `store-status.svelte.ts`: the
// layout calls `load()`, and this module's tests hand it a fake.
import type { BootStatusDto, IpcError } from './ipc';

/**
 * Every boot state except `ready` — the ones that get a takeover screen.
 *
 * Derived from the generated union with `Exclude` rather than restated, so a
 * fifth `BootStatusDto` variant lands here automatically and then fails to
 * compile in {@link bootRendering}'s switch and in `boot-takeover.derive.ts`'s,
 * exactly as a fifth `BootState` fails to compile in `boot_dto` and
 * `stderr_line` on the Rust side.
 */
export type DegradedBoot = Exclude<BootStatusDto, { kind: 'ready' }>;

/**
 * What `+layout.svelte` renders, decided in one place.
 *
 * `app` and `appDespiteFailedAsk` both render the children and are deliberately
 * NOT one case: only the second one also owes the user a banner, and a caller
 * that treated them as interchangeable would drop it silently.
 */
export type BootRendering =
	| { kind: 'pending' }
	| { kind: 'app' }
	| { kind: 'appDespiteFailedAsk'; error: IpcError }
	| { kind: 'takeover'; boot: DegradedBoot };

/**
 * The rendering decision, as a pure function of the two things the store knows.
 *
 * **The one rule that must NOT be copied from the store slice.**
 * `store-status.svelte.ts` treats a failed ask as silence, and that is right
 * there and wrong here, because the failure modes are opposite: a banner that
 * speaks on a failed ask makes a false claim about a healthy machine, whereas a
 * TAKEOVER on a failed ask hides a working app behind a screen that has nothing
 * true to say. So a failed ask renders the children — plus a banner, because
 * "we could not tell whether this launch worked" is worth saying out loud even
 * though it is not worth blanking the window over. Do not "fix" the
 * inconsistency between the two modules; it is the point.
 *
 * `pending` renders neither. It is a separate state from a failed ask rather
 * than a shortcut to one, and that is what makes spec §9.1 — no page shows
 * Tauri's `.manage()` string in any of the three degraded states — structural
 * instead of a race: with `pending` rendering the app, a degraded launch would
 * mount the real pages, fire the commands that cannot answer, and depend on
 * `boot_status` winning that race to replace what they rendered.
 */
export function bootRendering(
	status: BootStatusDto | null,
	askFailed: IpcError | null
): BootRendering {
	if (askFailed !== null) return { kind: 'appDespiteFailedAsk', error: askFailed };
	if (status === null) return { kind: 'pending' };
	switch (status.kind) {
		case 'ready':
			return { kind: 'app' };
		case 'alreadyRunning':
		case 'runDirUnusable':
		case 'homeUnresolvable':
			return { kind: 'takeover', boot: status };
		default: {
			// Not a wildcard: nothing can reach this arm, and its only statement is
			// an assignment that stops compiling the moment a fifth variant exists.
			// A `default` that RENDERED something would be the wildcard this repo
			// bans — a new state would silently inherit a fourth state's screen.
			const unreachable: never = status;
			return unreachable;
		}
	}
}

export interface BootStatusApi {
	bootStatus: () => Promise<BootStatusDto>;
}

export class BootStatusStore {
	/**
	 * The answer, or `null` before it arrives and after an ask that itself
	 * failed. `null` is never "everything is fine" here — see
	 * {@link bootRendering}, which reads `askFailed` first precisely so those two
	 * meanings of `null` cannot be confused.
	 */
	status = $state<BootStatusDto | null>(null);
	/**
	 * The failed ask itself. Unlike `StoreStatusStore.lastError`, this one IS
	 * rendered — as its own banner, never as a boot state.
	 */
	askFailed = $state<IpcError | null>(null);

	constructor(private api: BootStatusApi) {}

	async load(): Promise<void> {
		try {
			this.status = await this.api.bootStatus();
			this.askFailed = null;
		} catch (e) {
			// Back to "we could not tell", and loudly: `status` is cleared so a
			// stale answer cannot outlive the ask that could no longer confirm it,
			// and `askFailed` is what the banner renders.
			this.status = null;
			this.askFailed = e as IpcError;
		}
	}
}
