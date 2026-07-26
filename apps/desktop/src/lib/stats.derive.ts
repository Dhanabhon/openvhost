// SPDX-License-Identifier: GPL-3.0-or-later
// Pure formatting for the status bar. Kept out of the store so the strings the
// user actually reads are testable without timers or IPC.

const STEP = 1024;
const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const;

/** Rendered when a figure is unknown — a failed sample, or one not taken yet. */
export const UNKNOWN = '—';

/**
 * Human-readable byte count: 1024-based steps labelled B/KB/MB/GB/TB, one
 * decimal place when the mantissa is below 10 and none at or above it. So
 * `1.0 KB`, `10 KB`, `1.2 GB`, `12 GB`.
 *
 * 1024-based with decimal-looking labels is the convention `ps` and developer
 * tooling use; Finder's 1000-based labels would disagree with every other number
 * a developer sees next to this one.
 *
 * Zero is special-cased to `0 MB`: the only zero we ever render is "nothing is
 * running", and `0 B` reads like a measurement error while `0 MB` reads like a
 * total. It also stops the unit jumping when the first service starts.
 */
export function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes < 0) return UNKNOWN;
	if (bytes === 0) return '0 MB';
	let value = bytes;
	let unit = 0;
	while (value >= STEP && unit < UNITS.length - 1) {
		value /= STEP;
		unit += 1;
	}
	const digits = value < 10 && unit > 0 ? 1 : 0;
	return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** `0 → "no processes"`, `1 → "1 process"`, `n → "n processes"`. */
export function formatProcessCount(n: number): string {
	if (n === 0) return 'no processes';
	return `${n} ${n === 1 ? 'process' : 'processes'}`;
}
