# MariaDB as a running service (slice A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a packaged MariaDB 11.4 that starts under the Supervisor, serves SQL, stops cleanly and survives a restart — **while MySQL 8.4 runs alongside it, untouched.**

**Architecture:** see the spec (`docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md`) — **READ IT FIRST; D1–D6 are binding.** Its §2 table was measured against the real 11.4.9 artifact on 2026-08-04; do not re-derive those facts, and do not contradict them without measuring again.

**Scope boundary, stated so it is not discovered halfway:** slice A ends at a supervised service. **No UI, no IPC command, no `PackageKind`/`InstallKind` variant** — that is slice B, and it breaks the build in seven places by design. If a task needs a command or a UI prop, that is a finding to report, not scope to absorb.

## Global Constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By`**.
- No `unwrap`/`expect` outside tests. No `unsafe`. No wildcard match arms over new enums — a new variant must fail to compile, not be silently skipped. `openvhost-core` gains no tauri dependency.
- All file writes atomic (`atomicfile.rs`); never `std::fs::write`.
- **The password never reaches argv or env.** stdin, or an ephemeral 0600 defaults-file. `RootPassword` keeps its redacting `Debug` and gains no `Serialize`.
- **Never touch a datadir, a credential row, or `<home>/logs/` on any error path.** For "untouched" assertions, assert **content and inode** — a delete-and-recreate with identical bytes passes a content-only check, and this project has proven that.
- After changing a `query!`/`query_as!` or a migration: regenerate `.sqlx/` per CLAUDE.md and commit it.
- **This worktree may be shared.** Mutation experiments in a disposable detached worktree, never here. Stage by explicit path; never `git add -A`. Never leave a mutation on disk when you stop.
- Gates per task: `cargo test --workspace` → `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`. **Gate the commit on the test result** — `cargo test --workspace && git commit`, not two statements on one line. A commit hook here rejects a command line containing `-n` near `git commit` (it reads as `--no-verify`).
- Two known flakes, both in crates this slice does not touch: `a_force_quit_leftover_socket_still_lets_status_and_list_answer` (CLI) and `a_non_zero_validator_exit_is_a_rejection_on_the_named_field` (conf). Re-run in isolation, **name them**, say so, do not fix them here.
- The artifact is at `build/out/mariadb-11.4.9-macos-arm64.tar.gz`, sha256 `76ea96a4…`, matching the catalogue pin. Install it into a hermetic `OPENVHOST_HOME` under **`/tmp`, not `$TMPDIR`** — the 103-byte `sun_path` ceiling has bitten twice, most recently at 159 bytes.

---

## Task 1: paths, datadir classification, discovery

**Files:** `crates/openvhost-core/src/mariadb/{paths,datadir,discover}.rs`.

Per D1, D5.

**This task carries the data-loss risk and comes first for that reason.**

`classify_datadir` for MariaDB requires **both** `mysql/` (dir) and `mariadb_upgrade_info` (file). MySQL's rule is `mysql/` + `auto.cnf`, and MariaDB never writes the second. **Do not reuse `SENTINEL_FILE`.** *Corrected 2026-08-04:* an earlier draft said reusing it would call a populated datadir *uninitialized* and `--initialize` over the user's databases. It would not — `mysql/datadir.rs` has a catch-all that yields `Foreign`, so the real cost is every good MariaDB datadir permanently unusable behind an honest refusal. Bad, and not data loss; see the spec §2 for the full correction and for the half-state that IS reachable through a direct-bootstrap init.

`mariadb_upgrade_info` holds the version (`11.4.9-MariaDB`). A datadir whose recorded series disagrees with the one being started is **`Foreign`, not `Initialized`** — that is a migration, and this slice does not migrate.

Reuse in place, do not fork: `sweep_stale_staging` (`mysql/datadir.rs:278`) and the socket-length guard (`mysql/datadir.rs:70`, which already delegates to the php-fpm one). **Moving the misfiled generic helpers out of `mysql/` is not this slice's job** — a rename diff in front of the security gate helps nobody.

Discovery needs its own copy: `mariadbd` / `mariadb` / `mariadb-admin`, **all three or nothing**. Copy exactly — do not re-derive — the discipline at `mysql/discover.rs:241-253`: resolve `current` to a **concrete version directory** and record that path. Spawning *through* the symlink lets a `current` swap silently change which engine a restart brings up, and it cost a full misdiagnosis in the MySQL slice.

- [ ] **Step 1:** Tests RED first — an empty dir is `Empty`; a dir with only `mysql/` is not `Initialized`; a dir with only `mariadb_upgrade_info` is not `Initialized`; a real initialized dir **is** `Initialized`; a dir recording another series is `Foreign`. Prove the last two against a datadir initialized from the real artifact, not a fixture. Discovery: all three binaries present resolves, any one missing does not; the resolved path is a concrete version dir, **not** `current` — and prove a `current` swap does not change it.
- [ ] **Step 2:** Implement. Gates. Commit: `feat(core): MariaDB paths, datadir classification and runtime discovery`

## Task 2: the generated config — for both engines — and initialization

**Files:** `crates/openvhost-conf/templates/mariadb/my.cnf.tera`, `crates/openvhost-conf/src/mariadb.rs`, `crates/openvhost-conf/templates/mysql/my.cnf.tera`, `crates/openvhost-core/src/mariadb/init.rs`, `crates/openvhost-core/src/mariadb/repo.rs`, a new migration.

Per D3, D4, D6.

**Both templates gain `basedir`, `plugin_dir`, `character-sets-dir` and `lc_messages_dir`.** All four are absent from MySQL's today. This is the runtime half of the build-pipeline BLOCK arriving from the other direction: without them a server resolves those paths out of its compiled-in prefix, which is exactly how the first MariaDB artifact came to resolve `plugin_dir` out of a mode-1777 tree. **Touching MySQL's template means re-running `crates/openvhost-core/tests/mysql_live.rs`** — that cost is accepted, not a reason to skip it.

MariaDB gets **port 3307** (D2), likewise a literal. `mysqlx=OFF` must **not** appear — MariaDB rejects it.

Initialization keeps the two hard-won containments:

- **`--no-defaults` during init**, so the user's `!includedir` drop-ins cannot steer a server whose root is still open;
- **the temp server never goes through the Supervisor**, spawned directly with a manual process-group kill guard.

Two things must be **established live before the temp server starts for the first time**, not assumed:

1. **MariaDB 11.4's fresh root uses `unix_socket` auth.** `--auth-root-authentication-method=normal` is the lever; today's measurement used it. Verify what actually sets a password, and that it took.
2. **`--mysqlx=OFF` does not exist here.** For MySQL it is load-bearing — without it the temp server binds a mode-0777 `/tmp/mysqlx.sock` while root is open. **Find out whether MariaDB exposes an equivalent surface.** "There is none" is an acceptable answer; assuming it is not. Report what you established and how.

Credentials: table `mariadb_instances(major TEXT PRIMARY KEY, root_password TEXT NOT NULL, initialized_at INTEGER NOT NULL) STRICT`, a concrete repo beside `MysqlInstanceRepo`. Reuse `RootPassword`/`generate_root_password` in place.

- [ ] **Step 1:** Tests RED first — the rendered config contains all four directories and they point inside the package tree; MySQL's rendering gains them too and its existing assertions still hold; a failed init leaves **no** partial datadir and **never** touches `<home>/data/` (content + inode); the credential round-trips; `state.db` and its sidecars are 0600 after the write.
- [ ] **Step 2:** Implement; regenerate `.sqlx/`. Gates **plus `mysql_live.rs`**. Commit: `feat(core): pin runtime directories in both engines' configs, and initialize MariaDB`

## Task 3: supervision, and the live proof

**Files:** `apps/desktop/src-tauri/src/stack.rs`.

Per D2, D5.

A `mariadb_spec` beside `mysql_spec` (`stack.rs:243-267`): `program` = the discovered `mariadbd` at a **concrete** version path; argv is **exactly** `--defaults-file=<my.cnf>`; `id` = `mariadb-<major>`; `endpoint` = `127.0.0.1:3307`; readiness = `mariadb-admin ping` against the socket; grace at least MySQL's 15 s, which exists for InnoDB flush and which `quit.rs` pins its budget against.

`ensure_custom_confd` before spawn — a real server aborts if `!includedir` names a missing directory.

**Do not weaken the tray's endpoint dedupe** (`tray/model.rs:198-215`). It correctly treats two services sharing an address as alternatives; distinct ports are what makes two engines coexist. `tray/model.rs:440` pins the existing behaviour — it must still pass.

- [ ] **Step 1:** Tests RED first — the spec's argv is exactly one argument; the program path is a concrete version dir; the service id and endpoint are distinct from every MySQL major's; **both engines appear in `bulk_start_ids`** (the regression D2 exists to prevent).
- [ ] **Step 2:** Implement. Gates. Commit: `feat(desktop): supervise MariaDB alongside MySQL`

---

## Phase C — gate, PR, merge

- [ ] **Step 1:** Full gates: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`, plus `mysql_live.rs`.
- [ ] **Step 2:** Whole-branch review **and security-auditor**. The auditor's surface here: a credential written to disk, a temp server running with an open root, `--no-defaults` containment, the four newly pinned directories, and a second engine sharing the supervisor. **Keep the audit brief narrow** — four agents wedged on the last slice and the pattern was scope, not difficulty.
- [ ] **Step 3:** **Live proof**, spec §9, all six points, real output pasted:
      install the real artifact into a hermetic `OPENVHOST_HOME` under `/tmp` → initialize → set a root password → start under the Supervisor → create a table, insert → **restart, read the row back** → stop cleanly, no orphan, registry consistent → **with MySQL 8.4 running throughout**, both reachable, neither datadir touched (content **and** inode) → `mariadbd --verbose --help` names the four pinned directories from the config, not the compiled-in prefix.
- [ ] **Step 4:** One fix wave; re-run gates. PR; squash-merge on green.

## Recorded before it bites

- **The sentinel is the whole ballgame.** Get it wrong in the permissive direction and `--initialize` runs on a populated datadir. Every test that asserts "not initialized" must be proven able to fail.
- **`current` must never be spawned through.** Prove it by swapping the symlink under a running service and showing the process still holds the original path.
- **Publishing is owner-gated and slice A does not need it** — the live proof installs from the local artifact, which matches the pin. **Slice B does need it**: a Databases row whose Install returns `PackageNotPublished` is a broken promise on screen.
- The four pinned directories close the *dependence* on the compiled-in prefix; the build-time refusal closes the *reachability*. Neither replaces the other, and a future reader will be tempted to drop one.
