<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# OpenVHost's own package build pipeline — design

**Status:** approved. The four questions it raised were decided by the owner on
2026-08-02 and are recorded in §13.
**Date:** 2026-08-02.
**Why now:** the off-Homebrew programme (slices 3–5) cannot continue without it. MySQL
was the last service that could be *downloaded*; MariaDB, nginx and PHP publish no macOS
binaries at all, so from here we build or we stay on Homebrew.

## 1. Goal

Produce OpenVHost's own macOS packages for services upstream does not ship, as
**relocatable tarballs** that install into `~/.openvhost/packages/<name>/<major>/<version>/`
and run correctly from any path — the same contract Oracle's MySQL tarball already
satisfies, which is why slice 2 worked.

MariaDB proves the pipeline — **11.4 LTS only** (§13). nginx and PHP must then slot in
without changing the driver.

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
3. `WITH_SSL` is an **overloaded name**: the server accepts `bundled|system|<path>`, and it
   is `WITH_SSL=bundled` — wolfSSL for the server — that hands the bundled Connector/C to
   GnuTLS. `cmake/mariadb_connector_c.cmake` derives `CONC_WITH_SSL` from the top-level
   choice, and when the server picked wolfSSL it sets the connector to `GNUTLS` on every
   non-Windows platform; the connector then does `FIND_PACKAGE(GnuTLS REQUIRED)` and finds
   Homebrew's, dragging GnuTLS and Hogweed into the tree. One concrete prefix — ours — is
   the only value that gives both readers OpenSSL. (*Corrected 2026-08-03:* an earlier
   draft said the connector "falls into its GnuTLS branch on anything else"; it does not.
   Its GnuTLS branch is an exact `STREQUAL "GNUTLS"` match and its catch-all is
   `FATAL_ERROR "Invalid TLS/SSL option"`. The configuration was right, the reason given
   for it was not.) (→ D3).
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

**Decided (owner, 2026-08-02): static.** It removes the only remaining dylib, and with it the entire
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

**A `bundled` flag is not vendoring**, and "the source archive" is not the whole of the
input. MariaDB's `WITH_PCRE=bundled` and `WITH_LIBFMT=bundled` download pcre2 and fmt over
the network *during* the build and check them with `URL_MD5` and nothing else; both are
compiled into `mariadbd`, where §8's linkage check cannot see them, because a static
library that was compiled in leaves no entry in any link command. So the recipe fetches
both itself, verifies both itself — pcre2 by GPG signature, fmt by digest, since fmtlib
publishes no signature — seeds them where cmake's `ExternalProject` looks before it decides
to fetch, and runs the compile with the network taken away so that a download added by a
later upstream fails loudly instead of succeeding quietly. Verified means verified by us,
including for the inputs upstream's build system fetches on our behalf.

**The build manifest** published with every artifact records: upstream URL and its verified
sha256, the signing key fingerprint, every configure flag, the toolchain versions
(`cmake`, `bison`, `clang`, macOS SDK), the neutral build prefix, the output sha256, and —
for each input the build system would otherwise have fetched for itself — its URL, its
verified digest, and how far that verification actually goes. Not bit-reproducible — but
auditable, which is the achievable goal for a single-builder pipeline.

## 8. D6 — The artifact contract

A tarball is acceptable only if **all** of these hold. The audit is a script; a failure is
a failed build, not a warning.

1. Extracts to a single root containing at least `bin/` and `share/`.
2. **Linkage:** every `otool -L` entry of every Mach-O is `/usr/lib/*`, `/System/*`,
   `@loader_path/...` or `@rpath/...`, **and** every `LC_RPATH` is `@loader_path`-relative.
   Nothing else. *Amended 2026-08-02 during implementation:* real MariaDB ships
   `LC_ID_DYLIB = @rpath/libmariadb.3.dylib` alongside `LC_RPATH = @loader_path/../lib`,
   which is the idiomatic macOS pattern for a self-contained tree rather than a defect.
   Admitting `@rpath` on its own would have been a hole rather than a widening — it is only
   as relocatable as the `LC_RPATH` entries that resolve it, and `otool -L` never shows one —
   so the rpath condition arrives with it.
3. **Signature:** every Mach-O is signed and `codesign -v` passes.
4. **No builder identity anywhere in the tree.** Today's tree fails this — 13 files carry
   the staging path. See D8 for how the prefix is chosen so that what remains carries no
   identity, and this check enforces that the builder's real directories never appear.
   *Amended 2026-08-03:* the exemption for the build root is now one named directory — the
   install prefix the staged tree unavoidably is — and the `_work` subtree, whose paths
   appear only in compiler debug info. It used to be the whole root and everything under
   it, which is precisely how the contract came to be blind to check 7's finding.
5. **Runs from two different paths.** Automated: install to A, run, move to B, run again.
   A single-location test cannot detect the defect this whole pipeline exists to avoid.
6. **Serves and survives.** Start the server, create a table, insert, restart, read back —
   the proof performed today, run as a gate rather than once by hand.
7. **No absolute path embedded anywhere in the tree has a world-writable ancestor.**
   *Added 2026-08-03, after a security audit BLOCKed the first artifact.* Checks 4 and 7
   ask different questions about the same string, and only 7 asks the one that decides
   whether the package is safe: not "does this path name the builder" but "can anything
   unprivileged create it". `/tmp/openvhost-build/mariadb-11.4.9` names nobody and passed
   check 4 — and `mariadbd` resolves `basedir`, `plugin_dir` and `character-sets-dir` out
   of it, on machines where the tree does not exist and `/tmp` is mode 1777 (CWE-426,
   CWE-427). Checks 5 and 6 could not see it either: both pass `--basedir` explicitly.

   Where upstream's own corpus cannot satisfy this literally, the exceptions are declared
   **in the recipe**, and every one of them is printed on every audit run —
   `RECIPE_INERT_PATHS` for subtrees that are documentation or test fixtures, and
   `RECIPE_ALLOWED_WRITABLE_PATHS` for individual paths, each traced to the file that
   carries it. An exception that has to be written down next to a reason, and that the
   audit reads out loud, is a different thing from one built into the checker.

   Checks 5 and 6 execute the artifact, so they additionally require `--execute-artifact`.
   `build.sh` passes it because it built the tree it is auditing; someone auditing a
   tarball they were handed is not in that position and must opt in deliberately.

## 9. D7 — One driver, one recipe per package

`build/build.sh <name> <version>` drives a fixed sequence; `build/recipes/<name>.sh`
supplies only the package-specific parts:

```
fetch → verify signature → extract → configure → build → install
      → normalize (install names) → audit (§8) → sign → pack → manifest
```

MariaDB proves it. nginx and PHP must slot in **without changing the driver** — if either
needs the driver changed, that is a finding to report, not a change to make quietly.

## 10. D8 — Build under a neutral prefix that nothing unprivileged can create

Because 13 files embed the staging path and post-processing all of them is fragile, the
build uses a **stable prefix** — `<root>/<name>-<version>` — rather than a session temp
directory, and contract check 4 enforces that the owner's real paths (home directory,
project directories, session scratchpads) never appear in it.

**Corrected 2026-08-03, after a security audit BLOCKed the first artifact.** This section
originally read `/tmp/openvhost-build/<name>-<version>`, and justified it with "anything
that leaks is then inert". *That sentence was wrong*, and it was wrong in the way that
matters: it confused **carrying no information** with **having no effect**.

An embedded prefix is not a label. `mariadbd` resolves `basedir`, `plugin_dir` and
`character-sets-dir` from it, and it does so on a machine where that tree does not exist.
`/private/tmp` is mode 1777. So any unprivileged local process could have created
`/tmp/openvhost-build/mariadb-11.4.9/lib/plugin` and put a dylib in it, or a
`share/charsets/Index.xml`, and the server would have loaded it: CWE-426 / CWE-427. A
meaningless name that anyone can claim is not inert; it is an unclaimed name.

So the prefix must satisfy **two** properties, and both are enforced rather than assumed:

- **neutral** — it identifies nobody. Contract check 4.
- **un-plantable** — no ancestor of it is world-writable, so unprivileged code cannot
  create the tree the package names. `build.sh` refuses to run otherwise, and contract
  check 7 re-derives it from the finished artifact rather than trusting the driver.

The default root is therefore **`/opt/openvhost-build`**: `/opt` and `/` are root-owned and
mode 755, so on any Mac the path is un-plantable, and it names no user.

The consequence is not a wart to be engineered around — it is the same fact stated twice. A
directory unprivileged code cannot create is a directory unprivileged code cannot create,
including for us, so preparing a build machine costs one privileged `mkdir`, once:

```
sudo mkdir -p /opt/openvhost-build && sudo chown "$(id -u):$(id -g)" /opt/openvhost-build
```

`build.sh` prints exactly that command when the root is missing or unsafe. The root itself
is then verified to be owned by the builder and mode 0700 before any build writes to it.

One embedded path was fixed rather than tolerated: upstream compiles `MYSQL_UNIX_ADDR` as
`/tmp/mysql.sock` into `mariadbd`, every client, `libmariadb.3.dylib`, `mysql_config` and
`mariadb.pc` — 27 shipped files. Anything on the machine can bind that name first and
collect the credentials of every client that connects to "localhost" without an explicit
`--socket`, so the recipe pins it inside the prefix instead.

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

## 13. Decisions taken (owner, 2026-08-02)

All four were put to the owner as open questions and answered the same day.

1. **Single-builder trust — accepted.** Builds run on the owner's Mac with no independent
   reproduction. The build manifest (§7) is what makes it auditable; it is now mandatory,
   not a nicety.
2. **The security-update obligation — accepted explicitly**, not by implication. See §14:
   accepting it without a mechanism is how it gets forgotten, so it acquires one here.
3. **MariaDB 11.4 LTS only.** No 10.x, no 11.7. Every extra major is another tree to
   build, verify and patch, and the obligation in §14 scales with that count. Adding a
   major later is a decision with a cost, not a configuration change.
4. **Static OpenSSL.** The `@loader_path` bundling proven on 2026-08-02 stays documented in
   D3 as the fallback if static turns out impractical — falling back is a reported finding,
   not a quiet substitution.

## 14. The security-update mechanism

§11's obligation is now owned, so it needs a way to fire. An accepted obligation with no
trigger is an intention.

- **Watch list, recorded in the repo** next to the catalogue: MariaDB 11.4 releases and
  the OpenSSL 3.x advisory feed, being the two things we compile and ship. Add one entry
  per package as the pipeline grows to nginx and PHP. **pcre2 and fmt belong on it too** —
  both are compiled into `mariadbd` (see D5) — with one qualification worth stating,
  because it changes what the entry means: their versions are MariaDB's choice, not ours,
  since cmake insists on its own `URL_MD5`. The answer to a pcre2 CVE is therefore a
  MariaDB release that bumps it, not a number we edit.
- **The pin is the tripwire.** The catalogue already carries `{version, url, sha256}`; add
  the upstream release date and the date we last checked. A stale check is then visible in
  the source rather than remembered.
- **Rebuild is a slice, not a patch.** A CVE means: re-verify upstream's signature, rebuild
  through the same recipe, re-run the artifact contract (§8), publish, bump the catalogue.
  The contract is what makes that safe to do quickly.
- **We cannot silently inherit a fix any more.** Until now a user's `brew upgrade` patched
  their PHP and MySQL without us. From the first package we build, that stops being true
  for anything we ship — and the user has no other route to the fix.
