// SPDX-License-Identifier: GPL-3.0-or-later
// Pure copy for the one sentence the quit confirmation says about a package
// operation that is still in flight — the `brew install`, the MySQL init, or
// (since the package-uninstall slice) the `brew uninstall` holding
// `InstallLock`'s single slot.
//
// WHY THIS FILE EXISTS AT ALL, rather than the sentence living in
// `QuitDialog.svelte`'s markup where it used to:
//
// The dialog took its prop as `{ kind, label }` and wrote one sentence. Rust
// had already started sending `operation: 'install' | 'uninstall'` end to end —
// `PendingInstallDto` → bindings → `+layout.svelte` → this dialog — and
// TypeScript accepted the extra field in silence (structural typing does not
// excess-property-check a variable). So a user quitting during `brew uninstall`
// read "PHP 8.4 is still installing … discards the download/build in progress",
// where every clause was false. The branch review rated it HIGH.
//
// Two things stop that recurring. First, {@link PendingOperation} REQUIRES
// `operation`, so `+layout.svelte`'s assignment of the generated DTO is now a
// compile-time seam: the day Rust drops or renames that field, the layout fails
// to typecheck instead of the dialog quietly narrating the wrong operation.
// Second, the copy is computed here and asserted here — including that the two
// operations never produce the same sentence, which is the specific bug class
// this codebase keeps re-shipping (a state collapsed onto one rendering).

/** Which package family occupies the shared install lock. Mirrors the generated
 *  `InstallKindDto`; the assignment in `+layout.svelte` pins the two together. */
export type PendingOperationKind = 'php' | 'mysql';

/** Which direction the run is going. Mirrors the generated
 *  `PackageOperationDto`. */
export type PackageOperation = 'install' | 'uninstall';

/**
 * What `pending_install` reports, in the shape this dialog consumes.
 *
 * `label`'s shape differs deliberately by kind, mirroring exactly what the Rust
 * side tracks (`InstallLock`'s `set_running` call sites): a PHP run's label is
 * the BARE major (`"8.4"`), because {@link pendingOperationCopy} supplies the
 * leading "PHP" word itself; a MySQL run's label ALREADY reads as a complete
 * phrase (`"MySQL 8.4"`, `"MySQL 8.4 initialization"`), so rendering it verbatim
 * is correct and prepending another "MySQL" would double it.
 */
export interface PendingOperation {
	kind: PendingOperationKind;
	operation: PackageOperation;
	label: string;
}

/**
 * The sentence, split at the label so the dialog can render the label in the
 * monospace face the rest of this app uses for machine-shaped values (version
 * numbers, service ids, paths) without this file emitting markup.
 *
 * `lead + label + rest` is the whole sentence and reads as one; the tests
 * assert it that way.
 */
export interface PendingOperationCopy {
	/** The word this UI supplies before the label — `'PHP '` for PHP's bare
	 *  major, `''` for MySQL's already-complete phrase. */
	lead: string;
	/** Rendered verbatim, in mono. Never re-worded here. */
	label: string;
	/** Everything after the label, starting with the space that follows it. */
	rest: string;
}

/** Exhaustive over {@link PendingOperationKind} with the never-typed default arm
 *  this codebase uses everywhere — a third package family must fail to compile
 *  here rather than silently lose (or double) its leading word. */
function operationLead(kind: PendingOperationKind): string {
	switch (kind) {
		case 'php':
			return 'PHP ';
		case 'mysql':
			return '';
		default: {
			const unreachable: never = kind;
			return unreachable;
		}
	}
}

/**
 * What quitting costs, per direction. Exhaustive over {@link PackageOperation},
 * no `default` fallback — a third operation must fail to compile rather than
 * inherit a sentence written about a different one, which is precisely how the
 * uninstall case shipped narrated as an install.
 *
 * The install wording is unchanged, deliberately: it was correct, and the
 * failure being fixed was that it was the ONLY wording.
 *
 * The uninstall wording had to stop claiming a download is at risk, because
 * none is, and name what is genuinely at risk instead. The live proof produced
 * the exact failure: a `brew uninstall` killed part-way left `brew list` still
 * reporting `php@8.3 8.3.33` while the keg held 9 of 15 entries with `bin/` and
 * `INSTALL_RECEIPT.json` already gone. Child-process containment is correct —
 * nothing survives the quit — but Homebrew's own metadata and the filesystem
 * can disagree, and that is a state only the user (via brew) can settle, so the
 * copy points them at it rather than at "starting over".
 */
function operationRest(operation: PackageOperation): string {
	switch (operation) {
		case 'install':
			return (
				' is still installing. Quitting stops it immediately and discards the download/build in ' +
				'progress — there is no resuming it, only starting over.'
			);
		case 'uninstall':
			return (
				' is still being removed. Quitting stops Homebrew part-way, which can leave the package ' +
				'half-removed — still listed by brew while some of its files are already gone. Check it ' +
				'with brew after reopening OpenVHost, and uninstall it again if it is still listed.'
			);
		default: {
			const unreachable: never = operation;
			return unreachable;
		}
	}
}

/** The in-flight sentence for one pending operation. */
export function pendingOperationCopy(pending: PendingOperation): PendingOperationCopy {
	return {
		lead: operationLead(pending.kind),
		label: pending.label,
		rest: operationRest(pending.operation)
	};
}

/** The whole sentence as one string — what a reader sees, and what the tests
 *  assert on. The dialog renders the three parts separately only so the label
 *  can carry the mono face. */
export function pendingOperationSentence(pending: PendingOperation): string {
	const copy = pendingOperationCopy(pending);
	return `${copy.lead}${copy.label}${copy.rest}`;
}
