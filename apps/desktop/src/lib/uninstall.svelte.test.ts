// SPDX-License-Identifier: GPL-3.0-or-later
//
// `UninstallStore` unit tests. The fake api RECORDS every call, and every
// "nothing happened" assertion is paired with a positive control on the same
// fake — otherwise a store that had simply stopped calling the api at all
// would pass the refusal tests vacuously (plan Step 1: "give it a positive
// control so 'nothing happened' cannot pass vacuously").
//
// Same shape as `sites.svelte.test.ts`/`databases.svelte.test.ts`: an injected
// api object, no IPC, no DOM.

import { describe, expect, it, vi } from 'vitest';
import { UninstallStore, type UninstallApi } from './uninstall.svelte';
import type { PackageKind, UninstallPlan } from './uninstall.derive';

function plan(overrides: Partial<UninstallPlan> = {}): UninstallPlan {
	return {
		kind: 'php',
		major: '8.3',
		removes: ['the Homebrew formula php@8.3'],
		keeps: [{ what: 'Logs', path: '/home/logs/php-fpm-8.3' }],
		blockers: [],
		...overrides
	};
}

interface Recorder {
	api: UninstallApi;
	planCalls: Array<[PackageKind, string]>;
	uninstallCalls: Array<[PackageKind, string]>;
	/** Resolves the pending `uninstallPackage`, so a test can observe the
	 *  store's state WHILE the command is still in flight. */
	settle: () => void;
}

function recorder(
	options: {
		planResult?: UninstallPlan;
		planError?: unknown;
		uninstallError?: unknown;
		/** Hold `uninstallPackage` open until `settle()` is called. */
		hold?: boolean;
	} = {}
): Recorder {
	const planCalls: Array<[PackageKind, string]> = [];
	const uninstallCalls: Array<[PackageKind, string]> = [];
	let release: () => void = () => {};
	const api: UninstallApi = {
		uninstallPlan: async (kind, major) => {
			planCalls.push([kind, major]);
			if (options.planError !== undefined) throw options.planError;
			return options.planResult ?? plan();
		},
		uninstallPackage: async (kind, major) => {
			uninstallCalls.push([kind, major]);
			if (options.hold) await new Promise<void>((resolve) => (release = resolve));
			if (options.uninstallError !== undefined) throw options.uninstallError;
		}
	};
	return { api, planCalls, uninstallCalls, settle: () => release() };
}

describe('UninstallStore.request — the plan arrives before anything is offered', () => {
	it('fetches the plan for exactly the package that was asked for', async () => {
		const rec = recorder({ planResult: plan({ kind: 'mysql', major: '8.4' }) });
		const store = new UninstallStore(rec.api);
		await store.request('mysql', '8.4');
		expect(rec.planCalls).toEqual([['mysql', '8.4']]);
		expect(store.plan?.major).toBe('8.4');
		expect(store.isOpen).toBe(true);
	});

	it('opens with the error and no plan when the plan cannot be fetched', async () => {
		const rec = recorder({ planError: { kind: 'core', message: 'no such major' } });
		const store = new UninstallStore(rec.api);
		await store.request('php', '9.9');
		// Open, so the failure is visible: a button that silently does nothing is
		// the failure mode this avoids.
		expect(store.isOpen).toBe(true);
		expect(store.plan).toBeNull();
		expect(store.error).toContain('no such major');
		expect(store.canProceed).toBe(false);
	});

	// Lifecycle: two requests in flight at once (a fast double-click across two
	// rows). The one that was asked for LAST owns the dialog; a stale plan
	// landing afterwards must not overwrite it.
	it('ignores a plan that lands after a different package was requested', async () => {
		const planCalls: Array<[PackageKind, string]> = [];
		let releaseFirst: () => void = () => {};
		const api: UninstallApi = {
			uninstallPlan: async (kind, major) => {
				planCalls.push([kind, major]);
				if (major === '8.3') await new Promise<void>((r) => (releaseFirst = r));
				return plan({ kind, major });
			},
			uninstallPackage: async () => {}
		};
		const store = new UninstallStore(api);
		const first = store.request('php', '8.3');
		await store.request('php', '8.4');
		releaseFirst();
		await first;
		expect(planCalls).toHaveLength(2);
		expect(store.plan?.major).toBe('8.4');
		expect(store.target?.major).toBe('8.4');
	});

	it('does not start a new plan while an uninstall is running', async () => {
		const rec = recorder({ hold: true });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		const running = store.confirm();
		await store.request('php', '8.4');
		expect(rec.planCalls).toEqual([['php', '8.3']]);
		rec.settle();
		await running;
	});
});

describe('UninstallStore.confirm — a blocker is a refusal, not a warning', () => {
	it('spawns nothing when the plan carries a blocker', async () => {
		const rec = recorder({
			planResult: plan({
				blockers: [{ kind: 'serviceNotTerminal', id: 'php-fpm-8.3', state: 'running' }]
			})
		});
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		expect(store.canProceed).toBe(false);
		await expect(store.confirm()).resolves.toBe(false);
		expect(rec.uninstallCalls).toEqual([]);
	});

	// The positive control for the assertion above: the SAME fake, the SAME
	// store method, one blocker fewer. Without this, a `confirm()` that had
	// stopped calling the api entirely would pass the refusal test.
	it('does spawn when the same plan carries no blocker (positive control)', async () => {
		const rec = recorder({ planResult: plan({ blockers: [] }) });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		expect(store.canProceed).toBe(true);
		await expect(store.confirm()).resolves.toBe(true);
		expect(rec.uninstallCalls).toEqual([['php', '8.3']]);
	});

	it('refuses with no plan at all', async () => {
		const rec = recorder({ planError: new Error('nope') });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		await expect(store.confirm()).resolves.toBe(false);
		expect(rec.uninstallCalls).toEqual([]);
	});

	// Defensive, and cheap: the command is driven by `target`, the refusal
	// decision is read off `plan`. If those two ever disagree the store would
	// be uninstalling one version on the strength of another version's
	// blocker-free plan.
	it('refuses when the fetched plan does not describe the current target', async () => {
		const rec = recorder({ planResult: plan({ kind: 'php', major: '8.3' }) });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		store.target = { kind: 'php', major: '8.4' };
		await expect(store.confirm()).resolves.toBe(false);
		expect(rec.uninstallCalls).toEqual([]);
	});

	it('refuses a second confirm while the first is still running', async () => {
		const rec = recorder({ hold: true });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		const first = store.confirm();
		await expect(store.confirm()).resolves.toBe(false);
		expect(rec.uninstallCalls).toHaveLength(1);
		rec.settle();
		await expect(first).resolves.toBe(true);
	});

	it('marks the store busy with the major it is uninstalling, and clears it after', async () => {
		const rec = recorder({ hold: true });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		const running = store.confirm();
		expect(store.uninstalling).toBe('8.3');
		rec.settle();
		await running;
		expect(store.uninstalling).toBe('');
	});

	it('closes the dialog on success — the row it described is gone', async () => {
		const rec = recorder();
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		await store.confirm();
		expect(store.isOpen).toBe(false);
		expect(store.plan).toBeNull();
	});

	it('keeps the dialog open with the error when brew fails', async () => {
		const rec = recorder({ uninstallError: { kind: 'core', message: 'brew exited 1' } });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		await expect(store.confirm()).resolves.toBe(false);
		expect(store.isOpen).toBe(true);
		expect(store.error).toContain('brew exited 1');
		expect(store.uninstalling).toBe('');
	});

	it('never renders [object Object] for a message-less failure', async () => {
		const rec = recorder({ uninstallError: {} });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		await store.confirm();
		expect(store.error).not.toContain('[object Object]');
		expect(store.error.length).toBeGreaterThan(0);
	});
});

describe('UninstallStore.appendLog — the shared install channel', () => {
	// `uninstall_package` streams on the SAME event `install_php` uses, so this
	// store sees an INSTALL's lines too. Recording them would show the tail of
	// a `brew install` inside an uninstall dialog opened minutes later.
	it('ignores lines that arrive while no uninstall is running', () => {
		const store = new UninstallStore(recorder().api);
		store.appendLog('8.3', 'installing php@8.3');
		expect(store.log).toEqual([]);
	});

	it('records lines for the major it is uninstalling (positive control)', async () => {
		const rec = recorder({ hold: true });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		const running = store.confirm();
		store.appendLog('8.3', 'Uninstalling /opt/homebrew/Cellar/php@8.3');
		expect(store.log.map((l) => l.line)).toEqual(['Uninstalling /opt/homebrew/Cellar/php@8.3']);
		rec.settle();
		await running;
	});

	it('ignores lines belonging to a different major', async () => {
		const rec = recorder({ hold: true });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		const running = store.confirm();
		store.appendLog('8.4', 'not this one');
		expect(store.log).toEqual([]);
		rec.settle();
		await running;
	});

	it('keeps only the tail once the cap is passed', async () => {
		const rec = recorder({ hold: true });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		const running = store.confirm();
		for (let i = 0; i < 260; i += 1) store.appendLog('8.3', `line ${i}`);
		expect(store.log).toHaveLength(200);
		expect(store.log[store.log.length - 1].line).toBe('line 259');
		rec.settle();
		await running;
	});
});

describe('UninstallStore.close', () => {
	it('clears the dialog when nothing is running', async () => {
		const store = new UninstallStore(recorder().api);
		await store.request('php', '8.3');
		store.close();
		expect(store.isOpen).toBe(false);
		expect(store.plan).toBeNull();
		expect(store.error).toBe('');
	});

	// Deliberate deviation from `QuitDialog`, which cancels even mid-quit:
	// there the window is about to be destroyed, so there is nothing left to
	// look at. Here the live `brew uninstall` output is the ONLY feedback the
	// user has, and dismissing it would leave a page that shows nothing while
	// a package is being removed.
	it('refuses to close while an uninstall is running', async () => {
		const rec = recorder({ hold: true });
		const store = new UninstallStore(rec.api);
		await store.request('php', '8.3');
		const running = store.confirm();
		store.close();
		expect(store.isOpen).toBe(true);
		rec.settle();
		await running;
	});
});

describe('UninstallStore.fail', () => {
	it('records an outside failure on the same channel the dialog renders', () => {
		const store = new UninstallStore(recorder().api);
		store.fail(new Error('listener could not be registered'));
		expect(store.error).toContain('listener could not be registered');
	});
});

describe('UninstallStore timestamps', () => {
	it('stamps each log line with the time it arrived', async () => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-07-31T12:00:00Z'));
		try {
			const rec = recorder({ hold: true });
			const store = new UninstallStore(rec.api);
			await store.request('php', '8.3');
			const running = store.confirm();
			store.appendLog('8.3', 'one');
			expect(store.log[0].tsMs).toBe(Date.parse('2026-07-31T12:00:00Z'));
			rec.settle();
			await running;
		} finally {
			vi.useRealTimers();
		}
	});
});
