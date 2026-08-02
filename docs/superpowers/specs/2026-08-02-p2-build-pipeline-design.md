<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# OpenVHost's own package build pipeline — design

**Status:** design, awaiting owner approval on the open questions in §9.
**Date:** 2026-08-02.
**Why now:** the off-Homebrew programme (slices 3–5) cannot continue without it. MySQL
was the last service that could be *downloaded*; MariaDB, nginx and PHP publish no macOS
binaries at all, so from here we build or we stay on Homebrew.

## 1. Goal

Produce OpenVHost's own macOS packages for services upstream does not ship, as
**relocatable tarballs** that install into `~/.openvhost/packages/<name>/<major>/<version>/`
and run correctly from any path — the same contract Oracle's MySQL tarball already
satisfies, which is why slice 2 worked.

MariaDB proves the pipeline. nginx and PHP must then slot in without changing the driver.

## 2. What is already proven, and what it cost to learn

All of this was measured on 2026-08-02, not assumed. It is the evidence every decision
below rests on.

| Proven | How |
|---|---|
| MariaDB 11.4.9 builds **fully relocatable** on macOS | `INSTALL_LAYOUT=STANDALONE` + `CMAKE_INSTALL_RPATH=@loader_path/../lib` |
| It runs from a path it was never built for | installed, then **moved twice**; `@@basedir` reports the final location |
| It loads **its own** dylibs, not the host's | `DYLD_PRINT_LIBRARIES` shows `<tree>/lib/libssl.3.dylib` |
| It serves real SQL | created a database and table, inserted rows, aggregates, on **InnoDB** |
| Data survives a restart | clean shutdown → restart → both rows read back |
| Only one external dependency survives | `libssl.3` + `libcrypto.3`; everything else is `/System` or `/usr/lib` |
| `@loader_path` rewriting works end to end | `install_name_tool -change` + ad-hoc `codesign`, then moved again and re-run |

**ServBay solves the same problem the other way and we measured the cost.** Their
`mariadbd` links absolute paths (`/Applications/ServBay/package/common/lib/libz.1.dylib`),
so their binaries are correct only inside their own tree — and they put that tree's `bin`
on the user's `PATH`. Their shipped `bison` **cannot run at all** (its compiled-in data
directory was never packaged) and it shadowed every other bison on this machine and broke
this very build. We take their unavoidable decision — build it ourselves — and reject the
mechanism.

Four build failures, each of which became a requirement below:

1. ServBay's broken `bison` on `PATH` (→ D2, pin every tool).
2. The host leaked in from **two package managers at once** — `GNUTLS`/`HOGWEED` from
   `/opt/homebrew`, `GSSAPI_INCS` and `KRB5_CONFIG` from `/Applications/ServBay` (→ D2).
3. `WITH_SSL` is an **overloaded name**: the server accepts `bundled|system|<path>`, the
   bundled Connector/C accepts `ON|OPENSSL|GNUTLS` and falls into its GnuTLS branch on
   anything else (`libmariadb/CMakeLists.txt:346`) (→ D3).
4. Scrubbing `PATH` before resolving `cmake` removed `cmake` (→ D2).

And one thing the proof nearly missed: **13 files in the finished tree still embed the
build machine's staging path** — `mariadbd-safe`, `mysql_config`, `mysql.server`,
`mariadb.logrotate`, `my_config.h` and friends. The server ran anyway because the proof
passed `--basedir` explicitly. Zero files embed `/opt/homebrew`, so the hermetic flags
worked on linkage but not on text (→ D6, D8).

## 3. D1 — Where builds run

**Decision: on the owner's Mac, from a checked-in recipe under `build/`, not GitHub Actions.**

Actions is technically enabled but has not run since 2026-07-22 and the owner disabled it
to save minutes; local gates are the merge gate. A pipeline that assumes CI would be a
pipeline that never runs.

**The honest consequence, stated rather than buried:** artifacts are trusted because the
owner built them. There is no independent reproduction and no second pair of eyes on the
bytes. Every release therefore publishes a **build manifest** (§7) so the inputs are at
least auditable. Revisit when CI returns — the recipe is deliberately a plain script so a
runner can execute it unchanged.

## 4. D2 — Hermeticity is enforced by verification, not by intention

Scrubbing the environment is necessary and **not sufficient** — today's proof showed
leakage arriving by two routes at once, and a third (embedded text paths) that no linker
flag addresses.

The recipe does all of:

- resolve build tools (`cmake`, `make`, `bison`) to absolute paths **before** scrubbing;
- `PATH` reduced to the system toolchain plus those tools;
- unset `PKG_CONFIG_PATH`, `CPATH`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `LIBRARY_PATH`,
  `LDFLAGS`, `CPPFLAGS`;
- `-DCMAKE_IGNORE_PREFIX_PATH` covering `/opt/homebrew`, `/usr/local`, `/Applications/ServBay`;
- **every** `WITH_*` pinned explicitly — `auto` is the enemy, and is how `WITH_PCRE` picked
  up Homebrew's pcre2.

But the **gate** is the linkage audit in §8, which fails the build on any leak regardless
of how it arrived. Configuration expresses intent; the audit is what we actually rely on.

## 5. D3 — OpenSSL: build it ourselves, and link it statically

Building it ourselves is not optional: the proof tree currently contains **Homebrew's**
`libssl`/`libcrypto`, which we cannot redistribute.

**Prefer static linking.** It removes the only remaining dylib, and with it the entire
install-name rewriting step, the dylib re-signing step, and one whole class of runtime
failure. `otool -L` on the result should show nothing but `/usr/lib` and `/System`.

**Fallback, already proven, if static turns out impractical:** ship `libssl.3.dylib` and
`libcrypto.3.dylib` in the package's `lib/`, with `@loader_path` ids and `@loader_path/../lib/…`
references from `bin/`. That is exactly what Oracle does for MySQL and what today's proof
performed by hand.

Pass a single concrete OpenSSL prefix so the server and the bundled Connector/C agree
(failure 3 above). Do not pass `bundled`.

## 6. D4 — Signing is a build step, not a release step

Two facts make this structural rather than cosmetic:

- **Apple Silicon refuses to execute unsigned code.** A package that is not signed at all
  does not run, on any Mac.
- **Every `install_name_tool` edit invalidates the signature.** Signing must therefore be
  the *last* step, after all Mach-O modification.

So the recipe ad-hoc signs (`codesign -f -s -`) every Mach-O it produces, at the end.
Developer ID signing and notarization belong to the signing slice and are out of scope
here — but ad-hoc must be in place from day one or nothing we build will start.

We control quarantine: `com.apple.quarantine` is set by the *downloading application*, and
the downloader is our own `openvhost-pkg`, which does not set it. Recorded during the
extractor slice.

## 7. D5 — Distribution and provenance

**Distribution: GitHub Releases**, one release per `<name>-<version>`, carrying the
tarball, its `.sha256`, and the build manifest. It is free, versioned, and
`openvhost-pkg`'s download-and-verify path already consumes exactly this shape.

**The catalogue stays compiled in**, identical in shape to MySQL's
`{ version, url, sha256 }` (`crates/openvhost-core/src/mysql/package/catalogue.rs`).
Remote manifests remain slice 6; consistency beats novelty.

**Upstream provenance must be verified, not assumed.** MariaDB publishes a **GPG-signed**
`sha256sums.txt` — strictly better than Oracle, who published no SHA-256 at all and forced
us to compute one. The recipe verifies that signature, and the key fingerprint is
cross-checked against a second host, exactly as the MySQL slice did. Record the key id,
its expiry, and the verification date alongside the pin.

**The build manifest** published with every artifact records: upstream URL and its verified
sha256, the signing key fingerprint, every configure flag, the toolchain versions
(`cmake`, `bison`, `clang`, macOS SDK), the neutral build prefix, and the output sha256.
Not bit-reproducible — but auditable, which is the achievable goal for a single-builder
pipeline.

## 8. D6 — The artifact contract

A tarball is acceptable only if **all** of these hold. The audit is a script; a failure is
a failed build, not a warning.

1. Extracts to a single root containing at least `bin/` and `share/`.
2. **Linkage:** every `otool -L` entry of every Mach-O is `/usr/lib/*`, `/System/*`, or
   `@loader_path/...`. Nothing else.
3. **Signature:** every Mach-O is signed and `codesign -v` passes.
4. **No builder identity anywhere in the tree.** Today's tree fails this — 13 files carry
   the staging path. See D8 for how the prefix is chosen so that what remains is harmless,
   and this check enforces that the builder's real directories never appear.
5. **Runs from two different paths.** Automated: install to A, run, move to B, run again.
   A single-location test cannot detect the defect this whole pipeline exists to avoid.
6. **Serves and survives.** Start the server, create a table, insert, restart, read back —
   the proof performed today, run as a gate rather than once by hand.

## 9. D7 — One driver, one recipe per package

`build/build.sh <name> <version>` drives a fixed sequence; `build/recipes/<name>.sh`
supplies only the package-specific parts:

```
fetch → verify signature → extract → configure → build → install
      → normalize (install names) → audit (§8) → sign → pack → manifest
```

MariaDB proves it. nginx and PHP must slot in **without changing the driver** — if either
needs the driver changed, that is a finding to report, not a change to make quietly.

## 10. D8 — Build under a neutral prefix

Because 13 files embed the staging path and post-processing all of them is fragile, the
build uses a **stable, meaningless prefix** — `/tmp/openvhost-build/<name>-<version>` —
rather than a session temp directory. Anything that leaks is then inert, and contract
check 4 enforces that the owner's real paths (home directory, project directories, session
scratchpads) never appear.

OpenVHost spawns server binaries directly and **never** through `mysqld_safe`-style
wrappers — an existing rule from the MySQL slice, where `mysqld_safe` carried a hardcoded
`/usr/local/mysql/data`. That rule is what makes the residual embedded paths tolerable, so
it is now load-bearing and must not be relaxed.

## 11. Cost, stated plainly

| | measured today |
|---|---|
| MariaDB compile | ~3 min on 16 cores, heavy plugins disabled |
| build tree | 3.4 GB |
| installed tree | 465 MB |

The real cost is not compute. **Leaving Homebrew makes us responsible for security
updates.** Today a `brew upgrade` patches the user's PHP and MySQL; afterwards, a CVE in
OpenSSL, MariaDB, nginx or PHP is ours to notice, rebuild, re-verify and ship. That
obligation arrives with the first package we build, not with 1.0.

## 12. Out of scope

- Developer ID signing and notarization (signing slice).
- Remote manifests (programme slice 6).
- Retiring Homebrew discovery (programme slice 7).
- Windows. Intel Macs — no signature-checked x86_64 pin exists today, and this pipeline
  does not change that.
- Bit-for-bit reproducible builds.

## 13. Open questions for the owner

1. **Single-builder trust (D1).** Artifacts will be built on your Mac with no independent
   reproduction. Acceptable for now, or should this block until CI is back?
2. **The security-update obligation (§11).** This is the decision with the longest tail.
   Confirm explicitly rather than by implication.
3. **Which MariaDB versions to ship.** ServBay offers 10.4 → 11.7. Every additional major
   is another tree to build, verify and patch. Recommendation: **11.4 LTS only** to start.
4. **Static or bundled OpenSSL (D3)** — recommendation is static; confirm the fallback is
   acceptable if it proves impractical.
