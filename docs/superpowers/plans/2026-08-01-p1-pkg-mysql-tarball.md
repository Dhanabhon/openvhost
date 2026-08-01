# MySQL from the upstream tarball — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Installing MySQL 8.4 from the Databases page fetches Oracle's own tarball, verifies it, and lands it in our package tree — no Homebrew. The user's existing brew-installed MySQL keeps working throughout.

**Architecture:** see the spec (`docs/superpowers/specs/2026-08-01-p1-pkg-mysql-tarball-design.md`) — **READ IT FIRST; D1–D8 are binding.** Every fact it once left open is now measured; nothing here rests on an assumption.

**Programme:** slice 2 of 7. 0 payload proof ✅ · 1 extractor ✅ (PR #43) · **2 MySQL from tarball** · 3 MariaDB (a *build* target) · 4 nginx (first own build) · 5 PHP · 6 remote manifests · 7 retire brew.

## Global Constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By`**.
- No `unwrap`/`expect` outside tests. No `unsafe`. `openvhost-core` gains no tauri dependency.
- **Never touch a datadir, a credential row, or `<home>/logs/`** on any path including errors. That is the same rule the uninstall slice shipped under, now applying to a different install source.
- No wildcard match arms over new enums.
- TDD with vacuity proof per group; **state the method**. For "kept"/"untouched" assertions, assert **content and inode**, not a `Result` — a delete-and-recreate with identical bytes passes a content-only check, and this project has proven that.
- **This worktree may be shared.** Run mutation experiments in a **disposable detached worktree** (`git worktree add --detach <scratch> <sha>`), never here. Stage by explicit path; never `git add -A`. Never leave a mutation on disk when you stop — a docs commit swept one into a branch tip this week and shipped a deleted security clause.
- Gates per task: `cargo test --workspace` → `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` (+ `pnpm -C apps/desktop test`/`check` when the frontend is touched). **Gate the commit on the test result** — `cargo test && git commit`, not two statements on one line. Known pre-existing `openvhost-conf` timing flake (now diagnosed: it execs a freshly written binary under a 5 s budget and pays macOS's first-exec cost): rerun in isolation, say so, do not fix it here.

---

## Task 1: the catalogue, and installing into our tree

**Files:** `crates/openvhost-core/src/mysql/` (a new catalogue + package-source module), wiring to `openvhost-pkg`.

Per D1, D2, D4.

**`openvhost-pkg` is wired to nothing today. This task is its first production consumer.** It adds no install machinery — if you find yourself needing some, that is a finding to report, not a change to make quietly.

**The catalogue entry is compiled in, not a remote manifest** (the manifest repo is slice 6). Pinned exactly:

```
version  8.4.11
url      https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos15-arm64.tar.gz
sha256   b96e00493bc3499b9ffd7f08d65c5d64933af0383a8287d9873b64f94c2d6009
```

**That hash has a verified provenance and the spec records how** — Oracle's key `BCA43417C3B485DD128EC6D4B7B3B788A8D3785C`, valid to 2027-10-23, fingerprint cross-checked on a second host, `gpg --verify` good, signed bytes hashing to exactly that value. Do not change the entry without redoing that check. The OS tag is **version-coupled** (`macos15`); it cannot be templated from the version.

**Pass `bin/mysqld` as `with_warmup_binary`** so the staged warm-up pre-pays the Gatekeeper scan (809 ms → 16 ms). **Never `bin/mysqld_safe`** — it carries a hardcoded `/usr/local/mysql/data` and really does try to start a server.

**D4 — record the version at install; never probe for it.** We asked for it, so we know it. Probing exists only for discovering Homebrew runtimes we did not install.

- [ ] **Step 1:** Tests RED first — the catalogue entry's shape; target selection (`macos-arm64` vs `macos-x86_64`, and what happens on an unsupported arch); the recorded-version round trip; a failed install leaves no partial tree and **never touches `<home>/data/`** (content + inode).
- [ ] **Step 2:** Implement. `cargo test --workspace`; fmt/clippy. Commit: `feat(core): install MySQL from Oracle's tarball into our package tree`

## Task 2: discovery reads both sources, and the supervisor spawns a concrete path

**Files:** `crates/openvhost-core/src/mysql/discover.rs`, `apps/desktop/src-tauri/src/stack.rs`.

Per D3, D5, D7.

**D3 — ours wins, brew stays.** The owner is running a brew-installed `mysql@8.4 8.4.11` **right now**; stranding them in the first slice of the migration would be self-inflicted. Discovery gains a `packages/`-tree walk and keeps the Homebrew walk as a fallback. Brew discovery retires in slice 7, not here.

**The UI must be able to say where a runtime came from.** During a migration "which mysqld am I actually running" gets asked, and the honest answer needs a field, not a guess.

**D5 — resolve `current` to a concrete version path at spawn time and record that path with the process.** Never spawn *through* the symlink: a `current` swap would then silently change which engine a restart brings up, and it makes `mysqld`'s argv[0]-derived basedir ambiguous — the exact class of thing that cost a full misdiagnosis in the MySQL lifecycle slice.

**D7 — never touch a Homebrew keg.** No uninstall, no relink, no migration. Two install sources coexisting is the intended state during a migration.

- [ ] **Step 1:** Tests RED first — a `packages/` runtime is found and preferred over a brew one for the same major; a brew-only major is still found; each runtime reports its source; the spawn path is a concrete version directory, **not** `current` (assert the recorded path, and prove a `current` swap does not change a running service's binary).
- [ ] **Step 2:** Implement. `cargo test --workspace`; fmt/clippy. Commit: `feat(core): discover packaged MySQL alongside Homebrew's`

## Task 3: the Databases page installs from the tarball

**Files:** `apps/desktop/src-tauri/src/commands.rs` (or a sibling module — `commands.rs` is ~8,200 lines, prefer a sibling), `apps/desktop/src/routes/databases/+page.svelte` and components, IPC wiring.

Install now means **download → verify → extract**, not `brew install`. Reuse the existing live-output surface; the progress the user watches is `Progress::{Started, Verified, Extracted, Linked}` rather than brew's stdout.

**Two things the user must be able to see**, because both were invisible before and both matter during a migration:
- **which source a runtime came from** (packaged vs Homebrew);
- that verification happened — a download that is checked and one that is not should not look identical.

The uninstall slice's `PackageKind`/`Blocker` surface stays as it is; a packaged MySQL is not uninstallable through the brew path, and `openvhost-pkg` has **no uninstall counterpart at all** — that is slice 3's work, not a corner of this one. Make sure the UI does not offer it.

- [ ] **Step 1:** Tests RED first — progress states render distinctly (assert **pairwise**, not non-emptiness); a verification failure is reported as such and not as a generic network error; the source of each runtime is shown; no Uninstall affordance for a packaged runtime.
- [ ] **Step 2:** Implement; regenerate bindings. `cargo test --workspace` + `pnpm -C apps/desktop test` + `check`; fmt/clippy. Commit: `feat(ui): install MySQL from the upstream tarball`

---

## Phase C — gate, PR, merge

- [ ] **Step 1:** Full gates: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && pnpm -C apps/desktop test && pnpm -C apps/desktop build`
- [ ] **Step 2:** Whole-branch review **and security-auditor**. The auditor cleared the extractor and said the provenance precondition binds **this** slice — it is now satisfied, and the spec records how; ask it to confirm the record is sufficient. New surface for it: a network install path reachable from the UI for the first time.
- [ ] **Step 3:** **Live proof.** Install MySQL 8.4 from the real tarball into a hermetic `OPENVHOST_HOME` under `/tmp` (**not** `$TMPDIR` — the 103-byte `sun_path` ceiling), initialize a datadir, create a table, insert a row, restart the app, read it back. Then **confirm the owner's brew-installed `mysql@8.4` is untouched and still works** — `brew list --versions` identical before and after. Paste real output.
- [ ] **Step 4:** One fix wave; re-run gates. PR; squash-merge on green.

## Recorded before it bites

- **No wall clock bounds a download any more** — only a 30 s idle window and `SIZE_CAP`. A server dribbling one byte every 29 s holds the install permit effectively forever. Harmless while `install_package` had no caller; **this slice gives it one.** Close it here or say plainly why not, and give the user a cancel.
- `zip.rs` keeps its own collision policy, correct only because the pinned `zip` crate collapses duplicate raw names. Wants a canary test.
