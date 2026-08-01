// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import {
	SHARED_DATADIR_DISCLOSURE,
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
		source: null,
		offer: { kind: 'available', version: '8.4.11' },
		...overrides
	};
}

function inputs(overrides: Partial<MysqlRowInputs> = {}): MysqlRowInputs {
	return {
		instance: instance(),
		installingMajor: '',
		installProgress: null,
		installTotal: null,
		initializingMajor: '',
		initLog: [],
		initFailure: null,
		...overrides
	};
}

describe('mysqlRowState', () => {
	// Homebrew's presence is NOT an input any more. Installing MySQL is
	// download -> verify -> extract, so a machine that has never had brew
	// installs fine; what gates the row is whether this build publishes a
	// checksum-verified download for this host.
	it('is notInstalled, naming the exact version it would install, when a download exists', () => {
		expect(mysqlRowState(inputs())).toEqual({ kind: 'notInstalled', version: '8.4.11' });
	});

	it('is unavailable — an absence naming the target — when this host has no verified download', () => {
		const intel = instance({ offer: { kind: 'unavailable', target: 'macos-x86_64' } });
		expect(mysqlRowState(inputs({ instance: intel }))).toEqual({
			kind: 'unavailable',
			target: 'macos-x86_64'
		});
	});

	it('is installing while this exact major is mid-install, carrying the pipeline state', () => {
		expect(
			mysqlRowState(
				inputs({
					installingMajor: '8.4',
					installProgress: { kind: 'verified' },
					installTotal: 1024
				})
			)
		).toEqual({ kind: 'installing', progress: { kind: 'verified' }, total: 1024 });
	});

	it('is installing with a null progress before the first pipeline event arrives', () => {
		expect(mysqlRowState(inputs({ installingMajor: '8.4' }))).toEqual({
			kind: 'installing',
			progress: null,
			total: null
		});
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

	// The state an already-installed row must reach even where nothing could be
	// installed: an Intel Mac with a brew-installed MySQL is a supported machine,
	// and an absent DOWNLOAD must never hide a present RUNTIME.
	it('never reports unavailable for a major that is already installed', () => {
		const brewOnIntel = instance({
			installed: true,
			source: { kind: 'homebrew' },
			offer: { kind: 'unavailable', target: 'macos-x86_64' },
			datadirState: { kind: 'initialized' }
		});
		expect(mysqlRowState(inputs({ instance: brewOnIntel }))).toEqual({ kind: 'ready' });
	});

	it('is datadirForeign for an installed major with unexpected datadir content, whatever the offer says', () => {
		const foreign = instance({
			installed: true,
			offer: { kind: 'unavailable', target: 'macos-x86_64' },
			datadirState: { kind: 'foreign', detail: 'found unexpected.txt' }
		});
		expect(mysqlRowState(inputs({ instance: foreign }))).toEqual({
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

describe('SHARED_DATADIR_DISCLOSURE', () => {
	// The old copy described Homebrew's OWN formula creating a second datadir
	// as a side effect of pressing Install here. That stopped being true when
	// installing stopped going through brew, and stale copy that describes a
	// side effect the app no longer has is worse than none.
	it('describes the datadir being shared per version, not brew creating one', () => {
		expect(SHARED_DATADIR_DISCLOSURE).toMatch(/shared per version/i);
		expect(SHARED_DATADIR_DISCLOSURE).toMatch(/keeps those databases/i);
	});

	it('no longer claims a Homebrew formula creates anything', () => {
		expect(SHARED_DATADIR_DISCLOSURE).not.toMatch(/formula/i);
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
