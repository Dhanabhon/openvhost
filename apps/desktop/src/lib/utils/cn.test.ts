// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { cn } from './cn';

describe('cn', () => {
	it('joins truthy classes and drops falsy', () => {
		// eslint-disable-next-line no-constant-binary-expression -- intentional falsy fixture, mirrors the `cond && 'class'` idiom cn() must filter out
		expect(cn('a', false && 'b', 'c')).toBe('a c');
	});
	it('later tailwind utility wins on conflict', () => {
		expect(cn('px-2', 'px-4')).toBe('px-4');
	});
});
