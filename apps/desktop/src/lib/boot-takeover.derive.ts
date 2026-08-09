// SPDX-License-Identifier: GPL-3.0-or-later
// Every word the takeover screen says, decided in one exhaustive switch.
//
// Same split `site-readiness.derive.ts` established: the derive picks the copy,
// `BootTakeover.svelte` picks none. Two reasons, and both are load-bearing here.
//
//   1. TWO screens for THREE states (design D3). `runDirUnusable` and
//      `homeUnresolvable` share one screen with a different sentence — a shared
//      structure that only differs in its text is exactly the thing that drifts
//      when the text lives in markup branches.
//   2. A fifth `BootStatusDto` variant must FAIL TO COMPILE rather than
//      silently inherit a fourth's screen, the same guarantee `boot_dto` and
//      `stderr_line` give on the Rust side. A `{#if boot.kind === …}` chain in a
//      template gives no such guarantee.
//
// What every screen owes the reader, because this whole line of work exists
// because the app once said *"You must call `.manage()` before using this
// command"* at a user: what is wrong, what they can do about it, and — for
// `alreadyRunning` — that this is normal and not a failure.
import type { DegradedBoot } from './boot-status.svelte';

/**
 * One verbatim fact from the boot state: a real path, or raw OS error text.
 *
 * Rendered in the mono face and never parsed, summarised or truncated. On
 * `runDirUnusable` the path and the errno ARE the payload — *Permission denied
 * (os error 13)* is the only line on that screen a user can act on, and a
 * friendlier paraphrase would put it back where `.manage()` left them.
 */
export interface BootDetail {
	label: string;
	value: string;
	testId: string;
}

export interface TakeoverCopy {
	/** Per-state hook, so a test can prove each state renders its OWN screen. */
	testId: string;
	title: string;
	details: BootDetail[];
	lines: string[];
}

export function takeoverCopy(boot: DegradedBoot): TakeoverCopy {
	switch (boot.kind) {
		case 'alreadyRunning':
			return {
				testId: 'boot-already-running',
				title: 'OpenVHost is already running',
				details: [{ label: 'Working folder', value: boot.home, testId: 'boot-home' }],
				lines: [
					'Another copy of OpenVHost is already using this folder. Only one copy can use it at a ' +
						'time, so this window cannot manage your sites or services.',
					// The "this is normal" half. Without it the screen reads as a
					// failure report, and the user's next move is to go looking for
					// damage that is not there.
					'Nothing has gone wrong. The copy that is already running is still serving your sites, ' +
						'and none of your settings have changed. Switch to it from the Dock or the menu bar, ' +
						'then quit this window.'
				]
			};
		case 'runDirUnusable':
			return {
				testId: 'boot-run-dir-unusable',
				title: 'OpenVHost cannot use its working folder',
				details: [
					{ label: 'Folder', value: boot.path, testId: 'boot-run-dir' },
					{ label: 'Error', value: boot.reason, testId: 'boot-reason' }
				],
				lines: [
					'OpenVHost keeps the lock and socket files that coordinate its services in this folder, ' +
						'and it cannot start without them.',
					// Names the errno as the actionable line rather than restating the
					// title in longer words: this is a permissions problem the user can
					// fix, and the error text is what tells them which one.
					'The error above is the part to act on. A permission error means this folder, or the ' +
						'folder containing it, is not writable by your user account. Fix that and open ' +
						'OpenVHost again.'
				]
			};
		case 'homeUnresolvable':
			// The SAME screen as `runDirUnusable` — same component, same structure,
			// different sentences (design D3). It gets no bespoke UI on purpose: it
			// needs `$HOME` unset AND a failing passwd lookup, or a deleted working
			// directory, so it is near-unreachable on macOS and a designed screen
			// would over-serve a state nobody will see.
			return {
				testId: 'boot-home-unresolvable',
				title: 'OpenVHost cannot work out where to keep its files',
				details: [{ label: 'Error', value: boot.reason, testId: 'boot-reason' }],
				lines: [
					'OpenVHost keeps your sites, settings and service files together in one folder, and it ' +
						'could not work out where that folder should be. Normally it uses a folder named ' +
						'.openvhost inside your home directory.',
					'Set OPENVHOST_HOME to the full path of a folder you can write to, then open OpenVHost ' +
						'again.'
				]
			};
		default: {
			// Not a wildcard: nothing reaches this arm, and its only statement is an
			// assignment that stops compiling the moment a fifth variant exists. A
			// `default` that RETURNED copy would be the wildcard this repo bans.
			const unreachable: never = boot;
			return unreachable;
		}
	}
}
