// SPDX-License-Identifier: GPL-3.0-or-later
//
// Runs in the existing `node` vitest project: a rune-based store has no DOM
// dependency (same as `sites.svelte.test.ts`).
import { describe, expect, it } from 'vitest';
import type { ApplyPlanDto, WebServerSettingsDto } from './ipc';
import { WebSettingsStore, type WebSettingsApi } from './websettings.svelte';

const STORED: WebServerSettingsDto = {
	workerConnections: 1024,
	clientMaxBodySize: '256m',
	keepaliveTimeout: 65,
	tcpNodelay: true,
	fastcgiConnectTimeout: 60,
	fastcgiSendTimeout: 300,
	fastcgiReadTimeout: 300,
	gzip: false,
	gzipCompLevel: 1,
	gzipTypes: 'text/plain text/css'
};

function dto(over: Partial<WebServerSettingsDto> = {}): WebServerSettingsDto {
	return { ...STORED, ...over };
}

const emptyPlan: ApplyPlanDto = { changes: [] };

/** An api whose read succeeds and whose save and plan do nothing. */
function api(over: Partial<WebSettingsApi> = {}): WebSettingsApi {
	return {
		webServerSettings: async () => dto(),
		saveWebServerSettings: async () => {},
		planConfigApply: async () => emptyPlan,
		...over
	};
}

describe('loading', () => {
	it('loads the stored values', async () => {
		const s = new WebSettingsStore(
			api({ webServerSettings: async () => dto({ fastcgiReadTimeout: 900 }) })
		);
		await s.load();
		expect(s.values?.fastcgiReadTimeout).toBe(900);
	});

	it('starts clean, so nothing is dirty before an edit', async () => {
		const s = new WebSettingsStore(api());
		await s.load();
		expect(s.dirty).toBe(false);
	});

	it('surfaces a failed read instead of pretending the defaults are stored', async () => {
		const s = new WebSettingsStore(
			api({
				webServerSettings: async () => {
					throw { kind: 'core', message: 'state.db is locked' };
				}
			})
		);
		await s.load();
		expect(s.values).toBeNull();
		expect(s.error).toContain('state.db is locked');
	});
});

describe('editing', () => {
	it('marks the form dirty once a value changes', async () => {
		const s = new WebSettingsStore(api());
		await s.load();
		s.setNumber('fastcgiReadTimeout', '900');
		expect(s.values?.fastcgiReadTimeout).toBe(900);
		expect(s.dirty).toBe(true);
	});

	it('converts a number input to a number, not a string', async () => {
		// A string reaches `u32` deserialization and fails there — a transport
		// error naming no field, which the form cannot mark.
		const s = new WebSettingsStore(api());
		await s.load();
		s.setNumber('workerConnections', '2048');
		expect(s.values?.workerConnections).toBe(2048);
		expect(typeof s.values?.workerConnections).toBe('number');
	});

	it('keeps the last good value and marks the field when the box is emptied', async () => {
		const s = new WebSettingsStore(api());
		await s.load();
		s.setNumber('workerConnections', '');
		expect(s.values?.workerConnections).toBe(1024);
		expect(s.fieldErrors.worker_connections).toBeDefined();
		expect(s.canSave).toBe(false);
	});

	it('clears that mark as soon as a whole number is typed', async () => {
		const s = new WebSettingsStore(api());
		await s.load();
		s.setNumber('workerConnections', '');
		s.setNumber('workerConnections', '2048');
		expect(s.fieldErrors.worker_connections).toBeUndefined();
		expect(s.canSave).toBe(true);
	});

	it('sends nothing while a number box is unusable', async () => {
		let calls = 0;
		const s = new WebSettingsStore(api({ saveWebServerSettings: async () => void calls++ }));
		await s.load();
		s.setNumber('gzipCompLevel', 'abc');
		expect(await s.save()).toBe(false);
		expect(calls).toBe(0);
	});

	it('edits booleans and text too', async () => {
		const s = new WebSettingsStore(api());
		await s.load();
		s.setBool('gzip', true);
		s.setText('gzipTypes', 'text/html');
		expect(s.values?.gzip).toBe(true);
		expect(s.values?.gzipTypes).toBe('text/html');
	});
});

describe('saving', () => {
	it('sends what is on the form', async () => {
		let sent: WebServerSettingsDto | null = null;
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async (input) => {
					sent = input;
				}
			})
		);
		await s.load();
		s.setNumber('fastcgiReadTimeout', '900');
		expect(await s.save()).toBe(true);
		expect(sent!.fastcgiReadTimeout).toBe(900);
	});

	it('marks the offending field and leaves the others clean', async () => {
		// A whole-form error would make the user hunt for which input was wrong.
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async () => {
					throw {
						kind: 'validation',
						field: 'gzip_comp_level',
						message: '"99" must be between 1 and 9'
					};
				}
			})
		);
		await s.load();
		expect(await s.save()).toBe(false);
		expect(s.fieldErrors.gzip_comp_level).toContain('between 1 and 9');
		expect(s.fieldErrors.keepalive_timeout).toBeUndefined();
		expect(s.error).toBe('');
	});

	it('keys the mark by the BACKEND field name, which is snake_case', async () => {
		// The trap this whole seam exists for: `IpcError::Validation.field` is
		// `fastcgi_read_timeout` while the DTO field is `fastcgiReadTimeout`.
		// Keying by the DTO name would mark nothing, and a rejected save would
		// read to the user as a save that silently did nothing.
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async () => {
					throw {
						kind: 'validation',
						field: 'fastcgi_read_timeout',
						message: '"99999999" must be between 1 and 86400'
					};
				}
			})
		);
		await s.load();
		await s.save();
		expect(s.fieldErrors.fastcgi_read_timeout).toBeDefined();
		expect(s.fieldErrors.fastcgiReadTimeout).toBeUndefined();
	});

	it('routes a non-validation failure to the page error, not to a field', async () => {
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async () => {
					throw { kind: 'core', message: 'state.db is locked' };
				}
			})
		);
		await s.load();
		expect(await s.save()).toBe(false);
		expect(s.error).toContain('state.db is locked');
		expect(Object.keys(s.fieldErrors)).toHaveLength(0);
	});

	it('clears a field error once that field is saved successfully', async () => {
		let fail = true;
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async () => {
					if (fail) throw { kind: 'validation', field: 'gzip_comp_level', message: 'bad' };
				}
			})
		);
		await s.load();
		await s.save();
		expect(s.fieldErrors.gzip_comp_level).toBeDefined();
		fail = false;
		await s.save();
		expect(s.fieldErrors.gzip_comp_level).toBeUndefined();
	});

	it('refuses a second save while one is in flight', async () => {
		let calls = 0;
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async () => {
					calls += 1;
					await new Promise((r) => setTimeout(r, 5));
				}
			})
		);
		await s.load();
		await Promise.all([s.save(), s.save()]);
		expect(calls).toBe(1);
	});

	it('plans after saving so the diff reflects what was stored', async () => {
		// Planning from the form's local values instead would show a diff for
		// something that failed to save.
		const order: string[] = [];
		const s = new WebSettingsStore({
			webServerSettings: async () => dto(),
			saveWebServerSettings: async () => {
				order.push('save');
			},
			planConfigApply: async () => {
				order.push('plan');
				return emptyPlan;
			}
		});
		await s.load();
		await s.save();
		expect(order).toEqual(['save', 'plan']);
	});

	it('does not plan when the save was rejected', async () => {
		let planned = 0;
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async () => {
					throw { kind: 'validation', field: 'gzip_types', message: 'bad token' };
				},
				planConfigApply: async () => {
					planned += 1;
					return emptyPlan;
				}
			})
		);
		await s.load();
		expect(await s.save()).toBe(false);
		expect(planned).toBe(0);
	});

	it('re-reads the stored values, because the backend normalises them', async () => {
		// `gzip_types` is lowercased and re-joined on single spaces when it is
		// parsed, so what comes back is not byte-identical to what was typed.
		// Without the re-read the form would look permanently dirty and the user
		// would keep saving a value that was already stored.
		let saved = false;
		const s = new WebSettingsStore(
			api({
				webServerSettings: async () =>
					dto({ gzipTypes: saved ? 'text/html text/css' : 'text/plain text/css' }),
				saveWebServerSettings: async () => {
					saved = true;
				}
			})
		);
		await s.load();
		s.setText('gzipTypes', '  TEXT/HTML   TEXT/CSS  ');
		expect(await s.save()).toBe(true);
		expect(s.values?.gzipTypes).toBe('text/html text/css');
		expect(s.dirty).toBe(false);
	});

	it('says so when the values could not be read back, without claiming the save failed', async () => {
		let reads = 0;
		const s = new WebSettingsStore(
			api({
				webServerSettings: async () => {
					reads += 1;
					if (reads > 1) throw { kind: 'core', message: 'state.db went away' };
					return dto();
				}
			})
		);
		await s.load();
		s.setNumber('keepaliveTimeout', '30');
		// The save itself succeeded, so the diff is real and the caller should
		// still see it — the failed re-read is a caveat, not a failure.
		expect(await s.save()).toBe(true);
		expect(s.error).toContain('read back');
		expect(s.values?.keepaliveTimeout).toBe(30);
		expect(s.dirty).toBe(false);
	});

	it('refuses to save before anything has loaded', async () => {
		let calls = 0;
		const s = new WebSettingsStore(api({ saveWebServerSettings: async () => void calls++ }));
		expect(await s.save()).toBe(false);
		expect(calls).toBe(0);
	});

	it('lowers the in-flight flag even when the save throws', async () => {
		const s = new WebSettingsStore(
			api({
				saveWebServerSettings: async () => {
					throw { kind: 'core', message: 'boom' };
				}
			})
		);
		await s.load();
		await s.save();
		expect(s.saving).toBe(false);
		expect(s.canSave).toBe(true);
	});
});

/**
 * Typing DURING a save — the window between pressing Save and the diff opening.
 *
 * The Save button is disabled while a save is in flight, but the inputs are not,
 * so this window is reachable by anyone who keeps editing after clicking Save.
 * It is a real window too: the save is one IPC round trip, the re-read is a
 * second, and the plan that follows regenerates the whole config set.
 *
 * The rule these tests pin: a field the user touched during the window keeps
 * what they typed and leaves the form DIRTY; every other field still adopts the
 * stored (normalised) value.
 */
describe('editing while a save is in flight', () => {
	/** A promise the test resolves by hand, so a round trip can be held open
	 *  while the test types into the form. */
	function gate(): { held: Promise<void>; release: () => void } {
		let release = (): void => {};
		const held = new Promise<void>((resolve) => {
			release = () => resolve();
		});
		return { held, release };
	}

	/** What `GzipTypes::parse` does to the value on the way in — the reason the
	 *  re-read exists at all. */
	function normalise(types: string): string {
		return types.trim().toLowerCase().split(/\s+/).filter(Boolean).join(' ');
	}

	/**
	 * A backend that actually KEEPS what it is given (normalising `gzip_types`
	 * as the Rust parser does) and holds the save open until the test releases
	 * it. A mock that discards the write would let a broken merge look right:
	 * the re-read would return the original row either way.
	 */
	function server(): {
		api: WebSettingsApi;
		saveGate: ReturnType<typeof gate>;
		stored: () => WebServerSettingsDto;
	} {
		let stored = dto();
		const saveGate = gate();
		return {
			api: {
				webServerSettings: async () => ({ ...stored }),
				saveWebServerSettings: async (input) => {
					await saveGate.held;
					stored = { ...input, gzipTypes: normalise(input.gzipTypes) };
				},
				planConfigApply: async () => emptyPlan
			},
			saveGate,
			stored: () => stored
		};
	}

	it('does not lose an edit typed while the save was in flight', async () => {
		// The reviewer's scenario, exactly: edit A, press Save, edit B before the
		// round trip lands. Before the merge the re-read replaced `values`
		// wholesale, B snapped back to the stored 65, `dirty` read false, and the
		// form presented itself as saved and clean with no error at all.
		const { api: fake, saveGate } = server();
		const s = new WebSettingsStore(fake);
		await s.load();

		s.setNumber('fastcgiReadTimeout', '900');
		const inFlight = s.save();
		expect(s.saving).toBe(true);
		s.setNumber('keepaliveTimeout', '30');

		saveGate.release();
		expect(await inFlight).toBe(true);

		expect(s.values?.keepaliveTimeout).toBe(30);
		// And the form says so, rather than claiming to be saved: 30 is on screen
		// but 65 is what is stored.
		expect(s.dirty).toBe(true);
		expect(s.canSave).toBe(true);
	});

	it('still stores what was on the form when Save was pressed', async () => {
		// The other half of the same rule: the mid-flight edit is NOT smuggled
		// into a save the user never asked for.
		const { api: fake, saveGate, stored } = server();
		const s = new WebSettingsStore(fake);
		await s.load();

		s.setNumber('fastcgiReadTimeout', '900');
		const inFlight = s.save();
		s.setNumber('keepaliveTimeout', '30');
		saveGate.release();
		await inFlight;

		expect(stored().fastcgiReadTimeout).toBe(900);
		expect(stored().keepaliveTimeout).toBe(65);
	});

	it('keeps the local edit and reports the form dirty when the stored answer disagrees', async () => {
		// The conflict case, on the one field the backend rewrites. The user's
		// keystrokes win the FORM (they are newer, and they are still on screen to
		// correct), storage wins the baseline — so the disagreement surfaces as
		// "unsaved changes" instead of one side being silently dropped.
		const { api: fake, saveGate } = server();
		const s = new WebSettingsStore(fake);
		await s.load();

		s.setText('gzipTypes', 'TEXT/HTML  TEXT/CSS');
		const inFlight = s.save();
		s.setText('gzipTypes', 'TEXT/HTML TEXT/CSS APPLICATION/JSON');
		saveGate.release();
		await inFlight;

		expect(s.values?.gzipTypes).toBe('TEXT/HTML TEXT/CSS APPLICATION/JSON');
		expect(s.dirty).toBe(true);
		// Not an error: nothing failed, and a red banner here would send the user
		// looking for a problem that does not exist.
		expect(s.error).toBe('');
	});

	it('still adopts the normalised value for fields the user did NOT touch', async () => {
		// Guards the over-broad fix — "a pending edit anywhere, so skip the
		// re-read" — which would leave the form permanently dirty on a gzip_types
		// value that is already stored, exactly what the re-read exists to avoid.
		const { api: fake, saveGate } = server();
		const s = new WebSettingsStore(fake);
		await s.load();

		s.setText('gzipTypes', '  TEXT/HTML   TEXT/CSS  ');
		const inFlight = s.save();
		s.setNumber('keepaliveTimeout', '30');
		saveGate.release();
		await inFlight;

		expect(s.values?.gzipTypes).toBe('text/html text/css');
		expect(s.values?.keepaliveTimeout).toBe(30);
	});

	it('holds for the whole window, including the plan that follows the re-read', async () => {
		// The window does not end when the write lands: `planConfigApply`
		// regenerates the config set, and the form is live for all of it.
		const { api: fake, saveGate } = server();
		const planHeld = gate();
		const planStarted = gate();
		const s = new WebSettingsStore({
			...fake,
			planConfigApply: async () => {
				planStarted.release();
				await planHeld.held;
				return emptyPlan;
			}
		});
		await s.load();

		s.setNumber('fastcgiReadTimeout', '900');
		const inFlight = s.save();
		saveGate.release();
		// The save and the re-read have both settled by the time the plan starts;
		// this types into a form that still looks live to the user.
		await planStarted.held;
		s.setNumber('workerConnections', '2048');
		planHeld.release();
		await inFlight;

		expect(s.values?.workerConnections).toBe(2048);
		expect(s.dirty).toBe(true);
	});

	it('re-reads normally on the next save, once the window has closed', async () => {
		// A pending-edit log that outlived its window would make the FOLLOWING
		// save ignore the stored answer for those fields — the re-read's bug in
		// mirror image.
		const { api: fake, saveGate } = server();
		const s = new WebSettingsStore(fake);
		await s.load();

		const inFlight = s.save();
		s.setText('gzipTypes', 'TEXT/HTML');
		saveGate.release();
		await inFlight;
		expect(s.values?.gzipTypes).toBe('TEXT/HTML');

		// Second save: nothing is typed during it, so the normalised value must
		// come back and the form must go clean.
		expect(await s.save()).toBe(true);
		expect(s.values?.gzipTypes).toBe('text/html');
		expect(s.dirty).toBe(false);
	});
});
