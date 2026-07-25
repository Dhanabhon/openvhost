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
}

function isValidation(e: unknown): e is { kind: 'validation'; field: string; message: string } {
	return typeof e === 'object' && e !== null && (e as { kind?: unknown }).kind === 'validation';
}

export class SitesStore {
	sites = $state<SiteDto[]>([]);
	error = $state<IpcError | null>(null);
	fieldErrors = $state<Record<string, string>>({});

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
}
