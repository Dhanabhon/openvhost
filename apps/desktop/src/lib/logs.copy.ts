// SPDX-License-Identifier: GPL-3.0-or-later
// Every user-visible string the Logs page renders that isn't a bare label,
// authored in ONE place (mirrors `sites.derive.ts`'s `scaffoldNotice` —
// "the ONE place authoring them, so translation/extraction has a single
// seam later", CLAUDE.md's i18n instruction for the pre-Phase-2 period).
// Voice per brand guidelines §6.1: plain verbs, explain-and-point-forward,
// no hype.
import type { LogResetDto } from './ipc';

export function noSelectionCopy(): string {
	return 'Pick a log source above to get started.';
}

export function notYetCreatedCopy(): string {
	return "This log hasn't been created yet. It will appear once the service starts or the site is applied and served.";
}

export function emptyCopy(filtered: boolean): string {
	return filtered
		? 'No lines match your filter.'
		: 'This log is empty — nothing has been written to it yet.';
}

export function permissionDeniedCopy(): string {
	return "OpenVHost can't read this file — check its permissions, or use Open log folder to inspect it in Finder.";
}

export function genericReadErrorCopy(message: string): string {
	return `Could not read this log: ${message}`;
}

export function unavailableSourceCopy(label: string): string {
	return `${label} isn't available anymore — the site or service it belonged to may have been removed. Pick another source below.`;
}

/** Exhaustive over `LogResetDto`, no `default:` — a third reset reason
 *  added later fails typecheck here rather than falling through silently
 *  (same discipline as `sites.derive.ts`'s `scaffoldNotice`). */
export function resetNoticeCopy(reset: LogResetDto): string {
	switch (reset) {
		case 'rotated':
			return 'The log file was rotated or replaced — showing the newest lines.';
		case 'truncated':
			return 'The log file was truncated — showing the newest lines.';
		default: {
			const unreachable: never = reset;
			return unreachable;
		}
	}
}

export function scanBoundCopy(): string {
	return 'Stopped scanning early — there may be more matching lines further back. Narrow your filter to search further.';
}

export function sizeWarningCopy(): string {
	return "This file is over 100 MiB. OpenVHost doesn't rotate logs yet — open the folder to manage it manually.";
}

/** Spec D5: "no false redaction promises for error logs... the UI carries a
 *  plain 'these are local logs, they may contain credentials' note instead
 *  of a false guarantee." Rendered once, unconditionally, near the toolbar —
 *  not only for error-kind sources, since access logs can carry sensitive
 *  path/header data too even with query strings stripped. */
export function privacyNoteCopy(): string {
	return 'Logs are stored locally on this machine and may contain sensitive data from requests or errors.';
}
