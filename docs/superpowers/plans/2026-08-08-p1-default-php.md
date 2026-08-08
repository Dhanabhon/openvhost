<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Plan — the default PHP is chosen, not inherited

Spec: `docs/superpowers/specs/2026-08-08-p1-default-php-design.md`. Read it first; it is the
contract and this file does not restate it.

Branch `feat/p1-default-php`, worktree `.claude/worktrees/php-default`, based on `707ebe8`.

**Run `pnpm install --offline --frozen-lockfile` in `apps/desktop` before the first desktop gate.**
A fresh worktree has no `node_modules` and the failure reads as a missing package.

Two tasks. Each compiles and tests green at its end.

## T1 — Core: the stored preference and its resolution

1. Migration `0006_php_settings.sql`, mirroring `0002_web_server_settings.sql` — singleton
   (`CHECK (id = 1)`), `STRICT`, `updated_at`. **After changing any `query!`/`query_as!` or adding
   a migration, regenerate `.sqlx/` and commit it** — see CLAUDE.md's exact recipe; builds and CI
   run offline against that cache.
2. A repository for it, following `SiteRepository`/the settings repo already in the crate.
3. **Resolution is a separate step from storage** (spec D2). The preference is a stored major; what
   the catch-all gets is a *resolved* runtime, and an unresolvable preference is a **named state**,
   not `None`. Model it so a caller cannot accidentally treat "no preference" and "preference names
   something not installed" as the same thing.
4. `render_set` (`site/apply/mod.rs:164`) consumes the resolution instead of `.first()`.

**Prove, and report each by name:**

- **Spec claim 2 is the hard one.** With no preference, the generated default-site config is
  **byte-identical** to today's on the same inputs. Establish it by comparing generated output
  against `origin/main`, not by reasoning about the code path.
- A preference naming an installed major changes the generated config to that major's socket.
- A preference naming a **not-installed** major is reported as such and still yields a servable
  config — no panic, no empty upstream.
- Vacuity per test group: break it, watch it fail, restore it.
- Exhaustiveness: no wildcard arms on the new state; add a throwaway variant, report the count.
- **Say whether the apply pipeline's "settings touch only the main config" invariant fires.** There
  is a comment at `site/apply/mod.rs:~160` claiming it, with a test that "fails loudly". The new
  input is not part of `WebServerSettings`, so it may not fire at all — check and report which,
  rather than assuming either way.

## T2 — Desktop: setting it, and the seam

1. Thread the preference through the command surface and the Languages page (spec D6).
2. **Setting a default validates at save and then opens the diff**, mirroring
   `web-server/+page.svelte`'s `onSave` — `if (await settings.save()) applyDialogOpen = true;`
   (spec claim 6). Two validations, not one: shape and installed-membership at save, then the
   diff for the config rewrite. It is not a side-door write.

   *(This item was "corrected" mid-slice to drop the diff, on a reading of the Rust half of
   `save_web_server_settings` alone. Both gates caught it: the other half is on the page, and its
   own comment says skipping it leaves "a Save button that visibly does nothing on the page the
   user is actually on." The original stands.)*
3. Uninstalling the default major must leave the state legible (spec claim 4).

**Prove, and report each by name:**

- Setting a default produces a **diff preview** before it lands, and a rollback on validation
  failure.
- **Nothing changes on a machine with no preference** — every real machine today. Say how you
  established it.
- The not-installed case renders something a user can act on, not a blank.
- Whether the control appears at all when only one major is installed — **your call, report it**
  (spec D6 leaves it to you deliberately).
- Vacuity per group, and **name any existing test whose expectation you had to change**. Extending
  a fixture with a new required field preserves its meaning; **changing what it asserts is a
  finding**, because this slice claims to change nothing until a default is set.

## Binding on both tasks

- Report **against each proof obligation by name**, including any that come out negative or that
  you could not do. A silent omission reads as a pass and will be treated as a finding.
- **No sub-agents.** Report conclusions, not transcripts.
- Mutation experiments in a **disposable worktree with an isolated `CARGO_TARGET_DIR`**, removed
  afterwards. A shared target directory silently links a stale `openvhost-core` and **can make a
  gate falsely green** — two gates hit it last slice.
- **Never pipe a gate through `tail`** — the exit code you read becomes `tail`'s. Four separate
  agents hit that trap last slice, including me.
- Stage by explicit path; never `git add -A`.
- **No browser automation of any kind.** Do not kill a process you did not start.
- Never touch the user's real `~/.openvhost`, its `state.db`, `logs/`, any datadir or credential
  row, on any path including error paths. Note that **one existing unit test provisions the real
  home when `OPENVHOST_HOME` is unset** (`stack.rs:~1115`) — filed separately; set
  `OPENVHOST_HOME` when you run the suite so you do not add to it.
- Treat `/opt/openvhost-build` as strictly read-only.
- Conventional Commits, `git commit -s`, message via `git commit -F <file>` — a bare `-n` reads as
  `--no-verify` here.
- Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop check`.
- Known flakes, not yours: `mysql_ipc_tests::reset_redacts_…`,
  `settings::check::tests::a_non_zero_validator_exit_…`, and
  `apps/cli/tests/two_process.rs::an_unreachable_socket_with_a_live_app_is_still_a_failure_for_status`.
- If the task needs a design decision the spec does not make, **stop and report** rather than
  choosing. On each of the last four slices an implementer refused a spec item and was right.
