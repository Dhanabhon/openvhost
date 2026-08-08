<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The Languages page stops requiring Homebrew (off-Homebrew slice 5C)

**Status:** design, ready to plan.
**Date:** 2026-08-08.
**Follows:** 5A (`94635ba`, PR #60), which builds PHP 8.4.24 into our own package tree, and 5B
(`965eda1`, PR #61), which finds it and prefers it per major. Neither is visible to a user.

## 1. The headline is a screen that now says the opposite of what we built

`LanguagesEmpty.svelte:77` renders a full-stop dead end:

> **Homebrew is required to install PHP**
> OpenVHost installs PHP through Homebrew, and it was not found…

Its own comment says `brewFound` is checked **first** and takes **priority** over everything,
"because *no PHP, press Install* is a dead end one level further up on a machine that cannot
install anything at all."

That reasoning was correct when it was written. After 5A and 5B it is false: this build compiles
its own PHP, installs it into its own tree, and finds it there. **A machine with a packaged PHP
and no Homebrew is currently told it cannot install PHP, on a page that is simultaneously not
listing the PHP it already has.**

That screen is the single most user-visible thing the whole off-Homebrew programme was for, and
it is the last one still arguing against it.

## 2. Measured on `965eda1`

| Fact | Consequence |
|---|---|
| `LanguagesEmpty.svelte:77` gates on `!brewFound` before any other state | D2's whole subject. It is a `bool` standing where a state belongs — the fourth instance of that shape in this UI |
| `PhpRuntimeDto.full_version` is **`None` on every row today** — its own comment says the only prober returns `major.minor`, never a patch level, and that echoing `major` back "would imply a patch level was fetched when it was not" | 5B makes it real for packaged rows: the tree names `8.4.24`. The field was designed for this and has been dead since it was added |
| `MariadbPackageOfferDto` already models a **three-state** offer including `AwaitingRelease`, which "MySQL's own offer type does not need" | PHP is in MariaDB's situation, not MySQL's. Mirror the three-state offer, do not invent a fourth shape |
| `MysqlEnvironmentDto` carries the same `brewFound` bool (`databases.svelte.test.ts:58`) | The Databases page has the identical dead end. **Out of scope**, recorded in §9 |
| `install_mysql_package` (`lib.rs:68`) and `PhpPackageInstall` (`lib.rs:125`, from 5A) both exist | The install half is wiring, not new machinery |
| `PhpRuntimeDto.cataloged` already exists so a row can render without offering Install/Uninstall | The precedent for "show it, but offer nothing" is already here and should be reused, not re-derived |
| **`PHP_PACKAGES` has exactly one row — 8.4 — while `CATALOGUE` offers 8.1 through 8.5 for Homebrew**, because a package build is per-major work | **`Unavailable` is the common case, not an edge case.** It is the state of four rows out of five, and will stay that way for a long time |
| `macos-x86_64` is **deliberately absent**; an Intel host gets `NoPackageForTarget` rather than arm64 binaries | On Intel, *every* row is `Unavailable`. Homebrew is genuinely the only route there, and saying so is correct, not a failure |

## 3. D1 — `PhpPackageOfferDto`, mirroring MariaDB's three states

`MariadbPackageOfferDto` (`apps/desktop/src-tauri/src/mariadb_pkg.rs:65`) spells it
**`Available { version }` · `AwaitingRelease { tag }` · `Unavailable { target }`**. Mirror those
three exactly, matched **exhaustively, never through a wildcard**. `AwaitingRelease`'s own comment
is the one to keep: the next action "belongs to the maintainer, not the user", which is precisely
what the PHP row must communicate today.

**Per major, not per app.** MariaDB ships one series and deliberately left `major` off its own
types; PHP's whole point is several majors side by side, so the offer belongs on the row.

Today every offer resolves to `AwaitingRelease`, so nothing new is installable. That is not a
reason to fake it — see D4.

## 4. D2 — The no-brew screen becomes conditional, and keeps its dead end honest

Today: `!brewFound` → dead end, unconditionally, before anything else is considered.

**The first draft of this section said the dead end should appear only when there is "no route to
a PHP at all". That is wrong, and the catalogue is what corrects it.** Only **8.4** is pinned;
8.1, 8.2, 8.3 and 8.5 have no package and will not for a long time, because a package build is
per-major work. On Intel, nothing is packaged at all. So for most rows, **Homebrew genuinely is
required, permanently, and saying so is correct.**

The bug was never that the page mentions Homebrew. It is that **one machine-wide boolean answers
a question that is per-major**:

- *"Homebrew is required to install PHP"* — false. It is required to install **8.1, 8.2, 8.3 and
  8.5**, and on Intel, all of them.
- Blocking the whole page on it — wrong once any major is installable without it.

After: `brewFound` stops being a page-level gate and becomes an input to a **per-row** answer. A
row whose offer is `Available` needs no Homebrew. A row whose offer is `Unavailable` needs it, and
should say so on the row. The page-level screen survives for the case it was actually written for
— **nothing installed, and nothing installable by any route** — and keeps its blunt wording and
its verbatim searched-paths list.

**Do not soften that screen into a warning.** A user with no route to any PHP needs telling
plainly. The change is *when* it appears, not *what* it says.

**Today this changes nothing on any real machine**, because every offer resolves to
`AwaitingRelease` or `Unavailable` and no packaged PHP exists to find. That is the point: the
change lands while provably inert, so the day a release flips `availability` the page is already
right rather than being fixed in a panic.

## 5. D3 — The badge, and a field that has been dead since it was written

Mirror nginx 4C: `Packaged` rows carry a source badge, Homebrew rows carry none, and the badge
must reuse MySQL's existing CSS rather than a lookalike — 4C's review checked that line-for-line
and it is cheap to check again.

The substance is `full_version`. A packaged row can finally report **`8.4.24`**, taken from the
tree, with **no process spawned** — which is exactly the asymmetry 5B built and 4C's audit
demanded be *consumed* rather than declared. Homebrew rows stay `None`, and the existing comment
explaining why must survive rather than being deleted as stale.

**The trap this field's own comment already names:** never echo `major` into `full_version`. A
packaged 8.4 row shows `8.4` and `8.4.24`; a brew 8.5 row shows `8.5` and nothing. If those look
wrong side by side, the fix is the layout, not inventing a patch level.

## 6. D4 — Install routes to the package when offered, Homebrew otherwise

Mirror what MySQL and MariaDB already do. The row's own offer decides; the frontend does not
re-derive the rule.

**`Unavailable` is the ordinary path, not the failure path.** Four of five rows carry it today,
every row carries it on Intel, and `php_package_for_target` returns it for both reasons — no such
major, and no artifact for this host. An `Unavailable` row installs through Homebrew exactly as it
does now, with no apology and no degraded styling: that is a supported route, not a fallback the
user should feel they are on.

The three offer variants map cleanly onto the two ways a package can be missing, which is why
MariaDB's `Unavailable { target }` carries `target` — reuse it rather than adding a fourth arm.

**The packaged install path merges unproven, and this must be said out loud.** With every offer
`AwaitingRelease`, no test and no live proof can exercise a real packaged PHP install end to end
— exactly the position the MariaDB UI slice merged in. The obligation that carries: **before any
`availability` flips to `Published`, someone fetches the served bytes once and confirms the
SHA-256 by hand**, and only then is the install path proven.

What *can* be proven now, and must be: the routing decision itself, the `AwaitingRelease` render,
and that the Homebrew path is unchanged where it still applies.

## 7. D5 — `brewFound` keeps its job, and loses the one it should not have

`brew_found` still means "we looked for Homebrew and did not find it", and `brew_searched` still
lists the paths verbatim so a user can check the right place on their own machine. That is honest
and stays.

What it stops being is the page's **first and highest-priority state**. The boolean is fine; the
decision hung on it was not. This is the fourth time in this UI that a `bool` was standing where a
state belonged, and the previous three were all found only after they had misled someone.

## 8. What this slice must prove

1. With **no Homebrew and a packaged PHP present**, the page lists it and does not render the
   no-brew dead end.
2. With **no Homebrew and nothing installable by any route**, the dead end renders exactly as it
   does today, searched paths and all.
2b. With **no Homebrew and an `Available` 8.4 alongside `Unavailable` 8.1/8.3/8.5**, the page
   offers 8.4 and tells the 8.1/8.3/8.5 rows that Homebrew is what they need — **per row, not
   page-wide**. This is the case D2 exists for and the one the first draft got wrong.
3. A packaged row reports its **patch version from the tree** and spawns nothing to do it; a
   Homebrew row still reports `full_version: None`.
4. The badge appears only on packaged rows, reuses MySQL's existing style, and cannot read as a
   status pill beside one.
5. Install **routes** by the row's offer, and an `AwaitingRelease` row offers no Install button —
   it says what it is waiting for.
6. **Nothing changes on a machine with Homebrew and no package tree** — which is every real
   machine today, including the developer's. Establish it, do not assert it.
7. Every new enum arm is matched **exhaustively**; a throwaway variant must fail to compile.

## 9. Out of scope

Uninstalling a packaged PHP (5D) · retiring the Homebrew paths entirely (slice 7) · publishing
any release (owner-gated, deferred) · the shared packaged-resolver extraction and its symlink
confinement (already filed).

**Recorded because this slice surfaces it, not because it fixes it:**

- **The Databases page has the identical dead end.** `MysqlEnvironmentDto` carries the same
  `brewFound` bool. Whatever shape D2 lands on here is the shape that should be lifted there —
  but doing both in one slice doubles the blast radius of a change whose whole claim is that it is
  inert today.
- **The catch-all serves the oldest installed PHP** (5B §7). Still an owner decision, and the
  Languages page is where a user would most plausibly expect to see or set it.
- **`Availability` is declared once per engine, with the doc comments duplicated verbatim** —
  `mariadb/package/catalogue.rs:112`, `nginx/package/catalogue.rs:154`, and PHP's own from 5A.
  Same family as the four-way packaged-resolver duplication already filed. Do **not** unify it in
  this slice: an enum shared by three catalogues is a wider blast radius than a UI slice should
  carry, and the resolver extraction is the right place to decide whether these live together.
