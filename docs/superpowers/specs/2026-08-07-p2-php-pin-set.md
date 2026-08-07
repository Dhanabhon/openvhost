<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The PHP pin set — findings (off-Homebrew slice 5A, task 2a)

**Status:** done. Data lives in `build/recipes/_php-pins.sh`; this file is the reasoning.
**Date:** 2026-08-07. **Derived from:** static-php-cli **2.8.5**, commit
`4318ef8fa32a02460ec1554746674a7bc42b49fa` (2026-04-18).
**Reads:** `docs/superpowers/specs/2026-08-07-p2-php-recipe-design.md` D1, D2, D4.

This task did not build PHP and did not write the recipe. It produced the pin set.

## 1. What spc actually is, measured

The spike said spc verifies nothing. Reading 2.8.5's source confirms it and adds four
things worth having in writing, because each one changes what the recipe must do.

| Fact | Evidence |
|---|---|
| Downloads are `curl -sfSL --retry 2`, **no `--proto`/`--proto-redir`**, no digest compared | observed in `log/spc.output.log` on every one of 41 sources |
| `.lock.json` records a **SHA-1 taken after download** and **no URL at all** | `src/SPC/store/LockFile.php:127` (`sha1_file(...)`); a lock entry is `{source_type, filename, move_path, lock_as, hash}` |
| php.net's API returns a `sha256` per release and spc **reads only `version`** | `src/SPC/store/source/PhpSource.php:47` |
| 131 sources; **44** are `type: url`, 23 `ghrel`, 16 `ghtar`, 12 `ghtagtar`, 20 `git`, 11 `filelist`, 4 `pie`, 1 `custom` | counted from `config/source.json` |
| **`type: url` is not the same as version-pinned.** `ext-zip` is `https://pecl.php.net/get/zip` — no version in the URL. It resolved to **1.22.8** on this run and will resolve to 1.22.9 tomorrow | `config/source.json` + the extracted `zip-1.22.8/` prefix |
| **bzip2's primary source is spc's own CDN**, `dl.static-php.dev`, not upstream. On this run it 404'd and spc silently fell back to `sourceware.org` | `log/spc.output.log:124-131`. Six entries reference that CDN — **four as the primary** (`bzip2`, `jbig`, `pkg-config`, `icu-static-win`), two as the `alt` an upstream outage falls *through* to (`gmp`, `re2c`) |
| `spc extract` runs **fully offline** from a pre-populated `downloads/` + `.lock.json` | measured: **0** curl invocations across `php-src,openssl,sqlite` |

The bzip2 fallback is the one to remember. It is not a hypothetical that spc takes a core
build input from a mirror the PHP project does not run — it is what would have happened
today had that host been up.

### A version note that matters

spc's `main` is **3.0.0-alpha1** (commit `9daf9dab`, 2026-08-05) and has migrated the whole
config tree from `config/*.json` to `config/pkg/**/*.yml`. The pin set is derived from tag
**2.8.5**, the newest stable, because a drift alarm anchored to an alpha whose manifest
format is mid-rewrite would fire on churn rather than on drift.

## 2. The extension set, and why

### 2.1 The app itself requires nothing

A full sweep of production code, templates, tests, fixtures and CI found **zero** extension
names. The pool template (`crates/openvhost-conf/templates/php-fpm/pool.conf.tera`) has no
`php_admin_value`, no `extension`, no `extension_dir`. `php_fpm_spec`
(`apps/desktop/src-tauri/src/stack.rs:139`) passes exactly `-F -O -n -y <pool.conf>` — not one
`-d` pair today. The only PHP the app writes is `<?php echo PHP_VERSION;` and the e2e
fixtures use `parse_url` and `$_SERVER`. The MySQL/MariaDB integration never touches PHP; it
shells out to `mysql`/`mysqladmin`. `phpinfo()` is *banned*, with three regression tests
enforcing it.

So "what the app needs" cannot select an extension set. It selects almost nothing.

### 2.2 The baseline that can

**The app installs `brew install php@X.Y` today** (`crates/openvhost-core/src/php/brew.rs:127`,
catalogue `8.1 … 8.5`). Homebrew's `php` formula is therefore the **regression baseline**: an
extension it ships that our package lacks is a capability the user *loses* by switching to our
package on their own machine. That is a harder criterion than taste and it is measurable — so
it is the one used.

Measured on this host, `php -m`, `[PHP Modules]` section only:

* **Homebrew php 8.5.9** → **66** modules.
* **ServBay php 8.2.30** (the benchmark) → **61** modules. It adds exactly `imap`, `memcache`,
  `memcached`, `redis`; it omits exactly `ffi`, `odbc`, `pdo_odbc`, `pdo_dblib`, `snmp`,
  `sysvmsg`, `lexbor`, `uri` — **and `Zend OPcache`**. Its `-m` reports no Zend modules at all,
  i.e. no opcache by default.

Of Homebrew's 66, spc has **54**. Of the 12 it "lacks", **11** — `core date hash json lexbor
pcre random reflection spl standard uri` — are always-on PHP core and were never selectable.
**The only genuine gap is `pdo_dblib`.**

### 2.3 The calls

**IN — 47 static** (`PHP_PINS_EXT_STATIC`):
`bcmath bz2 calendar ctype curl dom exif fileinfo filter ftp gd gettext gmp iconv intl ldap
mbstring mysqli mysqlnd openssl pcntl pdo pdo_mysql pdo_pgsql pdo_sqlite pgsql phar posix
readline session shmop simplexml soap sockets sodium sqlite3 sysvmsg sysvsem sysvshm tidy
tokenizer xml xmlreader xmlwriter xsl zip zlib`

**IN — 2 shared** (`PHP_PINS_EXT_SHARED`): `opcache xdebug`.

| In | Because |
|---|---|
| `mysqli`, `mysqlnd`, `pdo`, `pdo_mysql` | The app installs MySQL *and* MariaDB. A PHP that cannot talk to the database the same app just installed is indefensible. Costs **no** pin — mysqlnd is built in |
| `opcache` | Spike requirement; every real PHP install has it; builtin, **no pin**. ServBay ships PHP with it *off* — shipping it loadable is a straight win |
| `xdebug` | Spike requirement, and the reason to have a dev-focused PHP at all. Neither Homebrew's formula nor ServBay's `-m` has it. 1 pin |
| `gd` (+ freetype, libpng, libjpeg, libwebp) | Thumbnails and image upload are table stakes. **4 pins — the largest single cost in the set** |
| `intl` | ICU, one pin but the largest download. Laravel/Symfony i18n; both baselines ship it |
| `curl`, `openssl`, `zip`, `zlib`, `bz2`, `dom`/`xml`/`simplexml` | Composer does not run without them. PHPUnit needs `dom`+`xml` |
| `pgsql`, `pdo_pgsql` | 1 pin. We ship no Postgres, but devs connect to external Postgres constantly, and both baselines ship it |
| `ldap`, `gettext`, `tidy`, `readline` | Kept, and this is measured rather than assumed: dropping all four saves **1 source out of 41** (`tidy`), because `curl`'s own suggestion closure already drags `ldap`, `krb5`, `gettext`, `libedit` and `ncurses` in whether we ask for them or not. Parity here is very nearly free |
| `bcmath`, `filter`, `ctype`, `session`, `posix`, `pcntl`, `sockets`, `calendar`, `exif`, `ftp`, `phar`, `shmop`, `sysv*`, `tokenizer`, `fileinfo`, `mbstring`, `iconv`, `soap` | Zero external libs. Free, and all in the baseline |

| Out | Because |
|---|---|
| `pdo_dblib` | **Not in spc at all.** MS SQL Server via FreeTDS. Recorded in `PHP_PINS_KNOWN_GAPS` — the one real parity gap |
| **`ffi`** | Loads arbitrary dylibs by path at runtime. In a *static* php-fpm launched with `-n`, it is the sharpest foot-gun in the catalogue and buys a local dev environment nothing. ServBay omits it. **A security call, not a cost call** |
| `snmp` (`net-snmp`) | Network-device management, not web development. ServBay omits it |
| `odbc`, `pdo_odbc` (`unixodbc`) | Generic DB bridge; we ship MySQL/MariaDB and pin pdo_pgsql/pdo_sqlite. ServBay omits both |
| `dba` (`qdbm`) | Berkeley-DB-style key/value, effectively unused in modern PHP. ServBay omits it |
| `imap` | Removed from PHP core in 8.4; not in Homebrew's 8.5. Including it would make the set diverge per major |
| `redis`, `memcached` | ServBay has them, Homebrew does not. Genuinely useful — but the app ships neither Redis nor Memcached as a service, so the extension would point at nothing. **The strongest follow-on candidate** once a cache service exists |
| **`libavif` → `libaom`** | See §3. The single worst source in the closure, and one image format is the whole cost of removing it |

## 3. `libaom` — the source that could not be pinned

`gd` suggests `libavif`; `libavif` hard-depends on `libaom`; and spc's `libaom` is:

```json
{"type":"git","rev":"main","url":"https://aomedia.googlesource.com/aom", ...}
```

cloned `--recursive`. That is an **unreleased development branch of a video codec, compiled
into the binary that runs users' web code**. There is no release to pin. Pinning a
`main`-branch commit would mean shipping whatever landed the day we looked.

`libavif` is dropped, which removes `libaom` with it. gd keeps JPEG, PNG, WebP and FreeType.
The cost is `imageavif()`, which fails loudly at the call site — the side that fails *loudly*,
which is the test this project already settled on for the docroot default.

`micro` is the other git source and is kept, pinned by commit
(`fb6d497b6f4cf138ee3851a30c905d64b7b19aed`). A commit is a cryptographic pin in its own
right and also fixes every submodule. It is in the closure only because spc's `php` pseudo-lib
hard-depends on `micro` and `frankenphp` (`config/lib.json`) regardless of build target —
**the recipe task should test whether `spc build:php-fpm` actually needs `micro`,
`frankenphp` and `watcher`, and drop three pins if it does not.**

## 4. The pin set

**41 sources discovered, 39 pinned, 2 excluded.** The design predicted "~31 URLs and digests
maintained by hand"; the measured number is **38 archives + 1 git commit + 3 php-src
releases**. Full table in `build/recipes/_php-pins.sh` — it is the deliverable and is not
duplicated here.

Shape, and why:

* **A sourced bash file of one-line records**, not JSON. `jq` is not a build tool this
  pipeline has, and adding one to the gate to read a pin file would be a new unverified
  dependency in the exact place we are trying to remove them. Bash 3.2 only — the sole bash
  on macOS is 3.2.57 and nothing in `build/` uses `declare -A`.
* **One pin per line**, whitespace-separated: `grep openssl build/recipes/_php-pins.sh` gives
  the whole record and a bump is a one-line diff. That is D4's "greppable and diffable".
* **Named `_php-pins.sh`.** `build.sh`'s package-name validator rejects any name not starting
  with `[a-z0-9]` (`build/build.sh:158-160`), so `build.sh _php-pins …` can never be a thing —
  the same guard that already protects `_template.sh`.
* **The `<spc-filename>` column is load-bearing.** spc locates a pre-downloaded archive by
  exactly that name; a renamed file becomes a re-download attempt, which under the no-network
  rule is a failed build.
* **URLs are the stable public form.** Where spc fetched
  `api.github.com/repos/<r>/releases/assets/<numeric-id>` the id was resolved to its
  `browser_download_url` (same bytes, readable URL). Where spc fetched an *unversioned* URL,
  the versioned equivalent is pinned instead — `pecl.php.net/get/zip-1.22.8.tgz`, confirmed
  byte-identical to what `pecl.php.net/get/zip` served.

Verified: the file sources under `/bin/bash` 3.2, every record has the right field count,
every digest is 64 lowercase hex, and `shellcheck` is clean.

### One caveat worth stating rather than hiding

**Eleven** pins are `api.github.com/.../tarball/<tag>` (or `zipball/<sha>`) archives that
GitHub generates on demand. Three fetches minutes apart of one of them produced identical
bytes, so they are reproducible **today** — but that is a measurement over minutes, not a
guarantee from GitHub. If one of those eleven ever mismatches, the first question is "did
GitHub change archive generation?", not "were we attacked".

## 5. `--custom-url` **does** pin `php-src` — tested

Run with `--with-php=8.4`, which would otherwise resolve 8.4.24 through php.net's API:

```
spc download --with-php=8.4 --ignore-cache-sources=php-src \
    --custom-url="php-src:https://www.php.net/distributions/php-8.3.33.tar.xz" php-src
```

spc printed `Downloading source php-src from custom url: …php-8.3.33.tar.xz`, put
`php-8.3.33.tar.xz` on disk, and locked it as `php-src`. `PhpSource::getLatestPHPInfo` was not
called at all. The mechanism is generic — `DownloadCommand.php:149-188` rewrites *any* chosen
source's config to `{type: url, url: …}` before dispatch — so `php-src` being a `custom`-type
source does not exempt it.

**But the recipe should not need it.** `spc extract` was measured running with **zero** network
calls against a pre-populated `downloads/` + `.lock.json`. Placing our own verified bytes there
under the names in the pin file is simpler than threading 39 `--custom-url` arguments through,
and it keeps the verification in `bp_verify_sha256` rather than in spc.

## 6. The PHP signing keys, and how they were established

**The signing key is per release, not per major** — whichever release manager cut the tarball
signed it. That is why the fingerprint sits in the `PHP_PINS_PHP_SRC` table beside each digest
rather than in a single `RECIPE_SIGNING_KEY_FPR` scalar the way `nginx.sh` has it.

| Release | Signed by | **Primary** fingerprint pinned |
|---|---|---|
| 8.3.33 | Eric A Mann, via **signing subkey** `4B1FC0D9DF92321CED9F615DBEC555E22A143553` | `AFD8691FDAEDF03BDF6E460563F15A9B715376CA` |
| 8.4.24 | Calvin Buckley (PHP), ed25519, primary key directly | `9D7F99A0CB8F05C8A6958D6256A97AF7600A39A6` |
| 8.5.9 | Volker Dusch, ed25519, primary key directly | `49D9AF6BC72A80D6691719C8AA23F5BE9C7097D4` |

8.3.33 is a **live reproduction** of the case `bp_gpg_verify_signature` was written for: its
`VALIDSIG` line is

```
[GNUPG:] VALIDSIG 4B1FC0D9DF92321CED9F615DBEC555E22A143553 … AFD8691FDAEDF03BDF6E460563F15A9B715376CA
```

— first field the subkey, **last field the primary**. Pinning the first would have pinned a
subkey that can be rotated under us. The helper already compares the last field; this is the
first PHP-side proof that it had to.

### How the fingerprints were shown to be php.net's

Not "the download page served them to me". Two hosts sharing no infrastructure:

1. **`https://www.php.net/gpg-keys.php`** over HTTPS — lists all three under their major's
   heading (8.5: Charron / Dusch / Scherzer; 8.4: Mann / Buckley / Takamachi; 8.3: Charron /
   Mann / Zelenka). Note the page prints **primary** fingerprints, which is why 8.3.33's
   subkey does not appear on it — a discrepancy that would look alarming without the
   subkey/primary distinction above.
2. **`php/web-php`'s `include/gpg-keys.inc` on GitHub** — the source that page is rendered
   from, in php.net's own version control, on infrastructure php.net does not run, **with
   history**. `git log -S` places each fingerprint's arrival in a reviewed commit:

   | Fingerprint | Commit | Date |
   |---|---|---|
   | `49D9AF6B…97D4` | `9de9a81b` "Add new PHP 8.5 rms gpg keys (#1274)" | 2025-06-10 |
   | `9D7F99A0…39A6` | `3814d0ba` "Add new PHP 8.4 gpg key branch (#992)" | 2024-06-04 |
   | `AFD8691F…76CA` | `8b97274b` "Add new PHP 8.3 gpg key branch (#778)" | 2023-06-03 |

Independently, every SHA-256 in `PHP_PINS_PHP_SRC` matches the `sha256` php.net's own releases
API publishes for the same file — a second, unrelated attestation of the same bytes. All three
`.asc` files verify `GOODSIG` + `VALIDSIG` with no `EXPKEYSIG`/`REVKEYSIG`/`BADSIG`. None of
the three keys expires.

**Key material** comes from the keyservers, not php.net, because php.net publishes
fingerprints but not keys. That is safe — the fingerprint is the trust anchor and a substituted
key cannot produce it. `keyserver.ubuntu.com` served all three; `keys.openpgp.org` served two
(Calvin Buckley's is a 404 there). `bp_gpg_import_key` logs a miss and continues, requiring
only that one host serve a key with the pinned primary, so listing both costs nothing.

## 7. D4 — the rot alarm

~39 hand-maintained pins will drift from spc's manifest and nothing would say so. The
`include_str!` tripwire that ties `catalogue.rs` to a recipe file already exists and already
fires. Three layers, cheapest first; **layer 1 is the new one and is the one that addresses
D4's actual worry.**

**Layer 1 — the spc anchor, fires on every build.** `_php-pins.sh` records
`PHP_PINS_SPC_COMMIT`. `recipe_fetch` resolves the spc checkout it is about to drive
(`git -C <spc> rev-parse HEAD`) and `bp_die`s on mismatch:

```
pin set was derived from spc 2.8.5 (4318ef8f…); this tree is <other>.
Regenerate the pins or check out the pinned tag.
```

That is a trigger, not an intention: bumping spc without regenerating pins becomes a failed
build rather than a silent divergence. It catches the exact direction D4 names — spc moved,
the pins did not.

**Layer 2 — the existing `include_str!` tripwire, fires in `cargo test`, no network.**
When 5B adds `crates/openvhost-core/src/php/package/catalogue.rs`, it `include_str!`s
`build/recipes/_php-pins.sh` and asserts that the PHP version and SHA-256 it advertises to
`openvhost-pkg` appear verbatim in the pin file. Same mechanism already proven for nginx in
4A. This catches the other direction — the catalogue moved, the pins did not.

**Layer 3 — freshness, visible in source.** `PHP_PINS_DERIVED_ON` and
`PHP_PINS_LAST_CHECKED` are §14's tripwire in the shape the recipe interface already uses.
A stale date must be readable in the diff, not remembered.

**The remedy has to be cheap or the alarm gets disabled.** Recommend the recipe task also land
`build/recipes/_php-pins-refresh.sh` — clone spc at `PHP_PINS_SPC_TAG`, resolve the closure,
download, hash, rewrite the arrays — so answering an alarm is one command rather than a day of
`shasum`. That script is how this pin set was produced; committing it converts a one-off into a
procedure.

## 8. D2 — `pkg-config` comes from the build host. Confirmed, and worse than the design said

The design says spc fetches pkg-config as "a third party's latest GitHub release". In 2.8.5 it
is actually:

```json
"pkg-config": {"type":"url","url":"https://dl.static-php.dev/static-php-cli/deps/pkg-config/pkg-config-0.29.2.tar.gz","provide-pre-built":true}
```

— a **fixed-version tarball from spc's own CDN**, with no digest, which spc then builds and
`strip`s (`src/SPC/builder/unix/library/pkgconfig.php:29`) and executes inside the build. With
`--prefer-pre-built` it would download a **prebuilt binary** from that CDN instead. Either way
the conclusion is the design's: it is a build tool, it belongs in `RECIPE_BUILD_TOOLS`, and it
is not in the pin set.

**A suitable one exists on the build host:**

| | |
|---|---|
| Path | `/opt/homebrew/bin/pkg-config` → `../Cellar/pkgconf/3.0.5/bin/pkg-config` |
| Version | **pkgconf 3.0.5**, Mach-O arm64 |

**And the ServBay trap has reproduced.** `which pkg-config` on this host resolves to
**`/Applications/ServBay/bin/pkg-config` (0.29.2)**, because `/Applications/ServBay/bin`
precedes `/opt/homebrew/bin` on `PATH`. This is the same shape as the bison incident that
broke the MariaDB build. `RECIPE_BUILD_TOOLS` + `bp_tool pkg-config` resolve before `PATH` is
scrubbed, but **whether that resolution picks ServBay's or Homebrew's depends on `PATH`
order** — so the recipe must not merely declare `pkg-config`, it must assert the resolved
binary is `pkgconf` ≥ 2 (or pin the absolute path), the way `mariadb.sh` pins
`BISON_EXECUTABLE`. Flagged for the recipe task.

## 9. What I could not establish

Stated plainly rather than estimated:

* **Whether `micro`, `frankenphp` and `watcher` are actually needed** for a `build:php-fpm`
  target. They are in the closure because spc's `php` pseudo-lib hard-depends on them, not
  because php-fpm reads them. All three are pinned so the build cannot fail on their absence;
  testing whether three pins can be deleted is a build-phase question.
* **Long-term byte stability of the 8 GitHub-generated tarballs.** Measured stable over
  minutes; GitHub offers no guarantee.
* **Whether the library closure differs across PHP majors.** The pin set was derived with
  `--with-php=8.4`. `config/ext.json` carries no per-major source differences that I found,
  but I did not re-derive the closure for 8.3 and 8.5, so 8.4.24 is the only *proven*
  reference target and `PHP_PINS_PHP_REFERENCE` says so.
* **Whether `spc build` succeeds from a hand-populated `downloads/`.** `spc extract` was
  proven offline; `spc build` was not run at all — that is the recipe task's live proof.
* **Any statement about the artifact.** Nothing was compiled. Contract checks 1-7, relocation,
  and opcache/xdebug loading remain the spike's claims, not this task's.
