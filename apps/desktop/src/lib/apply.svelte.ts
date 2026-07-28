// SPDX-License-Identifier: GPL-3.0-or-later
// Pending-changes state for the site apply pipeline. `refresh` is cheap by
// design (the Rust side spawns nothing), so it runs after every site mutation.
import type { ApplyOutcomeDto, ApplyPlanDto, FileChangeDto } from './ipc';

export interface ApplyApi {
	planConfigApply(): Promise<ApplyPlanDto>;
	applyConfig(): Promise<ApplyOutcomeDto>;
}

function errorMessage(e: unknown): string {
	if (typeof e === 'object' && e !== null && 'message' in e) {
		const m = (e as { message?: unknown }).message;
		if (typeof m === 'string' && m !== '') return m;
	}
	return 'The command failed.';
}

export class ApplyStore {
	changes = $state<FileChangeDto[]>([]);
	error = $state('');
	applying = $state(false);
	outcome = $state<ApplyOutcomeDto | null>(null);

	constructor(private api: ApplyApi) {}

	get pendingCount(): number {
		return this.changes.length;
	}

	async refresh(): Promise<void> {
		this.error = '';
		try {
			this.changes = (await this.api.planConfigApply()).changes;
		} catch (e) {
			this.error = errorMessage(e);
			this.changes = [];
		}
	}

	/**
	 * Apply, then re-plan. The re-plan is the honest source of the new pending
	 * count: assuming zero would hide anything the apply could not write.
	 *
	 * The re-entrancy guard lives here rather than only on the button's
	 * `disabled` attribute — deleting an attribute leaves no test failing.
	 */
	async run(): Promise<boolean> {
		if (this.applying) return false;
		this.applying = true;
		this.error = '';
		try {
			this.outcome = await this.api.applyConfig();
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.applying = false;
		}
		await this.refresh();
		return true;
	}
}
