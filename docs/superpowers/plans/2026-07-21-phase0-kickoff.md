# Phase 0 Kickoff Implementation Plan (Bootstrap + P0-1 + P0-2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bootstrap the OpenServ repo (8 agents, CLAUDE.md, COPYING), scaffold the Cargo/pnpm monorepo with tiered CI, and ship a Tauri 2 + SvelteKit shell whose one typed IPC command (`core_info`) round-trips from UI → Rust core → UI, with a visible error path.

**Architecture:** Cargo workspace whose layout mirrors the master-plan §6.2 ownership map (stub crates freeze delegation boundaries). `openserv-core` is tauri-free and owns `resolve_home()` + `CoreInfo`; the desktop crate exposes one thin command; all frontend IPC flows through `src/lib/ipc/`. CI is tiered: full-signal ubuntu job on every push, macOS+Windows matrix on PRs to main.

**Tech Stack:** Rust stable (edition 2024), Tauri 2.x, SvelteKit 2 + Svelte 5 (SPA via adapter-static), TypeScript strict, Tailwind 4, pnpm, Vitest, cargo-deny, tauri-specta v2 (spike) with ts-rs fallback.

**Spec:** `docs/superpowers/specs/2026-07-21-phase0-kickoff-design.md` · **Source of truth:** `docs/OPENSERV_MASTER_PLAN.md` (v1.1)

> **Rename note (2026-07-21):** product renamed **OpenServ → OpenVHost** mid-execution (source of truth is now `docs/OPENVHOST_MASTER_PLAN.md` v1.2). This plan is the historical execution record; `openserv*` identifiers it prescribes were renamed to `openvhost*` (CLI `openservctl` → `openvhost`) by the rename commits on `feat/p0-scaffold` after Task 6.

## Global Constraints

Every task's requirements implicitly include all of these:

- License **GPL-3.0-or-later**. `COPYING` at repo root. SPDX first-line comment `SPDX-License-Identifier: GPL-3.0-or-later` on every `.rs` `.ts` `.js` `.svelte` `.css` `.sh` file and `.github/workflows/*.yml` (comment syntax per language). Exempt: JSON, TOML, Markdown, lockfiles, generated files (`src/lib/ipc/bindings.ts`, `src/lib/ipc/gen/`, `src-tauri/gen/`, icons).
- Every commit: Conventional Commits **and** DCO sign-off — always `git commit -s`.
- Rust: toolchain pinned in `rust-toolchain.toml`; edition 2024; `cargo fmt` clean; `clippy --workspace -- -D warnings` clean; no `unwrap()`/`expect()` outside tests (workspace lint `unwrap_used`/`expect_used = "warn"`, promoted to errors by `-D warnings` in CI; test modules carry `#[allow(clippy::unwrap_used)]`).
- `openserv-core` must NEVER depend on tauri (CI guard enforces).
- Stub crates (`openserv-proc`, `openserv-pkg`, `openserv-conf`): **zero external dependencies**.
- TypeScript strict. ALL frontend IPC goes through `apps/desktop/src/lib/ipc/` — no `invoke("...")` anywhere else.
- pnpm only (`packageManager` pinned in package.json); Node current LTS pinned (`.nvmrc` + `engines`).
- Names: product **OpenServ** · crates `openserv-*` · CLI binary `openservctl` · desktop crate `openserv-desktop` · bundle identifier `dev.openserv.desktop` (provisional per plan OQ#8).
- `resolve_home()` honors the `OPENSERV_HOME` env override; default `~/.openserv` (`%USERPROFILE%\.openserv` on Windows).
- CI: `quick` (ubuntu) on every push+PR; `matrix` (macos-14 + windows-latest) on PRs to main + `workflow_dispatch`; merge only with matrix green; all GitHub Actions pinned by commit SHA.
- Version floors (exact versions resolved in Task 4 Step 1 recon): Tauri 2.x, @tauri-apps/api 2.x, SvelteKit 2, Svelte 5, Tailwind 4, TypeScript 5.
- Do not push to `main` except the Task 1 bootstrap commit; all code lands via the `feat/p0-scaffold` PR.

---

## File Structure

```
open-serv/
├── COPYING                          # GPL-3.0 full text (Task 1)
├── CLAUDE.md                        # verbatim from plan §6.4 (Task 1)
├── .claude/agents/*.md              # 8 files, verbatim from plan §6.3 (Task 1)
├── Cargo.toml                       # workspace: members, shared package keys, deps, lints (Task 2)
├── rust-toolchain.toml              # pinned stable (Task 2)
├── deny.toml                        # cargo-deny GPLv3-compat license allowlist (Task 2)
├── .gitignore                       # target/, node artifacts (Task 2)
├── crates/
│   ├── openserv-core/               # resolve_home + CoreInfo + CoreError (Tasks 2→3)
│   │   └── src/{lib.rs,error.rs,home.rs,info.rs}
│   ├── openserv-proc/src/lib.rs     # doc-only stub + trivial test (Task 2)
│   ├── openserv-pkg/src/lib.rs      # doc-only stub + trivial test (Task 2)
│   └── openserv-conf/src/lib.rs     # doc-only stub + trivial test (Task 2)
├── apps/
│   ├── cli/src/main.rs              # openservctl stub: prints version (Task 2)
│   └── desktop/                     # SvelteKit + Tauri (Tasks 4→5)
│       ├── src/routes/{+layout.ts,+page.svelte}
│       ├── src/lib/ipc/{index.ts,ipc.test.ts,bindings.ts|gen/}  # ONLY IPC module
│       └── src-tauri/src/{main.rs,lib.rs,commands.rs}
├── templates/README.md  tests/README.md  packaging/README.md    # owner stubs (Task 2)
├── scripts/check-spdx.sh            # SPDX header gate (Task 6)
├── .github/PULL_REQUEST_TEMPLATE.md # platform checklist (Task 6)
└── .github/workflows/ci.yml         # quick + matrix (Task 6)
```

---

### Task 1: Bootstrap — agents, CLAUDE.md, COPYING (direct to main)

**Files:**
- Create: `.claude/agents/{rust-core-engineer,platform-windows-specialist,platform-macos-specialist,tauri-frontend-engineer,config-template-engineer,ci-release-engineer,security-auditor,qa-test-engineer}.md`
- Create: `CLAUDE.md`, `COPYING`

**Interfaces:**
- Consumes: `docs/OPENSERV_MASTER_PLAN.md` §6.3/§6.4 (already committed).
- Produces: project agent definitions + `CLAUDE.md` golden rules that every later task (and subagent) operates under. Content is verbatim — do NOT add SPDX headers or edit anything inside these files.

- [ ] **Step 1: Confirm you are on main and clean**

Run: `git switch main && git status --short`
Expected: on `main`, no output from status.

- [ ] **Step 2: Extract the 8 agent files + CLAUDE.md verbatim from the master plan**

Create `scripts/extract-bootstrap.py` is NOT needed long-term — run this inline heredoc (it parses the committed plan, never chat history):

```bash
python3 - <<'EOF'
import re, pathlib
plan = pathlib.Path("docs/OPENSERV_MASTER_PLAN.md").read_text(encoding="utf-8")

agents = re.findall(
    r"#### `\.claude/agents/([a-z-]+)\.md`\n```markdown\n(.*?)\n```",
    plan, re.DOTALL)
assert len(agents) == 8, f"expected 8 agents, found {len(agents)}"
outdir = pathlib.Path(".claude/agents"); outdir.mkdir(parents=True, exist_ok=True)
for name, body in agents:
    (outdir / f"{name}.md").write_text(body + "\n", encoding="utf-8")
    print(f"wrote .claude/agents/{name}.md ({len(body.splitlines())} lines)")

m = re.search(
    r"### 6\.4 Repo-root `CLAUDE\.md` \(create verbatim\)\n```markdown\n(.*?)\n```",
    plan, re.DOTALL)
assert m, "CLAUDE.md block not found"
pathlib.Path("CLAUDE.md").write_text(m.group(1) + "\n", encoding="utf-8")
print("wrote CLAUDE.md")
EOF
```

Expected output: eight `wrote .claude/agents/...` lines and `wrote CLAUDE.md`.

- [ ] **Step 3: Verify extraction is verbatim and well-formed**

```bash
ls .claude/agents | sort
head -2 .claude/agents/rust-core-engineer.md
grep -c '^name: ' .claude/agents/*.md | grep -v ':1$' || echo "frontmatter OK"
grep -n 'GPL-3.0-or-later' CLAUDE.md
```

Expected: 8 files listed; head shows `---` then `name: rust-core-engineer`; `frontmatter OK`; CLAUDE.md grep hits golden rule 7.

- [ ] **Step 4: Fetch COPYING (GPL-3.0 full text)**

```bash
curl -fsSL https://www.gnu.org/licenses/gpl-3.0.txt -o COPYING
head -2 COPYING && wc -l COPYING
```

Expected: first lines contain `GNU GENERAL PUBLIC LICENSE` / `Version 3, 29 June 2007`; ~674 lines. If offline, copy the GPL-3.0 text from any local Rust/GNOME checkout — the content is canonical.

- [ ] **Step 5: Commit direct to main and push**

```bash
git add .claude/agents CLAUDE.md COPYING
git commit -s -m "docs: bootstrap agent definitions, CLAUDE.md, and COPYING (plan §6.3/§6.4, §1.3)"
git push origin main
```

Expected: 10 files changed. No CI runs (no workflow file exists yet — by design).

---

### Task 2: Workspace scaffold — stub crates, CLI, toolchain, deny.toml (on branch)

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `.gitignore`
- Create: `crates/openserv-{core,proc,pkg,conf}/{Cargo.toml,src/lib.rs}`
- Create: `apps/cli/{Cargo.toml,src/main.rs}`
- Create: `templates/README.md`, `tests/README.md`, `packaging/README.md`

**Interfaces:**
- Produces: workspace roots later tasks extend — root `Cargo.toml` `members` list (Task 4 appends `"apps/desktop/src-tauri"`), `[workspace.dependencies]` (`serde`, `thiserror`, `dirs`), `[workspace.lints.clippy]` (`unwrap_used`/`expect_used = "warn"`), and per-crate `[lints] workspace = true`. Task 3 rewrites `openserv-core`'s stub `lib.rs`.

- [ ] **Step 1: Create the branch**

Run: `git switch -c feat/p0-scaffold`

- [ ] **Step 2: Pin the toolchain and write workspace metadata**

```bash
cat > rust-toolchain.toml <<EOF
[toolchain]
channel = "$(rustc --version | awk '{print $2}')"
EOF
cat rust-toolchain.toml
```

Expected: `channel = "1.NN.M"` (whatever your current stable is — that becomes the pin).

Create `Cargo.toml` (repo root):

```toml
[workspace]
resolver = "2"
members = [
    "crates/openserv-core",
    "crates/openserv-proc",
    "crates/openserv-pkg",
    "crates/openserv-conf",
    "apps/cli",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "GPL-3.0-or-later"
publish = false

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
thiserror = "2"
dirs = "6"

[workspace.lints.clippy]
unwrap_used = "warn"
expect_used = "warn"
```

Create `.gitignore` (repo root):

```
/target
node_modules/
apps/desktop/build/
apps/desktop/.svelte-kit/
apps/desktop/src-tauri/gen/
.DS_Store
```

- [ ] **Step 3: Create the three zero-dependency stub crates**

For each of `proc`, `pkg`, `conf` create `crates/openserv-<name>/Cargo.toml`:

```toml
[package]
name = "openserv-proc"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[lints]
workspace = true
```

(Repeat with `name = "openserv-pkg"` and `name = "openserv-conf"` in their own directories.)

`crates/openserv-proc/src/lib.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openserv-proc — process supervisor (stub).
//!
//! Responsibility (master plan §3.1): spawn/stop/restart/status for every
//! managed service; state machine Stopped → Starting → Running → Failed;
//! graceful shutdown; orphan cleanup; Windows Job Objects; log capture;
//! health checks. Every child process in the codebase is spawned through
//! this crate — implementation lands in the P0-3 slice.

/// Crate marker used until the supervisor slice lands.
pub const CRATE_NAME: &str = "openserv-proc";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::CRATE_NAME, "openserv-proc");
    }
}
```

`crates/openserv-pkg/src/lib.rs` — same shape, with:
- doc: `//! openserv-pkg — package manager (stub).` / `//! Responsibility (master plan §3.1): download → SHA-256 verify → extract; packages/<name>/<major>/<full>/ layout with a current link per major; install/uninstall/upgrade/disable. Implementation lands in the P0-6 slice.`
- `pub const CRATE_NAME: &str = "openserv-pkg";` and matching assert.

`crates/openserv-conf/src/lib.rs` — same shape, with:
- doc: `//! openserv-conf — config generator (stub).` / `//! Responsibility (master plan §3.1): Tera templates → generated configs, atomic writes, native-validator + diff-preview pipeline, WebServerAdapter boundary. Implementation lands in the P0-7 slice.`
- `pub const CRATE_NAME: &str = "openserv-conf";` and matching assert.

- [ ] **Step 4: Create openserv-core as a stub (Task 3 makes it real)**

`crates/openserv-core/Cargo.toml`:

```toml
[package]
name = "openserv-core"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[lints]
workspace = true
```

`crates/openserv-core/src/lib.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openserv-core — domain model and state (stub until Task 3).
//!
//! Responsibility (master plan §3.1): domain model (Site, ServicePackage,
//! ServiceInstance, Certificate, HostsEntry), SQLite state, event bus.
//! MUST NEVER depend on tauri: consumed by both the desktop app and
//! the openservctl CLI.

/// Crate marker; replaced by real API in Task 3.
pub const CRATE_NAME: &str = "openserv-core";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::CRATE_NAME, "openserv-core");
    }
}
```

- [ ] **Step 5: Create the openservctl stub**

`apps/cli/Cargo.toml`:

```toml
[package]
name = "openservctl"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[lints]
workspace = true
```

`apps/cli/src/main.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openservctl — OpenServ CLI (stub: prints version and exits 0).
//! Real verbs (start|stop|restart|status|list --json) land in Phase 1.

fn main() {
    println!("openservctl {}", env!("CARGO_PKG_VERSION"));
}
```

- [ ] **Step 6: Create ownership stub READMEs**

`templates/README.md`:

```markdown
# templates/

Tera templates for generated service configs (nginx, apache, php, mysql, mariadb).
Owner: **config-template-engineer** (see master plan §6.2). First content lands in the P0-7 slice.
```

`tests/README.md`:

```markdown
# tests/

Cross-platform integration harness and fixtures.
Owner: **qa-test-engineer** (see master plan §6.2). First content lands in the P0-9 slice.
```

`packaging/README.md`:

```markdown
# packaging/

Installer/bundling assets beyond Tauri defaults.
Owner: **ci-release-engineer** (see master plan §6.2).
```

- [ ] **Step 7: Write deny.toml and run the license gate locally**

`deny.toml` (repo root):

```toml
# cargo-deny configuration — license gate (master plan §1.3).
# Only GPLv3-COMPATIBLE licenses may be allowed here. If `cargo deny` fails
# on a new transitive dependency: verify the license is GPLv3-compatible
# (gnu.org/licenses/license-list), add it below with a comment, and say so
# in the commit message. Unknown licenses stay blocked — never bypassed.

[licenses]
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "0BSD",
    "CC0-1.0",
    "BSL-1.0",          # Boost — GPL-compatible
    "MPL-2.0",          # file-level copyleft, GPLv3-compatible as secondary license
    "Unicode-3.0",      # unicode-ident and friends
    "Unicode-DFS-2016", # older unicode-ident releases
]

[licenses.private]
ignore = true  # our own GPL-3.0-or-later workspace crates are publish = false
```

Install and run:

```bash
brew install cargo-deny || cargo install cargo-deny --locked
cargo deny check licenses
```

Expected: `licenses ok` (workspace has zero external deps right now).

- [ ] **Step 8: Verify the whole workspace gate locally**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p openservctl
```

Expected: fmt/clippy silent; 4 tests pass (one per crate); `openservctl 0.1.0`.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -s -m "feat: scaffold cargo workspace, stub crates, openservctl, license gate config"
```

---

### Task 3: openserv-core — resolve_home + CoreInfo (TDD)

**Files:**
- Modify: `crates/openserv-core/Cargo.toml` (add deps)
- Rewrite: `crates/openserv-core/src/lib.rs`
- Create: `crates/openserv-core/src/error.rs`, `crates/openserv-core/src/home.rs`, `crates/openserv-core/src/info.rs`

**Interfaces:**
- Produces (Task 5 consumes exactly these):
  - `openserv_core::CoreError` — `enum CoreError { HomeDirUnavailable }` (thiserror).
  - `openserv_core::resolve_home() -> Result<std::path::PathBuf, CoreError>` — `OPENSERV_HOME` override, else `<home>/.openserv`.
  - `openserv_core::CoreInfo { app_version: String, os: String, arch: String, openserv_home: String }` — serde Serialize/Deserialize, `#[serde(rename_all = "camelCase")]`, optional `specta`/`ts` derive features.
  - `openserv_core::core_info(app_version: &str) -> Result<CoreInfo, CoreError>`.

- [ ] **Step 1: Add dependencies (still zero tauri)**

Edit `crates/openserv-core/Cargo.toml` to:

```toml
[package]
name = "openserv-core"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true
dirs.workspace = true
specta = { version = "2", optional = true, features = ["derive"] }
ts-rs = { version = "10", optional = true }

[features]
specta = ["dep:specta"]
ts = ["dep:ts-rs"]

[lints]
workspace = true
```

Note: `specta`/`ts-rs` stay **unused** until Task 5 picks a branch; exact versions may need adjusting to what Task 4 Step 1 recon reports (`cargo add --dry-run specta ts-rs` shows resolvable versions). Features keep them out of the default build, so `cargo build` stays dep-light.

- [ ] **Step 2: Write the failing tests for resolve_home (pure function first)**

Create `crates/openserv-core/src/home.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! OPENSERV_HOME resolution (master plan §3.2; spec §7.1).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Resolve the OpenServ home directory: `OPENSERV_HOME` env override wins,
/// otherwise `<user home>/.openserv`. The override is what makes tests and
/// the future integration harness hermetic.
pub fn resolve_home() -> Result<PathBuf, CoreError> {
    resolve_home_from(
        std::env::var_os("OPENSERV_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// Pure core of [`resolve_home`], testable without touching process env.
pub(crate) fn resolve_home_from(
    override_val: Option<&OsStr>,
    home_dir: Option<&Path>,
) -> Result<PathBuf, CoreError> {
    if let Some(v) = override_val {
        if !v.is_empty() {
            return Ok(PathBuf::from(v));
        }
    }
    home_dir
        .map(|h| h.join(".openserv"))
        .ok_or(CoreError::HomeDirUnavailable)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        let p = resolve_home_from(
            Some(OsStr::new("/custom/openserv-home")),
            Some(Path::new("/Users/x")),
        )
        .unwrap();
        assert_eq!(p, PathBuf::from("/custom/openserv-home"));
    }

    #[test]
    fn defaults_to_dot_openserv_under_home() {
        let p = resolve_home_from(None, Some(Path::new("/Users/x"))).unwrap();
        // Build expected via join so the separator is right on Windows too.
        assert_eq!(p, Path::new("/Users/x").join(".openserv"));
    }

    #[test]
    fn empty_override_falls_back_to_default() {
        let p = resolve_home_from(Some(OsStr::new("")), Some(Path::new("/Users/x"))).unwrap();
        assert_eq!(p, Path::new("/Users/x").join(".openserv"));
    }

    #[test]
    fn no_home_and_no_override_errors() {
        assert!(matches!(
            resolve_home_from(None, None),
            Err(CoreError::HomeDirUnavailable)
        ));
    }
}
```

Create `crates/openserv-core/src/error.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Core error type (thiserror in library crates — master plan §5).

/// Errors produced by openserv-core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The user home directory could not be determined and no
    /// `OPENSERV_HOME` override was provided.
    #[error("could not determine the user home directory (set OPENSERV_HOME to override)")]
    HomeDirUnavailable,
}
```

Rewrite `crates/openserv-core/src/lib.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openserv-core — domain model and state for OpenServ.
//!
//! Responsibility (master plan §3.1): domain model, SQLite state, event bus.
//! MUST NEVER depend on tauri: consumed by both the desktop app and the
//! openservctl CLI. Current slice: home-directory resolution + CoreInfo.

mod error;
mod home;
mod info;

pub use error::CoreError;
pub use home::resolve_home;
pub use info::{core_info, CoreInfo};
```

Create `crates/openserv-core/src/info.rs` with ONLY the failing-test scaffold for now (the type exists so tests compile, the function does not):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! CoreInfo — the payload of the first typed IPC command (spec §7.1).
```

- [ ] **Step 3: Run tests to verify the state**

Run: `cargo test -p openserv-core`
Expected: FAIL to compile — `lib.rs` references `info::{core_info, CoreInfo}` which don't exist yet. (That is the red step for `info.rs`; `home.rs` tests are also not yet passing because the crate doesn't build.)

- [ ] **Step 4: Implement info.rs (minimal to green)**

Replace `crates/openserv-core/src/info.rs` with:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! CoreInfo — the payload of the first typed IPC command (spec §7.1).

use crate::error::CoreError;
use crate::home::resolve_home;

/// Basic environment facts, assembled by core (not by the Tauri command —
/// commands stay thin per master plan §5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CoreInfo {
    /// Version of the calling application (desktop app or CLI).
    pub app_version: String,
    /// Operating system, from `std::env::consts::OS` ("macos", "windows", …).
    pub os: String,
    /// CPU architecture, from `std::env::consts::ARCH` ("aarch64", "x86_64", …).
    pub arch: String,
    /// Resolved OpenServ home directory, for display.
    pub openserv_home: String,
}

/// Assemble [`CoreInfo`] for the given application version.
pub fn core_info(app_version: &str) -> Result<CoreInfo, CoreError> {
    let home = resolve_home()?;
    Ok(CoreInfo {
        app_version: app_version.to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        openserv_home: home.display().to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn core_info_reports_current_platform() {
        let info = core_info("9.9.9").unwrap();
        assert_eq!(info.app_version, "9.9.9");
        assert_eq!(info.os, std::env::consts::OS);
        assert_eq!(info.arch, std::env::consts::ARCH);
        assert!(!info.openserv_home.is_empty());
    }

    #[test]
    fn core_info_serializes_camel_case() {
        let info = core_info("1.0.0").unwrap();
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"appVersion\""));
        assert!(json.contains("\"openservHome\""));
    }
}
```

The serialization test needs `serde_json` as a dev-dependency. Add to `crates/openserv-core/Cargo.toml`:

```toml
[dev-dependencies]
serde_json = "1"
```

- [ ] **Step 5: Run tests to verify green, then the full gate**

```bash
cargo test -p openserv-core
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cargo deny check licenses
```

Expected: 6 core tests pass (4 home + 2 info) plus the 3 stub-crate tests still pass workspace-wide; clippy clean. `cargo deny` now evaluates real deps (serde, thiserror, dirs and transitive `option-ext` MPL-2.0 — already allowlisted); expected `licenses ok`.

- [ ] **Step 6: Commit**

```bash
git add crates/openserv-core
git commit -s -m "feat: openserv-core resolve_home with OPENSERV_HOME override and CoreInfo"
```

---

### Task 4: Desktop app scaffold — SvelteKit + Tauri shell

**Files:**
- Create: `apps/desktop/**` (via `sv create` + `tauri init`, then explicit overwrites below)
- Create: `apps/desktop/.nvmrc`, `apps/desktop/src/routes/+layout.ts`
- Overwrite: `apps/desktop/svelte.config.js`, `apps/desktop/src-tauri/{Cargo.toml,tauri.conf.json,build.rs,src/main.rs,src/lib.rs}`
- Modify: root `Cargo.toml` (add workspace member)

**Interfaces:**
- Consumes: workspace roots from Task 2.
- Produces: `openserv-desktop` crate at `apps/desktop/src-tauri` (lib name `openserv_desktop_lib`, fn `run()`); pnpm scripts `dev`, `build`, `check`, `lint`, `tauri`; SvelteKit SPA building to `apps/desktop/build/`. Task 5 adds `src-tauri/src/commands.rs` and rewrites `lib.rs`.

- [ ] **Step 1: Version reconnaissance (mandatory — plan §2 caveat)**

```bash
node --version && pnpm --version && rustc --version
npm view svelte version && npm view @sveltejs/kit version && npm view @sveltejs/adapter-static version
npm view @tauri-apps/cli version && npm view @tauri-apps/api version
npm view tailwindcss version && npm view typescript version
cargo search tauri --limit 1 && cargo search tauri-specta --limit 1
cargo search specta --limit 1 && cargo search specta-typescript --limit 1 && cargo search ts-rs --limit 1
```

Record every output in your working notes. Floors: Svelte 5.x, Kit 2.x, Tauri (crate + CLI + api) 2.x, Tailwind 4.x, TS 5.x, Node LTS (even major). **If any floor is violated (e.g. a Svelte 6 / Tauri 3 has shipped), STOP and ask the human** — the stack table in master plan §2 is ✅-decided and this plan assumes those majors. Use the recorded versions wherever a later step says "version from recon".

- [ ] **Step 2: Scaffold SvelteKit**

```bash
cd apps
pnpm dlx sv@latest create desktop
```

Interactive answers (flags drift between sv releases; the prompts are stable):
- Template: **SvelteKit minimal**
- Type checking: **Yes, using TypeScript syntax**
- Add-ons: **prettier, eslint, vitest, tailwindcss** (nothing else; if an "adapter" add-on is offered, skip it — adapter-static is configured manually next)
- Package manager: **pnpm** (let it install)

Acceptance (run from `apps/desktop`): `cat package.json` shows devDependencies with `svelte` ^5, `@sveltejs/kit` ^2, `tailwindcss` ^4, `vitest`; files `eslint.config.js`, `vite.config.ts`, `tsconfig.json` exist; `grep '"strict": true' tsconfig.json` matches (sv's base config sets it — if it's in the extended `.svelte-kit/tsconfig.json`, that's fine; `pnpm check` later proves strictness).

If sv created a sample test: `rm -f src/demo.spec.ts` (a real vitest test arrives in Task 5).

- [ ] **Step 3: Pin Node + pnpm**

```bash
cd apps/desktop
node --version | sed 's/^v//' > .nvmrc
corepack use pnpm@latest || npm pkg set packageManager="pnpm@$(pnpm --version)"
npm pkg set engines.node=">=24"
```

Acceptance: `.nvmrc` holds your Node version (must be an LTS major); `package.json` has a `packageManager` field pinning pnpm and an `engines.node` entry. (If your LTS major differs from 24, use that major in `engines.node` — the pin must match reality.)

- [ ] **Step 4: SPA mode — adapter-static, SSR off**

```bash
pnpm remove -D @sveltejs/adapter-auto || true
pnpm add -D @sveltejs/adapter-static
```

Overwrite `apps/desktop/svelte.config.js`:

```js
// SPDX-License-Identifier: GPL-3.0-or-later
import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		// Single prerendered route → plain static output in build/ for Tauri.
		adapter: adapter()
	}
};

export default config;
```

Create `apps/desktop/src/routes/+layout.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// SPA shell for Tauri: no SSR; the single route prerenders to build/index.html.
export const ssr = false;
export const prerender = true;
```

Verify: `pnpm build` → Expected: SvelteKit build succeeds, `build/index.html` exists.

- [ ] **Step 5: Add Tauri**

```bash
pnpm add @tauri-apps/api
pnpm add -D @tauri-apps/cli
pnpm tauri init
```

`tauri init` answers: app name **OpenServ** · window title **OpenServ** · web assets **`../build`** · dev server **`http://localhost:5173`** · dev command **`pnpm dev`** · build command **`pnpm build`**. (This creates `src-tauri/` including default icons — keep the icons.)

- [ ] **Step 6: Replace the generated src-tauri files with workspace-integrated versions**

`apps/desktop/src-tauri/Cargo.toml`:

```toml
[package]
name = "openserv-desktop"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[lib]
name = "openserv_desktop_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
serde.workspace = true
serde_json = "1"
thiserror.workspace = true
openserv-core = { path = "../../../crates/openserv-core" }

[lints]
workspace = true
```

`apps/desktop/src-tauri/build.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
fn main() {
    tauri_build::build()
}
```

`apps/desktop/src-tauri/src/lib.rs` (minimal until Task 5):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenServ desktop — Tauri entry point. Command surface arrives in Task 5.

pub fn run() {
    let result = tauri::Builder::default().run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("fatal: tauri failed to run: {e}");
        std::process::exit(1);
    }
}
```

`apps/desktop/src-tauri/src/main.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    openserv_desktop_lib::run();
}
```

`apps/desktop/src-tauri/tauri.conf.json`:

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "OpenServ",
  "version": "0.1.0",
  "identifier": "dev.openserv.desktop",
  "build": {
    "beforeDevCommand": "pnpm dev",
    "devUrl": "http://localhost:5173",
    "beforeBuildCommand": "pnpm build",
    "frontendDist": "../build"
  },
  "app": {
    "windows": [{ "title": "OpenServ", "width": 960, "height": 640 }],
    "security": { "csp": null }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

If the generated `src-tauri` had extra files (`capabilities/default.json` etc.), keep them unchanged. Delete any template `greet` command remnants if present.

- [ ] **Step 7: Join the workspace**

Edit root `Cargo.toml` members:

```toml
members = [
    "crates/openserv-core",
    "crates/openserv-proc",
    "crates/openserv-pkg",
    "crates/openserv-conf",
    "apps/cli",
    "apps/desktop/src-tauri",
]
```

- [ ] **Step 8: SPDX header pass on scaffolded frontend sources**

Prepend `// SPDX-License-Identifier: GPL-3.0-or-later` (first line) to: `vite.config.ts`, `eslint.config.js`, `src/app.d.ts`.
Prepend `<!-- SPDX-License-Identifier: GPL-3.0-or-later -->` to `src/app.html`, and `/* SPDX-License-Identifier: GPL-3.0-or-later */` to `src/app.css` (before the Tailwind `@import`). `src/routes/+page.svelte` is fully replaced in Task 5 (its replacement carries the header). JSON/TOML/MD stay exempt.

- [ ] **Step 9: Verify and commit**

```bash
pnpm -C apps/desktop build
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm -C apps/desktop check
pnpm -C apps/desktop lint
cargo deny check licenses
pnpm -C apps/desktop tauri dev   # manual: a window titled "OpenServ" opens; Ctrl+C when confirmed
```

Expected: all green; `cargo deny` now covers the tauri tree (curate `deny.toml` per its comment block if a GPLv3-compatible license appears — record additions in the commit message).

```bash
git add -A
git commit -s -m "feat: add Tauri 2 + SvelteKit desktop shell (adapter-static, strict TS, Tailwind)"
```

---

### Task 5: Typed IPC — specta spike, core_info command, UI with error path

**Files:**
- Create: `apps/desktop/src-tauri/src/commands.rs`
- Rewrite: `apps/desktop/src-tauri/src/lib.rs`
- Create: `apps/desktop/src/lib/ipc/index.ts`, `apps/desktop/src/lib/ipc/ipc.test.ts`
- Create (branch A): `apps/desktop/src/lib/ipc/bindings.ts` (generated, committed) — or (branch B): `apps/desktop/src/lib/ipc/gen/*.ts` (generated, committed)
- Rewrite: `apps/desktop/src/routes/+page.svelte`
- Modify: `apps/desktop/{eslint.config.js,.prettierignore,package.json}`, `apps/desktop/src-tauri/Cargo.toml`, `crates/openserv-core` (feature use only)

**Interfaces:**
- Consumes: `openserv_core::{core_info, CoreInfo, CoreError}` from Task 3; `openserv_desktop_lib::run()` shell from Task 4.
- Produces: Rust command `core_info(simulate_error: Option<bool>) -> Result<CoreInfo, IpcError>`; TS `coreInfo(simulateError?: boolean): Promise<CoreInfo>` throwing `IpcError` (`{ kind: 'simulated' } | { kind: 'core'; message: string }`) — the ONLY IPC entry point, exported from `$lib/ipc`.

- [ ] **Step 1: Write the command surface (branch-independent)**

Create `apps/desktop/src-tauri/src/commands.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Tauri command surface — thin validation + delegation to openserv-core
//! (business logic never lives here; master plan §5).

use openserv_core::CoreInfo;

/// Serializable command error (spec §7.2). Establishes the pattern:
/// every command returns `Result<_, IpcError>` and the UI renders failures.
#[derive(Debug, Clone, serde::Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum IpcError {
    /// Dev-only simulated failure used to exercise the UI error path.
    #[error("simulated failure (dev only)")]
    Simulated,
    /// An error bubbled up from openserv-core.
    #[error("{message}")]
    Core { message: String },
}

impl From<openserv_core::CoreError> for IpcError {
    fn from(e: openserv_core::CoreError) -> Self {
        IpcError::Core { message: e.to_string() }
    }
}

#[tauri::command]
#[specta::specta] // Branch A only — DELETE this attribute if the spike fails (branch B)
pub fn core_info(simulate_error: Option<bool>) -> Result<CoreInfo, IpcError> {
    // Dev-only demo affordance (spec §7.1): ignored in release builds.
    if cfg!(debug_assertions) && simulate_error.unwrap_or(false) {
        return Err(IpcError::Simulated);
    }
    Ok(openserv_core::core_info(env!("CARGO_PKG_VERSION"))?)
}
```

- [ ] **Step 2: SPIKE (time-box: 3 hours) — attempt tauri-specta (Branch A)**

Add dependencies (versions from Task 4 recon; floors shown):

```bash
cd apps/desktop/src-tauri
cargo add specta@2 --features derive
cargo add specta-typescript
cargo add tauri-specta@2 --features derive,typescript
```

Enable the specta derive on `CoreInfo`: in `apps/desktop/src-tauri/Cargo.toml` change the core dependency line to:

```toml
openserv-core = { path = "../../../crates/openserv-core", features = ["specta"] }
```

Add `derive(specta::Type)` to `IpcError` in `commands.rs` (extend the derive list: `#[derive(Debug, Clone, serde::Serialize, thiserror::Error, specta::Type)]`).

Rewrite `apps/desktop/src-tauri/src/lib.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenServ desktop — Tauri entry point with typed (tauri-specta) commands.

mod commands;

use tauri_specta::{collect_commands, Builder};

pub fn run() {
    let specta_builder = Builder::<tauri::Wry>::new().commands(collect_commands![commands::core_info]);

    // Regenerate the committed TS bindings on every dev run (debug only).
    #[cfg(debug_assertions)]
    if let Err(e) = specta_builder.export(
        specta_typescript::Typescript::default(),
        "../src/lib/ipc/bindings.ts",
    ) {
        eprintln!("fatal: failed to export TS bindings: {e}");
        std::process::exit(1);
    }

    let result = tauri::Builder::default()
        .invoke_handler(specta_builder.invoke_handler())
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("fatal: tauri failed to run: {e}");
        std::process::exit(1);
    }
}
```

Run: `pnpm -C apps/desktop tauri dev` then Ctrl+C once the window opens.

**Spike success checklist** (ALL must hold within the time-box):
- [ ] `cargo build --workspace` compiles with the exact APIs above (or near-trivial renames)
- [ ] `apps/desktop/src/lib/ipc/bindings.ts` was generated and contains `coreInfo`, `CoreInfo`, `IpcError` types
- [ ] `pnpm -C apps/desktop check` passes with the generated file (strict TS)

**Decision point:** If all boxes check → continue with Branch A steps below and record the outcome. If ANY fails after the time-box → run `git restore apps/desktop/src-tauri && git restore --staged . 2>/dev/null; git clean -fd apps/desktop/src/lib/ipc` to discard the spike, then take **Branch B** (Step 3B).

- [ ] **Step 3A (Branch A): typed wrapper over generated bindings**

Create `apps/desktop/src/lib/ipc/index.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// The ONLY module allowed to touch Tauri IPC (master plan §5).
import { commands } from './bindings';
import type { CoreInfo, IpcError } from './bindings';

export type { CoreInfo, IpcError };

/** Fetch CoreInfo from the Rust core. Throws IpcError on failure. */
export async function coreInfo(simulateError = false): Promise<CoreInfo> {
	const result = await commands.coreInfo(simulateError ? true : null);
	if (result.status === 'error') throw result.error;
	return result.data;
}
```

Ignore the generated file in lint/format — append to the exported array in `apps/desktop/eslint.config.js`:

```js
	{ ignores: ['src/lib/ipc/bindings.ts', 'src/lib/ipc/gen/'] }
```

and run:

```bash
printf 'src/lib/ipc/bindings.ts\nsrc/lib/ipc/gen/\n' >> apps/desktop/.prettierignore
```

Skip Step 3B; continue at Step 4.

- [ ] **Step 3B (Branch B, only if the spike failed): ts-rs generated types + manual wrapper**

Remove the spike deps and the `#[specta::specta]` attribute:

```bash
cd apps/desktop/src-tauri
cargo remove specta specta-typescript tauri-specta
```

Set the core dependency to `features = ["ts"]` instead of `["specta"]`, and add ts-rs to the desktop crate: `cargo add ts-rs@10`. Add `ts_rs::TS` derive + `#[ts(export)]` to `IpcError` in `commands.rs` (derive list becomes `#[derive(Debug, Clone, serde::Serialize, thiserror::Error, ts_rs::TS)]` with `#[ts(export)]` on the line after `#[serde(...)]`).

`lib.rs` (Branch B version):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenServ desktop — Tauri entry point (manual typed IPC; specta spike rejected).

mod commands;

pub fn run() {
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![commands::core_info])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("fatal: tauri failed to run: {e}");
        std::process::exit(1);
    }
}
```

Generate the TS types (ts-rs exports during `cargo test`):

```bash
cd ../../..   # repo root
TS_RS_EXPORT_DIR=$PWD/apps/desktop/src/lib/ipc/gen cargo test -p openserv-core --features ts
TS_RS_EXPORT_DIR=$PWD/apps/desktop/src/lib/ipc/gen cargo test -p openserv-desktop
ls apps/desktop/src/lib/ipc/gen
```

Expected: `CoreInfo.ts` and `IpcError.ts` exist. Commit them (regeneration is manual; note the drift risk in the commit body).

Create `apps/desktop/src/lib/ipc/index.ts` (Branch B version):

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// The ONLY module allowed to touch Tauri IPC (master plan §5).
import { invoke } from '@tauri-apps/api/core';
import type { CoreInfo } from './gen/CoreInfo';
import type { IpcError } from './gen/IpcError';

export type { CoreInfo, IpcError };

/** Fetch CoreInfo from the Rust core. Throws IpcError on failure. */
export async function coreInfo(simulateError = false): Promise<CoreInfo> {
	try {
		return await invoke<CoreInfo>('core_info', { simulateError: simulateError ? true : null });
	} catch (e) {
		throw e as IpcError;
	}
}
```

Apply the same eslint/prettier ignores shown in Step 3A.

- [ ] **Step 4: Write the failing vitest test (identical for both branches)**

Create `apps/desktop/src/lib/ipc/ipc.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => invokeMock(...args)
}));

import { coreInfo } from './index';

const sample = {
	appVersion: '0.1.0',
	os: 'macos',
	arch: 'aarch64',
	openservHome: '/Users/x/.openserv'
};

describe('coreInfo', () => {
	beforeEach(() => invokeMock.mockReset());

	it('maps success to CoreInfo', async () => {
		invokeMock.mockResolvedValueOnce(sample);
		await expect(coreInfo()).resolves.toEqual(sample);
		expect(invokeMock).toHaveBeenCalledWith('core_info', { simulateError: null });
	});

	it('maps failure to a thrown IpcError', async () => {
		invokeMock.mockRejectedValueOnce({ kind: 'simulated' });
		await expect(coreInfo(true)).rejects.toEqual({ kind: 'simulated' });
		expect(invokeMock).toHaveBeenCalledWith('core_info', { simulateError: true });
	});
});
```

(If Branch A's generated binding wraps `invoke` with extra arguments, relax the two `toHaveBeenCalledWith` assertions to check only the first argument — the behavioral assertions on resolve/reject are the point.)

Ensure scripts exist: `npm pkg set scripts.test="vitest run"` (keep sv's `test:unit` if present).

Run: `pnpm -C apps/desktop test`
Expected: PASS (2 tests). If it fails, fix `index.ts` — the test defines the contract.

- [ ] **Step 5: The demo page with a visible error path**

Overwrite `apps/desktop/src/routes/+page.svelte`:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { coreInfo, type CoreInfo, type IpcError } from '$lib/ipc';

	let info = $state<CoreInfo | null>(null);
	let error = $state<IpcError | null>(null);
	let loading = $state(false);

	async function load(simulate = false) {
		loading = true;
		error = null;
		try {
			info = await coreInfo(simulate);
		} catch (e) {
			info = null;
			error = e as IpcError;
		} finally {
			loading = false;
		}
	}
</script>

<main class="mx-auto max-w-xl p-8 font-sans">
	<h1 class="text-2xl font-semibold">OpenServ — dev shell</h1>
	<p class="mt-1 text-sm opacity-70">Phase 0 slice: one typed IPC command.</p>

	<div class="mt-6 flex gap-3">
		<button
			class="rounded bg-emerald-700 px-4 py-2 text-white disabled:opacity-50"
			onclick={() => load(false)}
			disabled={loading}
			data-testid="load-btn"
		>
			{loading ? 'Loading…' : 'Load core info'}
		</button>
		{#if import.meta.env.DEV}
			<button
				class="rounded border border-red-600 px-4 py-2 text-red-600 disabled:opacity-50"
				onclick={() => load(true)}
				disabled={loading}
				data-testid="simulate-btn"
			>
				Simulate failure (dev)
			</button>
		{/if}
	</div>

	{#if error}
		<div
			class="mt-6 rounded border border-red-400 bg-red-50 p-4 text-red-800"
			role="alert"
			data-testid="error-banner"
		>
			<strong class="block">Command failed ({error.kind})</strong>
			<span>{'message' in error ? error.message : 'Simulated failure (dev only)'}</span>
		</div>
	{:else if info}
		<dl class="mt-6 grid grid-cols-2 gap-2 rounded border p-4" data-testid="core-info">
			<dt class="font-medium">App version</dt>
			<dd>{info.appVersion}</dd>
			<dt class="font-medium">OS</dt>
			<dd>{info.os}</dd>
			<dt class="font-medium">Arch</dt>
			<dd>{info.arch}</dd>
			<dt class="font-medium">OpenServ home</dt>
			<dd class="break-all">{info.openservHome}</dd>
		</dl>
	{/if}
</main>
```

Color note: `emerald-700` and the red banner are interim Tailwind approximations of the brand's Evergreen accent and `state-failed` semantic color (brand guidelines §4). The real token system (`apps/desktop/src/lib/tokens.css`, guidelines §7.1) arrives with Phase 1 UI — deliberately no custom hexes in this slice, and never blue (competitor territory, guidelines §1.2).

- [ ] **Step 6: Full verification (automated + manual)**

```bash
cargo build --workspace && cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check licenses
pnpm -C apps/desktop test && pnpm -C apps/desktop check && pnpm -C apps/desktop lint
pnpm -C apps/desktop build
pnpm -C apps/desktop tauri dev
```

Manual, in the opened window: click **Load core info** → the definition list shows app version 0.1.0, your OS/arch, and a home path ending in `.openserv` (set `OPENSERV_HOME=/tmp/osv-test pnpm -C apps/desktop tauri dev` to see the override reflected). Click **Simulate failure (dev)** → the red banner appears with kind `simulated`. Ctrl+C when done.

- [ ] **Step 7: Record the spike outcome in the spec and commit**

Append one line to the end of §7.3 in `docs/superpowers/specs/2026-07-21-phase0-kickoff-design.md`, either:
`- **Outcome (fill in date):** tauri-specta spike ACCEPTED; bindings generated on dev runs.` or
`- **Outcome (fill in date):** spike REJECTED (<one-line reason>); ts-rs fallback in effect.`

```bash
git add -A
git commit -s -m "feat: typed core_info IPC with visible error path (spike outcome recorded)"
```

(Split into two commits if you prefer: seam first, UI second — both DCO-signed.)

---

### Task 6: Tiered CI, license gate, SPDX check, PR template

**Files:**
- Create: `.github/workflows/ci.yml`, `.github/PULL_REQUEST_TEMPLATE.md`, `scripts/check-spdx.sh`

**Interfaces:**
- Consumes: everything the branch built (workspace, pnpm app, deny.toml).
- Produces: job names `quick` and `matrix (<os>)` (Task 7's merge gate), artifact names `bundles-macos-14` / `bundles-windows-latest`.

- [ ] **Step 1: SPDX gate script**

Create `scripts/check-spdx.sh`:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Fail if any tracked source file is missing the SPDX header (spec §6 invariant).
set -euo pipefail

offenders=()
while IFS= read -r f; do
  case "$f" in
    apps/desktop/src/lib/ipc/bindings.ts|apps/desktop/src/lib/ipc/gen/*|apps/desktop/src-tauri/gen/*) continue ;;
  esac
  if ! head -n 3 "$f" | grep -q 'SPDX-License-Identifier: GPL-3.0-or-later'; then
    offenders+=("$f")
  fi
done < <(git ls-files '*.rs' '*.ts' '*.js' '*.svelte' '*.css' '*.sh' '.github/workflows/*.yml')

if [ "${#offenders[@]}" -gt 0 ]; then
  printf 'missing SPDX header:\n'
  printf '  %s\n' "${offenders[@]}"
  exit 1
fi
echo "SPDX headers OK"
```

Run: `chmod +x scripts/check-spdx.sh && bash scripts/check-spdx.sh`
Expected: `SPDX headers OK`. If it lists offenders, add the headers (that is the gate working).

- [ ] **Step 2: PR template**

Create `.github/PULL_REQUEST_TEMPLATE.md`:

```markdown
## Summary

<!-- What & why; link the spec/plan section this implements -->

## Platform test checklist (master plan §5)

- [ ] macOS (manual or CI matrix)
- [ ] Windows (manual or CI matrix)
- [ ] n/a — docs/CI-only change

## Gates

- [ ] `quick` green (fmt · clippy -D warnings · tests · license gate · SPDX)
- [ ] `matrix` green (macOS + Windows build & bundle)
- [ ] Security-sensitive paths touched? → security-auditor APPROVE linked
```

- [ ] **Step 3: Resolve action SHAs (supply-chain pinning)**

```bash
for r in actions/checkout@v4 actions/upload-artifact@v4 dtolnay/rust-toolchain@stable \
         pnpm/action-setup@v4 actions/setup-node@v4 Swatinem/rust-cache@v2 \
         EmbarkStudios/cargo-deny-action@v2; do
  repo=${r%@*}; ref=${r#*@}
  echo "$r -> $(gh api "repos/$repo/commits/$ref" --jq .sha)"
done
```

Record each SHA. In the workflow below, replace every `@vN`/`@stable` with the resolved 40-char SHA and keep the human-readable tag as a trailing comment.

- [ ] **Step 4: The workflow**

Create `.github/workflows/ci.yml` (then apply the SHA pins from Step 3):

```yaml
# SPDX-License-Identifier: GPL-3.0-or-later
name: CI

on:
  push:
  pull_request:
    branches: [main]
  workflow_dispatch:

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  quick:
    name: quick
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4 # PIN-TO-SHA
      - name: Install Tauri system deps
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
            libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
      - uses: dtolnay/rust-toolchain@stable # PIN-TO-SHA (respects rust-toolchain.toml)
      - uses: Swatinem/rust-cache@v2 # PIN-TO-SHA
      - uses: pnpm/action-setup@v4 # PIN-TO-SHA (reads packageManager field)
        with:
          package_json_file: apps/desktop/package.json
      - uses: actions/setup-node@v4 # PIN-TO-SHA
        with:
          node-version-file: apps/desktop/.nvmrc
          cache: pnpm
          cache-dependency-path: apps/desktop/pnpm-lock.yaml
      - run: pnpm -C apps/desktop install --frozen-lockfile
      - name: Rust gates
        run: |
          cargo fmt --check
          cargo clippy --workspace --all-targets -- -D warnings
          cargo test --workspace
      - name: Guard - openserv-core must not depend on tauri
        run: |
          if cargo tree -p openserv-core -e normal | grep -qi tauri; then
            echo "::error::openserv-core depends on tauri"; exit 1
          fi
      - name: License gate - cargo-deny
        uses: EmbarkStudios/cargo-deny-action@v2 # PIN-TO-SHA
        with:
          command: check licenses
      - name: License gate - npm
        run: >
          pnpm -C apps/desktop dlx license-checker-rseidelsohn --production
          --excludePrivatePackages
          --onlyAllow "MIT;Apache-2.0;ISC;BSD-2-Clause;BSD-3-Clause;0BSD;Zlib;MPL-2.0;Unlicense;CC0-1.0"
      - name: License gate - SPDX headers
        run: bash scripts/check-spdx.sh
      - name: Frontend gates
        run: |
          pnpm -C apps/desktop lint
          pnpm -C apps/desktop check
          pnpm -C apps/desktop test
          pnpm -C apps/desktop build

  matrix:
    name: matrix (${{ matrix.os }})
    if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch'
    strategy:
      fail-fast: false
      matrix:
        os: [macos-14, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4 # PIN-TO-SHA
      - uses: dtolnay/rust-toolchain@stable # PIN-TO-SHA
      - uses: Swatinem/rust-cache@v2 # PIN-TO-SHA
      - uses: pnpm/action-setup@v4 # PIN-TO-SHA
        with:
          package_json_file: apps/desktop/package.json
      - uses: actions/setup-node@v4 # PIN-TO-SHA
        with:
          node-version-file: apps/desktop/.nvmrc
          cache: pnpm
          cache-dependency-path: apps/desktop/pnpm-lock.yaml
      - run: pnpm -C apps/desktop install --frozen-lockfile
      - run: cargo build --workspace
      - run: cargo test --workspace
      - name: Build unsigned bundles
        run: pnpm -C apps/desktop tauri build
      - uses: actions/upload-artifact@v4 # PIN-TO-SHA
        with:
          name: bundles-${{ matrix.os }}
          path: target/release/bundle/
          if-no-files-found: error
```

- [ ] **Step 5: Validate and commit**

```bash
grep -n 'PIN-TO-SHA' .github/workflows/ci.yml   # every hit must now show a 40-char SHA before the comment
brew install actionlint 2>/dev/null || true
actionlint .github/workflows/ci.yml              # expected: no output
bash scripts/check-spdx.sh                       # expected: SPDX headers OK
git add -A
git commit -s -m "ci: tiered workflow (ubuntu quick + macOS/Windows matrix), license gate, SPDX check, PR template"
```

---

### Task 7: PR, matrix verification, merge

**Files:** none new — this task exercises the pipeline.

**Interfaces:**
- Consumes: job/artifact names from Task 6; the branch `feat/p0-scaffold`.
- Produces: merged `main` satisfying spec §4 exit criteria.

- [ ] **Step 1: Push and confirm the quick job**

```bash
git push -u origin feat/p0-scaffold
gh run list --branch feat/p0-scaffold --limit 3
gh run watch
```

Expected: `quick` runs on the push and goes green (matrix does not run on push — by design).

- [ ] **Step 2: Open the PR**

```bash
gh pr create \
  --title "feat: phase 0 kickoff — workspace scaffold + Tauri/SvelteKit shell (P0-1, P0-2)" \
  --body "$(cat <<'EOF'
Implements docs/superpowers/specs/2026-07-21-phase0-kickoff-design.md:
Cargo workspace mirroring the §6.2 ownership map (stub crates, openservctl),
openserv-core resolve_home + CoreInfo, Tauri 2 + SvelteKit SPA shell,
typed core_info IPC with a visible error path, tiered CI with the
GPL-3.0-or-later license gate (cargo-deny + npm allowlist + SPDX headers).

## Platform test checklist (master plan §5)
- [x] macOS — manual `tauri dev` smoke (core info renders; simulated failure shows banner)
- [x] Windows — CI matrix build + bundle (manual artifact smoke to follow, per spec §4.4)

## Gates
- [x] quick green
- [ ] matrix green (runs on this PR)
- [x] Security-sensitive paths touched? → none in this slice
EOF
)"
```

- [ ] **Step 3: Watch the matrix; fix forward if red**

Run: `gh pr checks --watch`
Expected: `quick`, `matrix (macos-14)`, `matrix (windows-latest)` all green (first Windows run may take 15–20 min cold).

Troubleshooting the likely failures:
- **apt package not found** → the webkit package name moved; search `apt-cache search webkit2gtk` output in the log and adjust the install line.
- **cargo-deny / npm license gate red** → a transitive dep's license isn't allowlisted. Verify GPLv3-compatibility (gnu.org/licenses/license-list); if compatible, add to `deny.toml` or the `--onlyAllow` list with a comment; if not compatible, replace the dependency. Never delete the gate.
- **Windows bundling fails on MSI/WiX** → restrict targets: in `tauri.conf.json` set `"targets": ["nsis", "dmg", "app"]` and re-push.
- Every fix: `git commit -s` with a conventional message, push, watch again.

- [ ] **Step 4: Verify artifacts exist**

```bash
gh run download --name bundles-windows-latest --dir /tmp/openserv-bundles-win $(gh run list --branch feat/p0-scaffold --workflow CI --limit 1 --json databaseId --jq '.[0].databaseId')
ls -R /tmp/openserv-bundles-win | head -20
```

Expected: an NSIS `.exe` under `nsis/` (this is the Windows manual-smoke installer for later). macOS artifact contains `.dmg`/`.app`.

- [ ] **Step 5: Merge and verify main**

```bash
gh pr merge --squash --delete-branch
git switch main && git pull
cargo build --workspace && cargo test --workspace
pnpm -C apps/desktop tauri dev   # final manual smoke on main; Ctrl+C after
```

Expected: everything green on a fresh `main`; the squash commit body retains the DCO `Signed-off-by` trailers from the constituent commits.

- [ ] **Step 6: Exit-criteria check (spec §4)**

- [ ] §4.1 — agents + CLAUDE.md + COPYING + master plan in repo (Task 1)
- [ ] §4.2 — quick + matrix green incl. license gate (Tasks 6–7)
- [ ] §4.3 — button → CoreInfo rendered; simulated error → banner (Task 5 Step 6)
- [ ] §4.4 — macOS manual smoke done; Windows smoke via downloaded artifact whenever convenient

Post-slice follow-up (deliberately OUTSIDE this plan, per spec §8): enable branch protection on `main` requiring `quick` and both `matrix (...)` checks.

---

## Exit-criteria traceability

| Spec requirement | Task |
|---|---|
| Bootstrap files verbatim + COPYING (§2, §4.1) | 1 |
| Workspace = ownership map, stub crates, openservctl, deny.toml (§6) | 2 |
| `resolve_home` + `OPENSERV_HOME` override + CoreInfo in core (§7.1) | 3 |
| SPA shell, strict TS, Tailwind, bundle id, workspace membership (§7) | 4 |
| Typed IPC seam, spike + fallback, error path UI, vitest (§7.1–7.3, §9) | 5 |
| Tiered CI, license gate, SPDX, core-no-tauri guard, PR template (§8) | 6 |
| Matrix-gated merge, artifacts, manual smoke (§4, §5) | 7 |

