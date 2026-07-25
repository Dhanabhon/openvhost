# Phase 1 · Sites CRUD Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Sites CRUD — four typed IPC commands over the existing `SiteRepository`, plus the Sites panel and editor drawer from the mockups — reading and writing the real `state.db`.

**Architecture:** DTOs at the IPC boundary (`SiteDto` out, `SiteInput` in) whose inbound conversion runs every field through the domain newtypes' `parse`, so IPC input cannot bypass validation; commands build a `SqliteSiteRepository` per call from the managed `Db`. The frontend adds typed wrappers, a `SitesStore`, a `/sites` route, and a focus-trapped editor drawer with a native folder picker. Store-only — no config generation or reload.

**Tech Stack:** Rust 2024 + Tauri 2 + tauri-specta (typed bindings), `tauri-plugin-dialog` (folder picker), SvelteKit + Svelte 5 runes + Tailwind 4, vitest.

**Spec:** `docs/superpowers/specs/2026-07-25-p1-sites-crud-design.md`

## Global Constraints

- Branch `feat/p1-sites-crud` off `main`.
- SPDX line 1 of every NEW file: `.rs`/`.ts` → `// SPDX-License-Identifier: GPL-3.0-or-later`; `.svelte` → `<!-- SPDX-License-Identifier: GPL-3.0-or-later -->`. Do NOT hand-edit the generated `apps/desktop/src/lib/ipc/bindings.ts` (regenerate it).
- **`crates/openvhost-core` is NOT modified by this slice** — the DTOs live in the app's `commands.rs`; core stays tauri-free.
- **Boundary rule (the security core):** every inbound IPC string field becomes a domain type via its `parse` (`SiteName`/`Domain`/`Docroot`/`PhpVersion`/`WebServer`/`SiteId`). No `state.db` write may originate from an unvalidated string.
- **Server-owned identity:** `id`, `created_at`, `updated_at` are never taken from the client. `update_site` re-reads the row and reuses the stored `id`/`created_at`.
- No `unwrap()`/`expect()` outside `#[cfg(test)]`. Svelte 5 runes; TypeScript strict; no `console.log`; typed bindings only — no stringly `invoke("…")`.
- New deps: `tauri-plugin-dialog` (Rust) + `@tauri-apps/plugin-dialog` (npm), both MIT/Apache — `cargo deny check licenses advisories` must pass. The dialog capability grant must be **minimal** (folder-open only; no blanket filesystem access).
- **Security-auditor APPROVE is a MERGE GATE** for this slice (IPC command surface + new capability grant). The controller dispatches it; implementers just must not weaken the boundary conversion or widen the capability.
- DCO `git commit -s`, NO `Co-Authored-By`, Conventional Commits.
- Gate each task (offline — `.sqlx/` is committed, no `DATABASE_URL` needed): `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh` and, for frontend tasks, `pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build`. **Read `pnpm lint` output from a raw pipe** (`pnpm -C apps/desktop lint 2>&1 | tail -40`) — the command summarizer has silently dropped a crash before.

---

### Task 1: IPC boundary — DTOs, validation mapping, four commands

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs` (add `IpcError::Validation`, `SiteDto`, `SiteInput`, conversions, 4 commands, tests)
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register the 4 commands in `collect_commands!`)
- Regenerate + commit: `apps/desktop/src/lib/ipc/bindings.ts`

**Interfaces produced:**
- `SiteDto { id, name, domain, docroot, webServer, phpVersion: string, enabled: boolean, createdAt, updatedAt: number }` (camelCase over the wire).
- `SiteInput { name, domain, docroot, webServer, phpVersion: string, enabled: boolean }`.
- `IpcError::Validation { field: String, message: String }` → TS `{ kind: 'validation', field, message }`.
- Commands `list_sites`, `create_site`, `update_site`, `delete_site` → TS `commands.listSites()`, `createSite(input)`, `updateSite(id, input)`, `deleteSite(id)`.

- [ ] **Step 1: Branch**

```bash
git checkout main && git pull --ff-only && git checkout -b feat/p1-sites-crud
```

- [ ] **Step 2: Write the failing conversion tests**

Append to `apps/desktop/src-tauri/src/commands.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod site_ipc_tests {
    use super::*;

    fn valid_input() -> SiteInput {
        SiteInput {
            name: "myshop".into(),
            domain: "myshop.localhost".into(),
            docroot: "/srv/www/myshop".into(),
            web_server: "nginx".into(),
            php_version: "8.3".into(),
            enabled: true,
        }
    }

    #[test]
    fn valid_input_converts_to_newsite() {
        let new: NewSite = valid_input().try_into().unwrap();
        assert_eq!(new.name.as_str(), "myshop");
        assert_eq!(new.domain.as_str(), "myshop.localhost");
        assert_eq!(new.docroot.as_str(), "/srv/www/myshop");
        assert_eq!(new.web_server.as_str(), "nginx");
        assert_eq!(new.php_version.as_str(), "8.3");
        assert!(new.enabled);
    }

    /// Every hostile field must be rejected AND name the offending field, so
    /// the form can mark the right input. This is the IPC ingress guard.
    #[test]
    fn hostile_input_is_rejected_with_the_right_field() {
        let cases: &[(&str, SiteInput)] = &[
            ("name", SiteInput { name: "bad name".into(), ..valid_input() }),
            ("name", SiteInput { name: "quote\"".into(), ..valid_input() }),
            ("domain", SiteInput { domain: "evil\";inject".into(), ..valid_input() }),
            ("domain", SiteInput { domain: "has space.localhost".into(), ..valid_input() }),
            ("docroot", SiteInput { docroot: "relative/path".into(), ..valid_input() }),
            ("docroot", SiteInput { docroot: "/has\"quote".into(), ..valid_input() }),
            ("php_version", SiteInput { php_version: "8.x".into(), ..valid_input() }),
            ("web_server", SiteInput { web_server: "caddy".into(), ..valid_input() }),
        ];
        for (field, input) in cases {
            let err = NewSite::try_from(input.clone()).unwrap_err();
            match err {
                IpcError::Validation { field: f, .. } => {
                    assert_eq!(&f, field, "wrong field for {input:?}");
                }
                other => panic!("expected Validation for {input:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn dto_round_trips_a_site() {
        let new: NewSite = valid_input().try_into().unwrap();
        let site = Site {
            id: SiteId::new(),
            name: new.name,
            domain: new.domain,
            docroot: new.docroot,
            web_server: new.web_server,
            php_version: new.php_version,
            enabled: new.enabled,
            created_at: 111,
            updated_at: 222,
        };
        let dto = SiteDto::from(site.clone());
        assert_eq!(dto.id, site.id.as_str());
        assert_eq!(dto.name, "myshop");
        assert_eq!(dto.web_server, "nginx");
        assert_eq!(dto.created_at, 111);
        assert_eq!(dto.updated_at, 222);
        assert!(dto.enabled);
    }

    #[test]
    fn core_validation_error_maps_to_ipc_validation() {
        let core = openvhost_core::CoreError::Validation {
            field: "domain",
            reason: "bad".into(),
        };
        match IpcError::from(core) {
            IpcError::Validation { field, message } => {
                assert_eq!(field, "domain");
                assert_eq!(message, "bad");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-desktop site_ipc 2>&1 | tail -5`
Expected: compile errors — `SiteInput`, `SiteDto`, `IpcError::Validation` undefined.

- [ ] **Step 4: Add the `Validation` variant and remap `From<CoreError>`**

In `apps/desktop/src-tauri/src/commands.rs`, add to `enum IpcError` (after the `Proc` variant):

```rust
    /// A domain value failed validation; `field` names the offending input so
    /// the UI can mark it instead of showing a generic banner.
    #[error("{message}")]
    Validation { field: String, message: String },
```

Replace the existing `From<openvhost_core::CoreError> for IpcError` body with:

```rust
impl From<openvhost_core::CoreError> for IpcError {
    fn from(e: openvhost_core::CoreError) -> Self {
        match e {
            openvhost_core::CoreError::Validation { field, reason } => IpcError::Validation {
                field: field.to_string(),
                message: reason,
            },
            other => IpcError::Core {
                message: other.to_string(),
            },
        }
    }
}
```

- [ ] **Step 5: Add the DTOs and conversions**

Append to `apps/desktop/src-tauri/src/commands.rs` (imports first — add to the existing `use openvhost_core::…` line or a new one):

```rust
use openvhost_core::{
    Db, Docroot, Domain, NewSite, PhpVersion, Site, SiteId, SiteName, SiteRepository,
    SqliteSiteRepository, WebServer,
};

/// A site as it crosses IPC. `Site`'s fields are opaque validated newtypes
/// (deliberately not serializable), so the wire form is plain strings.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SiteDto {
    pub id: String,
    pub name: String,
    pub domain: String,
    pub docroot: String,
    pub web_server: String,
    pub php_version: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Site> for SiteDto {
    fn from(s: Site) -> Self {
        SiteDto {
            id: s.id.as_str().to_string(),
            name: s.name.as_str().to_string(),
            domain: s.domain.as_str().to_string(),
            docroot: s.docroot.as_str().to_string(),
            web_server: s.web_server.as_str().to_string(),
            php_version: s.php_version.as_str().to_string(),
            enabled: s.enabled,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

/// Client-supplied site fields. Note there is no `id`/`created_at`/
/// `updated_at`: those are server-owned and never taken from the client.
#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SiteInput {
    pub name: String,
    pub domain: String,
    pub docroot: String,
    pub web_server: String,
    pub php_version: String,
    pub enabled: bool,
}

impl TryFrom<SiteInput> for NewSite {
    type Error = IpcError;

    /// THE IPC INGRESS GUARD: every field goes through its domain newtype's
    /// `parse`, so no unvalidated string can reach `state.db`. `?` maps
    /// `CoreError::Validation` to `IpcError::Validation { field, .. }`.
    fn try_from(i: SiteInput) -> Result<NewSite, IpcError> {
        Ok(NewSite {
            name: SiteName::parse(&i.name)?,
            domain: Domain::parse(&i.domain)?,
            docroot: Docroot::parse(&i.docroot)?,
            web_server: WebServer::parse(&i.web_server)?,
            php_version: PhpVersion::parse(&i.php_version)?,
            enabled: i.enabled,
        })
    }
}
```

- [ ] **Step 6: Add the four commands**

Append to `apps/desktop/src-tauri/src/commands.rs`:

```rust
// These commands build a repository per call from the managed `Db` (cheap —
// cloning a pool handle) rather than managing a second type. If `state.db`
// failed to open at startup, `Db` is not managed and Tauri's State extraction
// fails; the frontend's normalizeError surfaces that in the error banner.
#[tauri::command]
#[specta::specta]
pub async fn list_sites(db: tauri::State<'_, Db>) -> Result<Vec<SiteDto>, IpcError> {
    let repo = SqliteSiteRepository::new(db.inner());
    Ok(repo.list().await?.into_iter().map(SiteDto::from).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn create_site(
    db: tauri::State<'_, Db>,
    input: SiteInput,
) -> Result<SiteDto, IpcError> {
    let new: NewSite = input.try_into()?;
    let repo = SqliteSiteRepository::new(db.inner());
    Ok(SiteDto::from(repo.create(new).await?))
}

#[tauri::command]
#[specta::specta]
pub async fn update_site(
    db: tauri::State<'_, Db>,
    id: String,
    input: SiteInput,
) -> Result<SiteDto, IpcError> {
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db.inner());
    let existing = repo.get(&site_id).await?.ok_or_else(|| IpcError::Core {
        message: format!("site {id} not found"),
    })?;
    let new: NewSite = input.try_into()?;
    // `id` and `created_at` come from the stored row, never the client.
    // `updated_at` is bumped by the repository.
    let updated = Site {
        id: existing.id,
        name: new.name,
        domain: new.domain,
        docroot: new.docroot,
        web_server: new.web_server,
        php_version: new.php_version,
        enabled: new.enabled,
        created_at: existing.created_at,
        updated_at: existing.updated_at,
    };
    Ok(SiteDto::from(repo.update(&updated).await?))
}

#[tauri::command]
#[specta::specta]
pub async fn delete_site(db: tauri::State<'_, Db>, id: String) -> Result<bool, IpcError> {
    let site_id = SiteId::parse(&id)?;
    let repo = SqliteSiteRepository::new(db.inner());
    Ok(repo.delete(&site_id).await?)
}
```

- [ ] **Step 7: Register the commands**

In `apps/desktop/src-tauri/src/lib.rs`, add to `collect_commands![…]` after `commands::service_log_tail,`:

```rust
            commands::list_sites,
            commands::create_site,
            commands::update_site,
            commands::delete_site,
```

- [ ] **Step 8: Run the tests to green**

Run: `cargo test -p openvhost-desktop site_ipc 2>&1 | tail -8`
Expected: 4 tests pass — valid conversion, hostile-input-with-right-field (8 cases), DTO round-trip, and the `CoreError::Validation` → `IpcError::Validation` mapping.

- [ ] **Step 9: Regenerate the typed bindings**

Run: `cargo test -p openvhost-desktop export_bindings 2>&1 | tail -3`
Then confirm the new commands and the `validation` error variant appear:

```bash
grep -n "listSites\|createSite\|updateSite\|deleteSite\|SiteDto\|SiteInput\|validation" apps/desktop/src/lib/ipc/bindings.ts | head -20
```

Expected: all present. Unlike prior slices, a bindings **diff is expected here** (4 new commands + a new error variant). Do not hand-edit the file.

- [ ] **Step 10: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add apps/desktop/src-tauri/src/commands.rs apps/desktop/src-tauri/src/lib.rs apps/desktop/src/lib/ipc/bindings.ts
git commit -s -m "feat(ipc): Sites CRUD commands with validated DTO boundary"
```

---

### Task 2: Frontend data layer — typed wrappers, derive helpers, store

**Files:**
- Modify: `apps/desktop/src/lib/ipc/index.ts` (4 wrappers + type re-exports)
- Create: `apps/desktop/src/lib/sites.derive.ts`
- Create: `apps/desktop/src/lib/sites.derive.test.ts`
- Create: `apps/desktop/src/lib/sites.svelte.ts`
- Create: `apps/desktop/src/lib/sites.svelte.test.ts`

**Interfaces:**
- Consumes (Task 1): generated `commands.listSites/createSite/updateSite/deleteSite`, types `SiteDto`/`SiteInput`/`IpcError` from `./bindings`.
- Produces: `listSites()`, `createSite(input)`, `updateSite(id, input)`, `deleteSite(id)`; `composeDomain`, `splitDomain`, `enabledPill`, `PHP_VERSIONS`; `SitesStore` with `sites`, `error`, `fieldErrors`, `load()`, `save(id|null, input)`, `remove(id)`.

- [ ] **Step 1: Add the typed IPC wrappers**

In `apps/desktop/src/lib/ipc/index.ts`, extend the type re-export line to include the new types and append the wrappers (they use the existing `unwrap` helper, so every error path is normalized to an `IpcError`):

```ts
export type { SiteDto, SiteInput };
```
(add `SiteDto, SiteInput` to the existing `import type { … } from './bindings';` list and to the existing `export type { … }` line)

```ts
export async function listSites(): Promise<SiteDto[]> {
	return unwrap(commands.listSites());
}
export async function createSite(input: SiteInput): Promise<SiteDto> {
	return unwrap(commands.createSite(input));
}
export async function updateSite(id: string, input: SiteInput): Promise<SiteDto> {
	return unwrap(commands.updateSite(id, input));
}
export async function deleteSite(id: string): Promise<boolean> {
	return unwrap(commands.deleteSite(id));
}
```

- [ ] **Step 2: Write the failing derive tests**

`apps/desktop/src/lib/sites.derive.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { composeDomain, enabledPill, splitDomain, PHP_VERSIONS } from './sites.derive';

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

describe('PHP_VERSIONS', () => {
	it('offers major.minor values only', () => {
		for (const v of PHP_VERSIONS) expect(v).toMatch(/^\d+\.\d+$/);
	});
});
```

Run: `pnpm -C apps/desktop test -- sites.derive 2>&1 | tail -5`
Expected: FAIL — `./sites.derive` not found.

- [ ] **Step 3: Implement the derive helpers**

`apps/desktop/src/lib/sites.derive.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers for the Sites UI. The `.localhost` suffix is fixed: such
// domains resolve without touching the hosts file, which is why this slice
// needs no privileged helper. Custom TLDs are a later slice.

const LOCALHOST_SUFFIX = '.localhost';

/** Compose the stored domain from the subdomain the user types. */
export function composeDomain(subdomain: string): string {
	return `${subdomain}${LOCALHOST_SUFFIX}`;
}

/**
 * Strip exactly one trailing `.localhost` for editing. A stored domain without
 * that suffix is only reachable by hand-editing `state.db`; it is shown as-is
 * rather than adding a second domain-entry mode.
 */
export function splitDomain(domain: string): string {
	return domain.endsWith(LOCALHOST_SUFFIX)
		? domain.slice(0, -LOCALHOST_SUFFIX.length)
		: domain;
}

/** Row pill for a site's stored `enabled` flag (reuses the shared pill classes). */
export function enabledPill(enabled: boolean): { label: string; cls: string } {
	return enabled
		? { label: 'enabled', cls: 'pill-running' }
		: { label: 'disabled', cls: 'pill-stopped' };
}

/**
 * Selectable PHP versions. Fixed for this slice — annotating which are
 * installed needs the package IPC (its own slice).
 */
export const PHP_VERSIONS = ['8.4', '8.3', '8.2', '8.1'] as const;
```

Run: `pnpm -C apps/desktop test -- sites.derive 2>&1 | tail -5` → PASS.

- [ ] **Step 4: Write the failing store tests**

`apps/desktop/src/lib/sites.svelte.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { SitesStore } from './sites.svelte';
import type { SiteDto, SiteInput } from './ipc';

const dto = (id: string, name: string): SiteDto => ({
	id,
	name,
	domain: `${name}.localhost`,
	docroot: `/srv/www/${name}`,
	webServer: 'nginx',
	phpVersion: '8.3',
	enabled: true,
	createdAt: 1,
	updatedAt: 1
});

const input: SiteInput = {
	name: 'shop',
	domain: 'shop.localhost',
	docroot: '/srv/www/shop',
	webServer: 'nginx',
	phpVersion: '8.3',
	enabled: true
};

function api(overrides: Partial<Record<string, unknown>> = {}) {
	return {
		listSites: vi.fn(async () => [dto('a', 'shop')]),
		createSite: vi.fn(async () => dto('a', 'shop')),
		updateSite: vi.fn(async () => dto('a', 'shop')),
		deleteSite: vi.fn(async () => true),
		...overrides
	} as never;
}

describe('SitesStore', () => {
	it('load() fills sites', async () => {
		const store = new SitesStore(api());
		await store.load();
		expect(store.sites.map((s) => s.name)).toEqual(['shop']);
	});

	it('save(null, input) creates then refetches', async () => {
		const a = api();
		const store = new SitesStore(a);
		expect(await store.save(null, input)).toBe(true);
		expect(a.createSite).toHaveBeenCalledWith(input);
		expect(a.listSites).toHaveBeenCalled();
	});

	it('save(id, input) updates then refetches', async () => {
		const a = api();
		const store = new SitesStore(a);
		expect(await store.save('a', input)).toBe(true);
		expect(a.updateSite).toHaveBeenCalledWith('a', input);
	});

	it('a validation error lands on fieldErrors and does not throw', async () => {
		const a = api({
			createSite: vi.fn(async () => {
				throw { kind: 'validation', field: 'domain', message: 'already taken' };
			})
		});
		const store = new SitesStore(a);
		expect(await store.save(null, input)).toBe(false);
		expect(store.fieldErrors.domain).toBe('already taken');
		expect(store.error).toBeNull();
	});

	it('a non-validation error lands on error', async () => {
		const a = api({
			listSites: vi.fn(async () => {
				throw { kind: 'core', message: 'state.db unavailable' };
			})
		});
		const store = new SitesStore(a);
		await store.load();
		expect(store.error?.kind).toBe('core');
	});

	it('remove() deletes then refetches, and a false result is still success', async () => {
		const a = api({ deleteSite: vi.fn(async () => false) });
		const store = new SitesStore(a);
		expect(await store.remove('a')).toBe(true);
		expect(a.listSites).toHaveBeenCalled();
	});
});
```

Run: `pnpm -C apps/desktop test -- sites.svelte 2>&1 | tail -5`
Expected: FAIL — `./sites.svelte` not found.

- [ ] **Step 5: Implement the store**

`apps/desktop/src/lib/sites.svelte.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// Sites panel state. Mutations refetch the list (there is no site event
// stream), and a per-field validation error is surfaced separately from a
// general error so the form can mark the offending input.
import type { IpcError, SiteDto, SiteInput } from './ipc';

export interface SitesApi {
	listSites(): Promise<SiteDto[]>;
	createSite(input: SiteInput): Promise<SiteDto>;
	updateSite(id: string, input: SiteInput): Promise<SiteDto>;
	deleteSite(id: string): Promise<boolean>;
}

function isValidation(e: unknown): e is { kind: 'validation'; field: string; message: string } {
	return typeof e === 'object' && e !== null && (e as { kind?: unknown }).kind === 'validation';
}

export class SitesStore {
	sites = $state<SiteDto[]>([]);
	error = $state<IpcError | null>(null);
	fieldErrors = $state<Record<string, string>>({});

	constructor(private api: SitesApi) {}

	/** Clear both error channels before a new attempt. */
	private reset(): void {
		this.error = null;
		this.fieldErrors = {};
	}

	async load(): Promise<void> {
		this.reset();
		try {
			this.sites = await this.api.listSites();
		} catch (e) {
			this.error = e as IpcError;
		}
	}

	/** `id === null` creates, otherwise updates. Returns true on success. */
	async save(id: string | null, input: SiteInput): Promise<boolean> {
		this.reset();
		try {
			if (id === null) await this.api.createSite(input);
			else await this.api.updateSite(id, input);
		} catch (e) {
			if (isValidation(e)) {
				this.fieldErrors = { [e.field]: e.message };
			} else {
				this.error = e as IpcError;
			}
			return false;
		}
		await this.load();
		return true;
	}

	/**
	 * Delete a site. A `false` result means the row was already gone — still a
	 * success from the user's point of view, so the list just refetches.
	 */
	async remove(id: string): Promise<boolean> {
		this.reset();
		try {
			await this.api.deleteSite(id);
		} catch (e) {
			this.error = e as IpcError;
			return false;
		}
		await this.load();
		return true;
	}
}
```

- [ ] **Step 6: Green + gate + commit**

```bash
pnpm -C apps/desktop test 2>&1 | tail -6
pnpm -C apps/desktop check && pnpm -C apps/desktop build 2>&1 | tail -2
(pnpm -C apps/desktop lint 2>&1 | tail -20)
git add apps/desktop/src/lib
git commit -s -m "feat(ui): Sites data layer — typed wrappers, derive helpers, store"
```

Expected: all vitest green (derive + store + the pre-existing ipc tests); `check` 0 errors; build succeeds; lint clean.

---

### Task 3: Sites route, panel, list rows, live rail nav

**Files:**
- Create: `apps/desktop/src/routes/sites/+page.svelte`
- Create: `apps/desktop/src/lib/components/SitesPanel.svelte`
- Create: `apps/desktop/src/lib/components/SiteListRow.svelte`
- Modify: `apps/desktop/src/lib/components/Rail.svelte` (Sites becomes a live link)

**Interfaces:**
- Consumes (Task 2): `SitesStore`, `listSites`/`createSite`/`updateSite`/`deleteSite`, `enabledPill`, types `SiteDto`.
- Consumes (existing): `AppShell` (props `{ runningCount: number, children: Snippet }`), `Button.svelte`, `StatusPill.svelte`, the `--vh-*` tokens.
- Produces: `SitesPanel` (props `{ sites: readonly SiteDto[], onAdd: () => void, onEdit: (site: SiteDto) => void }`), `SiteListRow` (props `{ site: SiteDto, onEdit: (site: SiteDto) => void }`).

- [ ] **Step 1: `SiteListRow` — port the mockup row**

`apps/desktop/src/lib/components/SiteListRow.svelte` — port `docs/design/main-window.html`'s `.row.site-row` (lines 74–86) and the matching `mock.css` rules into a scoped `<style>`, tokens only. Columns: name + domain (mono meta), PHP version (mono num), web server (meta), the enabled pill, and an Edit action. No status-running pill and no Open button (see the spec's deviation table).

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { SiteDto } from '$lib/ipc';
	import { enabledPill } from '$lib/sites.derive';
	import Button from './Button.svelte';

	let { site, onEdit }: { site: SiteDto; onEdit: (site: SiteDto) => void } = $props();
	const pill = $derived(enabledPill(site.enabled));
</script>

<div class="row site-row" data-testid="site-{site.id}">
	<div>
		<div class="primary">{site.name}</div>
		<div class="meta mono">{site.domain}</div>
	</div>
	<div class="mono num">PHP {site.phpVersion}</div>
	<div class="meta">{site.webServer}</div>
	<span class="pill {pill.cls}" data-testid="site-pill-{site.id}">
		<span class="dot"></span>{pill.label}
	</span>
	<div class="row-actions">
		<Button variant="quiet" size="sm" onclick={() => onEdit(site)}>Edit</Button>
	</div>
</div>

<style>
	/* Port .row, .site-row, .primary, .meta, .mono, .num, .pill, .pill-running,
	   .pill-stopped, .dot, .row-actions from docs/design/mock.css — var(--vh-*) only. */
</style>
```

(Check `Button.svelte`'s actual prop names from Task P1-A and match them; if it takes `variant`/`size`/`onclick` as above, use them verbatim — otherwise adapt and note it.)

- [ ] **Step 2: `SitesPanel` — head, list, empty state**

`apps/desktop/src/lib/components/SitesPanel.svelte` — port `main-window.html`'s page head (lines 60–70) + `.panel`/`.rowlist` (72–73) with the count sub-line:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { SiteDto } from '$lib/ipc';
	import Button from './Button.svelte';
	import SiteListRow from './SiteListRow.svelte';

	let {
		sites,
		onAdd,
		onEdit
	}: {
		sites: readonly SiteDto[];
		onAdd: () => void;
		onEdit: (site: SiteDto) => void;
	} = $props();

	const enabledCount = $derived(sites.filter((s) => s.enabled).length);
</script>

<div class="page-head">
	<div>
		<h1>Sites</h1>
		<p class="sub">
			{sites.length} {sites.length === 1 ? 'site' : 'sites'} · {enabledCount} enabled
		</p>
	</div>
	<div class="grow"></div>
	<Button variant="primary" onclick={onAdd}>Add site</Button>
</div>

<section class="panel" aria-label="Sites" data-testid="sites">
	{#if sites.length === 0}
		<div class="empty">
			<p class="primary">No sites yet</p>
			<p class="meta">Add a site to serve a project folder at a <span class="mono">.localhost</span> domain.</p>
		</div>
	{:else}
		<div class="rowlist">
			{#each sites as site (site.id)}
				<SiteListRow {site} {onEdit} />
			{/each}
		</div>
	{/if}
</section>

<style>
	/* Port .page-head, .sub, .grow, .panel, .rowlist, .empty from
	   docs/design/mock.css — var(--vh-*) only. */
</style>
```

- [ ] **Step 3: The `/sites` page**

`apps/desktop/src/routes/sites/+page.svelte`:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import SitesPanel from '$lib/components/SitesPanel.svelte';
	import { createSite, deleteSite, listSites, updateSite, type SiteDto } from '$lib/ipc';
	import { SitesStore } from '$lib/sites.svelte';

	const store = new SitesStore({ listSites, createSite, updateSite, deleteSite });

	onMount(() => {
		void store.load();
	});

	// The editor drawer lands in Task 4; until then Add/Edit are inert hooks.
	function onAdd(): void {}
	function onEdit(_site: SiteDto): void {}
</script>

<AppShell runningCount={0} active="sites">
	{#if store.error}
		<div class="banner-error" role="alert" data-testid="sites-error">
			<strong>Command failed ({store.error.kind})</strong>
			<span>{'message' in store.error ? store.error.message : ''}</span>
		</div>
	{/if}
	<SitesPanel sites={store.sites} {onAdd} {onEdit} />
</AppShell>

<style>
	/* .banner-error: reuse the same token-based treatment as the Services page. */
</style>
```

**Note on `AppShell`:** it currently takes `{ runningCount, children }` and hardcodes `Rail active="services"`. Add an optional `active` prop (`'services' | 'sites'`, default `'services'`) that it forwards to `Rail`, and give `Rail`'s `active` prop the same union. Keep the default so the existing Services page is unchanged. `runningCount={0}` on this page is honest: the Sites page shows no service count (a shared count source is a later refactor — do NOT fetch services here just to fill the titlebar).

- [ ] **Step 4: Make the rail's Sites item live**

In `apps/desktop/src/lib/components/Rail.svelte`, convert the Sites placeholder `<span aria-disabled="true">` into a real link mirroring the Services item — `href={resolve('/sites')}` (the repo's `svelte/no-navigation-without-resolve` rule requires `resolve` from `$app/paths`), with `aria-current={active === 'sites' ? 'page' : undefined}`. Widen `active`'s type to `'services' | 'sites'`. Leave Logs and Settings inert.

- [ ] **Step 5: Verify + gate + commit**

Run the dev server and confirm: `/sites` renders the shell with the Sites rail item active, the empty state shows (a fresh `state.db` has no sites), and navigating between `/` and `/sites` keeps both rail items correct.

```bash
pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
(pnpm -C apps/desktop lint 2>&1 | tail -20)
git add apps/desktop/src && git commit -s -m "feat(ui): Sites route, panel, list rows; rail Sites goes live"
```

---

### Task 4: Editor drawer — form, folder picker, delete confirm

**Files:**
- Create: `apps/desktop/src/lib/components/SiteDrawer.svelte`
- Modify: `apps/desktop/src/routes/sites/+page.svelte` (wire the drawer)
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add `tauri-plugin-dialog`)
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register the plugin)
- Modify: `apps/desktop/src-tauri/capabilities/default.json` (grant the dialog-open permission)
- Modify: `apps/desktop/package.json` (add `@tauri-apps/plugin-dialog`)

**Interfaces:**
- Consumes: `SitesStore` (`save`, `remove`, `fieldErrors`), `composeDomain`/`splitDomain`/`PHP_VERSIONS`, `SiteDto`/`SiteInput`, `Button.svelte`.
- Produces: `SiteDrawer` (props `{ site: SiteDto | null, fieldErrors: Record<string, string>, onSave: (id: string | null, input: SiteInput) => Promise<boolean>, onDelete: (id: string) => Promise<boolean>, onClose: () => void }`) — `site === null` means "create".

- [ ] **Step 1: Add the dialog plugin (Rust + npm + capability)**

```bash
pnpm -C apps/desktop add @tauri-apps/plugin-dialog
```

In `apps/desktop/src-tauri/Cargo.toml` `[dependencies]`: `tauri-plugin-dialog = "2"`.

In `apps/desktop/src-tauri/src/lib.rs`, register it on the builder before `.setup(…)`:

```rust
        .plugin(tauri_plugin_dialog::init())
```

In `apps/desktop/src-tauri/capabilities/default.json`, add the **minimal** permission to the `permissions` array — the folder-open permission only:

```json
    "dialog:allow-open"
```

**Confirm the exact identifier** against the installed plugin version (`ls apps/desktop/src-tauri/gen/schemas/` or the plugin's permission docs) before assuming `dialog:allow-open`; use the real one and note it in your report. Do NOT add `dialog:default` or any filesystem permission — the picker only needs open.

Verify: `cargo build -p openvhost-desktop 2>&1 | tail -3` and `cargo deny check licenses advisories 2>&1 | tail -5` (expect exit 0 — the plugin is MIT/Apache).

- [ ] **Step 2: Build the drawer**

`apps/desktop/src/lib/components/SiteDrawer.svelte` — port `docs/design/site-editor.html` lines 87–142 (`.drawer-backdrop`, `aside.drawer`, `.drawer-head/body/foot`, `.field`, `.input`, `.input-group`, `.input-suffix`, `.seg`, `.danger-zone`, `.consequence`) plus those rules from `mock.css`, tokens only.

Behaviour contract:
- `role="dialog"`, `aria-modal="true"`, `aria-labelledby` the heading; heading reads `Add site` when `site === null`, else `Edit site — {site.name}`.
- **Focus management:** move focus to the Name input on open; restore focus to the previously-focused element on close; **Esc closes**; keep focus inside the drawer while open (wrap Tab on the first/last focusable).
- Form state seeded from `site` (using `splitDomain(site.domain)` for the subdomain input) or empty for create; `enabled` defaults to `true` on create.
- On submit build `SiteInput` with `domain: composeDomain(subdomain)` and call `onSave(site?.id ?? null, input)`; on `true` close, on `false` stay open with input intact and render `fieldErrors[…]` under the matching field (`name`, `domain`, `docroot`, `php_version`, `web_server` — the backend's field names).
- Save is disabled while a submit is in flight.
- **Browse:** `import { open } from '@tauri-apps/plugin-dialog';` then
  ```ts
  const picked = await open({ directory: true, multiple: false, title: 'Choose project folder' });
  if (typeof picked === 'string') docroot = picked;
  ```
  A cancelled picker (null) leaves the field unchanged.
- **PHP version:** `<select>` over `PHP_VERSIONS`.
- **Enabled:** a checkbox labelled "Enabled" (an addition to the mockup — the row pill reflects it).
- **Danger zone** (edit mode only): "Delete site…" reveals "Really delete `{site.name}`?" with Cancel + a `btn-danger` confirm calling `onDelete(site.id)`; keep the mockup's reassurance that project files are untouched.

- [ ] **Step 3: Wire the drawer into the page**

In `apps/desktop/src/routes/sites/+page.svelte`, replace the inert `onAdd`/`onEdit` with drawer state and render it:

```svelte
	let editing = $state<SiteDto | null>(null);
	let drawerOpen = $state(false);

	function onAdd(): void {
		editing = null;
		drawerOpen = true;
	}
	function onEdit(site: SiteDto): void {
		editing = site;
		drawerOpen = true;
	}
```

```svelte
	{#if drawerOpen}
		<SiteDrawer
			site={editing}
			fieldErrors={store.fieldErrors}
			onSave={(id, input) => store.save(id, input)}
			onDelete={(id) => store.remove(id)}
			onClose={() => (drawerOpen = false)}
		/>
	{/if}
```

(The drawer closes itself via `onClose` after a successful save or delete.)

- [ ] **Step 4: Verify the round-trip against the real backend**

`pnpm -C apps/desktop tauri dev` (the real app, so `state.db` and the picker work). Confirm end-to-end:
1. Add site → Browse picks a folder → Save → the row appears with the right domain/PHP/server and an **enabled** pill.
2. Edit that site → change PHP version + untick Enabled → Save → the row shows PHP 8.2 and a **disabled** pill.
3. Add a second site with the **same name** → Save → the Name field shows "already taken" and the drawer stays open with input intact.
4. Delete → two-step confirm → the row disappears.
5. Relaunch the app → the sites are still listed (they are in `state.db`).

Record the outcomes in your report.

- [ ] **Step 5: Gate + commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh
pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
(pnpm -C apps/desktop lint 2>&1 | tail -20)
git add apps/desktop && git commit -s -m "feat(ui): site editor drawer with folder picker and delete confirm"
```

---

### Task 5: Accessibility, visual proof, full gate, PR

**Files:** none (verification + PR).

- [ ] **Step 1: Accessibility pass**

Confirm and fix any gap: the drawer is `role="dialog"`/`aria-modal` and labelled; focus moves in on open and is restored on close; Esc closes; Tab is trapped; every field has an associated `<label>`; the segmented web-server control is a `role="group"` with `aria-pressed`; per-field errors are associated with their input (`aria-describedby` + `aria-invalid`); the rail exposes `aria-current="page"` on the active destination; visible `:focus-visible` on every control; `prefers-reduced-motion` respected for the drawer transition.

- [ ] **Step 2: Visual proof**

With `pnpm -C apps/desktop tauri dev` running, capture: the Sites list with 2+ sites (one enabled, one disabled), the empty state, the drawer in create mode, the drawer in edit mode, and a per-field validation error. Save under the scratchpad; these are the artifact the controller relays for visual sign-off.

- [ ] **Step 3: Full gate**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh
pnpm -C apps/desktop lint 2>&1 | tail -20
pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
grep -c "listSites\|createSite\|updateSite\|deleteSite" apps/desktop/src/lib/ipc/bindings.ts
```

Expected: everything green offline; the bindings grep shows the 4 commands present.

- [ ] **Step 4: Push + PR (do NOT merge)**

```bash
git push -u origin feat/p1-sites-crud
gh pr create --title "feat: Phase 1 — Sites CRUD (typed IPC + Sites panel & editor drawer)" --body "Four typed IPC commands over the existing SiteRepository (list/create/update/delete) with a validated DTO boundary — every inbound field goes through its domain newtype's \`parse\`, so no unvalidated string can reach state.db — plus the Sites panel, list rows, and the editor drawer (native folder picker, segmented web-server control, PHP select, Enabled toggle, two-step delete confirm) from the mockups. \`IpcError::Validation { field, message }\` drives per-field form errors, so a duplicate name/domain marks the right input. \`id\`/\`created_at\`/\`updated_at\` are server-owned: update re-reads the row and never takes them from the client.

Store-only: no config generation and no nginx reload (the apply/diff pipeline is its own slice). Documented mockup deviations: \"Save\" instead of \"Save & restart nginx\"; a plain PHP select (installed-version annotations need package IPC); an Enabled/Disabled pill from real stored data instead of a fake running pill, plus an Enabled toggle so it can change; no \"Open\" button (nothing is served yet); absolute paths from the picker instead of \`~\`.

Verification: Rust tests cover the DTO round-trip and hostile IPC input rejected with the correct \`field\` (8 cases), plus the CoreError→IpcError mapping; vitest covers the derive helpers and the store (including validation-error routing); a real-app round-trip was exercised (create → edit → duplicate-name rejection → delete → relaunch persistence). Full local gates + cargo deny green offline; bindings regenerated with the 4 new commands. CI disabled (billing).

SECURITY: this slice adds IPC commands and a new capability grant — MERGE-BLOCKED pending security-auditor APPROVE. The auditor should rule specifically on (a) whether any IPC payload can reach state.db unvalidated, and (b) whether the dialog capability grant is minimal.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

- [ ] **Step 5: Hand back to controller** — final whole-branch review AND the security-auditor merge gate, then the owner's visual sign-off + merge decision. NOT the implementer's step.

---

## Self-review (controller: verify before dispatching Task 1)

- **Spec coverage:** §3 commands → T1; §4 DTOs/conversion/`IpcError::Validation` → T1; §5 route/panel/rows/rail → T3, drawer/store/derive → T2+T4, folder picker → T4; §6 deviations → T3 (no Open, enabled pill) + T4 (Save label, PHP select, Enabled toggle, absolute path); §7 error states → T2 (store routing) + T3 (banner) + T4 (per-field); §8 testing → T1/T2 unit + T4 round-trip + T5 a11y/visual/gates; §9 non-goals honored; §10 delivery → Global Constraints + T5. Every spec section maps to a task.
- **Type consistency:** `SiteDto`/`SiteInput` field names are snake_case in Rust and camelCase over the wire (`webServer`, `phpVersion`, `createdAt`, `updatedAt`) — the TS tests and components use the camelCase form throughout; `IpcError::Validation` → `{ kind: 'validation', field, message }`; `SitesStore.save(id|null, input)`/`remove(id)`/`load()`; `composeDomain`/`splitDomain`/`enabledPill`/`PHP_VERSIONS`; `SitesPanel{sites,onAdd,onEdit}`; `SiteListRow{site,onEdit}`; `SiteDrawer{site,fieldErrors,onSave,onDelete,onClose}`; `AppShell{runningCount,active?,children}`; `Rail{active}` widened to `'services'|'sites'`. Consistent across tasks.
- **Hazards flagged for implementers:** the backend's validation `field` names are **snake_case** (`php_version`, `web_server`) while the DTO is camelCase — `fieldErrors` keys therefore use the backend spelling; the drawer must look them up with that spelling (T4 Step 2 says so explicitly). `SqliteSiteRepository::new(db.inner())` — use `.inner()`, not `&db`, to avoid relying on deref coercion. `SiteRepository` must be in scope for the method calls. `AppShell`/`Rail` need the `active` prop widened without changing the Services page's behavior. Task 3's `runningCount={0}` is deliberate. A bindings diff IS expected in T1 (unlike prior slices). Confirm the real dialog permission identifier rather than trusting `dialog:allow-open`.
