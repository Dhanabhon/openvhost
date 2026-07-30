# P1 MySQL Lifecycle — Design

- **Date:** 2026-07-29
- **Status:** Approved under the owner's standing delegation (2026-07-29: orchestrator picks the slice and subagents make approvals; per-phase check-ins). Slice chosen as the largest remaining gap in Phase 1's "replace XAMPP" target: nginx ✓ PHP ✓ — no database.
- **Roadmap line:** "MySQL + MariaDB lifecycle: init datadir per major version, start/stop, port config, root password set/reset flow." This slice is **MySQL only**; deliberate narrowings are listed in §Deferred and flagged to the owner.
- **Design process:** high-stakes per CLAUDE.md (credentials + data integrity) — two blind independent designs (deep-reasoner, Codex), synthesized by the orchestrator. Agreements adopted as-is; divergences resolved below with rationale.
- **Plan:** `docs/superpowers/plans/2026-07-29-p1-db-mysql.md`

## What ships (end-to-end demonstrable)

Install `mysql@8.4` via Homebrew with live output → detect binaries → render + validate `my.cnf` → staged datadir init with a generated root password → start under the supervisor → readiness green (socket ping) → connection proof (`SELECT VERSION(), @@port`) → password reveal/copy + reset → clean stop. Surfaced on a new **Databases** page (mirroring Languages) plus a Services-panel row.

## Decisions

### D1 — Formula & detection: `mysql@8.4` only (both designers agreed; verified live)

Catalogue = `["8.4"]`, formula `mysql@8.4` (verified on this machine: 8.4.11, keg-only). The unversioned `mysql` formula tracks 9.x Innovation (9.7.1 today) which EOLs quarterly and would let `brew upgrade` silently move a datadir across majors — rejected by both designers independently. Detection mirrors `php/discover.rs`: walk `BREW_PREFIXES`, accept `mysql` and `mysql@*` dirs, require `bin/mysqld` + `bin/mysql` + `bin/mysqladmin`, probe the major from `mysqld --version` (bounded), same prefix-merge rules. Out-of-catalogue installs (e.g. a user's 9.x) render as rows **without** an Install button — honest display, no support burden. Multi-major *install* detection: in. Multi-major *parallel running*: deferred (fixed port 3306; a second instance fails to bind and renders honestly, exactly like nginx today). Brew invocation mirrors `php/brew.rs` law: absolute brew path, argv no shell, `HOMEBREW_NO_AUTO_UPDATE=1`, formula name composed server-side from the parsed major.

### D2 — Datadir & staged init (Codex's staging adopted over direct-init)

Layout per master plan §3.2, derived server-side from `resolve_home()` + catalogue-checked `MysqlMajor` (that derivation IS this write-path's confinement argument — Docroot lesson, write it in the code):

- datadir `<home>/data/mysql/<major>/` (0700)
- config `<home>/config/generated/mysql/<major>/my.cnf`
- socket `<home>/run/mysql-<major>.sock` (reuse `MAX_SOCKET_PATH_BYTES` guard)
- staging `<home>/data/mysql/.<major>.init-<uuid>/` — same parent as final, so the finishing `rename` is atomic

Init sequence (one orchestrated task, `install_php`-style: `run_task` + held `AbortHandle` + Drop guard):

1. `mysqld --defaults-file=<my.cnf> --initialize-insecure --datadir=<staging>` — no server, no network. `--initialize` (random expired password in the error log) lost in both designs: the log line is not an API, the password is expired-on-first-use so `ALTER USER` is needed anyway, and a parse failure strands an inaccessible datadir.
2. Spawn temp server: `mysqld --defaults-file=<my.cnf> --datadir=<staging> --skip-networking --socket=<home>/run/mysql-<major>-init.sock` — the "insecure window" never has a network listener; the socket lives in the user-only run dir.
3. Poll `mysqladmin --no-defaults --protocol=SOCKET --socket=<init.sock> --user=root --connect-timeout=1 --silent ping` (10 s cap).
4. Set password via `mysql --no-defaults --protocol=SOCKET --socket=<init.sock> --user=root` with `ALTER USER 'root'@'localhost' IDENTIFIED BY '<pw>';` **on stdin** — never argv, never env, never `mysqladmin password` (both leak via `ps`).
5. `mysqladmin shutdown` via an **ephemeral 0600 defaults-file** carrying the credential (RAII-deleted), await clean exit.
6. Verify sentinels in staging (`mysql/` dir + `auto.cnf`), then `rename(staging, final)`.

**Correction (2026-07-30, post-live-run against real mysql@8.4.11):** steps 1 and 2 as originally written above are wrong. Bisected and reproduced deterministically: combining `--defaults-file=<my.cnf>` (whose `datadir` setting names the FINAL datadir) with argv `--datadir=<staging>` makes mysqld resolve the two `datadir` settings inconsistently across init/start, corrupting InnoDB's undo-tablespace bookkeeping — `Can't create UNDO tablespace innodb_undo_001 since './undo_001' already exists`, surfaced live at step 2 (StartTempServer). Empirical matrix: no-defaults both phases = OK; my.cnf `datadir` matching staging = OK; the defaults-file+mismatched-argv shape above = FAIL; the corrected sequence below = fully green including the final `mysqladmin ping`. Corrected: both steps use `--no-defaults` plus fully explicit argv, never `--defaults-file`, anywhere before `finalize_staging` renames staging into the final datadir (bonus: `--no-defaults` also ignores any machine-wide option file, e.g. `/etc/my.cnf`).
1. (corrected) `mysqld --no-defaults --initialize-insecure --datadir=<staging>`.
2. (corrected) `mysqld --no-defaults --datadir=<staging> --skip-networking --socket=<home>/run/mysql-<major>-init.sock`.

Steps 3-6 are otherwise unchanged. The Render+Validate steps (spec D5) that precede this sequence are unaffected — validating the FINAL rendered my.cnf (which names the final datadir, not yet existing) against `--validate-config` is unrelated to this bug and was independently confirmed fine by the live run reaching step 2 at all.

**Second correction (2026-07-30, same live-run session, isolated with a clean single-variable matrix against real mysql@8.4.11):** independent of the defaults/argv mismatch above, a datadir whose BASENAME contains interior dots beyond the leading one cannot restart after `--initialize` — the identical InnoDB undo-tablespace symptom (`Can't create UNDO tablespace innodb_undo_001 since './undo_001' already exists`), reproduced regardless of `--no-defaults` or whether the directory is mysqld-created or pre-created. The ORIGINAL staging name, `.<major>.init-<uuid>` (e.g. `.8.4.init-<hex>`), has THREE dots: the leading one, the one inside `major` ("8.4"), and the one `.init-` itself starts with. Matrix: `.stg` (leading dot only) = OK; `stg8` (plain, no dots) = OK; `.8.4.init-<hex>` = FAIL. Corrected: the staging dirname shape is now `.init-<major-dashed>-<uuid>` (e.g. `.init-8-4-<32hex>`) — `major`'s own dot is dash-encoded, `init-` no longer carries its own leading dot, leaving EXACTLY ONE dot in the whole basename (the leading one, proven fine by the matrix). No back-compat sweep for the old dotted shape was added — the feature never shipped, so no installed datadir anywhere can have a staging leftover in that shape.

Failure at any step → outcome enum (`MysqlInitOutcome::Failed { step, reason }`, `step ∈ {Render, Validate, Initialize, StartTempServer, SetPassword, Shutdown, Finalize}` — discriminator, never parsed English; ScaffoldStep precedent), and only the **marked staging dir** is removed. The final datadir is never created, adopted, or deleted by a failed init. Datadir classification is read from disk, never a state.db boolean: absent → NotInitialized; sentinels present → Initialized; anything else non-empty → `DatadirForeign` (rendered, not "fixed"). Stale `.init-X-*` staging dirs are swept on rescan.

### D3 — Root credential: generated-only, stored in state.db, never crosses IPC inbound (deep-reasoner adopted over Keychain)

The password is generated server-side at init (`uuid::Uuid::new_v4().simple()` — 32 hex chars, 122 bits, no new dependency) and stored in a new state.db table (migration `0003_mysql_instances`: `major TEXT PRIMARY KEY, root_password TEXT NOT NULL, initialized_at INTEGER NOT NULL`, STRICT). Reset regenerates (`ALTER USER` over stdin via the running server's socket, using the stored current password through the ephemeral-defaults-file mechanism). No inbound secret ever crosses IPC; `mysql_root_password(major)` is the outbound reveal for the UI's masked field + Copy button.

Why Keychain lost (this slice): (a) the app has no code-signing identity yet (§7 OQ#5 unresolved), so Keychain ACLs rebind on every rebuild — repeated prompts and inaccessible items in the dev loop; (b) via the `security` CLI the effective ACL is "any process running as this user" — no real at-rest gain over a 0700-home file, especially since the UI deliberately reveals the password on demand; (c) Phase 2's phpMyAdmin/Adminer runs under php-fpm — a different process Keychain cannot cleanly serve; (d) it adds a cross-owner (platform-macos-specialist) dependency to an otherwise self-contained slice. **Keychain is re-decided at the signing/local-CA slice**, which forces the identical question for the CA key — recorded as a follow-up, and the UI copy must state plainly where the password is stored. Never-persisted lost (breaks authenticated shutdown, reset, and Phase 2); generated `~/.my.cnf` lost (silently changes the user's own `mysql` behavior).

Discipline requirements: the password type is redacted in `Debug`, never logged, never in argv/env of any child (stdin or 0600 ephemeral defaults-file only), and a test pins that no emitted event/log line contains it. **User-chosen passwords are deferred** — a deliberate narrowing of the roadmap's "set/reset" (ships reset-by-regenerate); adding an input later is purely additive. Flagged to owner.

### D4 — Supervision: readiness integrated into the supervisor + per-service grace (synthesis of both)

`mysql_spec(home, rt) -> ServiceSpec` beside `php_fpm_spec`: id `mysql-<major>`, display `MySQL <major>`, endpoint `127.0.0.1:3306`, argv exactly `["--defaults-file=<generated my.cnf>"]` (defaults-file first is a mysqld requirement; everything else lives in the file so the spec is stable). Foreground `mysqld`, never `mysqld_safe` (forks; defeats pid identity + orphan reaper).

`ServiceSpec` gains two backward-compatible fields, defaults preserving today's behavior for nginx/php-fpm (their specs don't change — the existing E2E harness staying green is the regression proof):

- `readiness: ReadinessProbe` — `AliveAfter(500ms)` (default, = today) vs `Command { argv, deadline }`. MySQL uses `mysqladmin --no-defaults --no-login-paths --protocol=SOCKET --socket=<sock> --user=root --connect-timeout=1 --silent ping`, deadline 15 s. `Starting` persists until the probe passes; probe deadline/exit → `Failed` carrying stderr + probe diagnostics. A supervisor-external readiness query lost: a "Running" pill over a connection-refusing server is the boolean-collapse bug in network form, and the ServiceSpec is being extended anyway (grace). `mysqladmin ping` needs no password (documented: succeeds while auth is denied) and probes the socket, not TCP — a port probe passes the instant mysqld binds, long before it accepts work.
- `grace: Duration` — default 5 s (= today's `GRACE_DEADLINE`), MySQL **15 s**. Rationale: SIGTERM is mysqld's documented clean shutdown; a flushing InnoDB can exceed 5 s and a SIGKILL then forces crash recovery on next start. 60 s (proposed) lost to quit-path UX — InnoDB is crash-safe by design, so the cap trades a rare slow-recovery for a bounded quit. SIGTERM→grace→SIGKILL process-group logic is otherwise unchanged.

Config hard lines: `bind-address=127.0.0.1` (no privileged helper exists; a café-Wi-Fi laptop must not expose 3306 with a UI-revealable password), `mysqlx=OFF` (kills the unrequested 33060 listener), `skip-name-resolve`, socket in `<home>/run`, `log-error` **unset** so stderr lands in the supervisor ring buffer and `Failed { stderr_tail }` carries "Address already in use" like nginx/php-fpm.

### D5 — Config: minimal my.cnf via openvhost-conf; NOT folded into the site apply pipeline (both agreed)

New `templates/mysql/my.cnf.tera` + concrete render/validate functions in openvhost-conf (no one-implementation DB trait). Keys, each justified, nothing else — no tuning: `[mysqld]` `datadir`, `socket`, `pid-file`, `port=3306`, `bind-address=127.0.0.1`, `skip-name-resolve`, `mysqlx=OFF`, `log-error-verbosity=2`, `!includedir <home>/config/custom/mysql/<major>/conf.d`; `[client]` `socket`, `port`. Written with `atomicfile::write_atomic` as a `GeneratedFile`. Pre-check: `mysqld --defaults-file=<candidate> --validate-config` behind the existing `ConfigValidator` seam — with two recorded caveats: (i) verify on real 8.4 during implementation that it doesn't touch/lock the datadir (if it does, drop the pre-check — a bad config then fails visibly at start, which readiness now surfaces); (ii) MySQL documents validation as incomplete, so start+ping remains the definitive check. `apply_config`'s restart list stays php-fpm+nginx — coupling DB restarts to web-config applies is worse than a second small write path; unification is a recorded follow-up.

### D6 — UI: Databases page + Services row (both agreed)

New `/databases` route + rail item mirroring `/languages` (install flow, live log, guides when brew missing); MySQL also appears in the Services panel via normal registration (reload the service store after registration — there is no registration event today). One exhaustive lifecycle enum drives the row: `NoBrew / NotInstalled / Installing{log} / InstalledNotInitialized / Initializing{log} / InitFailed{step, reason} / DatadirForeign / Ready` — and when `Ready`, the supervisor state (which now genuinely means ready, per D4) renders as the run pill. Password: masked field + Reveal + Copy + confirmed Reset (copy states it regenerates and where it's stored). Connection block (host 127.0.0.1, port 3306, socket path, user root), all copyable, plus a "Verify connection" affordance backed by `verify_mysql_connection` (`SELECT VERSION(), @@port` through the mysql CLI) — the "it works" moment, same philosophy as the scaffold placeholder. WCAG both-theme contrast checked against tokens (standing lesson).

### D7 — IPC surface (merged set)

Queries: `mysql_environment() -> MysqlEnvironmentDto { brew_found, brew_searched, instances }` (no spawns beyond bounded `--version` probes), `mysql_root_password(major) -> String`. Mutations/tasks: `rescan_mysql()`, `install_mysql(major)` (streams install log events; reuses the generalized `InstallLock` so quit-mid-install aborts brew — generalize `RunningInstall.major` to a label), `initialize_mysql(major) -> MysqlInitOutcomeDto` (streams init log events), `reset_mysql_root_password(major) -> MysqlResetOutcomeDto`, `verify_mysql_connection(major) -> MysqlConnectionProofDto`. Start/stop/logs reuse `start_service`/`stop_service`/existing log tail with id `mysql-<major>` — no new commands. Ingress newtype: `MysqlMajor` only (shape guard + catalogue membership); every path/formula/service-id derives server-side. No port newtype (3306 fixed this slice). No inbound secret exists anywhere on the surface.

## Security posture

- Tauri command surface grows → **security-auditor review mandatory before merge** (golden rule 2). Credentials in play → security-review triggers regardless.
- Never link MySQL client libraries — `mysql`/`mysqladmin` are child processes only (project law, GPL).
- Secret handling: generated server-side; stored in state.db under 0700 home; crosses IPC outbound only via `mysql_root_password`; stdin/0600-ephemeral-file to children; redacted Debug; pinned no-secret-in-logs test.
- Filesystem: all writes atomic; staging-init never touches a pre-existing datadir; foreign datadirs are rendered, never adopted or deleted.
- Network: 127.0.0.1 bind + mysqlx off + socket probes; no listener during the init password window.

## Deferred (recorded, flagged where they narrow the roadmap)

MariaDB (same seams, next slice) · custom port + persistence (roadmap "port config" — narrowing) · user-chosen root password (roadmap "set" — narrowing; reset-by-regenerate ships) · lost-password recovery via `--skip-grant-tables` (desync between state.db and a restored datadir surfaces as a distinct error state with manual-recovery copy; flow is a follow-up) · parallel majors running · 8.0 catalogue entry · phpMyAdmin/Adminer (Phase 2) · my.cnf unification into plan/apply · Keychain (re-decide at signing/local-CA slice) · uninstall/purge datadir.

## Owner caveats (surfaced in the PR/check-in, not blocking under delegation)

1. Homebrew's `mysql@8.4` post-install creates **its own** datadir (`$(brew --prefix)/var/mysql`) with no root password and offers a `brew services` launchd unit. OpenVHost must neither adopt nor delete it; the install UI discloses it, and a port-3306 conflict from a brew-services mysqld surfaces via the existing "Address already in use" failure path with copy pointing at `brew services stop mysql@8.4`.
2. Plaintext-in-state.db (0700 home) is the deliberate credential call for the unsigned-app era; Keychain is re-decided at signing time.
3. This slice ships reset-not-set for the root password (see Deferred).

## Verification owed to a human (GUI click-list)

1. Databases page with no brew → guide renders; with brew, `mysql@8.4` not installed → Install streams live brew output.
2. Install → Initialize streams; success lands in Ready with password present; Reveal/Copy work.
3. Start → pill goes Starting → Running only once connections actually work; "Verify connection" reports server version.
4. `mysql --socket <home>/run/mysql-8.4.sock -u root -p` with the copied password → `SELECT 1` works from a terminal.
5. Reset password (confirm dialog) → old password stops working, new one works.
6. Stop → clean stop well under 15 s on an idle instance; quit-with-running-MySQL stops it without a hung quit.
7. Foreign-datadir case (drop a stray file into an empty `<home>/data/mysql/8.4` before init) → honest DatadirForeign rendering, no destructive offer.
8. Port conflict: `brew services start mysql@8.4` first → our start fails with visible "Address already in use" + disclosure copy.
