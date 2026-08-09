<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The manifest of record for each pinned artifact

One file per pinned artifact, named exactly as the driver named it:
`<name>-<version>-macos-<arch>.manifest.json`. Each is the build manifest that
`build/build.sh` wrote beside the tarball whose digest the Rust catalogue pins,
**copied here byte-for-byte, never regenerated**.

| manifest | pins | catalogue |
|---|---|---|
| `nginx-1.30.4-macos-arm64.manifest.json` | `bc4c42a2618f2ac51145f7c23959421a8d019bde67e0d71946548d9cc9ac4563` | `crates/openvhost-core/src/nginx/package/catalogue.rs` |
| `mariadb-11.4.9-macos-arm64.manifest.json` | `854c34dcafef29dc72af2bcbd6d66271ae2e6167ab45e33c4f744d163675aeb0` | `crates/openvhost-core/src/mariadb/package/catalogue.rs` |
| `php-8.4.24-macos-arm64.manifest.json` | `c79b18c372f3f31f91bdefb79da08d81ffcc23e5f894f0a2b40060ffa6bcc2bb` | `crates/openvhost-core/src/php/package/catalogue.rs` |

This pipeline rests single-builder trust on the manifest published beside each
artifact — that is its stated model. Until these files were committed, no file in
the repository named any of the three pinned digests except the catalogues
themselves, so the account of *how* the bytes were made lived only in prose. Each
catalogue's Group 2 test now `include_str!`s its manifest and asserts
`output.sha256` equals the pin, which makes that account checkable rather than
asserted, and makes a pin bumped without its manifest a test failure.

**Copied, not regenerated, and that is load-bearing.** A manifest written today
from today's prefix would describe today's prefix, not the one the artifact came
out of — in PR #68's words, *"a precise, confident, wrong claim"*. `built_at` and
every recorded digest below is the original run's.

**Artifacts stay out of git.** `.gitignore` carves `/build/manifests/*.manifest.json`
back out of the artifact exclusions; `/build/out/` remains ignored outright, so a
manifest in the driver's own output directory is still untracked, and a stray
tarball dropped in this directory is still ignored. A 2.7 MB–125 MB tarball is
what golden rule 6 forbids; a 3.6 KB JSON record of how one was made is a
different thing.

## Four runs produced each of these artifacts, byte-identical

Each committed manifest is **one of four**: four independent runs, at four
different times, produced byte-identical output for the same pin.

For `mariadb` and `php` the other three differ **only** in `built_at` — every
other field, `output.sha256` included, is identical, so the four are the same
account of the same bytes written at four moments. For `nginx` the other three
also differ in `resumed_from`, `configure_flags` and `recipe`, because one of
the four was the complete build and the rest were repacks of it; that is a
difference in what each run *observed*, not in what it produced, and it is why
the complete run is the one committed (see below).

That is a stronger provenance statement than any single manifest makes, and it is
exactly what PR #67 set out to prove when it found the only non-determinism was
gzip's 4-byte MTIME field. Committing one timestamp silently would assert *"this
artifact was built at 14:23:08Z"*, when the truth on record is *"the build is
reproducible, and four runs demonstrated it"*. The earliest run is the one
committed, so the values are listed here in full and the claim can be checked
rather than taken on the strength of its phrasing.

All times 2026-08-08 UTC. Every row below produced the digest in the table above.

| engine | committed | and also |
|---|---|---|
| nginx | `17:03:03Z` | `17:04:36Z`, `17:04:38Z`, `17:10:48Z` |
| mariadb | `14:23:08Z` | `14:25:06Z`, `14:33:52Z`, `14:54:45Z` |
| php | `14:23:28Z` | `14:25:27Z`, `14:34:14Z`, `14:52:55Z` |

## Two of the three are repacks, and that asymmetry is true

`nginx`'s manifest is a **complete run**: `resumed_from: ""`, all 40
`configure_flags`, and a `dependencies` block carrying openssl 3.5.7's
`tree_sha256`.

`mariadb`'s and `php`'s are **repacks**: `resumed_from: "pack"`,
`configure_flags: []`, and no `dependencies` block. That is not a degraded copy
of a better file that exists somewhere — it is how PR #67 re-cut those two pins,
re-packing an already-built prefix to obtain deterministic bytes without
rebuilding. Both catalogues already say so in prose; these files make it
checkable.

**A repack manifest legitimately loses `configure_flags`.** The driver records
flags as `recipe_configure` calls `bp_record_flags`, and a run resumed at `pack`
never reaches that stage, so an empty list is a true statement that this run
observed no configure step — not a lost field. The same is why there is no
`dependencies` block. Do not "repair" these by copying flags in from a recipe:
the flags in a recipe today are a claim about what a build *would* do, and this
file records what one *did*.

PHP's real exposure is larger than this and is recorded rather than fixed: because
its shipping manifest is a repack, its `spc build` flags are in **no manifest at
all**, existing only in `build/recipes/php.sh`'s `_php_spc_build_args`. Only a
complete rebuild closes that.

## Why nginx's complete run is the one committed, and not one of its repacks

nginx's three repack siblings carry the same `output.sha256`, so any of them would
satisfy the catalogue test. The complete run is committed because it is strictly
richer, and one field shows why:

| field | complete run | its repacks |
|---|---|---|
| `resumed_from` | `""` | `"pack"` |
| `configure_flags` | 40 flags | `[]` |
| `recipe.pcre2.sha256_on_disk` | `59c8556fd45e…` | `"unknown"` |

**That `"unknown"` is current behaviour, not a dated file.**
`build/recipes/nginx.sh` still emits `"${pcre2_actual:-unknown}"` when the source
archive is no longer on disk — which is exactly the state `--from pack` leaves
once `$BUILD_DOWNLOADS` has been cleaned. It has emitted that since PR #57 and is
unchanged today, so a reader should not mistake it for a legacy shape that some
later fix wave already dealt with.

It is worth naming because the driver made the opposite choice one field over.
`dependencies.<name>.tree_sha256` is *"a 64-hex digest or `null`, never a sentinel
string"* (`build/recipes/README.md`), a rule PR #68 introduced on the argument
that a consumer testing `tree_sha256 is not None` takes the string for a digest.
`sha256_on_disk` has not had that treatment. Anything reading these manifests
must treat `sha256_on_disk` as possibly the four-character string `unknown` where
a 64-hex digest belongs — the field is not yet fail-safe the way `tree_sha256` is.

## `pipeline`: the files the run was assembled from, recorded and never enforced

Every manifest the driver writes from now on carries a `pipeline` block:

```json
"pipeline": {
  "driver":  [{"path": "build/build.sh",             "sha256": "…"},
              {"path": "build/audit.sh",             "sha256": "…"}],
  "sources": [{"path": "build/recipes/php.sh",       "sha256": "…"},
              {"path": "build/recipes/_php-pins.sh", "sha256": "…"}]
}
```

`sources` is **recipe-declared** (`RECIPE_SOURCE_FILES`), not merely the entry
file: `php.sh` sources `_php-pins.sh`, so a digest over the entry file alone
would name none of PHP's 41 pins — the single most important thing this block
records. `driver` is the two files the driver adds itself. Paths are relative to
the repository root for a file inside the checkout, absolute for one outside it,
so two builds of the same bytes from two checkouts record the same paths.

**Nothing compares these digests against anything, and that is the design
decision.** `build/recipes/nginx.sh` mixes ~30 declarable pins with ~600 lines of
stage code and prose, so editing a *comment* moves its whole-file digest — an
alarm that fires on comment edits is one people learn to override, and this
project has already refused a gate that could not fail (PR #68). The block is
evidence for a human reading a diff that changes a pin: the manifest of record
says which bytes of which files the artifact was made from, so *"was this recipe
edited after these bytes were cut?"* has an answer in committed evidence rather
than in memory. The enforceable version — pins in their own file, whose digest a
catalogue test may hard-assert because it moves only when a pin does — is
designed and deferred (design §6, D4). Do not turn this into an alarm.

**The three manifests committed here predate the block and do not carry it.**
That is correct and needs no repair, for the reason in *Copied, not regenerated*
above: a `pipeline` block written into them today would digest today's recipes,
not the ones those bytes came out of. No test requires the key. The next
artifact built carries it.

## Adding one

Copy the manifest the driver wrote next to the artifact, unmodified, name it after
the artifact, and extend that engine's Group 2 catalogue test to `include_str!` it.
A manifest nobody checks is prose with punctuation.
