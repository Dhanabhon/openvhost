<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# PHP discovery — packaged first, Homebrew as fallback (off-Homebrew slice 5B)

**Status:** design, ready to plan.
**Date:** 2026-08-07.
**Follows:** 5A, which builds PHP 8.4.24 into our own package tree and pins it. The app can
install a PHP it never looks for.

## 1. Goal and the boundary

Make the app **find** a packaged PHP and prefer it, per major, with Homebrew as fallback.

**Out:** the Languages page (5C), routing Install to our package (5C), uninstall (5D). **The
release is still deferred**, so on a real machine today discovery finds no packaged PHP and
falls back exactly as it does now. This slice is invisible until a package exists, and it is
provable in full against a hand-built tree.

## 2. Measured on `72f5796`

| Fact | Consequence |
|---|---|
| `PhpRuntime { major, fpm_bin }` (`site/apply/mod.rs:41`) carries **no source** | The field to add is one, and nginx and MySQL both already have its shape |
| `discover_installed_php` is called at `stack.rs:810` and feeds `InstalledRuntimes` at `:877` | The seam is one call, like nginx's |
| `discover.rs` is brew-shaped throughout: `BREW_PREFIXES`, `FPM_REL`, `resolve_keg`, `brew_formula(major)` | There is no packaged half to extend — it is new code |
| **`mysql/discover.rs:368` says its merge rules "mirror `crate::discover_php_in` exactly"** | PHP's rules are the original; MySQL is the copy that already grew a packaged pass. `mysql/discover.rs:356-363` is the template |
| PHP is a `Vec`, ordered, and "the first entry is the catch-all's runtime" | Merge order is **semantically load-bearing**, unlike nginx's single `Option` |
| `Discovery<PhpRuntime>` reports unidentified candidates rather than dropping them, so an empty `runtimes` means "nothing installed", never "I could not tell" | That contract must survive the packaged pass |

## 3. D1 — `PhpRuntimeSource`, and the version asymmetry used from birth

`Packaged { version }` and `Homebrew`, matched **exhaustively, never through a wildcard**.

nginx declared this asymmetry in 4B and nothing consumed it until 4C had to retrofit the
consumer — the audit's words were that the enum was "write-only in production". **Do not repeat
that here.** A packaged PHP's exact version comes for free from the catalogue and the directory
name; only a Homebrew one needs `resolve_keg` or a `php-fpm -v` probe.

To be exact about what "consuming it" means here, because §9 excludes the Languages page: the
**operative fork lands in discovery itself** — the packaged arm reads the version from the tree
and **spawns nothing**, the Homebrew arm probes. That is a behavioural branch in this slice's
own code, not a badge. The UI that *displays* the distinction is 5C. The test for whether this
slice earned the enum is 4C's: **a throwaway third variant must break compilation at more sites
than it did before**, and at least one of them must be a real behavioural fork rather than
plumbing.

**Measured in T1, and the bar above was set wrong — recorded rather than quietly moved.** The
throwaway variant breaks **2 sites**, both `PhpRuntimeSource`'s own accessors; **neither is a
behavioural fork**. The calibration that decides it: `MysqlRuntimeSource` also breaks at 2
*after its full UI slice*, and `NginxRuntimeSource` reaches 3 only because **4C** — a consumer
slice — added an install-path fork. So 2 is the normal count for a discovery-only slice here,
and §3 was comparing 5B against a slice one stage further along.

Substantively the asymmetry *is* consumed: making the packaged runtime report `Homebrew` fails
four tests. It is enforced **at construction** — two resolver functions, each producing one
source — rather than by a late `match`. That is the better shape (the value is parsed once, not
re-decided at every use), so the count is accepted as-is and **no `match` is manufactured to
raise it**.

The ordering worry that first appeared here turned out not to exist: merge order is neutralised
by a pre-existing `sort_by(major)`, so there is no statement-order contract for a third source to
get wrong. See §7. What a third source *would* have to get right is the per-major precedence,
which is one `any(|ours| ours.major == rt.major)` check in one place.

## 4. D2 — Packaged wins per major; brew's own preferences still govern the brew pass

Mirror `mysql/discover.rs:356-363`: run the packaged pass first, then push a brew runtime only
when its major is not already present.

**The two documented brew preferences keep applying, unchanged, *within* the brew pass** —
earlier prefix wins (Apple Silicon before Rosetta), and a versioned path beats the `php` alias
within the same prefix, with the first taking precedence because "a stale alias path is
cosmetic, but running the wrong architecture is not." The packaged pass sits **in front of**
that logic; it does not replace or reorder it.

Why packaged wins: it is the one we built, pinned, verified and can name the exact version of.
A brew keg of the same major is a runtime we know less about, and the migration's whole
direction is away from it.

## 5. D3 — Enumerate every packaged series, because PHP is not nginx

`packaged_nginx_runtime` resolves **one** hardcoded series. PHP must walk `packages/php/*/`
and resolve each series' `current` — multiple majors installed side by side is this app's
headline feature.

Copy `packaged_mariadb_runtime`'s discipline for each one: resolve through `PackagesRoot`'s
facade rather than spelling `join("current")` by hand, keep the structural check that the
resolved version directory is a **direct child** of the series directory, and record the
**concrete version path**, never `current`. Spawning through the symlink lets a later swap
silently change which binary a restart brings up, and it cost a full misdiagnosis in the MySQL
slice.

## 6. D4 — `Discovery`'s honesty contract survives

A packaged tree that exists but cannot be identified — a missing `bin/php-fpm`, a `current`
pointing nowhere — is reported as **unidentified**, not dropped. That is what keeps "empty
`runtimes`" meaning "nothing is installed".

**`bin/php-fpm`, not `sbin/php-fpm`** — this section said `sbin` in the first draft, which was
wrong. The recipe is authoritative (`RECIPE_SERVER_BIN="bin/php-fpm"`,
`RECIPE_REQUIRED_LAYOUT=(bin modules)`) and 5A's catalogue already asserts it. `sbin` is
Homebrew's layout, so the packaged walk must **refuse** a tree shaped that way rather than
quietly accept either.

This matters more for packaged than for brew: a half-installed package tree is a state our own
installer can produce, where a broken keg is someone else's doing.

## 7. D5 — Ordering is part of the contract

"The first entry is the catch-all's runtime" is a live property of the returned `Vec`, relied on
downstream: `render_set` takes `input.runtimes.php.first()` as the default upstream
(`site/apply/mod.rs:164`).

**This section's first draft was wrong and T2 caught it by writing the test.** It claimed
packaged-first changes which runtime the catch-all uses. It does not. `discover_php` ends with
`runtimes.sort_by(major)`, so **merge order is neutralised** — and that sort is **pre-existing**,
already in the brew-only walk on `main` before this slice. The catch-all has always been the
**lowest major installed**, and still is.

So the ordering contract this slice must hold is narrower than drafted: **packaged wins within a
major**, which is D2, and which T1 already pins by name in
`a_packaged_runtime_beats_a_homebrew_one_for_the_same_major` and
`the_first_entry_is_the_lowest_major_and_is_the_packaged_one_when_both_have_it`. Swapping the two
merge steps fails both. No new guard is needed and none is added.

**Separate, pre-existing, and not this slice's to decide:** *why* is the catch-all the lowest
major? `sort_by(major)` reads like a stable **display** order that `.first()` then borrows as a
**runtime selection** — two different jobs on one call. Today, brew 8.1 alongside brew 8.3 gives
the catch-all 8.1, the oldest. 5B applies that same rule to a larger set rather than changing it,
so it is out of scope here, but it is a real product question and it is recorded in §10.

## 8. What this slice must prove

1. With a packaged PHP present, discovery resolves it, records `Packaged { version }`, and
   hands out a **concrete version path** — never through `current`, proven by swapping the link
   under a resolved runtime.
2. **The version is taken from the tree, with no `php-fpm` spawned** for a packaged runtime.
3. With both a packaged and a brew PHP of the **same major**, packaged wins and brew's entry is
   dropped — not duplicated, not appended.
4. With brew majors the package tree does not have, both appear, and **brew's two preferences
   still hold** among the brew entries.
5. A packaged tree that cannot be identified is reported unidentified, not silently absent.
6. **Nothing user-visible changes on a machine with no package tree.** Sites, apply, per-site
   PHP versions and the Languages page behave exactly as before.
7. **Packaged wins within a major, pinned by name** — see §7, which corrects what this item
   said in the first draft. Swapping the two merge steps must fail
   `a_packaged_runtime_beats_a_homebrew_one_for_the_same_major` and
   `the_first_entry_is_the_lowest_major_and_is_the_packaged_one_when_both_have_it`. It does.

## 9. Out of scope

The Languages page and its source badge (5C) · routing Install to `openvhost-pkg` (5C) ·
uninstall, the first target whose plan depends on runtime state (5D) · retiring the brew paths
(slice 7) · the `_pid_gone`/`_free_port` extraction recorded against the fifth recipe ·
`InstallLedger` still living under `mysql/`.

**Recorded here because this slice surfaced them, not because it fixes them:**

- **The catch-all serves the lowest installed major** (§7). Probably not what anyone wants, and
  it is a `sort_by` meant for display being reused as a selection. Pre-existing; needs an owner
  decision about what the default *should* be (newest? explicitly chosen? per-project?).
- **A symlinked version directory defeats the direct-child check**, identically in PHP, nginx,
  MySQL and MariaDB. T1 pinned it here as an assertion about today's behaviour, with a comment
  saying the test must be rewritten when it is closed. The fix belongs in the shared resolver
  the four engines still do not have.
