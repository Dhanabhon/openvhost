# MariaDB on the Databases page (slice B) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** a MariaDB group on the Databases page that installs the pinned 11.4.9 from the
published release, initializes, starts/stops, shows and resets the root credential, and
uninstalls without touching the data — **with MySQL's rows rendering identically to before.**

**Architecture:** the spec (`docs/superpowers/specs/2026-08-05-p1-mariadb-ui-design.md`) —
**READ IT FIRST; D1–D7 are binding.** Its §2 table was measured against `a79a80f`; do not
re-derive those numbers, and do not contradict them without measuring again.

**Precondition, already satisfied when you start:** the GitHub release `mariadb-11.4.9`
exists and `catalogue.rs` says `Availability::Published`. If it does not, **stop and report**
— task 1 flips that constant and task 3's proof depends on it.

## Global Constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By`**.
- No `unwrap`/`expect` outside tests. No `unsafe`. **No wildcard match arms over any of the
  unions this slice touches** — `PackageKind`, `InstallKind`, `EngineKind`, the row-state
  union, the offer union. A new variant must fail to compile, not be silently skipped.
  Prove it: add a throwaway variant, show the build breaks, remove it.
- `openvhost-core` gains no tauri dependency. All child processes through `openvhost-proc`.
  All file writes atomic (`atomicfile.rs`); never `std::fs::write`.
- **The credential never reaches argv or env.** stdin or an ephemeral 0600 defaults-file.
  `RootPassword` keeps its redacting `Debug` and gains no `Serialize`. **Reveal and Copy stay
  separate — Copy must never un-mask the on-screen field.** That invariant has already been
  fixed once as a review finding; it is the reason D1 generalizes the row instead of copying
  it, so re-introducing a second copy of it defeats the slice's own design.
- **Never touch a datadir, a credential row, or `<home>/logs/` on any error path.** For
  "untouched", assert **content and inode**.
- After changing a `query!`/`query_as!` or a migration: regenerate `.sqlx/` per CLAUDE.md.
- **This worktree may be shared.** Mutation experiments in a disposable detached worktree.
  Stage by explicit path; never `git add -A`. Never leave a mutation on disk when you stop.
- Gates per task: `cargo test --workspace` **and** `pnpm -C apps/desktop test` → `cargo fmt
  --check && cargo clippy --workspace --all-targets -- -D warnings`. **Gate the commit on the
  test result** (`&&`, not two statements). A commit hook here rejects a command line
  containing `-n` near `git commit` (it reads as `--no-verify`).
- **Three known flakes, none in code this slice touches. Name them, do not fix them here:**
  `a_force_quit_leftover_socket_still_lets_status_and_list_answer` (CLI),
  `an_unchanged_stop_is_a_success` (CLI, `two_process.rs:304`),
  `a_non_zero_validator_exit_is_a_rejection_on_the_named_field` (conf).

---

## Task 1: the Rust surface — discriminators, events, commands

**Files:** `apps/desktop/src-tauri/src/commands.rs`, `.../uninstall/{mod,run}.rs`,
`.../mariadb_pkg.rs` (new, mirroring `mysql_pkg.rs`), `.../lib.rs`.

Per D3, D4, D5, D7.

Three enums gain a variant, and each one breaks the build somewhere useful: `InstallKind`,
`InstallKindDto` (wire, reaching the quit dialog), `PackageKind`.

**`MARIADB_INSTALL_RUN` and `MARIADB_INIT_RUN` are not optional.** `commands.rs:2199-2206`
records audit finding F1 — a `cancel_mysql_install` aborted a datadir *init* because both
runs were tagged `(Mysql, Install)` and differed only in a `label` that `abort_running_if`
deliberately does not consult. A MariaDB install sharing MySQL's pair would let **Cancel on
one engine kill the other's install**. Write the test that proves it cannot.

Three new event channels (D3): `mariadb-install-log-event`,
`mariadb-install-progress-event`, `mariadb-init-log-event`, registered in `collect_events!`.
Do **not** add a `kind` field to MySQL's payloads.

Eight commands (D7), none taking a series argument. Flip `catalogue.rs`'s `availability` to
`Availability::Published` and **verify the URL actually serves the pinned bytes** before
claiming it.

- [ ] **Step 1:** Tests RED first — a MariaDB install and a MySQL install cannot abort each
      other (both directions); the three event channels are registered and distinct; each of
      the three enums fails to compile with a wildcard arm removed and a variant added; the
      credential never appears in any argv the commands build.
- [ ] **Step 2:** Implement. Gates. Commit: `feat(desktop): the MariaDB command surface`

## Task 2: generalize the row — behaviour-preserving

**Files:** `apps/desktop/src/lib/components/{MysqlRow,MysqlCredentials}.svelte` → shared
engine row + credentials, `apps/desktop/src/lib/*.derive.ts`.

Per D1, D2.

**This task adds no MariaDB.** It ends with MySQL rendering byte-identically and every one of
the 53 `MysqlRow` + 21 `MysqlCredentials` tests green **unmodified**. If a test needs
editing, that is a behaviour change and a finding to report — not a fix to absorb.

Introduce `EngineKind` (closed) and an engine **descriptor** resolved once in a pure derive
function with a `const _: never` arm: `{label, idPrefix, defaultPort, portConflictHint,
datadirDisclosure, sourcePolicy, uninstallPolicy}`. Set MySQL's `idPrefix` to `'mysql'` so
existing test ids are unchanged. **No `{#if engine === …}` in any template** (D1) — the
template paints the descriptor, it does not decide from it.

`sourcePolicy`/`uninstallPolicy` carry real weight: `mysqlUninstallOffered` returns `false`
for a `packaged` source, so a shared row that inherited it would render
`PACKAGED_UNINSTALL_UNAVAILABLE` on every installed MariaDB row.

Add the ninth row state `awaitingRelease { tag }` (D2) and move the Homebrew sentence out of
the shared `unavailableBody` into MySQL's descriptor — there is no brew fallback for MariaDB.

`brewFormula` must admit absence (D5): `string | null`, and every caller handles it. Delete
the dead `DatabasesStore.brewFound` while here.

- [ ] **Step 1:** Tests RED first — the descriptor resolver is exhaustive (a new `EngineKind`
      fails to compile); `awaitingRelease` renders its own copy and **no install control**;
      `unavailable` and `awaitingRelease` produce visibly different text; a `packaged` source
      still offers Uninstall.
- [ ] **Step 2:** Implement. Gates, **plus the full existing databases suite unmodified**.
      Commit: `refactor(desktop): make the database row engine-agnostic`

## Task 3: the MariaDB group, and the live proof

**Files:** `apps/desktop/src/lib/mariadb.svelte.ts` (new), `mariadb.listeners.ts` (new),
`routes/databases/+page.svelte`, `lib/components/DatabasesEmpty.svelte`.

Per D1, D6.

`MariadbStore` holds **scalars, not dictionaries** — one series, no key. Do not port
`DatabasesStore`'s ten per-major maps; the backend refuses a series argument and a key here
would invent a namespace that cannot exist.

A parallel `subscribeMariadbEvents` (D6), not a wider signature on the existing one; the page
manages two disposers.

A second `<section>` group on the page under a "MariaDB" heading — the existing page comment
already anticipates exactly this ("becomes a new group here rather than a redesign").

- [ ] **Step 1:** Tests RED first — install → initialize → start → stop drives the right
      commands in the right order; a MariaDB install-log line never lands in MySQL's store
      and vice versa; Copy does not set the revealed flag; uninstall's confirm names the
      datadir that survives.
- [ ] **Step 2:** Implement. Gates. Commit: `feat(desktop): install and run MariaDB from the Databases page`

---

## Phase C — gate, PR, merge

- [ ] **Step 1:** Full gates, both languages.
- [ ] **Step 2:** Whole-branch review **and security-auditor**. The auditor's surface: a new
      IPC command family, a credential crossing the IPC boundary to a UI, a download+verify
      path reaching the network for the first time from a button, and an uninstall that must
      not take data with it. **Keep the audit brief narrow** — scope, not difficulty, is what
      wedges agents here.
- [ ] **Step 3:** **Live proof**, spec §10, all seven points, real output pasted. Point 1 is
      the one that has never run before: a real download from the real release, SHA-256
      verified.
- [ ] **Step 4:** One fix wave; re-run gates. PR; squash-merge on green.

## Recorded before it bites

- **The row refactor is the risk in this slice, not MariaDB.** It touches the one component
  a user sees for an engine they already depend on. Its gate is that MySQL's tests pass
  **unmodified** — the moment someone edits one to make it pass, the refactor has stopped
  being behaviour-preserving and nobody will notice later.
- **`brewFormula` returning `null` will ripple.** That ripple is the type system reporting a
  real gap, not a cost to route around with an empty string.
- **Two engines can now hold the install lock.** Every cancel path, the quit dialog's pending
  list, and the uninstall dialog all now have two possible occupants. F1 happened once with
  one engine.
- **`awaitingRelease` does not die when the release is published** — pin → build → publish is
  this project's own workflow, so every future version bump passes through it.
- The live proof needs the release to exist. If it does not, the download path is unprovable
  and the slice is not ready to merge, however green the unit tests are.
