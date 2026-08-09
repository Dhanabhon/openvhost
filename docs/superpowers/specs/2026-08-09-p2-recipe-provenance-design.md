<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The pins rest on prose, and their manifests are in a temp directory

**Status:** design, ready to plan.
**Date:** 2026-08-09.

## 1. The question this slice was filed under, and why it is the wrong one

Filed as: *the `include_str!` tripwire failed to catch a recipe edited after its artifact was cut.*
**It did not fail.** `nginx/package/catalogue.rs:272-274` says so plainly — it *"compares date literals
and a key fingerprint, not the recipe revision the bytes came from."* Its job is spec §14's
**staleness** check, keeping the catalogue's `upstream_release_date`, `last_checked_on` and signing-key
fingerprint in step with the recipe so a stale upstream check is visible in source rather than
remembered. Binding bytes to a recipe revision was never its contract.

The observed instance is real — PR #57's artifact was built at 15:50 +07 and `recipe.pcre2.last_checked`
entered the recipe at 21:31 the same day — but those bytes were superseded by PR #68's documented
rebuild. Nothing is live.

## 2. What the survey found instead, and what measurement then corrected

The survey reported that **no file names any of the three pinned digests except the Rust catalogues**,
and concluded the manifests of record were gone. Measured directly, that is half right and the half it
got wrong changes the plan:

| pin | manifest of record | quality |
|---|---|---|
| nginx `bc4c42a2…` | exists | **complete run** — `resumed_from: ""`, **40** `configure_flags`, `dependencies` block |
| mariadb `854c34dc…` | exists (×4 copies) | `resumed_from: "pack"`, `configure_flags: []`, no `dependencies` block |
| php `c79b18c3…` | exists (×4 copies) | `resumed_from: "pack"`, `configure_flags: []`, no `dependencies` block |

They are in **this session's scratchpad** — `/private/tmp/claude-501/<session-id>/scratchpad/` — which
is worse than the prunable worktree the survey worried about. The manifests of record for three pinned
artifacts are keyed to a conversation.

So the owner decision the survey escalated — *rebuild to obtain provenance, or accept prose until the
next build* — **dissolves**. Neither is needed. The third option is to commit what already exists.

The two repack manifests are degraded, and that is not a defect to hide: `resumed_from: "pack"` with
`configure_flags: []` is a **true** account of how PR #67 re-cut those pins, and both catalogues
already say so in prose. Committing them makes that account checkable instead of asserted.

## 3. D1 — Commit the manifest of record for each pin

New tracked directory `build/manifests/<name>-<version>-macos-<arch>.manifest.json`, carved out of
`.gitignore`'s `/build/**/*.manifest.json`. `build/out/` stays ignored — artifacts never enter git
(golden rule 6); a 3.6 KB JSON record of how they were made is a different thing.

**Rescue them from the scratchpad before it is pruned.** Verify each against its live pin by
`output.sha256` before copying, and copy rather than regenerate: a manifest written today from today's
prefix would be, in PR #68's own words, *"a precise, confident, wrong claim"*.

## 4. D2 — A committed file nobody checks is still prose

Extend each catalogue's existing Group 2 test to `include_str!` its manifest and assert agreement.
The load-bearing assertion is **`output.sha256 == entry.sha256`** — that is what ties the committed
record to the pin it describes. Then `name`/`version`, `upstream.release_date == upstream_released_on`,
`upstream.last_checked == last_checked_on`, and the signing-key fingerprint.

**The three tripwires are not the same shape, and levelling them is part of this.** Measured: nginx
binds 2 dates plus `RECIPE_SIGNING_KEY_FPR`; PHP binds version, source digest and key fingerprint as
one contiguous row plus 2 dates; **MariaDB binds the two dates only** — no key fingerprint, no source
digest. PHP's is the model.

## 5. D3 — Record the pipeline's own inputs; do not enforce them

`stage_manifest` gains a `pipeline` block: the SHA-256 of every file the recipe was assembled from,
plus `build.sh` and `audit.sh`.

The file set must be **recipe-declared** (`RECIPE_SOURCE_FILES`, defaulting to `$RECIPE_FILE`), not
just the entry file — `php.sh:60` sources `_php-pins.sh`, so an entry-file-only digest would miss
PHP's 41 pins entirely.

**Recording is noiseless. Enforcing equality against the current recipe is not, and must not be done
here.** `nginx.sh` mixes ~30 declarable pins with ~600 lines of stage code and prose, so a comment
moves the digest — and an alarm that fires on comments is one people learn to override, which is worse
than no alarm. This project has already refused a gate that could not fail; a gate that cries wolf
fails the same test from the other side.

## 6. D4 — Do not build the enforced recipe-revision gate

The mechanically-enforceable version is a real design: split nginx's and MariaDB's pins into
`_nginx-pins.sh` and `_mariadb-pins.sh` — **PHP already has exactly this** — and hard-assert the pins
file's digest in the catalogue test. Zero false alarms, because a pins file changes only when a pin
changes.

It is deferred because it is a two-recipe refactor bought by one instance that was caught anyway, on
bytes that have since been rebuilt. Revisit if D1–D3 miss something.

**The strongest alternative, and why it lost.** Re-deriving the manifest and diffing it field-by-field
catches the observed case cleanly and generalises past "a field that did not exist yet" to any changed
declared value. It loses on *where it can run*: re-derivation needs bash, so it cannot live in
`cargo test`; and in `build/audit.sh` it runs at build time, when recipe and manifest agree **by
construction** and it cannot fail. A check that must bite later has to read committed bytes, which
means Rust.

## 7. Recorded, because naming it is the honest half of deferring it

- **PHP's `spc build` flags are in no manifest at all.** Its shipping manifest is a repack, so
  `configure_flags` is `[]`, and the flags exist only in `php.sh`'s `_php_spc_build_args`. That is
  PHP's real exposure, larger than the tripwire question, and D3 does not close it — only a complete
  rebuild would.
- **MariaDB's `bison.path`/`version` are discovered, not declared**, so any future inputs digest must
  exclude them; its `vendored`/`vendored_on_disk` blocks are bound by nothing.
- Neither nginx's nor MariaDB's tripwire binds `RECIPE_SOURCE_SHA256`.

## 8. What this slice must prove

1. **Each committed manifest is the one that describes its live pin** — `output.sha256` equal to the
   catalogue's, verified before the copy and asserted after it.
2. **The catalogue test fails if a pin changes without its manifest** — both directions: change the
   pin, change the manifest.
3. The `pipeline` block records the right file set, including PHP's sourced pins file, and a recipe
   with no extra sources still emits valid JSON.
4. **Nothing is enforced that a comment can trip.** Show that editing a comment in a recipe changes the
   recorded digest and **fails no test** — that is the property, not an oversight.
5. MariaDB's tripwire now binds what nginx's and PHP's bind.
6. Nothing user-visible changes; every `availability` stays `AwaitingRelease`.

## 9. Out of scope

The pins-file split and its enforced digest (D4) · rebuilding MariaDB or PHP to obtain a complete-run
manifest (§7) · committing any artifact · publishing, which is owner-gated and deferred.
