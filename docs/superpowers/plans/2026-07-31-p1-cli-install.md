# `openvhost` on PATH — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `OpenVHost → Install Command Line Tool…` puts `openvhost` on the user's PATH with no admin prompt, and a fresh terminal can then run `openvhost list`. Closes owner call #2 from PR #40.

**Architecture:** see the spec (`docs/superpowers/specs/2026-07-31-p1-cli-install-design.md`) — **READ IT FIRST for every task; D1–D8 are binding.**

**Tech Stack:** Rust + Tauri v2 (`bundle.externalBin`, the existing app menu in `quit.rs`, `tauri-plugin-dialog` — already wired). No frontend work.

## Global Constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By` trailer**.
- No `unwrap`/`expect` outside tests. No `unsafe`. No new Tauri command and no `capabilities/*.json` change — the menu handler calls the Rust logic directly, exactly as the tray's handlers do. If you believe you need one, STOP and report; that is a design change.
- **Nothing outside the two candidate directories in D2 is ever written.** No privilege escalation, no shell-profile editing, no `sudo`, no `osascript with administrator privileges`.
- No wildcard match arms over the new state enums.
- TDD with vacuity proof: every group shown RED first or neuter-proven; **state the method per group**. Be most suspicious of the refusal tests — assert the existing file is **unchanged**, not merely that a `Result` was `Err`. A test that only checks `is_err()` passes against an implementation that deletes the file and then fails.
- **Standing self-check**, reported item by item: vacuity proof; filesystem semantics (symlinks, case-insensitive volumes, `~` expansion, permissions, atomic rename); reentrancy and lifecycle (running the action twice, running it while a previous one is mid-flight, the app being moved between launch and click); exhaustiveness; and the seams between tasks.
- Gates per task: focused tests → `cargo test --workspace` → `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`. Known pre-existing `openvhost-conf` flake (`settings::check::tests::a_non_zero_validator_exit_…`, a 5 s subprocess timeout under load): rerun in isolation if it trips and say so; do not fix it here.

---

## Task 1: the install logic (pure + filesystem), no UI

**Files:** `apps/desktop/src-tauri/src/clitool/{mod,detect,install}.rs` (new). Registered but not yet reachable from any menu.

**Published interface — Task 3 codes against exactly this:**

```rust
pub enum PathStatus { OnPath, NotOnPath { export_line: String, profile: PathBuf },
                      Unknown { reason: String, export_line: String, profile: PathBuf } }

pub enum InstallState { NotInstalled,
                        Installed { dir: PathBuf, path_status: PathStatus },
                        Broken   { dir: PathBuf, reason: String },
                        Blocked  { dir: PathBuf, what_is_there: String } }

pub enum InstallOutcome { Installed { dir, path_status }, AlreadyInstalled { dir, path_status },
                          Repaired { dir, path_status }, Refused { dir, what_is_there } }

pub fn source_binary() -> Result<PathBuf, CliToolError>;   // current_exe()'s sibling `openvhost`
pub fn candidate_dirs() -> Vec<PathBuf>;                   // D2 order; NEVER /opt/homebrew/bin
pub async fn login_shell_path() -> PathStatusProbe;        // 2 s timeout, three-state
pub fn detect() -> InstallState;
pub async fn install() -> Result<InstallOutcome, CliToolError>;
```

Binding details: source is `current_exe()`'s **parent joined with `openvhost`** — never a hardcoded `/Applications`, never a PATH search (D1). This is also what makes it work in a dev build, where `target/debug/openvhost` sits beside `openvhost-desktop`. Candidates are `/usr/local/bin` then `~/.local/bin` (created 0755 if absent); **`/opt/homebrew/bin` is deliberately excluded — `brew doctor` warns on unbrewed symlinks** (D2). Clobber rules and the temp-symlink-then-`rename` are D3, in full. The login-shell probe is `$SHELL -l -c 'printf %s "$PATH"'` via one-shot `tokio::process::Command` (the existing practice for `nginx -t`/`php -i`), bounded at **2 s**, and its failure is a third state, never a `false` (D4).

- [ ] **Step 1:** Tests RED first — candidate ordering; the clobber decision table over **every** node type (absent, our symlink, foreign symlink, regular file, directory) asserted exhaustively; PATH membership parsing with a trailing colon, an empty element, a duplicate, and a `~`-relative entry; all four `InstallState` variants; the `export` line and profile filename for zsh, bash and an unknown shell.
- [ ] **Step 2:** Real-filesystem tests in a tempdir — install into an empty dir; install twice is idempotent **and does not churn the inode**; a foreign symlink and a regular file are each refused **and left byte-identical**; a dangling symlink classifies `Broken` and repairs; a failure injected between temp-create and rename leaves **no residue**.
- [ ] **Step 3:** `cargo test --workspace`; fmt/clippy. Commit: `feat(desktop): resolve and install the openvhost command line tool`

## Task 2: bundle the binary, and gate the dev fixture out of release

**Files:** `apps/desktop/src-tauri/tauri.conf.json`, a pre-build step (script or `build.rs`), `apps/cli/src/main.rs`, `.gitignore` as needed.

Per D1 and D7. Two independent concerns, one task because both are packaging.

**Bundling:** `bundle.externalBin` so `openvhost` lands in `Contents/MacOS/`. Tauri requires the staged file to carry the target-triple suffix (`openvhost-aarch64-apple-darwin`); build `-p openvhost --release` and stage it. Keep the staging directory out of git. State plainly how a developer reproduces this locally and what a CI job would have to run — **do not invent a CI workflow file** in this slice.

**`__testchild` (D7):** `#[cfg(debug_assertions)]`-gate the interception in `apps/cli/src/main.rs` (~:43-74). **Verify first, do not trust the comment:** nothing in the workspace spawns `openvhost __testchild` — every supervisor test uses `CARGO_BIN_EXE_proc_testchild`. Confirm that, and check whether the debug-only `demo-ticker` registration references it. A test named roughly `the_testchild_fixture_still_runs_and_stays_hidden` exists and will need updating.

- [ ] **Step 1:** Verify and report the `__testchild` / `proc_testchild` situation **before** changing anything.
- [ ] **Step 2:** Tests: the fixture still works in a debug build; a **release** build does not carry it (assert on the release binary, not on a `cfg!` expression — a test that reads `cfg!(debug_assertions)` proves nothing about what shipped).
- [ ] **Step 3:** Verify the bundle actually contains the binary — `cargo tauri build` (or the project's build command) then assert `Contents/MacOS/openvhost` exists and runs. If a full bundle build is not feasible in this environment, say so plainly and state exactly what is unverified rather than implying it was checked.
- [ ] **Step 4:** `cargo test --workspace`; fmt/clippy. Commit: `build(desktop): ship the openvhost CLI inside the app bundle`

## Task 3: the menu item and the dialog

**Files:** `apps/desktop/src-tauri/src/quit.rs` (`app_menu`), a handler beside the existing menu routing, `apps/desktop/src-tauri/src/clitool/mod.rs` (report rendering).

Per D5 and D6. Add **OpenVHost → Install Command Line Tool…** after About with a separator. The label reflects `detect()`: `Install Command Line Tool…` / `Reinstall Command Line Tool…` when `Broken`. Route it through the existing menu-id dispatch — the same shape the tray uses, taking an **id, not a `MenuEvent`**, so it is reachable under `mock_builder`.

Report with `tauri-plugin-dialog` (already wired; see `tray/mod.rs` for the established direct-Rust-API usage). The dialog must state the directory, the D4 PATH verdict, and the `export` line whenever the status is `NotOnPath` **or** `Unknown` — on `Unknown` it must say plainly that the check did not succeed. **Never render "you're all set" on a guess.** A `Refused` outcome names the path and what is there.

- [ ] **Step 1:** Tests RED first under `mock_builder`: the handler dispatches on the menu id and is a no-op for an unknown id; each `InstallOutcome` and each `PathStatus` renders a distinct message; `Unknown` renders the caveat **and** the export line; `Refused` names the occupying path. Render tests should assert on a pure formatting function, not on a dialog.
- [ ] **Step 2:** Implement + wire. `cargo test --workspace`; fmt/clippy. Commit: `feat(desktop): install the command line tool from the app menu`

---

## Phase C — gate, PR, merge

- [ ] **Step 1:** Full gates: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && pnpm -C apps/desktop test && pnpm -C apps/desktop build`
- [ ] **Step 2:** Whole-branch review **and security-auditor — mandatory.** This is **the first thing the app writes outside `<home>`**, into a directory on the user's PATH, where replacing a file is code execution. The claims to verify are listed in the spec's Security posture section.
- [ ] **Step 3:** **Live proof** — run the app, use the menu action, then open a **fresh terminal** and run `openvhost list` with no absolute path. Then move the app and confirm `Broken` is detected and repairable. Then confirm a release build has no `__testchild` while `proc_testchild` still works. Paste real terminal output; a claim without output is not a proof. **Do not install into the owner's real PATH without saying so** — prefer a temp `HOME` where the logic allows, and state exactly what was touched on the real machine and that it was cleaned up.
- [ ] **Step 4:** One fix wave; re-run gates.
- [ ] **Step 5:** PR with the spec's click-list. Squash-merge on green.
- [ ] **Step 6:** Record dispatch count and wall clock in the ledger.
