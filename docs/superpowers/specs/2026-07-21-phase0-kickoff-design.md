# Phase 0 Kickoff — Bootstrap + Scaffold + Shell — Design

> **Rename note (2026-07-21):** the product was renamed **OpenServ → OpenVHost** (master plan v1.2, brand guidelines v1.1) while this slice was executing. This document is kept as the historical design record; all `openserv*` identifiers below (crates, env var, bundle id, CLI name `openservctl`) were renamed to their `openvhost*` equivalents (CLI: `openvhost`) by the rename commits on `feat/p0-scaffold`.

- **Date:** 2026-07-21
- **Status:** Approved in brainstorming session; updated same day for master plan v1.1 (license decided: GPL-3.0-or-later); pending user review
- **Source of truth:** `docs/OPENSERV_MASTER_PLAN.md` v1.1 (committed ahead of bootstrap when the v1.1 update arrived mid-review; superseded by `docs/OPENVHOST_MASTER_PLAN.md` v1.2 — see rename note above). This spec covers master-plan tasks **P0-1** and **P0-2** plus project bootstrap. It changes no master-plan decision.

## 1. Context

The repository holds only documentation so far (README, this spec, and the committed master plan v1.1). The master plan is complete and treated as approved. This is the first buildable slice; P0-3…P0-9 get their own specs later, written against the contracts this slice freezes (workspace layout, IPC seam, CI shape).

## 2. Goals

1. **Bootstrap:** the 8 subagent definitions in `.claude/agents/` and the repo `CLAUDE.md`, both extracted verbatim from master plan §6.3/§6.4 (v1.1 — includes the license golden rule and the updated ci-release-engineer), plus `COPYING` (GPL-3.0 full text) at the repo root per plan §1.3. The master plan itself is already committed at `docs/OPENSERV_MASTER_PLAN.md` (the exact path `CLAUDE.md` references).
2. **P0-1:** monorepo scaffold (Cargo workspace + pnpm app) and CI skeleton, green on both target OSes.
3. **P0-2:** Tauri 2 + SvelteKit shell with one typed IPC command whose result — and error path — render in the UI.

## 3. Non-goals

Process supervisor (P0-3), platform PHP proofs (P0-4/P0-5), download pipeline (P0-6), Tera templates (P0-7), orphan cleanup (P0-8), E2E harness (P0-9), Tauri events, tray menu, shadcn-svelte, signing/notarization, auto-update, i18n, branch protection, `openserv/manifests` work, `TRADEMARK.md` (blocked on plan OQ#8 brand check), the in-app "Open Source Licenses" page (needs real UI, Phase 1+), CI enforcement of DCO sign-offs (plan OQ#1 — needed before the first external PR), brand-asset production — final logomark, app/tray icons, `tokens.css`, font bundle (see `docs/OPENSERV_BRAND_GUIDELINES.md` §9; lands with Phase 1 UI).

## 4. Exit criteria

1. Agent files + `CLAUDE.md` + `COPYING` + `docs/OPENSERV_MASTER_PLAN.md` exist with plan-verbatim content.
2. `cargo build --workspace`, `cargo test --workspace`, and the frontend build are green — including the license gate — in the ubuntu quick job **and** the macOS + Windows matrix.
3. Launching the app and clicking the demo button renders `CoreInfo`; the simulated-error path renders the visible error state.
4. Manual smoke test passes on the developer's Mac. Windows smoke uses the CI-built artifact when convenient (the master plan's real-machines gate applies to Phase 0 exit, not this slice).

## 5. Delivery flow

0. *(Done 2026-07-21, when v1.1 arrived mid-review)* master plan committed to `docs/OPENSERV_MASTER_PLAN.md`.
1. **Bootstrap commit, direct to main** — agent configs, `CLAUDE.md`, and `COPYING` only; no code and no workflow file (CI ships with the scaffold PR, so nothing runs against this commit) — so `CLAUDE.md` and the ownership map exist before the first code PR is opened or delegated.
2. **One `feat:` PR** (branch `feat/p0-scaffold`) with scaffold + app — every new source file carrying its SPDX header — Conventional Commits, DCO-signed, merged only with the full matrix green.

## 6. Repo layout

Paths mirror the master-plan §6.2 ownership map exactly — the layout is the delegation map.

```
open-serv/
├── Cargo.toml                # workspace, resolver 2; members: crates/*, apps/cli, apps/desktop/src-tauri
├── rust-toolchain.toml       # pinned stable (current at implementation time)
├── COPYING                   # GPL-3.0 full text (plan §1.3)
├── deny.toml                 # cargo-deny config: GPLv3-compatibility license allowlist
├── .gitignore
├── crates/
│   ├── openserv-core/        # stub: crate-level doc from plan §3.1 + resolve_home() + tests
│   ├── openserv-proc/        # stub: responsibility doc, placeholder API, one trivial test
│   ├── openserv-pkg/         # stub: same shape
│   └── openserv-conf/        # stub: same shape
├── apps/
│   ├── cli/                  # openservctl stub bin: prints version, exits 0
│   └── desktop/              # Tauri 2 + SvelteKit app (§7 below)
├── templates/README.md       # stub naming owner: config-template-engineer
├── tests/README.md           # stub naming owner: qa-test-engineer
├── packaging/README.md       # stub naming owner: ci-release-engineer
├── docs/
│   ├── OPENSERV_MASTER_PLAN.md
│   └── superpowers/specs/    # this document
├── .claude/agents/           # 8 agent .md files, verbatim from plan §6.3
├── CLAUDE.md                 # verbatim from plan §6.4
└── .github/workflows/ci.yml
```

**Mechanically enforced invariants from day one:**

- `openserv-core` never depends on tauri — CI guard fails if `cargo tree -p openserv-core` mentions tauri.
- Stub crates carry **zero external dependencies** until their owning slice adds real work.
- Every dependency passes the license gate (`cargo-deny` licenses check + an npm license checker) — GPL-incompatible or unknown licenses are rejected (plan §1.3) — and every source file carries `SPDX-License-Identifier: GPL-3.0-or-later` (comment syntax per language).

**Naming:** repo directory stays `open-serv`; product name is OpenServ; crates are `openserv-*`. No rename.

## 7. Desktop app

- SvelteKit in SPA mode: `adapter-static`, SSR off (standard Tauri arrangement).
- Svelte 5, TypeScript strict, Tailwind CSS wired now; shadcn-svelte deferred to Phase 1.
- ESLint + Prettier gate from the first commit.
- The window is a plain dev surface — no design language in this slice.

### 7.1 The one command

`core_info(simulate_error: Option<bool>) → Result<CoreInfo, IpcError>`

- `CoreInfo { app_version, os, arch, openserv_home }` is **constructed by `openserv-core`**, not by the command handler — the handler stays thin per plan conventions.
- `openserv_home` comes from `openserv_core::resolve_home()`, which honors an `OPENSERV_HOME` env override (the hermetic-test hook the future harness relies on) and defaults per plan §3.2 (`~/.openserv` / `%USERPROFILE%\.openserv`).
- `simulate_error` is a dev-only flag so the error pipe is demonstrable and testable; it is not a product feature (how it is neutralized in release builds — compiled out or ignored — is an implementation-plan detail).
- This single call proves every load-bearing seam: UI → typed IPC → thin command → tauri-free core crate → typed struct → rendered.

### 7.2 Error handling

- `IpcError` is a small serde-serializable enum (not a bare string), establishing the command error pattern for all future commands.
- The demo page renders success as a result card and failure as a **distinct, visible error banner** — the plan's "Failed is never silent" frontend rule, instantiated on day one.

### 7.3 Typed IPC seam

- **Spike (time-boxed ≤ half a day):** tauri-specta v2 against current Tauri 2.x. Success = TS types + command wrapper generate and compile in CI alongside Svelte 5 strict TS.
- **Pre-decided fallback:** `ts-rs` derives the TS types; a ~30-line hand-written typed `invoke` wrapper provides the call surface.
- Either way, **all IPC flows through `src/lib/ipc/`** — call sites are identical under both outcomes, so a later swap touches one module. No raw `invoke("string")` anywhere in the frontend, ever.
- **Outcome (2026-07-21):** tauri-specta spike ACCEPTED; bindings generated on dev runs.

## 8. CI

One workflow, two tiers, in `.github/workflows/ci.yml` (repo is private; macOS minutes bill at 10×, Windows at 2×):

| Job | Runner | Triggers | Steps |
|---|---|---|---|
| `quick` | ubuntu-latest | every push + every PR | webkit2gtk system deps · `cargo fmt --check` · `cargo clippy --workspace -- -D warnings` · `cargo test --workspace` · core-no-tauri guard · eslint · `svelte-check` · vitest · frontend build · license gate (`cargo-deny check licenses` + npm license check + SPDX-header check) |
| `matrix` | macos-14 + windows-latest | PRs to main + `workflow_dispatch` | full workspace build + test · `tauri build` · upload **unsigned bundles as artifacts** |

- Ubuntu installs webkit2gtk deps (~90s at 1× cost) so clippy/tests cover the whole workspace including the Tauri crate; the matrix answers only "does it build and bundle on the real targets."
- Matrix artifacts double as the Windows smoke-test installer — no local Windows toolchain required.
- Cost hygiene: `concurrency` group cancels superseded runs; rust-cache + pnpm store caching.
- When the repo goes public (standard runners become free), widen matrix triggers; revisit then.
- **Follow-up (not this slice):** branch protection requiring both jobs — one `gh` command once the workflow exists.

## 9. Testing

- One trivial unit test per stub crate — proves the workspace-wide test harness.
- Real unit tests for `resolve_home()`: default path per OS and the `OPENSERV_HOME` override.
- One vitest test of the IPC wrapper with a mocked `invoke`: asserts the success mapping and the error mapping.
- E2E is P0-9's slice. Coverage tooling starts when there is real logic to cover; this slice has effectively one function.
- Manual verification per exit criterion 4.

## 10. Carried defaults (flagged, not silently assumed)

| Default | Note |
|---|---|
| pnpm, pinned via corepack `packageManager` field | Master plan's commands assume pnpm |
| Node: current LTS, pinned | `.nvmrc` + `engines` |
| Rust: stable pinned in `rust-toolchain.toml`; edition per current stable | Plan says "edition 2021+" |
| Conventional Commits, **DCO-signed** (`git commit -s`), from now on | Plan §5; formal CI enforcement of DCO deferred to plan OQ#1 gate (before first external PR) |
| Bundle identifier provisionally `dev.openserv.desktop` | Revisit at plan OQ#8 name check, before signing ever matters |
| **License: GPL-3.0-or-later** ✅ | Decided in plan §1.3 (supersedes the deferral in this spec's first version). `COPYING` lands in the bootstrap commit; SPDX headers on all new source files; license gate in CI |

## 11. Risks & mitigations

| Risk | Mitigation |
|---|---|
| tauri-specta (🟡) fails the spike or fights Svelte 5 | Pre-decided ts-rs fallback; single-module IPC seam keeps a later swap cheap |
| Private-repo Actions minutes exhausted | Tiered CI; matrix only on PRs/dispatch; concurrency cancellation |
| Windows regressions invisible to a Mac-based developer | Matrix on every PR to main; downloadable bundle artifacts for manual smoke |
| Stub crates drift into speculative APIs | Zero-dependency rule; stubs carry docs + placeholder only until their slice starts |
| Version drift vs. the plan's mid-2026 knowledge | Implementation plan starts with a "verify current versions" step (Tauri 2.x minor, Svelte 5, sqlx not yet needed) per plan §2 caveat |
| License-gate friction: transitive deps with exotic or unknown licenses | Curate the `deny.toml` allowlist explicitly (MIT, Apache-2.0, BSD, ISC, Zlib, and other GPLv3-compatible licenses); unknowns block until reviewed — never silently allowed |

## 12. After this slice

Next specs, in dependency order: P0-3 (`openserv-proc` v0 — needs both platform specialists' consultation), then P0-6 (download/verify/extract), P0-4/P0-5 (platform PHP proofs), P0-7 (templates), P0-8 (orphan cleanup), P0-9 (E2E harness). Each is written against the contracts frozen here.
