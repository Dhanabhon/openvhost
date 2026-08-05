// SPDX-License-Identifier: GPL-3.0-or-later
// Pure notice dispatch for `MysqlRow.svelte`'s three per-engine banners —
// pulled out of that component (fix wave item 3, whole-branch review MEDIUM:
// "extract the outcomeNotice/ledgerNotice/offerNotice $derived.by dispatch
// into a small pure module the component imports"). A PURE extraction: the
// per-engine switch logic below is unchanged from what `MysqlRow.svelte` used
// to run inline in its own `<script>` block — only where it lives moved, not
// what it does — so every existing test, none of which reaches into this
// file directly (only through the row's own rendered output), stays green
// unmodified.
//
// Kept as its own module rather than folded into `databases.derive.ts`: that
// file is a dependency of `mariadb-install.derive.ts` (`engineDescriptor`,
// `engineAwaitingReleaseNotice`), so giving it a reverse dependency on
// `mariadb-install.derive.ts`'s own notice functions — which is what these
// three would need — would close a cycle. This module sits above both
// `mysql-install.derive.ts` and `mariadb-install.derive.ts` instead, and
// neither of those imports it back.

import type { MariadbInstallResultDto, MysqlInstallOutcomeDto, MysqlPackageOfferDto } from './ipc';
import type { EngineKind, MysqlRowState } from './databases.derive';
import {
	mysqlInstallResultNotice,
	mysqlLedgerNotice,
	mysqlPackageOfferNotice
} from './mysql-install.derive';
import type { Notice } from './mysql-install.derive';
import {
	mariadbInstallResultNotice,
	mariadbLedgerNotice,
	mariadbPackageOfferNotice
} from './mariadb-install.derive';

/** MariaDB's own settled outcome, tagged with the major it belongs to — the
 *  same shape `MysqlRow.svelte`'s own `mariadbInstallOutcome` prop carries
 *  (see that prop's doc comment for why `MariadbInstallResultDto` is a
 *  SEPARATE parameter rather than a widened `installOutcome`: it is not a
 *  subtype of `MysqlInstallResultDto`, design D2/D5). Not imported by
 *  `MysqlRow.svelte` itself — its own inline prop type is left as-is (this is
 *  a PURE extraction, nothing about the component's public props changes) —
 *  only used here, for these three functions' own parameter types. */
export type MariadbRowOutcome = { major: string; result: MariadbInstallResultDto } | null;

/**
 * The settled-install banner (design D1 follow-through, task 3 finding):
 * dispatched on `engine` rather than widening {@link mysqlInstallResultNotice}'s
 * own parameter type, because `MariadbInstallResultDto` adds a member
 * (`awaitingRelease`) `MysqlInstallResultDto` does not have. Reading
 * `mariadbRowOutcome`/`rowInstallOutcome` — each already typed correctly for
 * its own engine, and pre-scoped to THIS row by the caller — is what lets each
 * branch call its own notice function with no cast. A `switch` with a
 * `const _: never` default, not an `engine === 'mariadb'` ternary (this
 * codebase's standing "no wildcard arm" rule, applied to {@link EngineKind}
 * itself): a third engine must fail to compile HERE, not silently fall into
 * MySQL's branch. Before this dispatch existed, EVERY engine rendered
 * `mysqlInstallResultNotice`'s hardcoded "MySQL …" copy: the one piece of
 * `MysqlRow.svelte`'s generalization the row-refactor task had not reached,
 * because it never exercised a non-null `installOutcome` under
 * `engine="mariadb"`.
 */
export function engineOutcomeNotice(
	engine: EngineKind,
	rowInstallOutcome: MysqlInstallOutcomeDto | null,
	mariadbRowOutcome: MariadbRowOutcome
): Notice | null {
	switch (engine) {
		case 'mariadb':
			return mariadbRowOutcome === null
				? null
				: mariadbInstallResultNotice(mariadbRowOutcome.result);
		case 'mysql':
			return rowInstallOutcome === null ? null : mysqlInstallResultNotice(rowInstallOutcome.result);
		default: {
			const unreachable: never = engine;
			return unreachable;
		}
	}
}

/**
 * A ledger row that could not be written — provenance lost, never the
 * install. `null` on every other outcome and on the happy path. Same
 * per-engine `switch` as {@link engineOutcomeNotice}, for the same reason.
 */
export function engineLedgerNotice(
	engine: EngineKind,
	rowInstallOutcome: MysqlInstallOutcomeDto | null,
	mariadbRowOutcome: MariadbRowOutcome
): string | null {
	switch (engine) {
		case 'mariadb':
			return mariadbRowOutcome !== null && mariadbRowOutcome.result.kind === 'installed'
				? mariadbLedgerNotice(mariadbRowOutcome.result.ledger)
				: null;
		case 'mysql':
			return rowInstallOutcome !== null && rowInstallOutcome.result.kind === 'installed'
				? mysqlLedgerNotice(rowInstallOutcome.result.ledger)
				: null;
		default: {
			const unreachable: never = engine;
			return unreachable;
		}
	}
}

/**
 * The row's own explanation for why there is (or is not) an Install button to
 * press — covers exactly the two states {@link mysqlPackageOfferNotice}'s
 * narrower MySQL-only union was always built for (`unavailable`/
 * `notInstalled`); `awaitingRelease` has its OWN notice
 * (`engineAwaitingReleaseNotice`, `databases.derive.ts`) because a build
 * existing but unpublished is a materially different fact (design D2), not a
 * naming variant of this one. `null` in every OTHER row state. The engine
 * `switch` is defined once and called from both branches — same
 * exhaustiveness reasoning as {@link engineOutcomeNotice}.
 */
export function engineOfferNotice(engine: EngineKind, rowState: MysqlRowState): Notice | null {
	const paint = (offer: MysqlPackageOfferDto) => {
		switch (engine) {
			case 'mariadb':
				return mariadbPackageOfferNotice(offer);
			case 'mysql':
				return mysqlPackageOfferNotice(offer);
			default: {
				const unreachable: never = engine;
				return unreachable;
			}
		}
	};
	if (rowState.kind === 'unavailable') {
		return paint({ kind: 'unavailable', target: rowState.target });
	}
	if (rowState.kind === 'notInstalled') {
		return paint({ kind: 'available', version: rowState.version });
	}
	return null;
}
