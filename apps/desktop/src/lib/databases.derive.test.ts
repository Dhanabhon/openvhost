// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import {
	anyMysqlInstalled,
	catalogedMajors,
	mysqlInitStepLabel,
	mysqlPortConflictHint,
	mysqlRowState,
	unreachableMysqlRowState,
	type MysqlInitFailure,
	type MysqlRowInputs
} from './databases.derive';
import type { MysqlInitStepDto, MysqlInstanceDto } from './ipc';

function instance(overrides: Partial<MysqlInstanceDto> = {}): MysqlInstanceDto {
	return {
		major: '8.4',
		cataloged: true,
		installed: false,
		path: null,
		socketPath: null,
		serviceId: null,
		datadirState: { kind: 'notInitialized' },
		...overrides
	};
}

function inputs(overrides: Partial<MysqlRowInputs> = {}): MysqlRowInputs {
	return {
		brewFound: true,
		instance: instance(),
		installingMajor: '',
		installLog: [],
		initializingMajor: '',
		initLog: [],
		initFailure: null,
		...overrides
	};
}

describe('mysqlRowState', () => {
	it('is noBrew when not installed and Homebrew is missing', () => {
		expect(mysqlRowState(inputs({ brewFound: false }))).toEqual({ kind: 'noBrew' });
	});

	it('is notInstalled when not installed but Homebrew is present', () => {
		expect(mysqlRowState(inputs({ brewFound: true }))).toEqual({ kind: 'notInstalled' });
	});

	it('is installing while this exact major is mid-brew-install, with the scoped log', () => {
		const log = [{ id: '8.4', tsMs: 1, level: 'info' as const, line: 'Fetching...' }];
		expect(
			mysqlRowState(inputs({ installingMajor: '8.4', installLog: log, brewFound: false }))
		).toEqual({ kind: 'installing', log });
	});

	it('never shows installing for a different major', () => {
		expect(mysqlRowState(inputs({ installingMajor: '8.5' }))).not.toEqual(
			expect.objectContaining({ kind: 'installing' })
		);
	});

	it('is initializing while this exact major is mid-staged-init, with the scoped log', () => {
		const log = [{ id: '8.4', tsMs: 1, level: 'info' as const, line: 'Rendering config...' }];
		expect(
			mysqlRowState(
				inputs({
					instance: instance({ installed: true }),
					initializingMajor: '8.4',
					initLog: log
				})
			)
		).toEqual({ kind: 'initializing', log });
	});

	it('prefers installing over initializing when (implausibly) both name this major', () => {
		// InstallLock is single-flight page-wide, so this combination cannot occur
		// in practice — the precedence is pinned anyway so the function stays
		// deterministic rather than accidentally order-dependent.
		expect(mysqlRowState(inputs({ installingMajor: '8.4', initializingMajor: '8.4' })).kind).toBe(
			'installing'
		);
	});

	it('is datadirForeign for an installed major with unexpected datadir content, regardless of brewFound', () => {
		const foreign = instance({
			installed: true,
			datadirState: { kind: 'foreign', detail: 'found unexpected.txt' }
		});
		expect(mysqlRowState(inputs({ instance: foreign, brewFound: false }))).toEqual({
			kind: 'datadirForeign',
			detail: 'found unexpected.txt'
		});
	});

	it('is ready for an installed major with an Initialized datadir', () => {
		const ready = instance({ installed: true, datadirState: { kind: 'initialized' } });
		expect(mysqlRowState(inputs({ instance: ready }))).toEqual({ kind: 'ready' });
	});

	it('is installedNotInitialized for an installed major with no init attempt remembered', () => {
		const notInit = instance({ installed: true, datadirState: { kind: 'notInitialized' } });
		expect(mysqlRowState(inputs({ instance: notInit }))).toEqual({
			kind: 'installedNotInitialized'
		});
	});

	it('is initFailed when the last init attempt for THIS major failed', () => {
		const notInit = instance({ installed: true, datadirState: { kind: 'notInitialized' } });
		const failure: MysqlInitFailure = { major: '8.4', step: 'setPassword', reason: 'boom' };
		expect(mysqlRowState(inputs({ instance: notInit, initFailure: failure }))).toEqual({
			kind: 'initFailed',
			step: 'setPassword',
			reason: 'boom'
		});
	});

	it('does not attribute a failed init attempt to a different major', () => {
		const notInit = instance({
			major: '9.9',
			installed: true,
			datadirState: { kind: 'notInitialized' }
		});
		const failure: MysqlInitFailure = { major: '8.4', step: 'setPassword', reason: 'boom' };
		expect(mysqlRowState(inputs({ instance: notInit, initFailure: failure }))).toEqual({
			kind: 'installedNotInitialized'
		});
	});

	it('lets a fresh Ready read supersede a stale remembered failure for the same major', () => {
		// Datadir classification is read from disk on every load (spec D2) — a
		// later successful attempt (or a rescan) must outrank a stale memory of
		// an earlier one's failure.
		const ready = instance({ installed: true, datadirState: { kind: 'initialized' } });
		const failure: MysqlInitFailure = { major: '8.4', step: 'setPassword', reason: 'boom' };
		expect(mysqlRowState(inputs({ instance: ready, initFailure: failure }))).toEqual({
			kind: 'ready'
		});
	});

	it('lets a fresh Foreign read supersede a stale remembered failure for the same major', () => {
		const foreign = instance({
			installed: true,
			datadirState: { kind: 'foreign', detail: 'x' }
		});
		const failure: MysqlInitFailure = { major: '8.4', step: 'render', reason: 'boom' };
		expect(mysqlRowState(inputs({ instance: foreign, initFailure: failure }))).toEqual({
			kind: 'datadirForeign',
			detail: 'x'
		});
	});
});

describe('unreachableMysqlRowState', () => {
	it('throws rather than silently doing nothing', () => {
		// Proves the runtime half of the exhaustiveness guard: `MysqlRow.svelte`'s
		// template calls this from its final `{:else}` branch, so if a ninth
		// `MysqlRowState` variant is ever added AND handled, this line still runs
		// as designed — a value that reaches it despite every named `kind` being
		// handled is a real bug, not a silent no-op.
		expect(() => unreachableMysqlRowState({ kind: 'bogus' } as never)).toThrow();
	});
});

describe('mysqlInitStepLabel', () => {
	const steps: MysqlInitStepDto[] = [
		'render',
		'validate',
		'initialize',
		'startTempServer',
		'setPassword',
		'shutdown',
		'finalize'
	];

	it('gives every step a non-empty, dev-plain label', () => {
		for (const step of steps) {
			expect(mysqlInitStepLabel(step).length).toBeGreaterThan(0);
		}
	});

	it('gives every step its own distinct label', () => {
		// Catches a copy-paste that leaves two steps reading identically, which
		// would make an InitFailed row's "failed while X" sentence useless for
		// telling steps apart.
		const labels = steps.map(mysqlInitStepLabel);
		expect(new Set(labels).size).toBe(steps.length);
	});
});

describe('mysqlPortConflictHint', () => {
	it('points at brew services stop when the stderr tail names the exact conflict', () => {
		const hint = mysqlPortConflictHint(['2026-07-29 [ERROR] Address already in use']);
		expect(hint).toContain('brew services stop mysql@8.4');
	});

	it('matches case-insensitively, since child stderr casing is not a contract', () => {
		expect(mysqlPortConflictHint(['ADDRESS ALREADY IN USE'])).not.toBeNull();
	});

	it('is null for an unrelated failure', () => {
		expect(mysqlPortConflictHint(['some other startup error'])).toBeNull();
	});

	it('is null for an empty tail', () => {
		expect(mysqlPortConflictHint([])).toBeNull();
	});
});

describe('anyMysqlInstalled', () => {
	it('is true when at least one row is installed', () => {
		expect(anyMysqlInstalled([instance({ installed: false }), instance({ installed: true })])).toBe(
			true
		);
	});

	it('is false when nothing is installed, including an empty list', () => {
		expect(anyMysqlInstalled([instance({ installed: false })])).toBe(false);
		expect(anyMysqlInstalled([])).toBe(false);
	});
});

describe('catalogedMajors', () => {
	it('lists only cataloged majors, in row order', () => {
		const rows = [
			instance({ major: '8.4', cataloged: true }),
			instance({ major: '9.7', cataloged: false })
		];
		expect(catalogedMajors(rows)).toEqual(['8.4']);
	});

	it('is empty when nothing is cataloged', () => {
		expect(catalogedMajors([instance({ cataloged: false })])).toEqual([]);
	});
});
