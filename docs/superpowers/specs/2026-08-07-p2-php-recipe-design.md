<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# PHP from source — recipe and artifact (off-Homebrew slice 5A)

**Status:** design, ready to plan. One owner decision already made (§3).
**Date:** 2026-08-07.
**Follows:** nginx 4A–4C (#57–#59), and spike 5-0, which answered the three questions this
design would otherwise have had to guess at.

## 1. Why PHP is the last one, and the hardest

PHP is the final thing tying this app to Homebrew. After it, a machine with no Homebrew can
serve a PHP site end to end — which is the entire point of the programme.

It is also the worst build target we have: nginx was one static binary with one dependency;
PHP has ~18 and a plugin surface users expect to extend.

## 2. What the spike established — do not re-derive any of this

| Question | Answer |
|---|---|
| Does an spc-built php-fpm run **our** pool config from a **relocated** tree? | **Yes.** Moved twice, second time into a path containing spaces, served a real `.php` through real nginx with the app's exact argv. Response **byte-identical** both times |
| Would the artifact contract accept it? | **Check 5 (relocation) passes as built.** Check 2 fails only on residual `LC_RPATH`, check 3 only on unsigned `.dwarf` — both fixable in `recipe_normalize`. Check 7's failures were an artefact of building in `/tmp` (1777); `/opt/openvhost-build` is 700 |
| Can a static php-fpm load a shared extension under `-n`? | **Yes**, and `extension_dir` is overridable at runtime with `-d`, so the compiled-in `/lib/…` (sealed system volume, unusable) stops mattering. **opcache and xdebug both load with no php.ini**, proven through a real HTTP request |
| Does spc verify what it downloads? | **Nothing.** No hash, no signature, anywhere in its config. `curl -fSL` with **no `--proto`/`--proto-redir`**. Only 44 of 131 sources name a fixed version; the rest resolve "latest" at download time; every `git` source tracks a branch. php.net's API hands spc a `sha256` and **spc discards it** |
| Does `spc build` touch the network? | **No**, on macOS. The exposure is confined to `spc download` |

Two behaviours found by accident that must survive into the design:

- **`-c` beats `-n`.** `php -n -c <dir>` loads the ini anyway. If this app ever gains a `-c`,
  `-n`'s guarantee silently evaporates.
- **Without `-n`, `PHP_INI_SCAN_DIR` alone injected two Zend extensions** into the process with
  no php.ini present. `-n` is doing real security work, and the pool's `clear_env` does **not**
  cover it — that scrubs the worker's environment for scripts, not the master's at startup.

## 3. D1 — Pin everything ourselves; `spc build` runs offline

**Owner decision, 2026-08-07.** The recipe declares every source URL and SHA-256;
`bp_download` and `bp_verify_sha256` do the fetching and verifying; only then does `spc build`
run, with no network available to it.

The alternative — letting `spc download` fetch ~131 unverified sources — was rejected because
the pipeline's entire value is being able to say which bytes we shipped, and PHP is the package
that runs the user's code. Accepting it would mean signing an artifact assembled from bytes we
never checked.

Cost, stated plainly rather than discovered: **~31 URLs and digests maintained by hand**, and
they will rot silently as spc's own manifest moves upstream. D4 is how that rot announces
itself.

**PHP itself gets the full treatment**, not just a hash: php.net publishes `.asc` signatures,
so the source tarball is GPG-verified the way nginx's and MariaDB's are — parse `--status-fd`,
require `VALIDSIG <pinned-fpr>`, reject `EXPKEYSIG|REVKEYSIG|BADSIG|ERRSIG|EXPSIG`. **`gpg
--verify` exits 0 on an expired key**; that was reproduced live during the nginx slice and is
not folklore.

## 4. D2 — `pkg-config` comes from the build host, never from spc

spc fetches `pkg-config` as **a third party's latest GitHub release, with no version pin and no
digest, and then executes it** inside the build. Pinning its URL does not fix that — the
binary would still be unverified.

It is a build tool, not a source, so it belongs in `RECIPE_BUILD_TOOLS`, resolved to an
absolute path before `PATH` is scrubbed. The precedent is exact: MariaDB pins
`BISON_EXECUTABLE` at the build host's bison **because ServBay shipped a broken one on `PATH`
that could not run at all and broke that very build.**

`spc doctor --auto-fix` runs `brew install` — it installed `automake` on the build host
unprompted during the spike. **The recipe must never call it.**

## 5. D3 — Extract the GPG logic into `build.sh` first, as this slice's first task

Import-and-verify is now written three times — `openssl.sh`, `mariadb.sh`, `nginx.sh` —
identical down to the `awk` that reads the primary fingerprint. PHP makes four.

This was deferred at 4A with a stated reason: the auditor had just attacked that parser five
ways, and extracting it then would have meant the merged code was not the code that was
audited. The condition given for revisiting was "when all four copies are in view." They are.

**Extract before writing PHP's recipe, not alongside it.** The gate is that all three existing
artifacts still audit **7/7** against the shared helper — provable in isolation, which it stops
being once new code is tangled with it.

## 6. D4 — Pinning that rots silently is worse than no pinning

~31 hand-maintained pins will drift from spc's manifest, and nothing will say so until a build
breaks — or worse, does not.

The `include_str!` tripwire that ties `catalogue.rs` to the recipe file already exists and
already fires (proven during 4A). Extend the same idea: the recipe records the spc commit its
pin set was derived from, and a check fails when the recipe claims a version the pinned set was
not built against. An obligation with no trigger is an intention.

## 7. D5 — The embedded OpenSSL module paths get the `plugin-dir` treatment

`bin/php` and `bin/php-fpm` embed statically-linked OpenSSL 3's `MODULESDIR` and `ENGINESDIR`
as `<build-prefix>/lib/ossl-modules` and `.../lib/engines-3`, and the tree really does ship
`lib/ossl-modules/legacy.dylib`.

This is the exact shape of the MariaDB finding where "a neutral prefix is not an inert one" —
that one passed every check until someone ran `mariadbd --verbose --help` and found
`plugin_dir` resolving out of a mode-1777 tree. Check 7 is what decides it here, and it will
pass **only because of where the pipeline builds**. Treat that as load-bearing, not as luck,
and say so in the recipe.

## 8. D6 — Extensions are `-d` pairs, not a php.ini generator

The app generates no php.ini today and must not start. Per-extension knobs become extra `-d`
pairs in `php_fpm_spec`'s argv, which keeps `-n` intact and keeps every knob in one auditable
place — rather than a new config surface whose precedence rules (§2) are subtle enough to have
surprised this spike.

`extension_dir` is set with `-d` to the package tree's own `modules/`, never left to the
compiled-in value.

## 9. What slice 5A must prove

1. The recipe builds PHP from **verified** upstream source: the tarball's GPG signature checked
   through `--status-fd`, and every pinned dependency's SHA-256 checked before `spc build` runs.
2. `spc build` runs with **no network reachable**, and the build fails if it tries.
3. All seven contract checks pass on a **real rebuild** — including check 6, a
   `recipe_serve_probe` that serves a real `.php` over FastCGI, compares the body, restarts and
   serves again. Not `php-fpm -t`, and not a version print.
4. The artifact is relocatable: move it twice and it still serves.
5. **opcache and xdebug load** from the relocated tree, through php-fpm, with no php.ini.
6. `openvhost-pkg` installs it into `packages/php/<major>/<version>/` and the compiled-in
   SHA-256 matches.
7. **Nothing user-visible changes.** Discovery is 5B; the Languages page is 5C.

## 10. Out of scope

Discovery and the `PhpRuntimeSource` enum (5B) · the Languages page and routing Install to our
package (5C) · uninstall, which is the first target whose plan depends on runtime state and
needs its own design note (5D) · a php.ini generator (D6 says never) · Windows.
