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
	offersUninstall,
	outOfCatalogueNote,
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
	{ kind: 'sitesPinned', domains: ['shop.test', 'blog.test'] },
	{
		kind: 'foreignKeg',
		formula: 'php@8.5',
		owner: 'php',
		keg: '/opt/homebrew/Cellar/php/8.5.9'
	},
	{ kind: 'unknownKeg', formula: 'php@8.3', searched: ['/opt/homebrew/opt/php@8.3'] }
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
		{ what: 'Your databases', path: '/Users/x/.openvhost/data/mysql/8.4', headline: true },
		{ what: 'The root password', path: null, headline: false }
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
		const moved: KeptItem[] = [
			{ what: 'Your databases', path: '/somewhere/else/mysql/8.4', headline: true }
		];
		const sentence = keptSentence('mysql', '8.4', moved);
		expect(sentence).toContain('/somewhere/else/mysql/8.4');
		expect(sentence).not.toContain('/Users/x/.openvhost/data/mysql/8.4');
	});

	// The sentence names ONE kept path, not all of them. Joining every path
	// reads as a lie: "your databases … stay in <datadir>, <my.cnf> and
	// <overrides dir>" claims the databases live in a config file. The other
	// kept paths are all rendered, in the dialog's own Kept list, right below
	// this sentence.
	it('names the headline kept path and not the incidental ones', () => {
		const many: KeptItem[] = [
			{ what: 'Your databases', path: '/a/data', headline: true },
			{ what: "This instance's my.cnf", path: '/a/data/my.cnf', headline: false },
			{ what: 'Your own MySQL overrides', path: '/a/custom', headline: false }
		];
		const sentence = keptSentence('mysql', '8.4', many);
		expect(sentence).toContain('/a/data,');
		expect(sentence).not.toContain('/a/custom');
	});

	// THE fix both gates asked for. The headline used to be "the first item
	// carrying a path", which made this dialog's central promise a function of
	// the order of a Rust `Vec`: re-ordering the inventory for an unrelated
	// reason would silently move "your databases … stay in X" onto a config
	// file, and nothing about that change would look like it touched this copy.
	// Selecting on the flag is what makes this fixture — headline LAST, an
	// incidental path FIRST — name the right directory.
	it('names the flagged item even when another kept path comes before it', () => {
		const sentence = keptSentence('mysql', '8.4', [
			{ what: "This instance's my.cnf", path: '/a/data/my.cnf', headline: false },
			{ what: 'Your own MySQL overrides', path: '/a/custom', headline: false },
			{ what: 'Your databases', path: '/a/data', headline: true }
		]);
		expect(sentence).toContain('/a/data,');
		expect(sentence).not.toContain('my.cnf');
		expect(sentence).not.toContain('/a/custom');
	});

	// Rust asserts exactly one headline per plan. If that assertion is ever
	// violated, this must say NOTHING about where things stay rather than pick
	// one — naming the wrong directory is worse than naming none, because the
	// user reads it as a promise about that directory.
	it('drops the clause rather than guessing when two items claim the headline', () => {
		const sentence = keptSentence('mysql', '8.4', [
			{ what: 'Your databases', path: '/a/data', headline: true },
			{ what: 'Your own MySQL overrides', path: '/a/custom', headline: true }
		]);
		expect(sentence).not.toContain('they stay in');
		expect(sentence).not.toContain('/a/data');
		expect(sentence).not.toContain('/a/custom');
		// …and still reads as a sentence, still true, still safe to act on.
		expect(sentence).toContain('Your databases are not touched');
	});

	it('drops the clause when no kept item is the headline', () => {
		const sentence = keptSentence('mysql', '8.4', [
			{ what: 'Your databases', path: '/a/data', headline: false },
			{ what: 'The root password', path: null, headline: false }
		]);
		expect(sentence).not.toContain('they stay in');
		expect(sentence).not.toContain('/a/data');
		expect(sentence).toContain('Your databases are not touched');
	});

	// `KeptItem.path` is nullable in the contract — a headline item with nothing
	// on disk behind it must still read as a sentence rather than as "they stay
	// in ." or "they stay in undefined".
	it('drops the "they stay in" clause when the headline item has no path', () => {
		const sentence = keptSentence('mysql', '8.4', [
			{ what: 'The root password', path: null, headline: true }
		]);
		expect(sentence).not.toContain('they stay in');
		expect(sentence).not.toContain('undefined');
		expect(sentence).toContain('Your databases are not touched');
	});

	it("names PHP's kept logs and says a pinned site keeps its setting", () => {
		const sentence = keptSentence('php', '8.3', [
			{ what: 'Logs', path: '/Users/x/.openvhost/logs/php-fpm-8.3', headline: true }
		]);
		expect(sentence).toContain('/Users/x/.openvhost/logs/php-fpm-8.3');
		expect(sentence).toContain('any site still set to PHP 8.3 keeps that setting');
	});

	// Same collapse hazard as the blockers: one sentence serving both kinds
	// would tell a MySQL user about logs, or a PHP user their databases are
	// safe when no database was ever involved.
	it('says something different for PHP than for MySQL', () => {
		const keeps: KeptItem[] = [{ what: 'Logs', path: '/a/logs', headline: true }];
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

	// The refusal that protects something OUTSIDE OpenVHost. On the owner's
	// machine `php@8.5` is an alias of the unversioned `php` formula, so `brew
	// uninstall php@8.5` would remove the linked `php 8.5.9` and break `php`
	// system-wide — the removal the user asked for and the removal brew would
	// perform are not the same removal.
	const foreign: Blocker = {
		kind: 'foreignKeg',
		formula: 'php@8.5',
		owner: 'php',
		keg: '/opt/homebrew/Cellar/php/8.5.9'
	};

	it('says the name is an alias, and names the formula that really owns the keg', () => {
		const message = blockerMessage(foreign);
		expect(message.obstacle).toContain('php@8.5');
		expect(message.obstacle).toContain('unversioned php formula');
	});

	// The keg path is what makes the refusal unarguable: the user can see which
	// directory would actually go.
	it('names the keg that would actually be removed', () => {
		expect(blockerMessage(foreign).obstacle).toContain('/opt/homebrew/Cellar/php/8.5.9');
	});

	it('says the consequence in the user’s terms: the linked php goes too', () => {
		const message = blockerMessage(foreign);
		expect(message.obstacle).toContain('your linked php');
		expect(message.obstacle).toContain('not just this version');
	});

	// Not a dead end: the user CAN do this, with the consequence visible. The
	// command names the OWNER, not the alias — telling them to run `brew
	// uninstall php@8.5` would be handing over the very trap being refused.
	it('hands over the command for the owner formula, not the alias', () => {
		const message = blockerMessage(foreign);
		expect(message.action).toContain('brew uninstall php');
		expect(message.action).not.toContain('brew uninstall php@8.5');
	});

	// Every name is the plan's, never this file's: a machine whose alias
	// resolves elsewhere must not be told about `php`.
	it('carries whatever formula, owner and keg the plan reported', () => {
		const message = blockerMessage({
			kind: 'foreignKeg',
			formula: 'mysql@8.4',
			owner: 'mysql',
			keg: '/usr/local/Cellar/mysql/9.1.0'
		});
		expect(message.obstacle).toContain('mysql@8.4');
		expect(message.obstacle).toContain('/usr/local/Cellar/mysql/9.1.0');
		expect(message.action).toContain('brew uninstall mysql');
		expect(message.obstacle).not.toContain('php');
	});

	// Unknown is not safe: brew resolves its own aliases whether or not an `opt`
	// link exists here, so the foreign-keg danger is fully present, just
	// unprovable. The message must say that rather than sounding like a glitch.
	it('says plainly that the keg could not be resolved, and where it looked', () => {
		const message = blockerMessage({
			kind: 'unknownKeg',
			formula: 'php@8.3',
			searched: ['/opt/homebrew/opt/php@8.3', '/usr/local/opt/php@8.3']
		});
		expect(message.obstacle).toContain('php@8.3');
		expect(message.obstacle).toContain('/opt/homebrew/opt/php@8.3');
		expect(message.obstacle).toContain('/usr/local/opt/php@8.3');
		expect(message.action).toContain('brew info php@8.3');
	});

	// `searched` can be empty (no Homebrew prefix at all). "there is nothing
	// under ." would read as a broken template.
	it('reads as a sentence when there was nowhere to look', () => {
		const message = blockerMessage({ kind: 'unknownKeg', formula: 'php@8.3', searched: [] });
		expect(message.obstacle).toContain('no Homebrew prefix to look in');
		expect(message.obstacle).not.toContain('nothing under .');
		expect(message.obstacle).not.toContain('undefined');
	});

	// The two keg refusals are the pair most likely to collapse: same subject,
	// adjacent code, one written just after the other. Asserted directly as
	// well as through the sweep below.
	it('keeps the two keg refusals apart', () => {
		const unknown = blockerMessage({
			kind: 'unknownKeg',
			formula: 'php@8.5',
			searched: ['/opt/homebrew/opt/php@8.5']
		});
		const known = blockerMessage(foreign);
		expect(unknown.obstacle).not.toBe(known.obstacle);
		expect(unknown.action).not.toBe(known.action);
		// …and they disagree about the thing that matters: one knows what would
		// be removed, the other does not.
		expect(known.obstacle).toContain('Cellar');
		expect(unknown.obstacle).not.toContain('Cellar');
	});

	// THE distinctness sweep. Pairwise, not "each is non-empty": a renderer
	// that collapsed two variants onto one sentence — the exact bug class this
	// codebase keeps producing — passes every non-emptiness check and fails
	// only here. Note it checks the obstacle AND the action separately, so the
	// SUBTLE collapse (a new blocker that names its own obstacle but reuses an
	// existing action) fails too.
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

describe('offersUninstall — installed is not enough', () => {
	// The MEDIUM the branch review found: `php_rows` lists an installed major
	// from outside the catalogue with `installed: true`, and the row offered a
	// button whose command could only refuse it.
	it('offers the action for an installed major this build manages', () => {
		expect(offersUninstall({ installed: true, cataloged: true })).toBe(true);
	});

	it('withholds it for an installed major this build does not manage', () => {
		expect(offersUninstall({ installed: true, cataloged: false })).toBe(false);
	});

	it('withholds it for anything not installed', () => {
		expect(offersUninstall({ installed: false, cataloged: true })).toBe(false);
		expect(offersUninstall({ installed: false, cataloged: false })).toBe(false);
	});
});

describe('outOfCatalogueNote — the explanation that replaces the button', () => {
	it('names the version and says OpenVHost will not remove it', () => {
		const note = outOfCatalogueNote('php', '7.4');
		expect(note).toContain('PHP 7.4');
		expect(note).toContain("OpenVHost won't uninstall it");
	});

	// "Use Homebrew" is not a next action; a command the user can paste is.
	it('hands over the exact brew command for a PHP version', () => {
		expect(outOfCatalogueNote('php', '7.4')).toContain('brew uninstall php@7.4');
	});

	it('hands over the exact brew command for a MySQL version', () => {
		expect(outOfCatalogueNote('mysql', '5.7')).toContain('brew uninstall mysql@5.7');
	});

	// Design D5: an out-of-band removal converges through the rescan, so the
	// note ends where the user should go next rather than at a dead end.
	it('points at the rescan that reconciles the app afterwards', () => {
		expect(outOfCatalogueNote('php', '7.4')).toContain('Check again');
	});

	// Same collapse hazard as everywhere else in this file: one note serving
	// both kinds would tell a MySQL user to uninstall a php formula.
	it('never gives two kinds the same note', () => {
		expect(outOfCatalogueNote('php', '8.0')).not.toBe(outOfCatalogueNote('mysql', '8.0'));
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
