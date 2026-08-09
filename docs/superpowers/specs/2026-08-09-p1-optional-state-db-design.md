<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# A degraded state.db tells the user to call a Rust API

**Status:** design, ready to plan.
**Date:** 2026-08-09.

## 1. What actually happens today, measured

`lib.rs:367-376` opens `state.db` **best-effort** and carries on when it fails — deliberately, because
a missing store must never stop the supervisor. So `app.manage(db)` runs only on the success arm.

**27 commands take `db: tauri::State<'_, Db>`**, and Tauri refuses the entire command when the state
is unmanaged. The refusal is not silent and it is not opaque: `State::from_command` produces a bare
string, `normalizeError` wraps it as `{kind:'core', message}`, and `languages/+page.svelte:191`
renders

> **Could not read the PHP environment**
> state not managed for field `db` on command `php_environment`. You must call `.manage()` before
> using this command.

**We are telling a user to call a Rust API**, in a page that has lost all its rows and controls. The
error surface already exists and already works. What is wrong is what we put in it.

`lib.rs:366` still says *"no IPC command reads `Db` yet — that lands with the Sites UI"*. That is the
comment justifying the best-effort open, and it has been false 27 times over.

## 2. D1 — One wrapper, managed unconditionally; never manage `Db` again

```rust
pub enum DbHandle { Ready(Db), Unavailable { reason: String } }

impl DbHandle {
    pub fn require(&self) -> Result<&Db, IpcError>;   // REFUSE — typed, names the reason
    pub fn optional(&self) -> Option<&Db>;            // DEGRADE — caller must handle None
    pub fn unavailable_reason(&self) -> Option<&str>; // for a DEGRADE path that wants to say why
}
```

Both arms of the open call `app.manage(handle)`. This is the shape `Option<StackPaths>` already
prescribes for itself at `lib.rs:411-423`, and both of its stated constraints were re-verified
against tauri 2.11.5: there is no `CommandArg` impl for `Option<State<'r, T>>`, and `Manager::manage`
does not overwrite, so a "manage `None` early, the real value later" split would pin every user to
`None`. `Manager::unmanage` exists — **do not** use it to fake a re-manage.

**Carry the reason, not just the absence.** Startup already has the `CoreError` and currently only
`eprintln!`s it. `Unavailable { reason }` lets a refusal say *permission denied* rather than only
*unavailable*.

**Rejected: `app.try_state::<Db>()` everywhere.** It loses on testability, not taste. It needs an
`AppHandle` on commands that have none (`list_sites`, `web_server_settings`,
`mariadb_root_password`), and `AppHandle<Wry>` is unconstructible under `mock_builder` — a dead end
this codebase documents in five places. Adding it to `list_sites` would delete that command's
existing tests. It survives in `php_pkg::run_package_install` only because that is **not a command**
and already holds an `&AppHandle` for event emission.

**No new `IpcError` variant.** `IpcError` is exported to TS, nothing branches on `kind`, and every
affected page renders only `.message`. A variant earns nothing until some UI switches on it.
`IpcError::Core` is honest — the error genuinely came from openvhost-core.

## 3. D2 — The classification is the design, and it is not uniform

**REFUSE (20)** — the data lives in state.db, so the honest answer is a typed, renderable error:
`list_sites`, `create_site`, `update_site`, `delete_site`, `open_site`, `plan_config_apply`,
`apply_config`, `web_server_settings`, `save_web_server_settings`, `set_default_php`,
`initialize_mysql`, `mysql_root_password`, `reset_mysql_root_password`, `verify_mysql_connection`,
`initialize_mariadb`, `mariadb_root_password`, `reset_mariadb_root_password`,
`verify_mariadb_connection`, `uninstall_plan`, `uninstall_package`.

Three of these refuse in a specific shape rather than the default:

- `verify_*_connection` refuse as `…ConnectionProofDto::Failed { detail }`, **not** `Err` — they
  already do exactly that for the no-stored-password case.
- `apply_config` **must fail closed**: an empty site list would render a config that *deletes every
  vhost*.
- `initialize_*` must refuse **pre-flight**. Degrading would leave a real datadir with a generated
  root password nobody can recover — the hazard `commands.rs:6085-6100` already documents.

**DEGRADE (5)** — the real work does not need the store; only bookkeeping does:
`php_environment`, `rescan_php_runtimes`, `install_mysql`, `install_mariadb`, `list_log_sources`.

The first two are one line each: `read_default_php` **already takes `Option<&Db>`** and its doc
argues this exact case; only its caller never passes `None`. The two installs follow
`php_pkg.rs:491-508`'s §8.6 argument and its audit-LOW-4 note — the package installs either way and a
failed ledger row costs provenance, never correctness. `MysqlLedgerWriteDto` and
`MariadbLedgerWriteDto` already carry `Failed { reason }`.

**SPLIT (2)** — `read_log_window`, `reveal_log_folder`. Push `Option<&Db>` into `check_catalogue`:
the nginx, php-fpm and ring arms need no store and proceed, while the `SiteAccess | SiteError` arm
**fails closed**, because that check is the path-confinement gate. This keeps the nginx error log
readable — the one thing a user needs when the app is broken.

## 4. D3 — `web_server_settings` refuses rather than serving defaults

Both are defensible; the tiebreak is **which side fails quietly**. A populated, editable form whose
Save always fails is the quiet one. Recorded because it flips to DEGRADE with a one-line change if
that reads wrong in use.

## 5. D4 — Widen two core install signatures

`install_mysql_package`/`install_entry` and `install_mariadb_package`/`install_entry` take
`ledger: &InstallLedger` and must take `Option<&InstallLedger>`, mirroring what PHP already does.
Four signatures. The alternative is making the two installs REFUSE, which is a real regression
against `php_pkg`'s own stated principle — accepted rather than silently dropped.

**Fix wave, +2 signatures: nginx as well.** `install_nginx_package`/`install_entry` were the one
packaged engine left on `&InstallLedger`. No desktop command reaches them today, so this changes no
behaviour — it is widened *with* the other three rather than after, because the day an
`install_nginx` command lands, the narrow signature would make it the only install that refuses on a
degraded store, contradicting the principle this section is written to defend.

## 6. D5 — DEGRADE is dishonest without one app-level banner

A shorter `list_log_sources` result is indistinguishable from *"you have no sites"*. That is a quiet
wrong answer, which is the failure mode this project keeps getting burned by. Same class, weaker, for
`php_environment`: a chosen default silently reads as "no preference".

So: one zero-arg command `state_store_status() -> Option<String>` and a banner in
`apps/desktop/src/routes/+layout.svelte`. App-level because the condition **is** app-level — the
store is down everywhere, not on one page — and it covers Languages, Logs, Databases and Sites at
once. Per-DTO envelopes were the runner-up and lose: `list_log_sources` returns a bare `Vec`, so it
would need a whole new return type for the same information.

REFUSE needs **no** frontend work: every affected page already renders `.message`.

## 7. D6 — What actually stops the next command reintroducing this

Be honest about the property. It is not a compile error; it is a **universal** failure instead of a
conditional one. Once `Db` is never managed, a new `db: State<'_, Db>` is refused on *every* machine
on its first invocation, including the developer's — it cannot reach a user. And neither accessor
hands out `&Db` without acknowledging absence: there must be **no `inner()`-shaped escape hatch**, so
the worst a new command can do is a typed refusal.

Plus a cheap guard test asserting the command files contain no `State<'_, Db>`.

## 8. What this slice must prove

1. **A machine whose `state.db` cannot open still starts, and every page renders something true** —
   no page shows Tauri's "you must call `.manage()`" string.
2. **Each REFUSE command returns its typed error** naming the reason, and the three special shapes
   behave as specified — including `apply_config` failing closed rather than rendering an empty site
   set.
3. **Each DEGRADE command completes its real work** with the store down: a package still installs and
   reports `ledger: Failed`, the Languages page still lists runtimes, the nginx log is still readable.
4. **The banner appears exactly when the store is down** and not otherwise.
5. **`tauri-specta` output is unchanged** — `FunctionArg for State<'_, T>` returns `None` regardless
   of `T`, so `DbHandle` needs only `Send + Sync + 'static`. Prove it with a zero-diff bindings run.
6. **No `.sqlx` implication** — no `query!`/`query_as!` and no migration is touched.
7. **Vacuity per group.** Every `Unavailable` branch is new coverage with no precedent; the
   `check_catalogue` site arm especially, whose whole point is failing closed.

## 9. Out of scope

Making `Db::open` itself more robust · retrying the open after startup · per-DTO status envelopes
(D5's runner-up) · any `IpcError` variant (D1) · the three already-filed chips (recipe tripwire,
`bp_rm_tree` on resume, `PROBE_TIMEOUT`).
