// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { formatBytes, formatProcessCount } from './stats.derive';

describe('formatBytes', () => {
	// Exact strings, because the strip's whole job is to be readable at a glance
	// and a rounding change is a visible change.
	it('steps through 1024-based units with the specified precision', () => {
		// M-b: the B tier's `unit > 0` term (no decimal at the bottom tier, even
		// though the mantissa is below 10) is deliberate but had no vector.
		// Deleting it turns this into `5.0 B`.
		expect(formatBytes(5)).toBe('5 B');
		expect(formatBytes(999)).toBe('999 B');
		expect(formatBytes(1024)).toBe('1.0 KB');
		// The mantissa-10 switch: one decimal below 10, none at or above.
		expect(formatBytes(10 * 1024)).toBe('10 KB');
		expect(formatBytes(1024 ** 3)).toBe('1.0 GB');
		expect(formatBytes(1024 ** 3 * 12)).toBe('12 GB');
		// M-a: the code already ships a TB tier the spec's §4.4/§7 omitted —
		// amended there rather than removed here, since the code is the correct
		// side of that disagreement. Pin it so it stays unpinned no longer.
		expect(formatBytes(1024 ** 4)).toBe('1.0 TB');
	});

	// Zero is only ever "nothing is running". "0 B" reads like a measurement
	// error; "0 MB" reads like a total, and keeps the unit from jumping between
	// the running and idle states.
	it('renders zero as 0 MB', () => {
		expect(formatBytes(0)).toBe('0 MB');
	});

	// The store passes `null` through as unknown, but a negative or non-finite
	// number reaching here would be a bug — render it as unknown rather than
	// printing "NaN B" at the user.
	it('renders a nonsensical input as unknown rather than NaN', () => {
		expect(formatBytes(-1)).toBe('—');
		expect(formatBytes(Number.NaN)).toBe('—');
		expect(formatBytes(Number.POSITIVE_INFINITY)).toBe('—');
	});
});

describe('formatProcessCount', () => {
	it('agrees in number and names the empty case', () => {
		expect(formatProcessCount(0)).toBe('no processes');
		expect(formatProcessCount(1)).toBe('1 process');
		expect(formatProcessCount(2)).toBe('2 processes');
	});
});
