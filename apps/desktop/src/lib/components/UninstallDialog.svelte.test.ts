// SPDX-License-Identifier: GPL-3.0-or-later
//
// SSR tests for the uninstall confirmation (package-uninstall design D6).
// Rendered through `svelte/server` like every other component test here, so it
// runs in the existing `node` vitest project — scoped `<style>` is invisible to
// this harness, which is why the component also carries its load-bearing
// `white-space` inline (the `ApplyDialog.svelte` precedent).
//
// What is asserted is the CONTRACT of the dialog, not its layout: a plan with
// blockers offers no way forward, a plan without them names both what goes and
// what stays, and the kept paths on screen are the ones the plan carried.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import UninstallDialog from './UninstallDialog.svelte';
import type { Blocker, UninstallPlan } from '$lib/uninstall.derive';

function plan(overrides: Partial<UninstallPlan> = {}): UninstallPlan {
	return {
		kind: 'mysql',
		major: '8.4',
		removes: ['the Homebrew formula mysql@8.4', 'the supervisor entry mysql-8.4'],
		keeps: [
			{ what: 'Your databases', path: '/Users/x/.openvhost/data/mysql/8.4' },
			{ what: 'The stored root password', path: null }
		],
		blockers: [],
		...overrides
	};
}

function html(props: Partial<Parameters<typeof UninstallDialog>[1]> = {}): string {
	return render(UninstallDialog, {
		props: {
			plan: plan(),
			planning: false,
			uninstalling: false,
			error: '',
			log: [],
			onCancel: () => {},
			onConfirm: () => {},
			...props
		}
	}).body;
}

describe('the uninstall confirmation, unblocked', () => {
	it('asks by name and says what goes', () => {
		const body = html();
		expect(body).toContain('Uninstall MySQL 8.4?');
		expect(body).toContain('This removes the MySQL 8.4 program files.');
	});

	// D6's whole point: the user learns their data is safe HERE, or nowhere.
	it('names what survives, with the path the plan carried', () => {
		const body = html();
		expect(body).toContain('Your databases are not touched');
		expect(body).toContain('/Users/x/.openvhost/data/mysql/8.4');
		expect(body).toContain('reinstalling 8.4 picks up where you left off');
	});

	// The anti-hardcode assertion at the render layer, mirroring
	// `uninstall.derive.test.ts`'s: change the plan's path and the sentence on
	// screen has to change with it.
	it('follows the plan when the plan names a different path', () => {
		const body = html({
			plan: plan({ keeps: [{ what: 'Your databases', path: '/elsewhere/mysql/8.4' }] })
		});
		expect(body).toContain('/elsewhere/mysql/8.4');
		expect(body).not.toContain('/Users/x/.openvhost/data/mysql/8.4');
	});

	it("lists the executor's own removals verbatim rather than re-wording them", () => {
		const body = html();
		expect(body).toContain('the Homebrew formula mysql@8.4');
		expect(body).toContain('the supervisor entry mysql-8.4');
	});

	it('lists every kept item, including the ones with no path', () => {
		const body = html();
		expect(body).toContain('Your databases');
		expect(body).toContain('The stored root password');
	});

	// Against the REAL MySQL inventory (four keeps, three of them with paths):
	// the headline sentence names the datadir only, while the Kept list still
	// shows every path. Joining all four into the sentence would have it claim
	// the databases live in a config file.
	it('keeps the sentence about the datadir while the list carries the rest', () => {
		const body = html({
			plan: plan({
				keeps: [
					{ what: 'Your databases', path: '/home/data/mysql/8.4' },
					{ what: 'The stored root password', path: null },
					{ what: "This instance's my.cnf", path: '/home/data/mysql/8.4/my.cnf' },
					{ what: 'Your own MySQL overrides', path: '/home/config/custom/mysql/8.4' }
				]
			})
		});
		const sentence = body.match(/data-testid="uninstall-kept-sentence"[^>]*>([^<]*)</)?.[1] ?? '';
		expect(sentence).toContain('/home/data/mysql/8.4,');
		expect(sentence).not.toContain('my.cnf');
		// …and nothing is hidden: the full list is right below it.
		expect(body).toContain('/home/data/mysql/8.4/my.cnf');
		expect(body).toContain('/home/config/custom/mysql/8.4');
	});

	it('offers a confirm and a cancel', () => {
		const body = html();
		expect(body).toContain('data-testid="uninstall-confirm"');
		expect(body).toContain('data-testid="uninstall-cancel"');
	});

	it('says PHP things for a PHP plan, not MySQL ones', () => {
		const body = html({
			plan: plan({
				kind: 'php',
				major: '8.3',
				keeps: [{ what: 'Logs', path: '/Users/x/.openvhost/logs/php-fpm-8.3' }]
			})
		});
		expect(body).toContain('Uninstall PHP 8.3?');
		expect(body).toContain('any site still set to PHP 8.3 keeps that setting');
		expect(body).not.toContain('Your databases are not touched');
	});
});

describe('the uninstall confirmation, blocked (design D3: a refusal, not a warning)', () => {
	const blockers: Blocker[] = [
		{ kind: 'serviceNotTerminal', id: 'mysql-8.4', state: 'running' },
		{ kind: 'sitesPinned', domains: ['shop.test', 'blog.test'] }
	];

	it('offers NO way to proceed', () => {
		const body = html({ plan: plan({ blockers }) });
		expect(body).not.toContain('data-testid="uninstall-confirm"');
	});

	it('still offers a way out', () => {
		const body = html({ plan: plan({ blockers }) });
		expect(body).toContain('data-testid="uninstall-cancel"');
	});

	it('says plainly that it is refused', () => {
		const body = html({ plan: plan({ blockers }) });
		// Svelte's SSR escapes `<`, `>` and `&` in text but leaves an apostrophe
		// alone, so this is the literal sentence a reader sees.
		expect(body).toContain("MySQL 8.4 can't be uninstalled yet.");
	});

	it('names the service and its state', () => {
		const body = html({ plan: plan({ blockers: [blockers[0]] }) });
		expect(body).toContain('mysql-8.4 is running');
		expect(body).toContain('openvhost stop mysql-8.4');
	});

	it('names every pinned site', () => {
		const body = html({ plan: plan({ blockers: [blockers[1]] }) });
		expect(body).toContain('shop.test');
		expect(body).toContain('blog.test');
		expect(body).toContain('2 sites still use');
	});

	// The collapse check at the render layer: both blockers on screen at once
	// must produce two different paragraphs, not the same sentence twice.
	it('renders two different blockers as two different messages', () => {
		const body = html({ plan: plan({ blockers }) });
		const service = body.indexOf('mysql-8.4 is running');
		const sites = body.indexOf('2 sites still use');
		expect(service).toBeGreaterThan(-1);
		expect(sites).toBeGreaterThan(-1);
		expect(service).not.toBe(sites);
	});

	// A blocked plan is about the obstacle, not about the inventory — showing
	// the removal list next to a refusal invites reading it as a preview of
	// something that is about to happen.
	it('does not show the removal inventory next to a refusal', () => {
		const body = html({ plan: plan({ blockers }) });
		expect(body).not.toContain('the Homebrew formula mysql@8.4');
	});
});

describe('the uninstall confirmation, before and during the work', () => {
	it('offers no confirm while the plan is still being fetched', () => {
		const body = html({ plan: null, planning: true });
		expect(body).not.toContain('data-testid="uninstall-confirm"');
		expect(body).toContain('Checking what this would remove');
	});

	it('offers no confirm when the plan could not be fetched', () => {
		const body = html({ plan: null, planning: false, error: 'no such major' });
		expect(body).not.toContain('data-testid="uninstall-confirm"');
		expect(body).toContain('no such major');
	});

	it('disables both buttons and says what it is doing while the uninstall runs', () => {
		const body = html({ uninstalling: true });
		const confirm = body.match(/<button[^>]*data-testid="uninstall-confirm"[^>]*>/)?.[0] ?? '';
		const cancel = body.match(/<button[^>]*data-testid="uninstall-cancel"[^>]*>/)?.[0] ?? '';
		expect(confirm).toContain('disabled');
		expect(cancel).toContain('disabled');
		expect(body).toContain('Uninstalling…');
	});

	it('leaves both buttons enabled when nothing is running', () => {
		const body = html({ uninstalling: false });
		const confirm = body.match(/<button[^>]*data-testid="uninstall-confirm"[^>]*>/)?.[0] ?? '';
		const cancel = body.match(/<button[^>]*data-testid="uninstall-cancel"[^>]*>/)?.[0] ?? '';
		expect(confirm).not.toContain('disabled');
		expect(cancel).not.toContain('disabled');
	});

	it("shows brew's live output while it runs", () => {
		const body = html({
			uninstalling: true,
			log: [{ id: '8.4', tsMs: 0, level: 'info' as const, line: 'Uninstalling /opt/homebrew/…' }]
		});
		expect(body).toContain('Uninstalling /opt/homebrew/…');
	});

	it('keeps the output on screen after a failure, next to the reason', () => {
		const body = html({
			uninstalling: false,
			error: 'brew exited with code 1',
			log: [{ id: '8.4', tsMs: 0, level: 'info' as const, line: 'Error: Refusing to uninstall' }]
		});
		expect(body).toContain('Error: Refusing to uninstall');
		expect(body).toContain('brew exited with code 1');
	});
});
