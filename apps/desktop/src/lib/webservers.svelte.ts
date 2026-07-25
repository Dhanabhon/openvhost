// SPDX-License-Identifier: GPL-3.0-or-later
import {
	listWebServers,
	readWebServerConfig,
	validateWebServerConfig,
	type IpcError,
	type ValidationReportDto,
	type WebServerDto
} from '$lib/ipc';

export interface WebServersApi {
	listWebServers: () => Promise<WebServerDto[]>;
	readWebServerConfig: (id: string) => Promise<string>;
	validateWebServerConfig: (id: string) => Promise<ValidationReportDto>;
}

export class WebServersStore {
	servers = $state<WebServerDto[]>([]);
	/** Page-level failure (the list could not be loaded at all). */
	error = $state<IpcError | null>(null);
	configText = $state<Record<string, string>>({});
	/** PER-ROW failure. Kept off `error` so one row's problem cannot blank the page. */
	configError = $state<Record<string, string>>({});
	reports = $state<Record<string, ValidationReportDto>>({});
	validating = $state<Record<string, boolean>>({});

	constructor(private api: WebServersApi) {}

	async load(): Promise<void> {
		this.error = null;
		try {
			this.servers = await this.api.listWebServers();
		} catch (e) {
			this.error = e as IpcError;
		}
	}

	async showConfig(id: string): Promise<void> {
		this.configError = { ...this.configError, [id]: '' };
		try {
			const text = await this.api.readWebServerConfig(id);
			this.configText = { ...this.configText, [id]: text };
		} catch (e) {
			const message = (e as IpcError & { message?: string }).message ?? String(e);
			this.configError = { ...this.configError, [id]: message };
		}
	}

	async validate(id: string): Promise<void> {
		this.validating = { ...this.validating, [id]: true };
		this.configError = { ...this.configError, [id]: '' };
		// The previous verdict is dropped as the run STARTS, not merely overwritten on
		// success. A validator that cannot even be launched throws an `IpcError`
		// instead of returning a report (see the catch), so without this the row would
		// render a fresh red "validator could not be launched" directly beside a green
		// "Config is valid" from an earlier click — two statements about the SAME
		// operation, one of them stale, with nothing marking it so. Clearing here also
		// means no verdict is on screen while "Validating…" is: whatever is shown is
		// always the answer to the click the user just made. Same principle as the
		// row's stale-config-text suppression, applied to the channel the store owns.
		this.reports = Object.fromEntries(Object.entries(this.reports).filter(([key]) => key !== id));
		try {
			const report = await this.api.validateWebServerConfig(id);
			this.reports = { ...this.reports, [id]: report };
		} catch (e) {
			// A validator that could not even be launched is an IpcError, not a
			// report. It must still reach the row rather than vanishing.
			const message = (e as IpcError & { message?: string }).message ?? String(e);
			this.configError = { ...this.configError, [id]: message };
		} finally {
			this.validating = { ...this.validating, [id]: false };
		}
	}
}

export const webServersStore = new WebServersStore({
	listWebServers,
	readWebServerConfig,
	validateWebServerConfig
});
