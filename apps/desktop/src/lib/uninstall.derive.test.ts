// SPDX-License-Identifier: GPL-3.0-or-later
//
// Pure-function tests for the uninstall confirmation (package-uninstall design
// D6). Everything user-visible about an uninstall — the title, the lead, the
// "what survives" sentence, every refusal message — is computed HERE and
// asserted here, so the markup in `UninstallDialog.svelte` stays a renderer
// with no copy decisions of its own.
//
// The distinctness assertions below are the point of this file, not a
// formality: this project has shipped a state-collapsed-into-one-rendering bug
// four times, and a renderer that gave two different `Blocker`s the same
// sentence would pass every "is it non-empty" check ever written.

import { describe, expect, it } from 'vitest';
import {
	blockerMessage,
	keptSentence,
	mayProceed,
	packageLabel,
	refusalHeadline,
	uninstallActionDisabled,
	uninstallConfirmLabel,
	uninstallLead,
	uninstallTitle,
	type Blocker,
	type KeptItem,
	type PackageKind
} from './uninstall.derive';

const KINDS: PackageKind[] = ['php', 'mysql'];

/** Every `Blocker` variant, once each — the list the distinctness sweeps below
 *  iterate. A new variant added to the union without a line here still fails
 *  `blockerMessage`'s own never-typed default arm at compile time; this list is
 *  what makes the RUNTIME sweeps cover it too. */
const BLOCKERS: Blocker[] = [
	{ kind: 'serviceNotTerminal', id: 'php-fpm-8.3', state: 'running' },
	{ kind: 'sitesPinned', domains: ['shop.test', 'blog.test'] }
];

describe('packageLabel', () => {
	it('names PHP and MySQL the way the rest of the app does', () => {
		expect(packageLabel('php')).toBe('PHP');
		expect(packageLabel('mysql')).toBe('MySQL');
	});

	// A label function that returned one constant would satisfy every "contains
	// PHP" check written against a single kind. Pinning that the two kinds
	// disagree is what makes those checks mean anything.
	it('gives the two kinds different labels', () => {
		expect(packageLabel('php')).not.toBe(packageLabel('mysql'));
	});
});

describe('uninstallTitle / uninstallLead (design D6, verbatim)', () => {
	it('asks by name and version, for both kinds', () => {
		expect(uninstallTitle('mysql', '8.4')).toBe('Uninstall MySQL 8.4?');
		expect(uninstallTitle('php', '8.3')).toBe('Uninstall PHP 8.3?');
	});

	it('says what goes in one plain sentence, for both kinds', () => {
		expect(uninstallLead('mysql', '8.4')).toBe('This removes the MySQL 8.4 program files.');
		expect(uninstallLead('php', '8.3')).toBe('This removes the PHP 8.3 program files.');
	});

	it('never produces the same title or lead for two different kinds', () => {
		expect(uninstallTitle('php', '8.4')).not.toBe(uninstallTitle('mysql', '8.4'));
		expect(uninstallLead('php', '8.4')).not.toBe(uninstallLead('mysql', '8.4'));
	});
});

describe('keptSentence — the paths come from the plan, never from this file', () => {
	const mysqlKeeps: KeptItem[] = [
		{ what: 'Your databases', path: '/Users/x/.openvhost/data/mysql/8.4' },
		{ what: 'The root password', path: null }
	];

	it("names MySQL's datadir path out of `keeps`", () => {
		const sentence = keptSentence('mysql', '8.4', mysqlKeeps);
		expect(sentence).toContain('Your databases are not touched');
		expect(sentence).toContain('/Users/x/.openvhost/data/mysql/8.4');
		expect(sentence).toContain('root password is kept');
		expect(sentence).toContain('reinstalling 8.4 picks up where you left off');
	});

	// The anti-hardcode assertion. A `keptSentence` with the datadir path baked
	// into a template literal passes the test above and fails this one: the
	// sentence has to MOVE when the plan says a different path.
	it('follows the plan when the plan names a different path', () => {
		const moved: KeptItem[] = [{ what: 'Your databases', path: '/somewhere/else/mysql/8.4' }];
		const sentence = keptSentence('mysql', '8.4', moved);
		expect(sentence).toContain('/somewhere/else/mysql/8.4');
		expect(sentence).not.toContain('/Users/x/.openvhost/data/mysql/8.4');
	});

	// The sentence names the HEADLINE kept path — the first item carrying one,
	// which the Rust inventory orders deliberately ("Your databases" for MySQL,
	// "Your PHP <major> logs" for PHP). Joining every path instead reads as a
	// lie: "your databases … stay in <datadir>, <my.cnf> and <overrides dir>"
	// claims the databases live in a config file. The other kept paths are all
	// rendered, in the dialog's own Kept list, right below this sentence.
	it('names the headline kept path and not the incidental ones', () => {
		const many: KeptItem[] = [
			{ what: 'Your databases', path: '/a/data' },
			{ what: "This instance's my.cnf", path: '/a/data/my.cnf' },
			{ what: 'Your own MySQL overrides', path: '/a/custom' }
		];
		const sentence = keptSentence('mysql', '8.4', many);
		expect(sentence).toContain('/a/data,');
		expect(sentence).not.toContain('/a/custom');
	});

	// The headline is the first item WITH a path, not literally `keeps[0]` — a
	// path-less item sorted first must not blank the clause.
	it('skips past a path-less kept item to find the headline path', () => {
		const sentence = keptSentence('mysql', '8.4', [
			{ what: 'The stored root password', path: null },
			{ what: 'Your databases', path: '/a/data' }
		]);
		expect(sentence).toContain('/a/data');
	});

	// `KeptItem.path` is nullable in the contract — a plan whose kept items are
	// all path-less (the root password alone) must still read as a sentence
	// rather than as "they stay in ." or "they stay in undefined".
	it('drops the "they stay in" clause when no kept item has a path', () => {
		const sentence = keptSentence('mysql', '8.4', [{ what: 'The root password', path: null }]);
		expect(sentence).not.toContain('they stay in');
		expect(sentence).not.toContain('undefined');
		expect(sentence).toContain('Your databases are not touched');
	});

	it("names PHP's kept logs and says a pinned site keeps its setting", () => {
		const sentence = keptSentence('php', '8.3', [
			{ what: 'Logs', path: '/Users/x/.openvhost/logs/php-fpm-8.3' }
		]);
		expect(sentence).toContain('/Users/x/.openvhost/logs/php-fpm-8.3');
		expect(sentence).toContain('any site still set to PHP 8.3 keeps that setting');
	});

	// Same collapse hazard as the blockers: one sentence serving both kinds
	// would tell a MySQL user about logs, or a PHP user their databases are
	// safe when no database was ever involved.
	it('says something different for PHP than for MySQL', () => {
		const keeps: KeptItem[] = [{ what: 'Logs', path: '/a/logs' }];
		expect(keptSentence('php', '8.4', keeps)).not.toBe(keptSentence('mysql', '8.4', keeps));
	});
});

describe('blockerMessage — a refusal names its own subject', () => {
	it('names the service and the state that is in the way', () => {
		const message = blockerMessage({
			kind: 'serviceNotTerminal',
			id: 'mysql-8.4',
			state: 'running'
		});
		expect(message.obstacle).toContain('mysql-8.4');
		expect(message.obstacle).toContain('running');
		expect(message.action).toContain('Stop it');
	});

	it('carries the state verbatim rather than assuming which one it is', () => {
		const starting = blockerMessage({
			kind: 'serviceNotTerminal',
			id: 'mysql-8.4',
			state: 'starting'
		});
		expect(starting.obstacle).toContain('starting');
		expect(starting.obstacle).not.toContain('running');
	});

	it('names every pinned site domain', () => {
		const message = blockerMessage({
			kind: 'sitesPinned',
			domains: ['shop.test', 'blog.test']
		});
		expect(message.obstacle).toContain('shop.test');
		expect(message.obstacle).toContain('blog.test');
		expect(message.action).toContain('another PHP version');
	});

	it('counts one pinned site in the singular', () => {
		const one = blockerMessage({ kind: 'sitesPinned', domains: ['shop.test'] });
		expect(one.obstacle).toContain('1 site still uses');
		expect(one.obstacle).not.toContain('sites still use');
	});

	it('counts several pinned sites in the plural', () => {
		const many = blockerMessage({ kind: 'sitesPinned', domains: ['a.test', 'b.test'] });
		expect(many.obstacle).toContain('2 sites still use');
	});

	// THE distinctness sweep. Pairwise, not "each is non-empty": a renderer
	// that collapsed both variants onto one sentence — the exact bug class this
	// codebase keeps producing — passes every non-emptiness check and fails
	// only here.
	it('gives every pair of blocker variants a different obstacle and a different action', () => {
		expect(BLOCKERS.length).toBeGreaterThan(1);
		for (let i = 0; i < BLOCKERS.length; i += 1) {
			for (let j = i + 1; j < BLOCKERS.length; j += 1) {
				const a = blockerMessage(BLOCKERS[i]);
				const b = blockerMessage(BLOCKERS[j]);
				expect(a.obstacle).not.toBe(b.obstacle);
				expect(a.action).not.toBe(b.action);
			}
		}
	});

	it('tags each message with the kind it came from, so a list can key on it', () => {
		for (const blocker of BLOCKERS) {
			expect(blockerMessage(blocker).kind).toBe(blocker.kind);
		}
	});

	it('never returns an empty obstacle or an empty action', () => {
		for (const blocker of BLOCKERS) {
			const message = blockerMessage(blocker);
			expect(message.obstacle.length).toBeGreaterThan(0);
			expect(message.action.length).toBeGreaterThan(0);
		}
	});
});

describe('refusalHeadline', () => {
	it('says the uninstall is refused, naming the package', () => {
		expect(refusalHeadline('mysql', '8.4')).toContain('MySQL 8.4');
		expect(refusalHeadline('php', '8.3')).toContain('PHP 8.3');
	});

	it('reads as a refusal, not as a warning to click past', () => {
		for (const kind of KINDS) {
			expect(refusalHeadline(kind, '8.4')).toContain("can't be uninstalled");
		}
	});
});

describe('uninstallActionDisabled', () => {
	// One property, four inputs, asserted in both directions — a hardcoded
	// `true` and a hardcoded `false` each fail one of these.
	it('is enabled when nothing is in flight', () => {
		expect(
			uninstallActionDisabled({ installingMajor: '', initializingMajor: '', uninstallingMajor: '' })
		).toBe(false);
	});

	it('is disabled while an install is running', () => {
		expect(
			uninstallActionDisabled({
				installingMajor: '8.4',
				initializingMajor: '',
				uninstallingMajor: ''
			})
		).toBe(true);
	});

	it('is disabled while an initialize is running', () => {
		expect(
			uninstallActionDisabled({
				installingMajor: '',
				initializingMajor: '8.4',
				uninstallingMajor: ''
			})
		).toBe(true);
	});

	// "Another uninstall" includes this row's own: a second click while the
	// first is still running must not reach the command a second time.
	it('is disabled while any uninstall is running', () => {
		expect(
			uninstallActionDisabled({
				installingMajor: '',
				initializingMajor: '',
				uninstallingMajor: '8.3'
			})
		).toBe(true);
	});
});

describe('mayProceed', () => {
	// The single predicate both the store's refusal and the dialog's
	// "render the confirm button at all" read, so it is asserted directly
	// rather than only through those two.
	it('allows a plan with no blocker', () => {
		expect(mayProceed({ kind: 'php', major: '8.3', removes: [], keeps: [], blockers: [] })).toBe(
			true
		);
	});

	it('refuses a plan with any blocker', () => {
		for (const blocker of BLOCKERS) {
			expect(
				mayProceed({ kind: 'php', major: '8.3', removes: [], keeps: [], blockers: [blocker] })
			).toBe(false);
		}
	});

	// Not fetched yet, or the fetch failed — the UI must never offer to
	// uninstall on the strength of nothing.
	it('refuses a missing plan', () => {
		expect(mayProceed(null)).toBe(false);
	});
});

describe('uninstallConfirmLabel', () => {
	it('says what the button will do while idle', () => {
		expect(uninstallConfirmLabel(false)).toBe('Uninstall');
	});

	it('says what is happening while the uninstall runs', () => {
		expect(uninstallConfirmLabel(true)).toBe('Uninstalling…');
	});
});
