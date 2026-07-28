// SPDX-License-Identifier: GPL-3.0-or-later
//
// Re-reads the web-server list after a successful apply.
//
// `ApplyStore.run()` already re-plans on success (see its own doc comment),
// but a re-plan is not a re-read of `list_web_servers` — the row's
// `configExists` (see `webservers.derive.ts`'s `startStopFor`) is a
// SEPARATE field from anything `plan_config_apply` returns. Without this,
// a fresh install's first Apply writes `nginx.conf` to disk, closes the
// dialog, and the Start button still reads the pre-apply `configExists:
// false` and still shows "No config generated yet — apply your changes
// first" on the very row that just got one — the exact dead end
// docs/superpowers/specs/2026-07-28-p1-webserver-start-stop-design.md §4's
// guard exists to prevent. It only healed on a full navigation away and
// back, because that is the only other place `store.load()` ran.
//
// A FAILED apply must NOT reload: `run()` already leaves `applyStore.error`
// set and resolves `false` for that case (see its own doc comment), and
// reloading anyway would spend a round trip on a list that did not change
// and risk overwriting a genuine `store.error` with a quiet success.
export async function reloadAfterApply(
	run: () => Promise<boolean>,
	reload: () => Promise<void>
): Promise<boolean> {
	const applied = await run();
	if (applied) await reload();
	return applied;
}
