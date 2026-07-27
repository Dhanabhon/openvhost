// SPDX-License-Identifier: GPL-3.0-or-later
// State for the Languages page: the PHP environment snapshot, the one install
// that may be running, its live log, and the outcome of the last attempt.
//
// Shape mirrors `apply.svelte.ts`: an injected API object (so tests never touch
// real IPC), plain `$state` fields, and a re-entrancy guard that lives HERE
// rather than only on a button's `disabled` attribute — deleting that attribute
// must leave a test failing, not just a UI regression nothing catches.
import { errorMessage } from './errors';
import type { InstallOutcomeDto, PhpEnvironmentDto } from './ipc';

export interface LanguagesApi {
	phpEnvironment(): Promise<PhpEnvironmentDto>;
	rescanPhpRuntimes(): Promise<PhpEnvironmentDto>;
	installPhp(major: string): Promise<InstallOutcomeDto>;
}

/** Same shape `LogPane.svelte` already renders (`services.svelte.ts`'s `UiLog`),
 *  redeclared here rather than imported so this store stays decoupled from the
 *  services store — the only thing shared is the shape LogPane expects. */
export interface UiLog {
	id: string;
	tsMs: number;
	level: 'info' | 'warn' | 'error';
	line: string;
}

export class LanguagesStore {
	env = $state<PhpEnvironmentDto | null>(null);
	/** '' when idle, otherwise the major currently installing. */
	installing = $state('');
	log = $state<UiLog[]>([]);
	error = $state('');
	outcome = $state<InstallOutcomeDto | null>(null);

	constructor(private api: LanguagesApi) {}

	get brewFound(): boolean {
		return this.env?.brewFound ?? false;
	}

	get anyInstalled(): boolean {
		return this.env?.runtimes.some((r) => r.installed) ?? false;
	}

	/** Cheap, spawn-free snapshot (spec: safe on mount and after every install). */
	async refresh(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.phpEnvironment();
		} catch (e) {
			this.error = errorMessage(e);
			this.env = null;
		}
	}

	/** The "Check again" button: unlike {@link refresh}, this spawns a probe
	 *  per candidate binary — explicit and user-initiated only. */
	async rescan(): Promise<void> {
		this.error = '';
		try {
			this.env = await this.api.rescanPhpRuntimes();
		} catch (e) {
			this.error = errorMessage(e);
			this.env = null;
		}
	}

	/** Record an outside failure (e.g. the page's install-log subscription
	 *  could not be registered) on the same channel this store's own calls use. */
	fail(e: unknown): void {
		this.error = errorMessage(e);
	}

	/** Append one line of `brew install` output. `PhpInstallLogEvent` carries a
	 *  stream (stdout/stderr), not a severity — brew's output has none for a
	 *  classifier to assign — so every line lands as `info`. `major` becomes the
	 *  entry's `id`, matching `UiLog`'s shape without inventing a second one. */
	appendLog(major: string, line: string): void {
		this.log = [...this.log, { id: major, tsMs: Date.now(), level: 'info', line }];
	}

	/**
	 * Install `major` via Homebrew. Always re-reads the environment with
	 * {@link refresh} on success rather than assuming the row is now installed —
	 * `detected` exists precisely because brew can exit 0 without the version
	 * being found afterwards, and assuming would hide that.
	 *
	 * The re-entrancy guard lives here, not only on the Install button's
	 * `disabled` attribute: deleting that attribute must still leave a second
	 * concurrent call refused.
	 */
	async install(major: string): Promise<boolean> {
		if (this.installing !== '') return false;
		this.installing = major;
		this.error = '';
		try {
			this.outcome = await this.api.installPhp(major);
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.installing = '';
		}
		await this.refresh();
		return true;
	}
}
