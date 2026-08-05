<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# nginx from source — recipe and artifact (off-Homebrew slice 4A)

**Status:** design, ready to plan.
**Date:** 2026-08-06.
**Follows:** the build pipeline (#49–#52), which proved the driver on MariaDB 11.4.9.

## 1. Why nginx, and why now

**Without Homebrew this app cannot serve a single site.** `find_brew_binaries`
(`demo_stack.rs:31-41`) returns `None` unless it finds both `opt/nginx/bin/nginx` and
`opt/php/sbin/php-fpm`, and there is no nginx package tree. MySQL and MariaDB can now be
installed by the app itself — and nothing can serve their sites.

nginx is also the cheapest build target we will ever have: **one static binary**, no plugins,
no charsets, no message catalogues. MariaDB was 125 MB; this is ~3 MB.

## 2. Scope — this slice ends at a verified artifact

**In:** `build/recipes/nginx.sh`, whatever the driver needs in order to accept a
non-database package, and a compiled-in catalogue entry so `openvhost-pkg` can install it.

**Out, and deliberately:** discovery (slice B), replacing Homebrew at runtime (slice C), any
UI. Nothing user-visible changes in this slice. A machine still serves from Homebrew's nginx
when this merges.

**Not in scope and not owed:** publishing. Like MariaDB, this ships
`Availability::AwaitingRelease` and the download path stays unexercised until the owner
publishes. That debt is already recorded against MariaDB; this slice adds a second package to
the same recorded precondition rather than a new one.

## 3. Measured before deciding

Every line below came from reading the code on `a2cd8e2`.

| Fact | Consequence |
|---|---|
| `build.sh` and `audit.sh` contain **zero** MariaDB identifiers in executable code; everything package-specific arrives through `RECIPE_*` variables | A second recipe is genuinely a recipe, not a driver fork |
| `RECIPE_DEPENDS=("openssl:3.5.7")` already works (`recipes/mariadb.sh:150`) | `--with-http_ssl_module` against our staged static OpenSSL costs nothing extra |
| `audit.sh:359` hardcodes `for want in bin share` | **Collides.** nginx's `make install` produces `sbin/ conf/ html/ logs/` — see D1 |
| `audit.sh:706` prints `"started, created, inserted, restarted, read back"` on a check-6 pass | A database-shaped sentence in the generic gate. For nginx it would be a lie — see D4 |
| The generated nginx config is **entirely home-relative** — no `include mime.types`, no `load_module`, no `error_page`, no prefix-relative anything | **The MariaDB analogy does not carry.** There is no four-directory equivalent |
| The MIME map is inlined (`main.conf.tera:43-63`) and all 18 `fastcgi_param` lines are written out (`php-location.conf.tera:4-21`) | The package ships the binary and nothing else |
| `-e <err_log>` is already mandatory on every nginx invocation (`inspect.rs:308-313`) | Hardening aimed at Homebrew's prefix is exactly what a packaged nginx needs |
| `nginx -V` bakes the whole `configure` line into the binary | Check 4 (identity) is **more** load-bearing here than for MariaDB; check 7 (plantable) is far cheaper |
| nginx has no `--version`; it is `-v` | `RECIPE_SERVER_VERSION_ARGS` must say so |

## 4. D1 — `bin/`, and the driver learns that `share/` is optional

nginx installs to `sbin/` by default and has no use for `share/`. Two things must be settled.

**The binary goes to `bin/nginx`**, via `--sbin-path`. Not because `sbin` is wrong, but
because `PackagesRoot` and every discovery path in this app already speak `bin/`, and a
package tree where one member is shaped differently is a trap for the code that walks it.
(`build/recipes/README.md:63` uses `sbin/nginx` as its example; that example is now wrong and
should be corrected in the same change.)

**`share/` must become optional, in the driver.** The recipes README says to report a driver
collision rather than edit around it — this is that report, and the edit is the resolution:
make the required-layout list recipe-declarable, defaulting to today's `bin share`. Creating
an empty `share/` to satisfy a check would be a lie told to a gate, which is worse than the
check being wrong.

## 5. D2 — the `<major>` slot is nginx's minor line

The tree is `packages/<name>/<major>/<version>/`. MariaDB uses `11.4`, MySQL `8.4`. nginx has
no "series" concept, but its minor line carries the same meaning: `1.28.3` and `1.28.4` are
drop-in for each other, `1.28` and `1.30` are not necessarily.

So `<major>` is the minor line — `packages/nginx/1.28/1.28.3/`. Consistent with the two
existing packages, and it gives the same upgrade-within-a-line story without inventing a
concept.

## 6. D3 — stable, not mainline

nginx publishes two lines. Mainline gets features; stable gets only critical fixes.

**Take stable.** We become the publisher, so every version we ship is a maintenance
obligation we have signed up for, and the argument that mainline is "what nginx.org
recommends" is an argument aimed at operators who can upgrade on their own schedule. A user
of this app cannot — they get whatever we pinned.

The implementer must **verify the current stable version and its signature at build time**
rather than trusting this document's arithmetic; record the exact pin in the recipe.

## 7. D4 — check 6 becomes a real serve probe, and the driver stops claiming otherwise

For MariaDB, check 6 started a server, created a table, inserted, restarted and read back.
The nginx equivalent: start with `-c` on a generated config, **GET a file over HTTP and
compare the bytes**, restart, GET again.

`audit.sh:706`'s hardcoded pass note must become recipe-supplied. A gate that prints
"inserted, restarted, read back" about a web server is a small lie in the one place this
project has decided lies are least acceptable — the security gate's own output.

## 8. D5 — provenance: the gpg trap applies unchanged

nginx publishes `.asc` signatures and PGP keys. Reuse the build pipeline's existing
discipline verbatim: parse `gpg --status-fd` and require `VALIDSIG <pinned-fpr>` with none of
`EXPKEYSIG|REVKEYSIG|BADSIG|ERRSIG|EXPSIG`.

**`gpg --verify` exits 0 on an expired key.** A recipe that trusts the exit code has no
signature check at all. This cost us a real incident on the OpenSSL key; it is not a
hypothetical.

## 9. D6 — static, with exactly one SSL story

Link against the staged static OpenSSL 3.5.7 that `RECIPE_DEPENDS` already provides. No
dynamic modules (`--with-*_module`, never `--with-*_module=dynamic`) — a dynamic module would
reintroduce a `load_module` path that resolves off the compiled-in prefix, which is the exact
class of bug check 7 exists to catch and which the config layer currently has zero of.

Everything not needed by the generated config is compiled out. The generated config is the
authority on what is needed; read it rather than guessing.

## 10. Recorded now, because it belongs to slice B

**Nothing passes `-p <prefix>` to nginx today.** nginx resolves *relative* config paths
against its compiled-in prefix. No relative path is generated today, so this is latent, not
live — but it is a decision that should be made deliberately with a test, not discovered.
Slice B's spawn spec is where it belongs.

## 11. What this slice must prove

1. The recipe builds nginx from verified upstream source, with the signature checked through
   `--status-fd`, not the exit code.
2. All seven contract checks pass on a **real rebuild**, not a cached tree. Check 7 caught a
   `/tmp/mysql.sock` credential-harvesting default in 27 shipped files on the MariaDB build
   that both other gates missed; it earns its runtime.
3. Check 6 serves a real HTTP request and compares bytes, survives a restart, and serves again.
4. The artifact is **relocatable**: move the tree twice and it still serves.
5. `openvhost-pkg` installs the artifact into `packages/nginx/1.28/<version>/` and the
   compiled-in SHA-256 matches.
6. **Nothing user-visible changed** — a machine with Homebrew still serves exactly as before.

## 12. Out of scope

Discovery and the `stack.rs:810/814` choke point (slice B) · replacing Homebrew at runtime
(slice C) · publishing the release · PHP (slice 5, and the recorded position is that it is the
worst build target and should adopt `static-php-cli`) · retiring the brew paths (slice 7) ·
moving `InstallLedger`/`PackageTarget` out of the `mysql` module, which is a naming wart this
slice inherits rather than fixes.
