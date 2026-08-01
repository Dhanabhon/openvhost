# P1 Package uninstall — Design

- **Date:** 2026-07-31
- **Status:** Approved under the owner's standing delegation.
- **Roadmap line:** "Package manager UI: install / uninstall / upgrade PHP·MySQL·MariaDB·Nginx·Apache versions from manifests" — master plan §4, Phase 1. This slice does **uninstall**; upgrade is deferred with reasons (D7).
- **Design process:** written directly. Not dual-blind — the hard constraint (principle 1.2, *never destroy user data*) is already decided by the plan, and the remaining choices follow from it. Dual-blind stays reserved.
- **Plan:** `docs/superpowers/plans/2026-07-31-p1-pkg-uninstall.md`

## Why this slice

The Languages page installs PHP majors and the Databases page installs MySQL. Neither can remove anything. A user who installs 8.3 to test one thing carries it forever: a supervised service row, a generated pool config, a log directory, and a brew formula. The install half has shipped and been used daily; this is the other half of a feature the owner already sees.

It also exercises the plan's second non-negotiable principle head-on. Uninstalling a **database engine** must not touch the databases, and must not throw away the credentials that open them.

## What ships

On the Languages page, each installed PHP major gets an **Uninstall** action. On the Databases page, each installed MySQL major gets one. Both show, before doing anything, exactly what will be removed and exactly what will be kept — and both refuse, naming the obstacle, when something depends on the target.

Afterwards the service row is **gone** from the Services page, the tray, and `openvhost list` — not left behind failing.

## Decisions

### D1 — Uninstall is `brew uninstall`, mirroring install

Install is `brew install php@8.4` / `brew install mysql@8.4`, serialized by the existing `InstallLock` and streaming live output. Uninstall is `brew uninstall <formula>` through the **same** lock and the same output surface. Symmetry is the point: the user who watched it arrive watches it leave, and one lock means an install and an uninstall can never interleave.

We do **not** pass `--ignore-dependencies`. If brew refuses because another formula depends on this one, that refusal is surfaced verbatim — brew knows things about the user's machine that we do not, and overriding it is how a package manager breaks someone's system.

### D2 — What is removed, what is kept, and why (the heart of the slice)

| | Removed | Kept |
|---|---|---|
| **PHP major** | the brew formula; the generated `php-fpm-<major>` pool config; the supervisor row | the per-major **log directory** under `<home>/logs/`; every site's stored PHP version |
| **MySQL major** | the brew formula; the supervisor row | **the datadir at `<home>/data/mysql/<major>/`**; the stored root credentials; the per-major log directory |

Three of those "kept" entries are load-bearing and none is an oversight:

- **The datadir is the user's databases.** `brew uninstall mysql@8.4` removes binaries; it has no idea our datadir exists. Deleting it would be the single most destructive thing this app could do, and the roadmap already names `data/` as untouchable. Reinstalling the same major finds the data still there.
- **The credentials survive the engine.** If we dropped the root password from `state.db` on uninstall, a user who reinstalls is locked out of data we deliberately preserved. **Keeping the data and throwing away the key is the same as destroying it.** The credential row stays.
- **Logs survive.** They are the answer to "why did this fail", and a user often uninstalls *because* it failed. Removing the evidence at the moment it becomes relevant is backwards.

Generated pool config is removed because it is ours, regenerable, and a pool pointing at a missing binary is noise. Site PHP versions are **not** rewritten — see D3.

### D3 — Refuse when something depends on it; never silently repoint

Two obstacles, both refusals rather than warnings:

- **The service is not in a terminal state.** Refuse, naming the service and its state. Do not auto-stop: stopping a database mid-write as a side effect of a menu click is exactly the surprise this app should not spring. The user has three ways to stop it already (Services page, tray, `openvhost stop`).
- **Sites are pinned to this PHP major.** Refuse, naming them. The tempting alternative — silently repoint them to another major — is wrong twice over: it edits the user's configuration without asking, and it can move a site onto a PHP version its code does not run on.

The refusal names the obstacle and what to do about it. It does not offer a "force" button; a user who wants it gone can change the sites first, which is the same work with the consequences visible.

**Site PHP versions are left pointing at the uninstalled major.** That is the honest record of what the user configured, and the apply pipeline already rejects an uninstalled major with a validation error (`commands.rs`'s `uninstalled_php_major_is_rejected_with_a_validation_error`). Rewriting them would be a silent edit; leaving them is a visible, recoverable state.

### D4 — `Supervisor::unregister`, and the event that goes with it

Today `Supervisor` has no unregister at all. A major removed outside the app leaves a row pointing at a missing binary, which — per the code's own comment — "simply fails honestly the next time it is started." That is acceptable for an external removal we did not initiate. It is **not** acceptable for one the user just asked for: the whole point of uninstalling is that the thing goes away.

Add:
- `Supervisor::unregister(id) -> Result<(), ProcError>` — refuses unless the service is in a terminal state (`Stopped` or `Failed`), removes the entry under the same entries mutex that `register`/`start`/`stop` use, and emits the event below. Refusing on a live service is what keeps the orphan registry honest: we must never forget a child we are still supervising.
- `SupervisorEvent::Unregistered { id }` — the mirror of `Registered { status }`, which the tray slice added for exactly the opposite reason.

**Every exhaustive match on `SupervisorEvent` must be found and updated before implementing** — Rust, the generated bindings, and the frontend store. The tray slice's Task 1 did this for `Registered` and the same list applies. A service that vanishes has to be handled by the Services page, the tray menu, and the control handler; none of them may fall into a wildcard.

### D5 — Reconciliation converges with the external case

`rescan_php_runtimes` already treats a major that disappeared as "not returned here". After this slice, an in-app uninstall and a `brew uninstall` run behind the app's back must leave the same observable state: no supervisor row, no pool config, data and logs intact. The in-app path gets there directly; the rescan path gets there by noticing the major is gone and unregistering it.

That means **rescan gains an unregister step** for majors that vanished. It is the same primitive, and it fixes the pre-existing stale-row bug as a side effect rather than leaving two divergent behaviours.

### D6 — The confirmation states removals and keeps, not a generic "are you sure"

A native dialog listing, in the user's words:

> **Uninstall MySQL 8.4?**
> This removes the MySQL 8.4 program files.
> **Your databases are not touched** — they stay in `<home>/data/mysql/8.4`, and your root password is kept, so reinstalling 8.4 picks up where you left off.

A generic confirmation trains people to click through. Naming what survives is what makes this safe to click, and it is the only place the user learns their data is safe. For PHP, the same shape: the pool config goes, the logs stay, and any site still set to this version is named.

Use the existing `tauri-plugin-dialog` path already used by the tray and the CLI-install action.

### D7 — Upgrade is deferred, with reasons

Not in this slice:
- For a pinned major (`php@8.3`), "upgrade" means brew moving 8.3.x forward, on brew's schedule, not ours. An in-app button would be a `brew upgrade` wrapper.
- It is not a quiet operation: a running php-fpm or mysqld must be restarted, generated config regenerated, and for MySQL a minor upgrade can require `mysql_upgrade`-class work on the datadir — which is data-touching, and therefore deserves its own design rather than a corner of this one.
- Uninstall is complete and useful on its own.

### D8 — Not in scope

- **MariaDB** — no engine exists yet; that is its own slice.
- **nginx / Apache uninstall** — nginx is the web server the app is currently serving through; removing it from inside the running app is a different problem (the app would be uninstalling its own legs). Apache is not implemented.
- **Removing the app itself** ("uninstaller that leaves `www/` and `data/` untouched") — a separate roadmap line and a packaging concern.
- **A `--force` path.** D3.
- **CLI verbs for uninstall.** The CLI's `Request` type deliberately cannot express anything but a registered service id, and widening it to carry package operations is a control-surface change that would need its own audit. GUI only for now.

## Testing

- **Pure:** the removal/keep inventory as data, asserted exhaustively per package kind (a new kind must fail to compile); the refusal predicates (non-terminal state, sites pinned) over every `ServiceState`; the confirmation text naming both what goes and what stays.
- **Supervisor:** `unregister` refuses `Running` and `Starting`, succeeds on `Stopped` and `Failed`, emits `Unregistered` exactly once, and the entry is gone from `snapshot()`. **Adding a `ServiceState` variant must fail to compile** in the refusal predicate.
- **Real filesystem, tempdir:** uninstall removes the pool config and leaves the datadir, the credentials row and the log directory **byte-identical** — asserted on content and inode, not on a `Result`. This is the highest-value test in the slice; a bug here destroys a user's databases.
- **Vacuity proof required per group.** Be most suspicious of the "kept" assertions: a test that only checks the operation returned `Ok` passes against an implementation that deleted the datadir.
- **Live proof:** install a PHP major, uninstall it, and confirm brew agrees it is gone, the Services row disappears without a restart, and the log directory survives. For MySQL: initialize a datadir, put a row in a table, uninstall the engine, reinstall it, and **read the row back**. That round trip is the only real proof of D2.

## Security posture

Touches child processes (`brew uninstall`), file paths under `<home>`, and the credential store. Per the delivery pipeline, **security-auditor review is required**. Claims to verify: the formula name reaching `brew` cannot be influenced by anything but a validated major; nothing outside the generated-config paths is ever deleted; the datadir and credential rows are never written or removed on any path, including error paths; and the refusal cannot be bypassed by racing a service start.

## Verification owed to a human

1. Languages page → uninstall a PHP major you do not use; the dialog names what stays; the row disappears from Services without a restart.
2. Try to uninstall a major a site still uses — refused, and the site is named.
3. Try to uninstall a running service — refused, naming its state.
4. Databases → uninstall MySQL; confirm `<home>/data/mysql/<major>/` is still there afterwards.
5. Reinstall the same major and confirm your databases and root password still work.
