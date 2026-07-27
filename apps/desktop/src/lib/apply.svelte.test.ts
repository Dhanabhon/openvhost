// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { ApplyStore } from './apply.svelte';
import type { ApplyOutcomeDto, ApplyPlanDto } from './ipc';

const change = (path: string, kind: string) => ({ path, kind, diff: `--- a\n+++ b\n+${path}\n` });

function api(
	overrides: Partial<{ plan: ApplyPlanDto; outcome: ApplyOutcomeDto; fail: unknown }> = {}
) {
	return {
		planConfigApply: async () => {
			if (overrides.fail) throw overrides.fail;
			return overrides.plan ?? { changes: [] };
		},
		applyConfig: async () => {
			if (overrides.fail) throw overrides.fail;
			return overrides.outcome ?? { applied: 0, restarted: [], notStarted: [], needsAttention: [] };
		}
	};
}

describe('ApplyStore', () => {
	it('reports nothing pending for an empty plan', async () => {
		const s = new ApplyStore(api());
		await s.refresh();
		expect(s.pendingCount).toBe(0);
	});

	it('counts the changes a plan returns', async () => {
		const s = new ApplyStore(
			api({ plan: { changes: [change('/a.conf', 'added'), change('/b.conf', 'removed')] } })
		);
		await s.refresh();
		expect(s.pendingCount).toBe(2);
	});

	it('surfaces a failed plan as an error and keeps the count at zero', async () => {
		const s = new ApplyStore(api({ fail: { kind: 'core', message: 'nginx is missing' } }));
		await s.refresh();
		expect(s.error).toBe('nginx is missing');
		expect(s.pendingCount).toBe(0);
	});

	it('clears the pending changes after a successful apply', async () => {
		// A plain flag rather than `s.outcome` inside the closure that builds `s`:
		// referencing `s` from within its own constructor argument makes TypeScript
		// unable to infer the arrow functions' types (circular), which surfaces as a
		// noImplicitAny error under svelte-check even though it evaluates fine at
		// runtime — `applied` sidesteps the circularity while keeping the same
		// "second plan call sees the post-apply state" behaviour under test.
		let applied = false;
		const s = new ApplyStore({
			planConfigApply: async () => ({ changes: applied ? [] : [change('/a.conf', 'added')] }),
			applyConfig: async () => {
				applied = true;
				return { applied: 1, restarted: ['nginx'], notStarted: [], needsAttention: [] };
			}
		});
		await s.refresh();
		expect(s.pendingCount).toBe(1);
		expect(await s.run()).toBe(true);
		expect(s.outcome?.restarted).toEqual(['nginx']);
		expect(s.pendingCount).toBe(0);
	});

	it('re-plans after applying instead of assuming everything was written', async () => {
		// A partial apply leaves work behind. Assuming zero would tell the user
		// everything is live when it is not — so run() must ask again. The second
		// plan call returns a non-empty (but different) list: a regression that
		// replaced `await this.refresh()` with a bare `this.changes = []` would
		// pass a test whose mock returns `[]` on the second call, so this mock
		// deliberately does not.
		let planCalls = 0;
		const s = new ApplyStore({
			planConfigApply: async () => {
				planCalls += 1;
				return planCalls === 1
					? { changes: [change('/a.conf', 'added'), change('/b.conf', 'added')] }
					: { changes: [change('/b.conf', 'added')] };
			},
			applyConfig: async () => ({ applied: 1, restarted: [], notStarted: [], needsAttention: [] })
		});
		await s.refresh();
		expect(s.pendingCount).toBe(2);
		expect(await s.run()).toBe(true);
		expect(planCalls).toBe(2);
		expect(s.pendingCount).toBe(1);
	});

	it('keeps the changes and shows the validator output when apply fails', async () => {
		const s = new ApplyStore({
			planConfigApply: async () => ({ changes: [change('/a.conf', 'added')] }),
			applyConfig: async () => {
				throw { kind: 'core', message: 'nginx: [emerg] unknown directive' };
			}
		});
		await s.refresh();
		expect(await s.run()).toBe(false);
		expect(s.error).toContain('unknown directive');
		expect(s.pendingCount).toBe(1);
		expect(s.applying).toBe(false);
	});

	it('refuses a second concurrent apply', async () => {
		let calls = 0;
		const s = new ApplyStore({
			planConfigApply: async () => ({ changes: [change('/a.conf', 'added')] }),
			applyConfig: async () => {
				calls += 1;
				await new Promise((r) => setTimeout(r, 5));
				return { applied: 1, restarted: [], notStarted: [], needsAttention: [] };
			}
		});
		await s.refresh();
		await Promise.all([s.run(), s.run()]);
		expect(calls).toBe(1);
	});

	// `needsAttention` did not exist when this store's shape was first sketched — it
	// was added after a review found Apply could stop a service, fail to bring it
	// back, and still report success. A non-empty list means Apply did NOT fully
	// succeed, so the store must keep it verbatim rather than folding it away.
	it('keeps needsAttention on the outcome after a run', async () => {
		const s = new ApplyStore({
			planConfigApply: async () => ({ changes: [change('/a.conf', 'added')] }),
			applyConfig: async () => ({
				applied: 1,
				restarted: [],
				notStarted: [],
				needsAttention: [{ id: 'nginx', reason: 'nginx stopped and would not start again.' }]
			})
		});
		await s.refresh();
		expect(await s.run()).toBe(true);
		expect(s.outcome?.needsAttention).toEqual([
			{ id: 'nginx', reason: 'nginx stopped and would not start again.' }
		]);
	});
});
