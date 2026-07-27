// SPDX-License-Identifier: GPL-3.0-or-later
//
// State for the Web server page's settings form. Shaped like `ApplyStore`
// (`apply.svelte.ts`): the api is injected so the store is testable without
// Tauri, and every guard lives here rather than on a `disabled` attribute —
// deleting an attribute leaves no test failing.
import type { ApplyPlanDto, WebServerSettingsDto } from './ipc';
import {
	errorKey,
	parseCount,
	sameSettings,
	NOT_A_NUMBER,
	type BoolKey,
	type NumberKey,
	type TextKey
} from './websettings.derive';

export interface WebSettingsApi {
	webServerSettings(): Promise<WebServerSettingsDto>;
	saveWebServerSettings(input: WebServerSettingsDto): Promise<void>;
	/**
	 * What Apply would change across the whole generated config. Called after a
	 * successful save so the diff the user is about to be shown describes what
	 * was actually STORED, not what is sitting in the form.
	 *
	 * On the page this is wired through `ApplyStore.refresh()` rather than
	 * straight to the IPC function, so the page keeps one change list — see the
	 * comment on the route's adapter.
	 */
	planConfigApply(): Promise<ApplyPlanDto>;
}

function isValidation(e: unknown): e is { kind: 'validation'; field: string; message: string } {
	return typeof e === 'object' && e !== null && (e as { kind?: unknown }).kind === 'validation';
}

/** A renderable message for any thrown value — same fallback reasoning as
 * `sites.svelte.ts`: `IpcError`'s `simulated` variant carries no `message`, and
 * `String(e)` on an object renders "[object Object]". */
function errorMessage(e: unknown): string {
	if (typeof e === 'object' && e !== null && 'message' in e) {
		const m = (e as { message?: unknown }).message;
		if (typeof m === 'string' && m !== '') return m;
	}
	return 'The command failed.';
}

/** Shown when the save landed but the follow-up read did not. Deliberately does
 * NOT say the save failed — it succeeded, and saying otherwise would invite the
 * user to redo something that is already stored. */
const REREAD_FAILED =
	'Saved, but the stored values could not be read back — what you see below may not be exactly what was stored. Reopen this page to check.';

export class WebSettingsStore {
	/** The form's live values. `null` until the first read settles, and left
	 * `null` when that read fails, so the form never renders the defaults as if
	 * they were the stored row. */
	values = $state<WebServerSettingsDto | null>(null);
	/** Page-level failure — a failed read, a failed save that named no field. */
	error = $state('');
	saving = $state(false);

	/** The last known STORED snapshot, for the dirty flag. */
	private saved = $state<WebServerSettingsDto | null>(null);
	/** Errors the backend sent back, keyed by its own snake_case field name. */
	private serverErrors = $state<Record<string, string>>({});
	/** Errors raised here, before anything is sent: a number box that does not
	 * hold a number. Kept apart from `serverErrors` so `save()` can refuse
	 * while one is outstanding, and so clearing the server's errors at the start
	 * of a save does not clear these too. */
	private localErrors = $state<Record<string, string>>({});

	constructor(private api: WebSettingsApi) {}

	/**
	 * Everything the form should mark, from both channels, keyed the way the
	 * BACKEND names its fields (snake_case). Server errors win a collision: they
	 * describe the value that was actually rejected.
	 */
	get fieldErrors(): Record<string, string> {
		return { ...this.localErrors, ...this.serverErrors };
	}

	/** Whether the form holds anything not yet stored. */
	get dirty(): boolean {
		if (this.values === null || this.saved === null) return false;
		return !sameSettings(this.values, this.saved);
	}

	/**
	 * Whether Save can be pressed at all. Note it does NOT require `dirty`:
	 * saving an unchanged form is how a user reaches the diff for changes that
	 * are pending for another reason (a first launch renders directives the old
	 * config never had), and a Save that greys itself out has no way to explain
	 * that.
	 */
	get canSave(): boolean {
		return !this.saving && this.values !== null && Object.keys(this.localErrors).length === 0;
	}

	async load(): Promise<void> {
		this.error = '';
		this.serverErrors = {};
		this.localErrors = {};
		try {
			const stored = await this.api.webServerSettings();
			this.values = { ...stored };
			this.saved = { ...stored };
		} catch (e) {
			this.error = errorMessage(e);
		}
	}

	/** One field, replaced immutably — `this.values` is never mutated in place. */
	private patch<K extends keyof WebServerSettingsDto>(
		key: K,
		value: WebServerSettingsDto[K]
	): void {
		if (this.values === null) return;
		const next = { ...this.values };
		next[key] = value;
		this.values = next;
	}

	private markLocal(field: string, message: string): void {
		this.localErrors = { ...this.localErrors, [field]: message };
	}

	private clearLocal(field: string): void {
		this.localErrors = Object.fromEntries(
			Object.entries(this.localErrors).filter(([key]) => key !== field)
		);
	}

	/**
	 * Take a number input's raw string value.
	 *
	 * `input.value` is a string even on `type="number"`, and these fields are
	 * `u32` on the Rust side, so the conversion happens here. A box that holds
	 * nothing usable leaves the last good value in place and marks the field —
	 * storing `NaN` would cross the wire as `null` and fail deserialization with
	 * an error naming no field at all.
	 */
	setNumber(key: NumberKey, raw: string): void {
		const field = errorKey(key);
		const parsed = parseCount(raw);
		if (parsed === null) {
			this.markLocal(field, NOT_A_NUMBER);
			return;
		}
		this.clearLocal(field);
		this.patch(key, parsed);
	}

	setBool(key: BoolKey, value: boolean): void {
		this.patch(key, value);
	}

	setText(key: TextKey, value: string): void {
		this.patch(key, value);
	}

	/**
	 * Store the values, then re-read them, then plan.
	 *
	 * Three steps, in that order, each for its own reason:
	 *
	 *  1. **Save.** A rejected field lands on `fieldErrors` under the backend's
	 *     own name and NOTHING is written, so the stored row is untouched.
	 *  2. **Re-read.** `gzip_types` is normalised as it is parsed (lowercased,
	 *     re-joined on single spaces), so the stored value is not byte-identical
	 *     to what was typed. Without this the form would stay dirty forever.
	 *  3. **Plan.** The diff has to describe what was STORED. Planning from the
	 *     form's own values would show a diff for something that never landed.
	 *
	 * `true` means the diff is ready to show. It does NOT mean nginx accepted
	 * anything: no `nginx -t` runs here (see `save_web_server_settings`'s own
	 * doc comment). That check happens inside Apply, which rolls back if it
	 * fails.
	 */
	async save(): Promise<boolean> {
		if (!this.canSave) return false;
		const payload = this.values;
		if (payload === null) return false;
		this.saving = true;
		this.error = '';
		this.serverErrors = {};
		try {
			await this.api.saveWebServerSettings({ ...payload });
			// Adopt what was sent as the baseline BEFORE the re-read, so a
			// failed re-read still leaves the form clean rather than inviting
			// the user to save again what is already stored.
			this.saved = { ...payload };
			await this.reread();
			await this.api.planConfigApply();
		} catch (e) {
			if (isValidation(e)) this.serverErrors = { [e.field]: e.message };
			else this.error = errorMessage(e);
			return false;
		} finally {
			// `finally`, so an early return in the catch cannot leave Save
			// disabled for the rest of the session.
			this.saving = false;
		}
		return true;
	}

	/** Refresh from storage after a save. Failure is reported but not thrown:
	 * the save itself already succeeded, and the diff is still worth showing. */
	private async reread(): Promise<void> {
		try {
			const stored = await this.api.webServerSettings();
			this.values = { ...stored };
			this.saved = { ...stored };
		} catch {
			this.error = REREAD_FAILED;
		}
	}
}
