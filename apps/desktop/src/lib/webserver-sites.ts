// SPDX-License-Identifier: GPL-3.0-or-later
//
// Reads the sites list for the Web server page's 502 warning (`stoppedPoolsFor`
// in `webservers.derive.ts`), and — unlike a bare `.catch(() => {})` — routes a
// failed read onto a channel the page already renders instead of swallowing it.
//
// The empty-list fallback on failure is still right: this page has no site
// list to render, only one warning line derived from it, and a missing hint
// is a smaller harm than blanking the page (see the call site's own doc
// comment). What was wrong is that the fallback was INDISTINGUISHABLE from a
// genuinely empty result — a real 502 (an enabled site's pool down) produced
// no warning and no sign anything had gone wrong.
//
// `onFail` is the caller's channel, not this module's — a failed sites read
// is not a failure of `list_web_servers` (this page's OWN page-level error,
// rendered via `store.error`/`ErrorBanner`), so this must not be hardcoded to
// that banner and misreport it.
import type { IpcError, SiteDto } from './ipc';

export async function loadSitesOrFail(
	listSites: () => Promise<SiteDto[]>,
	onFail: (error: IpcError) => void
): Promise<SiteDto[]> {
	try {
		return await listSites();
	} catch (e) {
		onFail(e as IpcError);
		return [];
	}
}
