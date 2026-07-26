// SPDX-License-Identifier: GPL-3.0-or-later
// Sites panel state. Mutations refetch the list (there is no site event
// stream), and a per-field validation error is surfaced separately from a
// general error so the form can mark the offending input.
import type { IpcError, SiteDto, SiteInput } from './ipc';

export interface SitesApi {
	listSites(): Promise<SiteDto[]>;
	createSite(input: SiteInput): Promise<SiteDto>;
	updateSite(id: string, input: SiteInput): Promise<SiteDto>;
	deleteSite(id: string): Promise<boolean>;
	openSite(id: string): Promise<void>;
}

function isValidation(e: unknown): e is { kind: 'validation'; field: string; message: string } {
	return typeof e === 'object' && e !== null && (e as { kind?: unknown }).kind === 'validation';
}

/**
 * A renderable message for any thrown value.
 *
 * Deliberately not `(e as IpcError).message ?? String(e)`: `IpcError`'s `simulated`
 * variant carries no `message`, and `String(e)` on an object renders the literal
 * text "[object Object]" on the row. A fixed fallback is worse copy but never
 * nonsense.
 */
function errorMessage(e: unknown): string {
	if (typeof e === 'object' && e !== null && 'message' in e) {
		const m = (e as { message?: unknown }).message;
		if (typeof m === 'string' && m !== '') return m;
	}
	return 'The command failed.';
}

export class SitesStore {
	sites = $state<SiteDto[]>([]);
	error = $state<IpcError | null>(null);
	fieldErrors = $state<Record<string, string>>({});
	/**
	 * PER-ROW failure, keyed by site id — separate from `error`, which is page-level.
	 * A row action that fails must not blank the whole list, and the message has to
	 * appear on the row the user acted on rather than in a banner above everything.
	 */
	rowError = $state<Record<string, string>>({});
	/**
	 * In-flight row operations, keyed by site id. Enforced HERE and not only via a
	 * `disabled` attribute on the button: a UI attribute is the wrong place for the
	 * only copy of a re-entrancy guard, because deleting it leaves no test failing.
	 */
	busy = $state<Record<string, boolean>>({});

	constructor(private api: SitesApi) {}

	/** Clear both error channels before a new attempt. */
	private reset(): void {
		this.error = null;
		this.fieldErrors = {};
	}

	/** Clear both error channels. Call when opening a fresh drawer session so a
	 *  previous attempt's per-field error can't render on a new/blank form. */
	clearErrors(): void {
		this.reset();
	}

	async load(): Promise<void> {
		this.reset();
		try {
			this.sites = await this.api.listSites();
		} catch (e) {
			this.error = e as IpcError;
		}
	}

	/** `id === null` creates, otherwise updates. Returns true on success. */
	async save(id: string | null, input: SiteInput): Promise<boolean> {
		this.reset();
		try {
			if (id === null) await this.api.createSite(input);
			else await this.api.updateSite(id, input);
		} catch (e) {
			if (isValidation(e)) {
				this.fieldErrors = { [e.field]: e.message };
			} else {
				this.error = e as IpcError;
			}
			return false;
		}
		await this.load();
		return true;
	}

	/**
	 * Delete a site. A `false` result means the row was already gone — still a
	 * success from the user's point of view, so the list just refetches.
	 */
	async remove(id: string): Promise<boolean> {
		this.reset();
		try {
			await this.api.deleteSite(id);
		} catch (e) {
			this.error = e as IpcError;
			return false;
		}
		await this.load();
		return true;
	}

	/**
	 * One row-scoped mutation: guard re-entrancy, clear that row's error, run, and
	 * refetch on success. Failures land on `rowError[id]`, never on `error`.
	 *
	 * `remove` above is left alone on purpose — the drawer's danger zone calls it and
	 * needs its failure inside the drawer, not on a row behind the scrim.
	 */
	private async mutateRow(id: string, op: () => Promise<unknown>): Promise<boolean> {
		if (this.busy[id] === true) return false;
		this.busy = { ...this.busy, [id]: true };
		this.rowError = { ...this.rowError, [id]: '' };
		try {
			await op();
		} catch (e) {
			this.rowError = { ...this.rowError, [id]: errorMessage(e) };
			return false;
		} finally {
			// `finally`, so an early `return false` in the catch cannot leave the row
			// stuck busy and its buttons disabled for the rest of the session.
			this.busy = { ...this.busy, [id]: false };
		}
		await this.load();
		return true;
	}

	/**
	 * Flip a site's `enabled` flag.
	 *
	 * Goes through `updateSite` rather than a dedicated `set_site_enabled` command for
	 * two reasons. Adding an IPC command is merge-blocked behind a security-auditor
	 * gate (CLAUDE.md golden rule 2) — a steep price for a toggle. And `update_site`
	 * already re-reads the row, rebuilds from the stored `id`/`created_at`, and
	 * re-validates every field through `TryFrom<SiteInput>`, so the toggle crosses the
	 * same ingress guard an edit does instead of getting a narrower path of its own.
	 *
	 * The cost is a whole-object write: every field round-trips. Fine for a
	 * single-user desktop app with no second editor to clobber.
	 */
	async setEnabled(site: SiteDto, enabled: boolean): Promise<boolean> {
		return this.mutateRow(site.id, () =>
			this.api.updateSite(site.id, {
				name: site.name,
				// `SiteInput.domain` is the FULL domain, same as `SiteDto.domain` — no
				// `splitDomain` round-trip, which would risk rewriting a custom TLD.
				domain: site.domain,
				docroot: site.docroot,
				webServer: site.webServer,
				phpVersion: site.phpVersion,
				enabled
			})
		);
	}

	/** Delete from the list row, surfacing failure on that row. */
	async removeRow(id: string): Promise<boolean> {
		return this.mutateRow(id, () => this.api.deleteSite(id));
	}

	/**
	 * Open a site in the browser.
	 *
	 * Deliberately NOT routed through `mutateRow`: opening a browser changes nothing
	 * in state.db, so the refetch `mutateRow` performs on success would be a round
	 * trip that can only produce the list we already have. The busy guard and the
	 * per-row error channel are still worth reusing — a double-click must not open
	 * two tabs, and a failure belongs on the row rather than in a page banner.
	 */
	async open(id: string): Promise<boolean> {
		if (this.busy[id] === true) return false;
		this.busy = { ...this.busy, [id]: true };
		this.rowError = { ...this.rowError, [id]: '' };
		try {
			await this.api.openSite(id);
		} catch (e) {
			this.rowError = { ...this.rowError, [id]: errorMessage(e) };
			return false;
		} finally {
			this.busy = { ...this.busy, [id]: false };
		}
		return true;
	}
}
