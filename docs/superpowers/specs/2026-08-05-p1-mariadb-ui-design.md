<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# MariaDB on the Databases page — design (slice B)

**Status:** design, ready to plan. The one owner decision from slice A (§10) is **made**:
publish the release first, then build.
**Date:** 2026-08-05.
**Follows:** slice A (#54, `a79a80f`). MariaDB runs as a supervised service; nothing in the
app can install, initialize or see it.

## 1. Goal and the boundary

Slice B makes MariaDB **reachable by a user**: a MariaDB group on the Databases page whose
row installs the packaged 11.4.9, initializes its datadir, starts and stops the service,
shows and resets the root credential, and uninstalls without touching the data.

**Out:** a second MariaDB series, migration between series, MariaDB on Intel, per-site
database bindings, and any change to MySQL's behaviour. The row refactor in §4 is a
**behaviour-preserving** refactor of MySQL's row — if MySQL's rendering changes, that is a
defect, not a side effect.

## 2. Measured before deciding, not assumed

Every number below came from reading the code on `a79a80f`, and each one moved a decision.

| Fact | Consequence |
|---|---|
| `MysqlRow.svelte` is 704 lines, of which the **control flow is ~90% engine-generic**; the hardcodes are ~40 lines of literals plus 4 test-id prefixes | The row is worth generalizing (D1) |
| `instance.major` appears **51 times** in that file as the identity key | The generalization is real work, not a rename |
| `DatabasesStore` has 21 `$state` fields, **10 of them per-major dictionaries** | The store is *not* worth generalizing (D1) |
| `MariadbInstanceRepo` "binds `MARIADB_SERIES` itself; none takes a series from a caller" (`repo.rs:45`) | A shared store would reinvent a key the backend deliberately removed |
| `MysqlPackageOfferDto` is a **closed two-member union** with a `never` arm whose doc says a third state "must decide here rather than inherit 'Install is fine'" | `AwaitingRelease` is a new state (D2) |
| `PackageKind = 'php' \| 'mysql'` is switched exhaustively in **exactly 7** places | Slice A's "breaks the build in seven places" was literally right (D5) |
| Audit finding F1: a `cancel_mysql_install` aborted a datadir *init*, because both runs were tagged `(Mysql, Install)` and differed only in prose | Every new run needs its own discriminator (D4) |
| `DatabasesStore.brewFound` has **no consumer** on this page | Dead; delete it while we are here |

## 3. D1 — Generalize the row, keep the stores separate

This is the slice's central decision and it deliberately splits.

**The row generalizes.** `MysqlCredentials.svelte` carries the invariant *"Copy must never
un-mask the on-screen field"* — a screen-share scenario — and this project's own history
records it as **already fixed once as a review finding**. A second near-verbatim copy means
it can be fixed in one file and silently regress in the other. That risk outweighs the line
count, and the line count already favours generalizing: a parallel implementation is
≈3,400 new lines with ~1,000 near-verbatim; the shared one is ≈+400 net.

**The store does not.** Ten per-major dictionaries are pure overhead for a one-series
engine, and the backend has structurally eliminated the key. A shared store would have to
reintroduce a namespace and a concurrency that cannot exist — the same criticism
`databases.svelte.ts:70-77` already levels at a hypothetical per-major `installProgress`.
`MariadbStore` holds scalars where `DatabasesStore` holds maps.

**How the row is generalized matters as much as whether.** A shared row containing
`{#if engine === 'mariadb'}` is precisely what `mysql-install.derive.ts:7-12` ("no wildcard
arm"), `catalogue.rs:100-110` ("a state where a state belongs") and the F1 finding exist to
forbid. Instead the row takes an **engine descriptor value** — resolved once, in a pure
derive function, by a `switch` over a closed `EngineKind` with a `const _: never` arm —
carrying `{label, idPrefix, defaultPort, portConflictHint, datadirDisclosure, sourcePolicy,
uninstallPolicy}`. The decision happens once and is unit-testable; the template only paints
it. Setting MySQL's `idPrefix` to `'mysql'` keeps all 53 existing `MysqlRow` tests green,
which is the refactor's own gate.

`sourcePolicy` and `uninstallPolicy` are not cosmetic. `mysqlUninstallOffered` returns
`false` for a `packaged` source, so a naively shared row would render
`PACKAGED_UNINSTALL_UNAVAILABLE` on **every installed MariaDB row** — the exact class of
bug that ships when a shared component inherits an assumption nobody restated.

## 4. D2 — `awaitingRelease` is a ninth row state, carrying the tag

Not a flavour of `unavailable`. Two different facts with two different next actions:

- **`unavailable`** — there is no build for this machine. On Intel that is permanent today,
  and the user's next action is *nothing*; they need to know so they stop looking.
- **`awaitingRelease`** — a build exists and is pinned, but the release tag does not exist
  yet, so the URL 404s. The next action belongs to **the maintainer**, not the user.

Collapsing them tells an Apple Silicon owner their machine is unsupported when the truth is
"nobody can have this yet." `notInstalledRowState`'s `never` arm forces every consumer to
decide, which is the feature.

The copy must also stop lying by inheritance: `unavailableBody` currently ends with
*"Homebrew is the way to install MySQL on {target} today."* There is no Homebrew fallback
for MariaDB anywhere in this app, so that sentence belongs to the MySQL descriptor.

**This state is not hypothetical and does not disappear when the release is published.**
This project's own workflow is pin → build → publish, so every future version bump
reproduces it. It should stay cheap, though: a sentence and no control, not an affordance.

## 5. D3 — Its own event channels, not a discriminated payload

Precedent is unambiguous: PHP and MySQL already have separate log channels, and
`uninstall/run.rs:475-497` routes per-kind to a *different channel* through an exhaustive
match, so a new `PackageKind` arm fails to compile until it picks one. MariaDB gets
`mariadb-install-log-event`, `mariadb-install-progress-event`, `mariadb-init-log-event`.

Adding a `kind` field to the existing payloads would work and is the wrong direction: it
moves a distinction from the type system into a runtime field, which is what F1 punished.

## 6. D4 — Every new run gets its own discriminator

`InstallKind::Mariadb`, `InstallKindDto::Mariadb` (wire-exposed through `PendingInstallDto`
to the quit dialog), and `MARIADB_INSTALL_RUN` / `MARIADB_INIT_RUN` pairs.

`commands.rs:2199-2206` states the mandate verbatim: *"Every distinct run that can hold this
slot must be distinguishable HERE, in the discriminators, not merely in prose a human
reads."* That comment exists because a cancel button aborted the wrong operation. A MariaDB
install sharing `(Mysql, Install)` would let **Cancel on a MySQL install kill a MariaDB
one**, which is F1 again with new labels.

## 7. D5 — `PackageKind::Mariadb`, and the one switch with no answer

Seven exhaustive switches must each decide. Six take a MariaDB sentence. The seventh,
`brewFormula`, has **no correct value**: a packaged MariaDB has no Homebrew origin and never
will.

Do not return `''` or a plausible-looking `"mariadb"` — the first is a silent empty string
in user-facing copy, and the second names a formula this app never installs and cannot
uninstall. The signature must admit absence (`string | null`), and every caller must handle
it. If that ripples, the ripple is the type system reporting a real gap rather than a cost.

## 8. D6 — A parallel subscriber, not a wider signature

`subscribeDatabaseEvents(api, store, uninstall, isDisposed)` does not extend cleanly to two
engines. A parallel `subscribeMariadbEvents` mirrors the store split, keeps each function
small enough to read, and leaves the existing 9 listener tests untouched. The page manages
two disposers, which is honest rather than clever.

Install-log routing keeps its existing "is an uninstall in flight" check; the uninstall
store already carries the kind (`uninstallStore.request('mysql', major)`).

## 9. D7 — The command surface

Seven, mirroring MySQL's six plus cancel:

`mariadb_environment` · `rescan_mariadb` · `install_mariadb` · `cancel_mariadb_install` ·
`initialize_mariadb` · `mariadb_root_password` · `reset_mariadb_root_password` ·
`verify_mariadb_connection`

None takes a series argument — the backend refuses one, and adding one at the IPC boundary
would create an input that must then be validated against a constant. Slice A's ingress
discipline applies unchanged: **nothing from IPC reaches a path, an argv or a datadir
without a parse step.**

The credential rules carry from slice A and are **not** relaxed for the UI: the password
reaches a process only by stdin or an ephemeral 0600 defaults-file, never argv, never env;
`RootPassword` keeps its redacting `Debug` and gains no `Serialize`; and reveal/copy stay
split so Copy cannot un-mask.

## 10. What slice B must prove

1. On a machine with no MariaDB installed, the row **installs** the pinned 11.4.9 from the
   published release, verifying the SHA-256 — the first end-to-end exercise of the download
   path, which is why the release had to exist first.
2. Initialize, then start, then a real SQL round-trip, then stop — driven from the UI's
   command surface, not from a test harness calling core directly.
3. The credential shown in the UI is the one the server accepts, and **Copy does not
   un-mask**.
4. Uninstall removes the package and **leaves the datadir and the credential row intact**
   — asserted by content **and inode**.
5. **MySQL is untouched throughout**: its rows render identically before and after the row
   refactor, and its datadir is unchanged by content and inode.
6. Cancel on a MySQL install does **not** abort a MariaDB install, and vice versa (D4/F1).
7. Both engines installed, both running, both visible, neither's controls driving the other.

## 11. Out of scope

A second MariaDB series · series migration · MariaDB on Intel (no signature-checked x86_64
pin exists) · per-site database bindings · moving the misfiled generic helpers out of
`mysql/` · the three MySQL merge preconditions recorded in slice A's plan, which belong to
the packaged-MySQL slice · the two CLI test-harness flakes.
