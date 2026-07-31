// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import {
	composeDomain,
	defaultPhpVersion,
	docrootRisk,
	docrootWarningText,
	enabledPill,
	findMissingRuntimeSite,
	phpVersionMissing,
	phpVersionOptions,
	scaffoldNotice,
	scaffoldPreview,
	splitDomain,
	type DocrootRisk
} from './sites.derive';
import type { ScaffoldOutcomeDto, SiteDto } from './ipc';

const site = (overrides: Partial<SiteDto> = {}): SiteDto => ({
	id: 'a1',
	name: 'hello',
	domain: 'hello.localhost',
	docroot: '/srv/www/hello',
	webServer: 'nginx',
	phpVersion: '8.4',
	enabled: true,
	createdAt: 1,
	updatedAt: 1,
	...overrides
});

describe('composeDomain / splitDomain', () => {
	it('composes a subdomain onto .localhost', () => {
		expect(composeDomain('myshop')).toBe('myshop.localhost');
	});
	it('round-trips a composed domain', () => {
		expect(splitDomain(composeDomain('blog'))).toBe('blog');
	});
	it('strips exactly one trailing .localhost', () => {
		expect(splitDomain('a.localhost.localhost')).toBe('a.localhost');
	});
	it('returns a non-suffixed domain unchanged', () => {
		expect(splitDomain('example.test')).toBe('example.test');
	});
});

describe('enabledPill', () => {
	it('maps enabled/disabled to label + pill class', () => {
		expect(enabledPill(true)).toEqual({ label: 'enabled', cls: 'pill-running' });
		expect(enabledPill(false)).toEqual({ label: 'disabled', cls: 'pill-stopped' });
	});
});

describe('phpVersionOptions', () => {
	it('offers the versions actually installed', () => {
		const opts = phpVersionOptions(undefined, ['8.1', '8.3']);
		expect(opts.map((o) => o.value)).toEqual(['8.1', '8.3']);
	});

	it('keeps the stored version selectable when it is not installed', () => {
		// Dropping it would make the <select> render blank and silently rewrite
		// the site's PHP version to something the user never chose.
		const opts = phpVersionOptions('7.4', ['8.3']);
		expect(opts[0].value).toBe('7.4');
		expect(opts[0].label).toMatch(/not available|not installed/i);
	});

	it('does not duplicate the stored version when it is installed', () => {
		const opts = phpVersionOptions('8.3', ['8.1', '8.3']);
		expect(opts.filter((o) => o.value === '8.3')).toHaveLength(1);
	});

	it('still offers something when nothing is installed', () => {
		// An empty <select> would leave the user unable to save at all.
		const opts = phpVersionOptions('8.3', []);
		expect(opts.length).toBeGreaterThan(0);
		expect(opts[0].value).toBe('8.3');
	});

	it('adds nothing when there is no stored version and nothing is installed (a doomed Add form)', () => {
		expect(phpVersionOptions(undefined, [])).toEqual([]);
		expect(phpVersionOptions('', [])).toEqual([]);
	});
});

describe('defaultPhpVersion', () => {
	it('defaults a new site to the newest installed version', () => {
		// A site that is broken before the user has touched anything is the
		// second of the three mistakes in spec §5.0.
		expect(defaultPhpVersion(['8.1', '8.3', '8.5'])).toBe('8.5');
	});

	it('has no default to offer when nothing is installed', () => {
		expect(defaultPhpVersion([])).toBeUndefined();
	});

	it('compares major.minor numerically, not lexically', () => {
		// "8.9" > "8.10" as strings, but 8.10 is the newer release.
		expect(defaultPhpVersion(['8.9', '8.10'])).toBe('8.10');
	});
});

describe('phpVersionMissing', () => {
	// Task 8 stops a NEW site from choosing a version this machine lacks, but the
	// machine can change under an EXISTING one (`brew uninstall php@8.3`) at any
	// time, so this has to warn independent of whether Apply has ever run.
	it('is true when the stored version is not installed', () => {
		expect(phpVersionMissing(site({ phpVersion: '8.4' }), ['8.5'])).toBe(true);
	});

	it('is false when the stored version is installed', () => {
		expect(phpVersionMissing(site({ phpVersion: '8.5' }), ['8.5'])).toBe(false);
	});

	// I2 (branch-review-fix-report.md): `null` means the environment is UNKNOWN
	// (still loading, or the read failed) — a distinct fact from "known and
	// empty" (`[]`), which the caller used to collapse into the same `[]` via
	// `phpEnv?.runtimes ?? []`. This must return `false` (no badge) for `null`
	// even though the SAME site would be flagged against an empty-but-KNOWN
	// list — otherwise "unknown" would just be a slower way of saying "missing".
	it('is false (no badge) when the environment is unknown, unlike a known-empty one', () => {
		expect(phpVersionMissing(site({ phpVersion: '8.4' }), null)).toBe(false);
		expect(phpVersionMissing(site({ phpVersion: '8.4' }), [])).toBe(true);
	});
});

describe('findMissingRuntimeSite', () => {
	// Mirrors `render_set`'s `MissingRuntime` pre-check in
	// crates/openvhost-core/src/site/apply/mod.rs: enabled + nginx (`is_servable`),
	// first offender in list order. A disabled site's stale version is not a
	// reason Apply would fail, so it must not gate the banner's actions.
	it('finds the first enabled, nginx-served site missing its PHP version', () => {
		const found = findMissingRuntimeSite(
			[
				site({ id: 'a1', name: 'shop', phpVersion: '8.5' }),
				site({ id: 'a2', name: 'hello', phpVersion: '8.4' })
			],
			['8.5']
		);
		expect(found?.name).toBe('hello');
	});

	it('ignores a disabled site even if its version is missing', () => {
		const found = findMissingRuntimeSite([site({ enabled: false, phpVersion: '8.4' })], ['8.5']);
		expect(found).toBeNull();
	});

	it('is null when every servable site has an installed version', () => {
		expect(findMissingRuntimeSite([site({ phpVersion: '8.5' })], ['8.5'])).toBeNull();
	});
});

describe('scaffoldNotice', () => {
	// Exhaustive over ScaffoldOutcomeDto's three variants (spec D7) — one test per
	// variant, each asserting the tone/role pairing the banner's `role`/`data-tone`
	// attributes render from, plus the substrings the copy must contain.
	it('created: ok/status, names the starter-page path', () => {
		const outcome: ScaffoldOutcomeDto = { kind: 'created' };
		const notice = scaffoldNotice('hello', '/srv/www/hello', outcome);
		expect(notice.tone).toBe('ok');
		expect(notice.role).toBe('status');
		expect(notice.text).toContain('/srv/www/hello/index.html');
	});

	it('keptExisting: ok/status, names the file it kept', () => {
		const outcome: ScaffoldOutcomeDto = { kind: 'keptExisting', existing: 'index.php' };
		const notice = scaffoldNotice('hello', '/srv/www/hello', outcome);
		expect(notice.tone).toBe('ok');
		expect(notice.role).toBe('status');
		expect(notice.text).toContain('index.php');
		expect(notice.text).toContain('/srv/www/hello');
	});

	it('failed: warn/alert (NOT the fail-red tone), names the site and the reason', () => {
		const outcome: ScaffoldOutcomeDto = {
			kind: 'failed',
			step: 'createDir',
			reason: 'Permission denied (os error 13)'
		};
		const notice = scaffoldNotice('hello', '/srv/www/hello', outcome);
		expect(notice.tone).toBe('warn');
		expect(notice.role).toBe('alert');
		expect(notice.text).toContain('hello');
		expect(notice.text).toContain('Permission denied (os error 13)');
	});
});

describe('docrootRisk', () => {
	// The exact incident: ~/Downloads picked as the Project folder, checkbox left
	// off. Every tier from spec D1, plus the near-miss cases the task calls out by
	// name so a shape check that is one path segment too greedy (or too strict)
	// gets caught immediately.
	it('flags a well-known personal folder directly under home', () => {
		expect(docrootRisk('/Users/tom/Downloads')).toEqual({
			kind: 'personalFolder',
			folder: 'Downloads'
		});
	});

	it('flags a personal folder with a trailing slash the same way', () => {
		expect(docrootRisk('/Users/tom/Downloads/')).toEqual({
			kind: 'personalFolder',
			folder: 'Downloads'
		});
	});

	it('does NOT flag a real project folder one level inside Downloads', () => {
		// The near-miss this whole feature must not false-positive on: a site
		// legitimately rooted at a subfolder of Downloads is not the incident.
		expect(docrootRisk('/Users/tom/Downloads/my-site')).toBeNull();
	});

	it('flags every well-known personal folder, not only Downloads', () => {
		const folders = [
			'Downloads',
			'Desktop',
			'Documents',
			'Movies',
			'Music',
			'Pictures',
			'Public',
			'Library'
		];
		for (const folder of folders) {
			expect(docrootRisk(`/Users/tom/${folder}`)).toEqual({ kind: 'personalFolder', folder });
		}
	});

	it('does not flag a folder whose name merely resembles a well-known one', () => {
		// Genuine near-misses — different real folders, not the same one under a
		// different case (see the case-insensitivity block below for that).
		expect(docrootRisk('/Users/tom/Downloader')).toBeNull();
		expect(docrootRisk('/Users/tom/Downloads2')).toBeNull();
	});

	it('flags the home directory itself', () => {
		expect(docrootRisk('/Users/tom')).toEqual({ kind: 'homeItself' });
	});

	it('does NOT flag a real project folder directly under home', () => {
		expect(docrootRisk('/Users/tom/Projects')).toBeNull();
	});

	it('flags home with a trailing slash the same way', () => {
		expect(docrootRisk('/Users/tom/')).toEqual({ kind: 'homeItself' });
	});

	it('flags every listed system/shared root', () => {
		const roots = [
			'/',
			'/Users',
			'/Applications',
			'/System',
			'/Library',
			'/Volumes',
			'/tmp',
			'/private',
			'/etc',
			'/usr',
			'/var'
		];
		for (const root of roots) {
			expect(docrootRisk(root)).toEqual({ kind: 'systemRoot', root });
		}
	});

	it('flags a system root with a trailing slash the same way', () => {
		expect(docrootRisk('/etc/')).toEqual({ kind: 'systemRoot', root: '/etc' });
	});

	it('does not flag a subfolder of a system root', () => {
		// D1 lists these as specific paths, not prefixes — /etc/nginx is not a
		// folder a user would ever pick as a project folder.
		expect(docrootRisk('/etc/nginx')).toBeNull();
	});

	it('does not flag an ordinary project path', () => {
		expect(docrootRisk('/Users/tom/Sites/my-app')).toBeNull();
		expect(docrootRisk('/srv/www/hello')).toBeNull();
	});

	it('returns null for a blank or whitespace-only path, matching scaffoldPreview', () => {
		expect(docrootRisk('')).toBeNull();
		expect(docrootRisk('   ')).toBeNull();
	});

	describe('case-insensitivity (default APFS is case-insensitive but case-preserving)', () => {
		// A reviewer reconstructed and RAN the pre-fix classifier: on the default
		// macOS volume format, `/Users/tom/downloads` and `/Users/tom/DOWNLOADS`
		// are not lookalikes, they are the exact same real folder as
		// `/Users/tom/Downloads` — the exact incident's folder, just retyped in a
		// different case. A case-sensitive comparison silently missed both.
		it('flags a personal folder regardless of case, preserving the AS-TYPED case for display', () => {
			expect(docrootRisk('/Users/tom/downloads')).toEqual({
				kind: 'personalFolder',
				folder: 'downloads'
			});
			expect(docrootRisk('/Users/tom/DOWNLOADS')).toEqual({
				kind: 'personalFolder',
				folder: 'DOWNLOADS'
			});
			expect(docrootRisk('/Users/tom/DoWnLoAdS')).toEqual({
				kind: 'personalFolder',
				folder: 'DoWnLoAdS'
			});
		});

		it('flags a system root regardless of case, preserving the AS-TYPED case for display', () => {
			expect(docrootRisk('/ETC')).toEqual({ kind: 'systemRoot', root: '/ETC' });
			expect(docrootRisk('/Etc')).toEqual({ kind: 'systemRoot', root: '/Etc' });
		});

		it('flags home itself and a personal folder even when "Users" is typed lowercase', () => {
			// Same case-insensitive-volume argument, one segment earlier in the path.
			expect(docrootRisk('/users/tom')).toEqual({ kind: 'homeItself' });
			expect(docrootRisk('/USERS/tom/Downloads')).toEqual({
				kind: 'personalFolder',
				folder: 'Downloads'
			});
		});

		it('still does not flag a genuine near-miss once case is folded', () => {
			// Folding case must not make the match GREEDIER than the real folder
			// set — only members of PERSONAL_FOLDERS/SYSTEM_ROOTS, case aside.
			expect(docrootRisk('/Users/tom/DOWNLOADER')).toBeNull();
			expect(docrootRisk('/ETCETERA')).toBeNull();
		});
	});

	describe('separator collapsing', () => {
		// Only reachable by hand-typing/pasting into the freely-editable field
		// (Browse always returns a clean OS path) — lower stakes than the
		// case-insensitivity gap above, but the field IS freely editable.
		it('collapses a doubled internal separator before matching', () => {
			expect(docrootRisk('/Users/tom//Downloads')).toEqual({
				kind: 'personalFolder',
				folder: 'Downloads'
			});
			expect(docrootRisk('/Users//tom/Downloads')).toEqual({
				kind: 'personalFolder',
				folder: 'Downloads'
			});
		});

		// The two cases above are ALSO covered, independently, by the segment
		// split-and-filter `homeItself`/`personalFolder` matching does regardless
		// of whether `normalizeDocrootPath` collapses anything (an empty segment
		// from a doubled `/` is dropped by the filter either way) — real,
		// correct behaviour, but not a proof that the collapsing step itself is
		// load-bearing. `systemRoot` compares the whole normalized string as one
		// unit instead of segment by segment, so a doubled separator immediately
		// before a single-segment root is the one case that genuinely has no
		// fallback — this is the assertion that actually goes red without the
		// collapsing step (verified by temporarily reverting it).
		it('collapses a doubled separator immediately before a system root', () => {
			expect(docrootRisk('//etc')).toEqual({ kind: 'systemRoot', root: '/etc' });
		});

		it('collapses a doubled trailing separator the same way as a single one', () => {
			expect(docrootRisk('/Users/tom/Downloads//')).toEqual({
				kind: 'personalFolder',
				folder: 'Downloads'
			});
		});
	});

	describe('a leading ~ is deliberately not expanded', () => {
		// Docroot::parse requires an absolute path, so a `~`-prefixed docroot
		// already fails validation at Save regardless of this classifier — see
		// the doc comment on docrootRisk for the full reasoning. Pinned here so
		// that reasoning is a checked claim, not just a comment: if a future
		// change to the ingress validator ever allowed `~` through unexpanded,
		// this pairing would need to be revisited.
		it('does not flag an unexpanded ~-relative path', () => {
			expect(docrootRisk('~/Downloads')).toBeNull();
			expect(docrootRisk('~')).toBeNull();
		});
	});
});

describe('docrootWarningText', () => {
	// Exhaustive over DocrootRisk's three variants (compile-time enforced by the
	// helper's own never-typed default arm) — one assertion per variant proving
	// the consequence copy and the fix copy, crossed with both modes to prove the
	// fix text genuinely differs (create points at the checkbox, edit does not).
	it('names the personal folder, states the consequence, and offers the checkbox fix in create mode', () => {
		const risk: DocrootRisk = { kind: 'personalFolder', folder: 'Downloads' };
		const text = docrootWarningText(risk, 'create');
		expect(text).toContain('Downloads');
		expect(text).toContain("reachable at this site's domain");
		expect(text).toContain('.php');
		expect(text).toContain('Create a site folder inside this folder');
	});

	it('offers the subfolder fix in edit mode instead of the checkbox', () => {
		const risk: DocrootRisk = { kind: 'personalFolder', folder: 'Downloads' };
		const text = docrootWarningText(risk, 'edit');
		expect(text).toContain('subfolder');
		expect(text).not.toContain('Create a site folder inside this folder');
	});

	it('names "home folder" for the homeItself tier', () => {
		const text = docrootWarningText({ kind: 'homeItself' }, 'create');
		expect(text).toContain('home folder');
	});

	it('names the actual root path for the systemRoot tier', () => {
		const text = docrootWarningText({ kind: 'systemRoot', root: '/etc' }, 'edit');
		expect(text).toContain('/etc');
	});
});

describe('scaffoldPreview', () => {
	it('joins parent and name', () =>
		expect(scaffoldPreview('/Users/x/Downloads', 'my-site')).toBe('/Users/x/Downloads/my-site'));
	it('normalizes trailing slashes', () =>
		expect(scaffoldPreview('/Users/x/Downloads//', 'my-site')).toBe('/Users/x/Downloads/my-site'));
	it('handles the root parent', () => expect(scaffoldPreview('/', 'a')).toBe('/a'));
	it('returns null while name is empty', () => expect(scaffoldPreview('/x', '')).toBeNull());
	it('returns null while parent is blank', () => expect(scaffoldPreview('  ', 'a')).toBeNull());
});
