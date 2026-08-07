<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Plan — PHP discovery, packaged first (slice 5B)

Spec: `docs/superpowers/specs/2026-08-07-p2-php-discovery-design.md`. Read it first; it is the
contract and this file does not restate it.

Branch `feat/p2-php-discovery`, worktree `.claude/worktrees/php-discover`. **Rebase onto main
once 5A merges** before T1 starts — the branch is currently based on 5A's tip for its catalogue.

Two tasks. Each one compiles and tests green at its end; neither leaves a half-added field.

## T1 — `openvhost-core`: the source, the packaged pass, the merge

Everything inside `crates/openvhost-core/src/php/`, plus every construction site of `PhpRuntime`
within the crate, so the crate builds at the end of this task.

1. `PhpRuntimeSource { Packaged { version }, Homebrew }` on `PhpRuntime`. Model it on
   `NginxRuntimeSource`/`MysqlRuntimeSource` — do not invent a third shape.
2. A packaged pass that walks `packages/php/*/` and resolves **each** series, per spec §5:
   through `PackagesRoot`'s facade, direct-child check kept, **concrete version path recorded,
   never `current`**.
3. Merge packaged-first per spec §4, mirroring `mysql/discover.rs:356-363`. **Do not touch the
   two documented brew preferences** in `discover_php_in` — the packaged pass goes in front of
   them. If you find yourself editing that loop, stop and report instead.
4. The packaged arm takes the version from the tree and **spawns nothing**; only `Homebrew`
   probes.
5. `Discovery`'s unidentified contract per spec §6 — a packaged tree that cannot be identified
   is reported, not dropped.

**Prove, and report against each:**

- **Vacuity, per test group.** Break the thing under test, show the test fails, restore it. A
  group whose failure you did not witness does not count as covered.
- **The `current` swap.** Resolve a runtime, then repoint `current` at a different version and
  show the recorded path still names the original. This is spec §8.1 and it is the one that
  caught a real misdiagnosis in the MySQL slice.
- **No spawn on the packaged arm.** Not by reading the code — make it observable. A fixture
  binary that fails the test if executed is the shape used before.
- **Same-major collision** (spec §8.3): packaged wins, brew's entry is *dropped*, not appended.
  Assert the length as well as the contents.
- **Brew preferences intact** (§8.4): the existing tests for prefix ordering and alias-vs-versioned
  must still pass **unmodified**. If one needs editing to stay green, that is a finding — report
  it, do not edit it.
- **Exhaustiveness:** no wildcard arms on the new enum. Add a throwaway third variant, record
  how many sites fail to compile, remove it, and report the count.
- **Filesystem semantics:** case-insensitive volume, and a symlinked version directory. The
  symlink case is a **known open gap** shared with nginx/MySQL/MariaDB — if it defeats the
  direct-child check here too, that is the expected answer. Report it; do not fix it here.

## T2 — the app seam and the gates

1. Wire it at `stack.rs:810` / `:877`. Follow what 4B did at the same seam.
2. Every call site that gains an argument or field gets a value that **preserves what that test
   already proved** — no test's meaning may change to accommodate the field.
3. Full gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `pnpm -C apps/desktop test` and `check`.
4. **Spec §8.6 is the headline:** on a machine with no package tree, nothing changes. Say how
   you established that, not that you believe it.

## Binding on both tasks

- Report **against each proof obligation above by name**, including the ones that came out
  negative. A silent omission reads as a pass and will be treated as a finding.
- Reentrancy and lifecycle: discovery runs on rescan as well as startup.
- **No sub-agents.** Do the work yourself and report conclusions.
- Mutation experiments go in a **disposable worktree** and are removed afterwards; never leave a
  weakened check on disk, and never `git add -A` — stage by explicit path.
- **No browser automation of any kind.**
- Do not kill a process you did not start. Do not touch a datadir, a credential row, or
  `<home>/logs/` on any path, including error paths.
- If the task turns out to need a design decision the spec does not make, stop and report rather
  than choosing.
