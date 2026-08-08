<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Four bytes stand between this pipeline and a reproducible artifact

**Status:** design, ready to plan.
**Date:** 2026-08-08.

## 1. Measured first, and the answer is smaller than the question

Packing the same staged prefix three times produced three different tarballs:

```
9fecd3a2…   (the original, and what the catalogue pins)
77e975e0…
f508c262…
```

That reads like "the build is not reproducible." It is not what is happening. Decompressing two of
them:

```
raw tar A: df0dfb79c99ad02b6b0abfccdb74167f6ad8e89e08d28239f384a5405c3f63ae
raw tar B: df0dfb79c99ad02b6b0abfccdb74167f6ad8e89e08d28239f384a5405c3f63ae
```

**Byte-identical.** Entry list, modes and mtimes all match. The entire difference is in the gzip
header:

```
1f8b 0800 | 8313 776a | 0003
1f8b 0800 | bc2c 776a | 0003
            ^^^^^^^^^  MTIME
```

gzip writes the current time into a 4-byte header field unless told not to. `build.sh:694` is
`tar -czf`, so that is exactly what happens.

**Everything this pipeline does up to and including tar is already deterministic.** Four bytes
undo it.

## 2. D1 — `--options gzip:'!timestamp'`, not a pipe to `gzip -n`

Both were tested on this toolchain (bsdtar 3.5.3 / libarchive 3.7.4) and both produce identical
bytes across runs.

`--options gzip:'!timestamp'` wins on one ground: **it names the intent.** `-n` means "no name",
and suppressing the timestamp is a side effect of the same flag — a reader a year from now has to
know that to see why it is there.

The usual objection to bsdtar-only syntax does not apply: this pipeline is macOS-only by
construction (`codesign`, `otool`, the artifact contract's Mach-O checks), and Windows is cut from
development.

`set -euo pipefail` is already on line 31, so a pipe would have been *safe* — the choice is about
legibility, not correctness. Recorded because this repo has been bitten by pipes hiding exit codes
four times in one week, and the next reader deserves to know that was considered rather than
overlooked.

## 3. D2 — Every pin must be re-cut, or the fix is not finished

This is the part that makes it a slice rather than a one-line commit.

After the change the pipeline is reproducible — and **every hash in every catalogue then names
bytes the pipeline will no longer produce.** The pins would describe artifacts nobody can rebuild,
which is a worse state than today's: today nothing reproduces and the pins at least match files
that exist.

So: repack all three from their staged prefixes, re-pin all three, and confirm each pin is now
**reproducible** rather than merely *current*.

| engine | pinned today | tarball on disk |
|---|---|---|
| MariaDB 11.4.9 | `76ea96a4…` | present, matches |
| nginx 1.30.4 | `a29e7d61…` | present, matches |
| PHP 8.4.24 | `9fecd3a2…` | **gone** — the worktree holding it was cleaned up |

PHP is the reason this cannot wait: its pinned artifact no longer exists, so PHP **cannot be
released at all** until the pin is re-cut against something reproducible.

## 4. D3 — Say exactly what is proven, and what is not

**Proven by this slice:** from a given staged prefix, `pack` produces identical bytes every time.

**Not proven, and must not be claimed:** that a full build from source reproduces. That would need
a clean rebuild of each engine — for PHP, an spc checkout and roughly seventy minutes — and it can
fail for reasons that have nothing to do with gzip (embedded build timestamps, `__DATE__`, archive
member ordering, parallel-link nondeterminism).

The honest sentence after this lands is *"repacking a staged prefix is reproducible,"* not *"our
builds are reproducible."* The second is a much larger claim and this slice does not earn it.

It does, however, make it **testable for the first time** — the tar-level evidence above says
everything below gzip already agrees, so a full-rebuild comparison now has a meaningful chance of
succeeding, where before gzip guaranteed it would fail.

## 5. What this slice must prove

1. **Packing twice produces identical bytes**, on a real staged prefix, not a fixture.
2. **The raw tar is unchanged by the fix** — same content, same entries, same modes. Only the
   container changed. Otherwise this is a content change wearing a reproducibility label.
3. **All three artifacts repack, audit 7/7, and are re-pinned** to their new hashes.
4. **Each new pin reproduces**: pack again after re-pinning and get the recorded hash back.
5. The artifact contract still passes on each repacked tarball — a reproducible artifact that fails
   its own audit is not progress.
6. **Nothing user-visible changes.** Every `availability` stays `AwaitingRelease`; this slice
   changes which bytes we would serve, not whether we serve.

## 6. Out of scope

Proving a full source rebuild reproduces (D3) · `SOURCE_DATE_EPOCH` or any broader
reproducible-builds programme · publishing the releases, which is owner-gated and deferred · the
manifest's own `resumed_from` semantics, which already record honestly when a build was resumed.

**Recorded:** once a pin is reproducible, the release's owed step changes shape. Today it is *fetch
the served bytes and confirm the hash* — a check that the upload was not corrupted. After this, the
same check also confirms the **pipeline** still produces what the catalogue claims, which is a
different and stronger property.

## Postscript, 2026-08-08 — nginx's pin was not re-cut: an accepted, recorded deviation from D2

D2 and §5.3 above predate a fact found only in this slice's fix wave: between this pin being cut
(2026-08-06) and the fix wave running, `/opt/openvhost-build/nginx-1.30.4` was relinked against a
rebuilt OpenSSL (2026-08-07). A repack from today's prefix does not reproduce the pinned bytes —
`bin/nginx` differs, 611 byte positions, same size — even though it still passes the artifact
contract 7/7. **Passing the contract is not having provenance**, which is the reason this is
recorded rather than quietly re-pinned to whatever the drifted prefix now produces.

**This is a deviation, not a reinterpretation.** D2 says "repack all three… and re-pin all three",
and §5 item 3 makes it a proof obligation; nginx meets neither, so the slice ships one of its own
requirements unmet. Calling that anything softer would be the same move this branch exists to stop
— a document quietly adjusting until it agrees with what happened.

What it is instead: a deviation the evidence justifies. One of the three pins cannot be re-cut from
the current prefix without a documented OpenSSL rebuild first, which is separate work this slice
does not do, and re-cutting it anyway would trade an audited artifact for an undocumented one. The
obligation stands, still open, and the full account lives next to nginx's `sha256` field in
`crates/openvhost-core/src/nginx/package/catalogue.rs`.
