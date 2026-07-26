// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { errorMessage } from './errors';

describe('errorMessage', () => {
	it('uses a real message when there is one', () => {
		expect(errorMessage({ kind: 'proc', message: 'nginx would not stop' })).toBe(
			'nginx would not stop'
		);
	});

	// `IpcError`'s `simulated` variant has no `message` at all. `String(e)` here
	// would put "[object Object]" in front of the user.
	it('never renders [object Object] for a message-less object', () => {
		const msg = errorMessage({ kind: 'simulated' });
		expect(msg).not.toContain('[object Object]');
		expect(msg).not.toBe('');
	});

	// An empty string is technically a `string` — falling through to it would show
	// an error banner with no text, which reads as a rendering bug.
	it('falls back when the message is an empty string', () => {
		expect(errorMessage({ message: '' })).not.toBe('');
	});

	it('handles values that are not objects at all', () => {
		expect(errorMessage(undefined)).not.toBe('');
		expect(errorMessage('boom')).not.toBe('');
	});
});
