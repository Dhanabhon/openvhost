<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Uninstalling a packaged PHP (off-Homebrew slice 5D)

**Status:** design, ready to plan.
**Date:** 2026-08-08.
**Follows:** 5A (`94635ba`) builds it, 5B (`965eda1`) finds it, 5C (`c4b0732`) installs and shows
it. This removes it.

## 1. Why this one needed its own design note

Every earlier slice recorded the same line in its out-of-scope list: *"uninstall, the first target
whose plan depends on runtime state."* Now that the code is in front of us, that is exactly right,
and the reason is a **documented invariant** in `uninstall/mod.rs:450`:

> A pure function of `(target, home)`: it does not stat anything, so the plan a dialog shows and
> the sequence the executor runs are the same value even if the disk changed in between.

For a **destructive** operation that is a TOCTOU defence, not a style preference. If the plan were
recomputed against the disk at execution time, the dialog could say it removes X while the
executor removes Y.

A packaged PHP breaks it on two counts: the version directory name comes from reading `current`,
and whether a major is packaged at all comes from discovery. MariaDB slipped through because its
path is compile-time constants (`MARIADB_PACKAGE_NAME`/`MARIADB_SERIES`) joined onto the home.

## 2. The answer is already in the file

`build_plan` (`mod.rs:709`) takes `services`, `sites` and **`keg: Option<&KegProvenance>`** as
arguments. The caller queries the world; the plan stays a pure function of what it was handed.
`KegProvenance` is the exact precedent — the Homebrew alias check needed runtime state too, and it
was **passed in rather than statted**.

So this slice adds no new pattern. It uses the one that is already there.

## 3. Measured on `c4b0732`

| Fact | Consequence |
|---|---|
| `inventory` is a pure function of `(target, home)`, stated and reasoned in its own doc | The design constraint. Runtime state arrives as a parameter or not at all |
| `build_plan` already threads `services`, `sites`, `keg` in from the caller | D1 is precedent, not invention |
| `Removal::PackageTree { path, what }` exists, built for MariaDB, and its doc says `path` is "always built from compile-time constants" | That sentence stops being true for PHP. It must be rewritten, not quietly outgrown |
| `Target::formula()` returns `Option<String>`; PHP's arm returns `Some(brew_formula(m))` **unconditionally** (`:307`) | The seam. MariaDB returns `None`, which is what routes it to `PackageTree` |
| `inventory`'s PHP arm builds `Removal::BrewFormula` unconditionally (`:466`) | A packaged-only major would today be "uninstalled" by running `brew uninstall` on a formula that is not installed |
| `KegProvenance` and the `php@8.5` → `php` alias trap are already modelled (`:94–125`) | Uninstall already knows that "which thing does this name actually remove" is the dangerous question here |
| 5B: discovery shows **one row per major**, packaged winning | A machine with packaged 8.4 *and* brew 8.4 shows one row. D3 is about what its Uninstall button means |

## 4. D1 — Runtime state arrives as a parameter; `inventory` stays pure

The resolved packaged version directory is passed in, exactly as `keg` is. `inventory` gains a
parameter and gains no `stat`.

**Do not** relax the purity invariant "just for the packaged path". Its comment explains why it
exists, and a destructive operation is the worst place to trade a stated guarantee for
convenience. If an implementation seems to require statting inside `inventory`, that is a signal
the resolution belongs one level up, in the caller that already ran discovery.

## 5. D2 — `Target::formula()` must be able to say "none" for PHP

Today PHP's arm is unconditionally `Some`. A packaged-only major must return `None`, so it routes
to `PackageTree` the way MariaDB does — and so the plan never offers to run `brew uninstall` on a
formula that is not installed.

`Removal::PackageTree`'s doc comment currently asserts its `path` comes from compile-time
constants. **Rewrite it.** A comment that quietly stops being true is worse than one that was
never written, and this project has now found four comments that were right in conclusion and
wrong in reason.

## 6. D3 — Both installed: remove what the row described, and say what survived

A machine can have a packaged 8.4 *and* a Homebrew 8.4. Discovery shows **one row**, packaged.

The Uninstall button on that row removes **the packaged tree only**, and the confirmation lists
the Homebrew keg under **`keeps`** — "The Homebrew PHP 8.4 keg — untouched".

Rejected: removing both (destroys more than the row described, and the row never mentioned brew);
and removing the packaged one silently (a rescan would show 8.4 still present and the user would
reasonably conclude the uninstall failed).

This reuses the mechanism the module already has for exactly this job: `keeps` exists so the
confirmation can state what survives. It also needs the same runtime state D1 threads in, so it
costs no new plumbing.

## 7. D4 — The path is the resolved version directory, never `current`

Same rule 5B established for discovery, and it matters more here because this one calls
`remove_dir_all`. Record the **concrete version path**; never hand the executor a path that goes
through the `current` symlink.

**The known containment gap applies here at its sharpest.** A symlink anywhere in the prefix —
version directory *or* series directory — defeats the lexical direct-child check, and here the
consequence is not "we serve the wrong binary" but "we recursively delete a directory outside the
package tree." The fix is filed separately as a shared canonicalising resolver.

**This slice must not merge a `remove_dir_all` that relies on the broken check.**

**Decision: the executor confines its own target, and does not wait for the shared fix.** Before
removing, canonicalise the path and the packages root and require the first to be under the
second; refuse otherwise.

Not because the shared extraction is far off, but because it is the right place regardless: a
`remove_dir_all` should validate its own target rather than trust a check made three layers up by
a caller it cannot see. Defence in depth at the destructive call is standard, and it keeps 5D the
size of its siblings. The shared resolver, when it lands, replaces the *lexical* checks in
discovery; this guard stays.

## 8. What this slice must prove

1. A packaged PHP is removed: version directory gone, `current` gone or repointed, the pool config
   and service row handled exactly as the brew path handles them.
2. **The plan is still pure.** `inventory` stats nothing; the same `(target, home, state)` yields
   the same value twice, and the dialog and the executor see one plan.
3. A packaged-only major produces **no `BrewFormula` step**.
4. A brew-only major is **unchanged** — byte-for-byte the same inventory as today.
5. With both installed, the packaged tree goes, the keg stays, and the confirmation **says so**.
6. **`remove_dir_all` cannot escape the packages root**, proven by construction with a symlinked
   series directory and a symlinked version directory — the two shapes the audit reproduced live.
7. Logs, pool overrides and every site's saved PHP version are kept, exactly as the brew path
   already promises.
8. **Nothing changes on a machine with no package tree** — every real machine today.

## 9. Out of scope

Retiring the Homebrew paths (slice 7) · publishing any release (owner-gated, deferred) · the
`Availability`-per-engine duplication (recorded in 5C §9) · the `install_mysql`/`install_mariadb`
`State<Db>` hazard (filed separately).

**Carried forward, still owed before any `availability` flips:** hash-confirm the served bytes ·
the `awaiting_release_is_the_only_non_absence_offer…` tripwire is a re-audit signal, not a test to
update · the mixed-offer page has no `brew.sh` control · the packaged install has no
already-installed pre-check.
