# Package uninstall — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A PHP major or a MySQL major can be uninstalled from the app. The service row disappears everywhere. The user's databases, credentials and logs survive — and the dialog says so before anything happens.

**Architecture:** see the spec (`docs/superpowers/specs/2026-07-31-p1-pkg-uninstall-design.md`) — **READ IT FIRST for every task; D1–D8 are binding.**

**Tech Stack:** Rust (`openvhost-proc` supervisor, desktop commands) + SvelteKit for the two page actions.

## Global Constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By` trailer**.
- No `unwrap`/`expect` outside tests. No `unsafe`. `openvhost-core` gains no tauri dependency.
- **The datadir at `<home>/data/mysql/<major>/`, the stored credentials, and the per-major log directories are never written or removed on ANY path, including error paths.** This is the plan's second non-negotiable principle and the reason this slice exists.
- No wildcard match arms over `SupervisorEvent`, `ServiceState`, or the new package/refusal enums.
- TDD with vacuity proof: every group RED first or neuter-proven; **state the method per group**. Be most suspicious of the "kept" assertions — a test that only checks the operation returned `Ok` passes against an implementation that deleted the datadir. Assert content and inode, not a `Result`.
- **Standing self-check**, reported item by item: vacuity proof; filesystem semantics (symlinks, case-insensitive volumes, permissions, what happens when a path is already gone); reentrancy and lifecycle (uninstall racing an install, racing a service start, the app quitting mid-uninstall); exhaustiveness; and the seams between tasks.
- Gates per task: focused tests → `cargo test --workspace` → `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` (+ `pnpm -C apps/desktop test` and `check` when the frontend is touched). Known pre-existing `openvhost-conf` flake (`settings::check::tests::a_non_zero_validator_exit_…`, a 5 s subprocess timeout under load): rerun in isolation if it trips and say so; do not fix it. Also pre-existing and not yours: `pnpm lint`'s prettier failure on `QuitDialog.svelte`.

---

## Task 1: `Supervisor::unregister` and `SupervisorEvent::Unregistered`

**Files:** `crates/openvhost-proc/src/{events,supervisor}.rs` (+ tests), every exhaustive consumer of `SupervisorEvent`.

Per D4. This is the mirror of the `Registered` event the tray slice added, and it has the same hazard: a new variant must not fall into a wildcard anywhere.

```rust
pub fn unregister(&self, id: &str) -> Result<(), ProcError>;   // terminal states only
// SupervisorEvent gains: Unregistered { id: String }
```

`unregister` refuses unless the service is `Stopped` or `Failed`, removes the entry under the **same entries mutex** `register`/`start`/`stop` use, and emits `Unregistered` exactly once. Refusing on a live service is what keeps the orphan registry honest — we must never forget a child we are still supervising.

- [ ] **Step 1:** Find **every** exhaustive match on `SupervisorEvent` — Rust, the generated bindings, and the frontend store — and **list them in your report before changing anything**. The tray slice did this for `Registered`; the same consumers apply, plus the control handler and tray added since.
- [ ] **Step 2:** Tests RED first: `unregister` refuses `Running` and `Starting` naming the state; succeeds on `Stopped` and `Failed`; the entry is gone from `snapshot()`; `Unregistered` is emitted exactly once; unregistering an unknown id is a typed error, not a silent success. **Prove exhaustiveness by construction** — adding a `ServiceState` variant must fail to compile in the refusal predicate.
- [ ] **Step 3:** Implement; update every consumer found in Step 1, including the frontend store (a service that vanishes must leave the Services page and the tray without a restart) and regenerate bindings.
- [ ] **Step 4:** `cargo test --workspace` + `pnpm -C apps/desktop test` green; fmt/clippy clean.
- [ ] **Step 5:** Commit: `feat(proc): let the supervisor forget a service that is gone`

## Task 2: the uninstall operation

**Files:** `apps/desktop/src-tauri/src/commands.rs` (or a new sibling module — `commands.rs` is already ~7,700 lines, so a new module is preferred), plus whatever `openvhost-core` needs for the path inventory.

Per D1, D2, D3, D5.

**The inventory is data, not scattered `if`s.** Model what a package kind removes and keeps as a value, so the confirmation text and the executor read from one source and cannot disagree. A new package kind must fail to compile rather than silently removing nothing.

- **Refusals first, before any process is spawned** (D3): the service is not in a terminal state → refuse naming the service and its state; sites are pinned to this PHP major → refuse naming them. No `--force`.
- **Then `brew uninstall <formula>`** through the **existing `InstallLock`** and the same live-output surface `install_php` uses — do not fork either. No `--ignore-dependencies`; if brew refuses, surface its message verbatim.
- **Then cleanup:** remove the generated pool config and unregister the supervisor row. **Never touch** `<home>/data/mysql/<major>/`, the credential rows, or `<home>/logs/`.
- **Rescan converges (D5):** `rescan_php_runtimes` gains an unregister step for majors that vanished, so an in-app uninstall and an external `brew uninstall` leave the same observable state. This also fixes the pre-existing stale-row bug.

- [ ] **Step 1:** Tests RED first — the inventory asserted exhaustively per kind; each refusal predicate over every `ServiceState`; a refusal spawns **no** process (assert with a recording fake, and give it a positive control so "nothing happened" cannot pass vacuously); brew failure is surfaced verbatim and changes no local state.
- [ ] **Step 2:** Real-filesystem tests in a tempdir — after a successful uninstall the pool config is gone **and** the datadir, credential row and log directory are **byte- and inode-identical**. Do the same after a **failed** brew run and after a refusal. This is the highest-value test in the slice.
- [ ] **Step 3:** `cargo test --workspace`; fmt/clippy. Commit: `feat(desktop): uninstall a PHP or MySQL version without touching its data`

## Task 3: the two page actions and the confirmation

**Files:** `apps/desktop/src/routes/languages/+page.svelte`, `apps/desktop/src/routes/databases/+page.svelte`, shared derive/component code, IPC wiring.

Per D6. An **Uninstall** action per installed major on each page. The confirmation names what is removed **and what survives**, in the user's words — a generic "are you sure" trains people to click through, and naming the survivors is the only place the user learns their data is safe.

Follow the established patterns: the live-output surface from `install_php`, the native dialog usage from the tray and the CLI-install action, and the existing disabled/busy states so an uninstall cannot be fired twice or during an install.

- [ ] **Step 1:** Tests RED first — the action is disabled while an install or another uninstall is in flight; a refusal renders the obstacle and names it (the site list, or the service state); the confirmation text contains the kept-paths sentence; a service that disappears leaves the list without a page reload.
- [ ] **Step 2:** Implement. `cargo test --workspace` + `pnpm -C apps/desktop test` + `check`; fmt/clippy.
- [ ] **Step 3:** Commit: `feat(ui): uninstall an installed PHP or MySQL version`

---

## Phase C — gate, PR, merge

- [ ] **Step 1:** Full gates: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && pnpm -C apps/desktop test && pnpm -C apps/desktop build`
- [ ] **Step 2:** Whole-branch review **and security-auditor** — this touches child processes, file paths under `<home>`, and the credential store. Claims to verify are in the spec's Security posture section.
- [ ] **Step 3:** **Live proof.** The MySQL round trip is the one that matters: initialize a datadir, create a table, insert a row, uninstall the engine, reinstall it, and **read the row back**. Also: uninstall a PHP major and confirm brew agrees it is gone, the Services row disappears without a restart, and the log directory survives. Paste real terminal output. **Use a hermetic `OPENVHOST_HOME`** and do not uninstall anything from the owner's real brew installation without saying so — prefer a major the owner does not have, or state exactly what was touched and restored.
- [ ] **Step 4:** One fix wave; re-run gates.
- [ ] **Step 5:** PR with the spec's click-list. Squash-merge on green.
- [ ] **Step 6:** Record dispatch count and wall clock in the ledger.
