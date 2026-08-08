<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Plan — the first screen tells you what a site actually needs

Spec: `docs/superpowers/specs/2026-08-08-p1-first-run-readiness-design.md`. Read it first; it is
the contract and this file does not restate it.

Branch `feat/p1-first-run`, worktree `.claude/worktrees/first-run`, based on `d7b00a2`.

**Run `pnpm install --offline --frozen-lockfile` in `apps/desktop` first** — a fresh worktree has
no `node_modules` and the failure reads as a missing package.

**One task.** The slice is frontend-only: `WebServerDto.binary_path` and `source` are already on
the wire (`commands.rs:1204`), so nothing in Rust needs to change. If you find yourself editing
`src-tauri/`, stop and report — that means the premise is wrong.

## The work

1. A derive module for the rule — a sibling of `php-install.derive.ts` and `php-default.derive.ts`,
   which is where this codebase puts decisions that a component would otherwise make inline. The
   page should ask it, not compute it.
2. Read the web-server list on the Sites page the way it already reads `phpEnvironment()`, keeping
   the same three-state discipline: **not looked yet / looked and absent / read failed**.
3. Replace the `no-php-banner` with the readiness banner (spec D1) — one banner, naming everything
   missing, a link per remedy. With only PHP missing it must render as it does today.

## Prove, and report each by name

- **Spec §7.1** — no nginx, PHP installed: the banner appears and names nginx. This is the state
  that renders **nothing** today, so it is the one that justifies the slice.
- **§7.2** — no PHP, nginx installed: reads as today. Name any existing assertion you had to touch.
- **§7.3** — neither: **one** banner, not two.
- **§7.4** — both installed: no banner. Every developed machine today, including this one.
- **§7.5** — before either read returns: nothing, and no flash. Extend the `phpEnvKnown` discipline
  rather than inventing a second one.
- **§7.6** — a failed read must not become a claim of absence, on either side. There is an `I2` fix
  in `+page.svelte`'s comments explaining why this distinction exists; do not undo it.
- **Vacuity per group**: break it, watch it fail, restore it. In particular, prove the nginx half
  can fail — a test that passes whether or not nginx is checked is the defect this slice exists to
  remove, reproduced in a test.
- **Exhaustiveness**: if you introduce a state type, no wildcard arms; add a throwaway variant and
  report the count.

## Binding

- Report **against each obligation by name**, including any that come out negative. A silent
  omission reads as a pass and will be treated as a finding.
- **No sub-agents.** Report conclusions, not transcripts.
- Mutation experiments in a **disposable worktree with an isolated `CARGO_TARGET_DIR`**, removed
  afterwards. A shared target dir silently links a stale crate and **can make a gate falsely
  green**.
- **Never pipe a gate through `tail`** — the exit code you read becomes `tail`'s.
- Stage by explicit path; never `git add -A`.
- **No browser automation of any kind.** Do not kill a process you did not start.
- **Set `OPENVHOST_HOME` to a scratch dir** when running the suite — one existing test provisions
  the real home when it is unset (`stack.rs:~1115`, filed separately). Never touch the user's real
  `~/.openvhost`.
- Conventional Commits, `git commit -s`, message via `git commit -F <file>` — a bare `-n` reads as
  `--no-verify` here.
- Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop check`,
  `pnpm -C apps/desktop lint`.
- Known flakes, not yours: `mysql_ipc_tests::reset_redacts_…`,
  `settings::check::tests::a_non_zero_validator_exit_…`, and two in
  `apps/cli/tests/two_process.rs`.
- If the task needs a design decision the spec does not make, **stop and report** rather than
  choosing. On each of the last five slices an implementer refused a spec item and was right —
  including twice where the spec item was mine and wrong.
