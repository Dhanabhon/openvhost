<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Plan — uninstalling a packaged PHP (slice 5D)

Spec: `docs/superpowers/specs/2026-08-08-p2-php-uninstall-design.md`. Read it first; it is the
contract and this file does not restate it.

Branch `feat/p2-php-uninstall`, worktree `.claude/worktrees/php-uninstall`, based on `c4b0732`.

**Run `pnpm install --offline --frozen-lockfile` in `apps/desktop` before the first desktop gate.**
A fresh worktree has no `node_modules` and the failure reads as a missing package.

Two tasks. Each compiles and tests green at its end.

## T1 — The plan: runtime state in, purity kept

`apps/desktop/src-tauri/src/uninstall/mod.rs` and whatever it needs from core.

1. Thread the packaged runtime's resolved state into `inventory`/`build_plan` **as a parameter**,
   mirroring `keg: Option<&KegProvenance>`. **`inventory` must still stat nothing** (spec D1).
2. `Target::formula()` returns `None` for a packaged-only major (D2), so it routes to
   `PackageTree` the way MariaDB does and never plans a `brew uninstall` for a formula that is not
   installed.
3. `Removal::PackageTree`'s doc comment asserts its `path` comes from compile-time constants.
   **Rewrite it** — that stops being true here.
4. D3: with both a packaged and a Homebrew 8.4, the plan removes the packaged tree and lists the
   keg under `keeps` — "The Homebrew PHP 8.4 keg — untouched".
5. The recorded path is the **concrete version directory**, never through `current` (D4).

**Prove, and report each by name:**

- **Purity.** `inventory` stats nothing — establish it, do not assert it. The same inputs must
  yield the same value twice, and the dialog's plan and the executor's must be one value.
- **A brew-only major's inventory is unchanged, byte-for-byte**, against today's.
- **A packaged-only major produces no `BrewFormula` step.**
- **The both-installed case** lists the keg in `keeps` and the packaged tree in `removes`.
- Vacuity per test group: break it, watch it fail, restore it.
- Exhaustiveness: no wildcard arms; add a throwaway variant, report the count.

## T2 — The executor: a destructive call that validates its own target

`apps/desktop/src-tauri/src/uninstall/run.rs`.

1. Execute `Removal::PackageTree` for PHP.
2. **Confine before removing** (spec D4's decision): canonicalise the target and the packages root,
   require the target to be under the root, refuse otherwise. This is deliberately *not* the
   lexical direct-child check discovery uses — that check is known broken and its replacement is
   filed separately.
3. `current` is handled: gone, or repointed, never left dangling at a removed tree.

**Prove, and report each by name:**

- **`remove_dir_all` cannot escape**, proven by construction with **both** shapes the security
  audit reproduced live: a symlinked **version** directory and a symlinked **series** directory.
  Build a tree where the old lexical check passes and the target resolves outside; show the guard
  refuses. A guard whose failure you never witnessed does not count.
- **Nothing outside the packages root is removed** in any of those cases — assert the outside
  directory still exists afterwards, not merely that the call returned an error.
- **The brew path is untouched** — every existing uninstall test passes **unmodified**. If one
  needs editing to stay green, that is a finding, not a chore.
- Logs, pool overrides and every site's saved PHP version survive, as the brew path already
  promises.
- Vacuity per test group.

## Binding on both tasks

- Report **against each proof obligation by name**, including any that come out negative or that
  you could not do. A silent omission reads as a pass and will be treated as a finding.
- **No sub-agents.** Report conclusions, not transcripts.
- Mutation experiments in a **disposable worktree**, removed afterwards. Never leave a weakened
  check on disk. Stage by explicit path; never `git add -A`.
- **This slice deletes directories.** Every test that exercises removal works inside a
  `tempfile::TempDir`. Never touch the user's real `~/.openvhost`, its `state.db`, `logs/`, any
  datadir or credential row, on any path including error paths. Treat `/opt/openvhost-build` as
  read-only.
- **No browser automation of any kind.** Do not kill a process you did not start.
- Conventional Commits, `git commit -s`, message via `git commit -F <file>` — a bare `-n` in a
  commit message reads as `--no-verify` here.
- Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop check`.
  **Never pipe a gate through `tail`** — the exit code you read becomes `tail`'s. Redirect to a
  file and read `$?`.
- Known pre-existing flakes, not yours: `mysql_ipc_tests::reset_redacts_…` and
  `settings::check::tests::a_non_zero_validator_exit_…`, both bounded-timeout tests under load.
- If the task needs a design decision the spec does not make, **stop and report** rather than
  choosing. On each of the last three slices an implementer refused a spec item and was right —
  one prevented a successful install rendering as a killed brew, one caught the spec removing a
  working button, one found a `kind === 'brew'` check hiding eight unrendered states.
