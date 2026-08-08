<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# nginx's pin has no provenance, and the hole that made that invisible is still open

**Status:** design, ready to plan.
**Date:** 2026-08-08.

## 1. What the reproducible-pack slice left behind

`nginx` was the one engine that slice refused to re-pin. Its staged prefix differs from the bytes
the catalogue names by exactly one file — `bin/nginx`, same size 6 131 760, **611 differing byte
positions** — with a different `LC_UUID` and a different embedded OpenSSL string:

```
built on: Mon Aug  3 15:05:42 2026 UTC   ← the artifact the catalogue pins
built on: Fri Aug  7 02:52:02 2026 UTC   ← the prefix on disk today
```

Refusing was right. A repack of the drifted prefix **passes the artifact contract 7/7** — verified
live, zero checks skipped — so swapping the pin would have replaced an audited artifact with an
unexplained one while every gate stayed green. **Passing the contract is not having provenance.**

That left the pin naming bytes whose only copy is gitignored output inside a prunable worktree, and
a build root whose nginx prefix disagrees with the catalogue. This slice closes both.

## 2. Measured: what actually drifted, and why nothing noticed

Not inferred — read off the disk:

| path | mtime |
|---|---|
| `/opt/openvhost-build/openssl-3.5.7` | Aug 7 09:52:10 |
| `/opt/openvhost-build/nginx-1.30.4/bin/nginx` | Aug 7 09:52:59 |

49 seconds apart, and 09:52 local is **02:52 UTC** — the exact timestamp embedded in the drifted
binary. OpenSSL was rebuilt and nginx was linked against it in the same driver run.

**The hole that made this silent is `build.sh:566`:**

```bash
dep_prefix="$(bp_dep_prefix "$dep_name" "$dep_version")"   # <root>/<name>-<version>
if [ -d "$dep_prefix" ]; then
    bp_log "dependency already built: $dep_name $dep_version"
    continue
fi
```

A dependency is satisfied by **directory existence alone**. No digest, no manifest, no content
check. And the consumer records only the version string:

```
"openssl": {"version": "3.5.7", "linkage": "static"}
```

Two different builds of 3.5.7 produce that identical line. So a rebuilt OpenSSL silently changes
what every consumer links against, and **no artifact anywhere carries a value that would differ.**
`openssl-3.5.7` has three consumers — nginx, PHP and MariaDB.

MariaDB and PHP did not drift only because nothing re-ran *their* link stage afterwards. That is
luck, not a property. The same silence is waiting for both.

## 3. D1 — Rebuild rather than repack, even though repacking would probably work

The Aug 7 prefix was very likely produced by a complete hermetic pipeline run: the mtimes line up
with a dependency build immediately followed by a consumer build, and `openssl.sh` verifies its
source by GPG signature *and* pinned SHA-256 before compiling.

**"Very likely" is the thing this line of work exists to stop accepting.** No manifest, no log, and
no artifact survive from that run, so its provenance can only be inferred — and a pin resting on an
inference is the defect, not the fix. A rebuild we observe produces provenance nobody has to
reconstruct.

**Rebuild OpenSSL too**, from its verified source, rather than reusing the directory that happens to
be there. Its *source* provenance is good; its *build* has no record. Reusing it would put the new
nginx pin back on exactly the footing this slice is removing, one layer down.

## 4. D2 — The pin's property is prefix-level, and must be stated that way

After this lands, nginx has the property MariaDB and PHP already have: **repacking its staged prefix
reproduces the pinned bytes.** That is what the gzip fix bought and it is worth having.

It is *not* "a full rebuild reproduces". The reproducible-pack audit measured that as **provably
false today**, not merely unproven: all 100 nginx OSO debug stabs are rooted at
`/opt/openvhost-build/_work/…` and carry per-object mtimes, and every one differs between two
builds. Two rebuilds will not agree until `ZERO_AR_DATE`, `-no_uuid` and deterministic object mtimes
are in play, which is out of scope here.

So the new pin is cut once from an observed build, and its reproducibility claim is the repack.

## 5. D3 — Record which dependency build a consumer linked, so the next drift is visible

The rebuild alone is a one-time cleanup. Without this, the same silent drift recurs and is again
discovered 611 bytes at a time.

Record, in the manifest, a digest identifying the dependency prefix each consumer was built
against. Requirements:

- **Computed centrally, in `build.sh`, from `RECIPE_DEPENDS`** — not in each recipe. Three recipes
  reimplementing it is three chances to diverge, and this pipeline has already been bitten by
  thrice-duplicated GPG code where one copy compared the wrong field.
- **Over the staged dependency prefix's content**, so it changes when the dep is rebuilt. Stable
  across runs that do not rebuild the dep — a value that changes on every invocation records
  nothing.
- **Present for every engine**, so MariaDB's and PHP's manifests gain it too. Regenerating those is
  a `--from pack` per engine and their pins are already proven to reproduce, so their `sha256`
  entries must come back **identical** — if either moves, stop and report, because the manifest is a
  sidecar and cannot change the tarball.

Explicitly **not** in scope: content-addressed dependency prefixes, rebuilding a consumer when its
dep digest moved, or refusing to reuse a mismatched dep. Recording makes the next drift *visible*;
acting on it automatically is a larger design and a separate slice.

## 6. D4 — A scoped, inventoried exception to the read-only build root

`/opt/openvhost-build` is treated as strictly read-only by every brief in this project: it holds the
only build output of four engines and there is no second copy. A rebuild necessarily writes there.

The exception is **exactly these four paths** — `openssl-3.5.7`, `nginx-1.30.4`,
`_work/openssl-3.5.7`, `_work/nginx-1.30.4` — and nothing else. `mariadb-11.4.9` (562 MB),
`php-8.4.24` (141 MB) and the pre-existing `_work/php-8.3.33` and `_work/php-8.5.9` trees must be
byte-identical before and after, proven by a full inventory on both sides rather than asserted.

Regenerating MariaDB's and PHP's manifests under D3 reads their prefixes and writes only to
`build/out`, so it does not widen this.

**The currently pinned artifact must be preserved before anything is overwritten.** Its only copy
is `.claude/worktrees/nginx-build/build/out/nginx-1.30.4-macos-arm64.tar.gz`. It stops mattering the
moment the re-pin lands, but until then it is the bytes the shipped catalogue names.

## 7. What this slice must prove

1. **OpenSSL and nginx both rebuilt from verified source**, observed, with the driver's own output
   kept — not reconstructed afterwards.
2. **The new pin repacks reproducibly**: pack the rebuilt prefix twice, get identical bytes, and
   after re-pinning get the recorded hash back.
3. **`build/audit.sh --execute-artifact` passes 7/7** on the new tarball, zero skipped.
4. **The manifest records the OpenSSL build**, and the value demonstrably changes when the dep is
   rebuilt and holds steady when it is not.
5. **MariaDB's and PHP's `sha256` pins are unchanged** after their manifests are regenerated.
6. **Nothing user-visible changes**: every `availability` stays `AwaitingRelease`.
7. **The build root outside the four exempt paths is byte-identical** before and after.
8. The catalogue's nginx doc comment and the reproducible-pack spec's postscript both stop
   describing an exception that no longer exists — D2's obligation is now met, and the record should
   say so rather than leaving a reader to work it out.

## 8. Out of scope

Making a full source rebuild reproduce (D2) · acting automatically on a changed dependency digest
(D3) · publishing any release, which is owner-gated and deferred · the `bp_rm_tree "$BUILD_WORK"`
question already filed separately · the `PROBE_TIMEOUT` question already filed separately.
