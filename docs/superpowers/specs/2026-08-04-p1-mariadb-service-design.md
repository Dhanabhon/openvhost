<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# MariaDB as a running service — design (slice A)

**Status:** design, ready to plan. One owner decision outstanding (§10).
**Date:** 2026-08-04.
**Follows:** the build pipeline (#49–#52). We can build and install MariaDB 11.4.9; we
cannot yet run it.

## 1. Goal and the boundary

Slice A makes a **packaged MariaDB 11.4 run**: paths, datadir initialization, generated
config, discovery, supervision, and a root credential. It ends when `mariadbd` is a
supervised service that starts, serves SQL, stops cleanly and survives a restart, proven
against the real artifact.

**No UI, no IPC commands, no `PackageKind` variant.** Making MariaDB appear on the
Databases page is **slice B** — it is ~6 new commands, two enum variants that break the
build in seven places by design, and a parallel row state machine. Folding it in here would
produce a change too large for the end gate to actually inspect, which is how the last
slice's audits stalled. If a task in slice A finds itself needing a command or a UI prop,
that is a finding to report, not scope to absorb.

## 2. Measured today, not assumed

| Fact | How |
|---|---|
| **MariaDB writes no `auto.cnf`** | initialized a real datadir from the 11.4.9 artifact; root holds `mysql/`, `mariadb_upgrade_info`, `ib*`, `aria_log*`, `sys`, `test`, `undo00[1-3]` |
| `mariadb_upgrade_info` exists after a successful init and **contains `11.4.9-MariaDB`** | `cat` |
| A killed `mariadb-install-db` leaves **an empty directory** | 8 kills across the run, 2 s to 95 % of a 7 s init, process-group `SIGKILL`; the script stages elsewhere and moves in at the end |
| The local tarball matches the catalogue pin | `76ea96a4…` on disk and in `catalogue.rs` |

**The first row is the reason this spec exists rather than a rename of the MySQL one.**
`classify_datadir` requires **both** `mysql/` and `auto.cnf` (`mysql/datadir.rs:135-136`),
and a MariaDB datadir has the first and never the second.

*Corrected 2026-08-04, before implementation, by the task that read the code:* an earlier
draft of this paragraph said the MySQL rule would call such a datadir *uninitialized* and
that `--initialize` would then run over the user's databases. **It would not.**
`mysql/datadir.rs` has a catch-all — non-empty and not both sentinels yields `Foreign`, not
`NotInitialized` — so reusing the constant would have made every good MariaDB datadir
**permanently unusable behind an honest refusal**, which is bad and is not data loss. The
conclusion is unchanged and the reason is now the one the code supports. Recording the
correction rather than quietly editing it, because a spec that overstates a risk teaches the
next reader to discount it.

The half-state the rule guards against is nonetheless real. `mariadb-install-db` stages
elsewhere and moves in, so killing *it* leaves nothing — measured eight times. But
initialization through the server binary directly, the way the MySQL path does it, writes in
place: task 1 observed a killed run leaving `mysql/` complete with 88 system tables and no
`mariadb_upgrade_info`. **Requiring both sentinels covers either init path**, which is why
the rule does not depend on knowing which one a future change picks.

## 3. D1 — Sentinels: `mysql/` **and** `mariadb_upgrade_info`

Both, never either alone, mirroring the MySQL rule's shape for the same reason: a directory
holding one of the two is a half-written datadir, and "probably fine" is not a verdict to
act on when the action is destructive.

`mariadb_upgrade_info` also carries the version string, which is more than MySQL's sentinel
offers. **A datadir whose recorded version disagrees with the series we are about to start
is `Foreign`, not `Initialized`** — starting 11.4 on an 11.8 datadir is a migration, not a
start, and we do not do migrations in this slice.

The implementer must still verify the two failure directions live: an empty dir classifies
`Empty`, and a populated one classifies `Initialized` with `--initialize` never reached.

## 4. D2 — Port 3307, and why not a port newtype

MySQL's port is the literal `3306` in `templates/mysql/my.cnf.tera` and again as the
endpoint string in `stack.rs:251`. `openvhost-conf/src/mysql.rs:60-65` records the decision
to keep it a literal — "a variable here would be a knob that does not actually turn".

MariaDB gets **3307**, likewise fixed.

The alternative — a port newtype and an allocator — reopens a settled decision and buys
nothing a user asked for. The asymmetry is the price and it is small.

**The collision this avoids is not the port itself.** `tray/model.rs:198-215` dedupes
services by endpoint string, on the correct assumption that two services claiming one
address are alternatives. Two MySQL majors are; two *engines* are not. A MariaDB service
declaring `127.0.0.1:3306` would be **silently dropped from "Start all"** — no error, just a
service that never starts. `tray/model.rs:440` already pins that behaviour for two MySQL
majors, so the mechanism is deliberate and must not be weakened; giving MariaDB its own
address is what keeps both engines runnable at once.

## 5. D3 — Its own template, and four directives both engines are missing

`openvhost-conf/src/mysql.rs:6-14` states the standing decision: no shared DB trait,
separate template trees, "a second implementation, when it arrives, gets its own template
tree and its own concrete functions." Slice A follows it — `templates/mariadb/my.cnf.tera`,
a `MariadbCtx`, concrete functions. `mysqlx=OFF` alone forces it: MariaDB's server rejects
the directive outright.

**And both templates gain `basedir`, `plugin_dir`, `character-sets-dir` and
`lc_messages_dir`.** All four are absent from MySQL's template, from `MysqlCtx`, and from
the spawn argv today.

This is the runtime half of the build-pipeline BLOCK, arriving from the other direction. A
package resolves those four out of its *compiled-in prefix* when the config does not say
otherwise — which is exactly how the first MariaDB artifact came to resolve `plugin_dir`
out of a mode-1777 tree. The build-time fix (a prefix nothing unprivileged can create)
removed the reachable attack; **pinning them in the config removes the dependence on the
prefix altogether.** Doing it for MariaDB alone would leave MySQL relying on a property of
Homebrew's prefix that nobody has checked.

Cost, stated so it is not a surprise: touching MySQL's template means re-running
`crates/openvhost-core/tests/mysql_live.rs`.

## 6. D4 — Its own credential table

`mariadb_instances(major TEXT PRIMARY KEY, root_password TEXT NOT NULL, initialized_at INTEGER NOT NULL) STRICT`
— a new migration and a concrete repo beside `MysqlInstanceRepo`.

Not a discriminator column on `mysql_instances`: its primary key is `major`, so a shared
table needs a composite key, a table rewrite, and a name that has become a lie. Not a second
row either — `major` is the PK and `11.4` versus `8.4` not colliding today is an accident,
not a constraint.

The three protections carry over unchanged and are **not optional**: `state.db` at 0600 on
every open including its `-wal`/`-shm` sidecars; `RootPassword`'s redacting `Debug` with no
`Serialize`; and the password reaching a process only by stdin or an ephemeral 0600
defaults-file, **never argv, never env**.

## 7. D5 — Reuse what is already generic, copy what only looks it

Already generic and already shared with MariaDB: `InstallLedger`, `PackageTarget`.

Generic in substance, misfiled under `mysql/` — reuse in place rather than fork:
`write_generated_config` (`init.rs:56`), `RootPassword` / `generate_root_password`
(`init.rs:89,122`), `sweep_stale_staging` (`datadir.rs:278`), and
`MysqlPaths::check_socket_lengths` (`datadir.rs:70`, which already delegates to the php-fpm
guard). **Moving them out of `mysql/` is not this slice's job** — that is a mechanical
follow-up, and doing it here would put a rename diff in front of the security gate.

Needs its own copy, because the names differ rather than the shape: discovery
(`mariadbd`/`mariadb`/`mariadb-admin`, all-three-or-nothing) and path derivation (the
`"mysql"` segment and `mysql-<major>` basenames are hardcoded).

**Copy exactly, do not re-derive:** resolving `current` to a concrete version directory at
spawn time and recording that path with the process (`discover.rs:241-253`). Spawning
*through* the symlink makes a `current` swap silently change which engine a restart brings
up, and it cost a full misdiagnosis in the MySQL slice.

## 8. D6 — Initialization is not a copy of MySQL's

The shape holds — render, validate, initialize into staging, start a temp server, set the
password, shut down, atomically finalize — and the two hard-won containments carry:

- **`--no-defaults` during init**, so the user's `!includedir` drop-ins cannot steer a
  server that still has an empty root password;
- **the temp server never goes through the Supervisor**, and is spawned with a manual
  process-group kill guard.

Two things genuinely differ and must be verified live rather than assumed:

1. **MariaDB 11.4's fresh root authenticates via `unix_socket`**, not an empty password.
   `alter_user_sql`'s text is valid MariaDB, but whether it is the right *statement* is a
   live question. `mariadb-install-db --auth-root-authentication-method=normal` is the lever
   and it is what today's measurement used.
2. **`--mysqlx=OFF` does not exist.** It is load-bearing for MySQL — without it the temp
   server binds a mode-0777 `/tmp/mysqlx.sock` while root is still open. **Establish
   what MariaDB's equivalent exposure is, if any, before the temp server is started for the
   first time.** Answering "there is none" is fine; assuming it is not.

## 9. What slice A must prove

The live proof is the gate this project never trades away:

1. install the real artifact into a hermetic `OPENVHOST_HOME` under **`/tmp`, not `$TMPDIR`**
   (the 103-byte `sun_path` ceiling, hit twice, most recently at 159 bytes);
2. initialize a datadir, set a root password;
3. start under the Supervisor, create a table, insert, **restart, read the row back**;
4. stop cleanly — no orphan, registry consistent;
5. **MySQL 8.4 running at the same time throughout**, both reachable, neither's datadir
   touched — assert **content and inode**, since a delete-and-recreate with identical bytes
   passes a content-only check and this project has proven that;
6. `mariadbd --verbose --help` names the four pinned directories from the config, not from
   its compiled-in prefix.

## 10. The one owner decision

**Publishing the GitHub Release is still owner-gated, and slice A does not need it.**
`Availability::AwaitingRelease` gates only the download path; the live proof installs from
the local artifact, which matches the pin. So slice A can be built and proven now.

But **slice B cannot ship without it** — a Databases row whose Install button returns
`PackageNotPublished` is a broken promise on screen. The release is the thing to decide
before slice B starts, not before slice A.

## 11. Out of scope

UI and IPC (slice B) · a `PackageKind`/`InstallKind` variant (slice B) · datadir migration
between majors · MariaDB on Intel (no signature-checked x86_64 pin exists) · moving the
misfiled generic helpers out of `mysql/` · Galera (`-DWITH_WSREP=OFF`, and
`mariadbd --verbose --help` has zero `wsrep` occurrences).
