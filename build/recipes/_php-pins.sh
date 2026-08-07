# SPDX-License-Identifier: GPL-3.0-or-later
# shellcheck shell=bash
# shellcheck disable=SC2034  # every PHP_PINS_* variable here is read by
#                              recipes/php.sh, which sources this file
#
# The PHP pin set — off-Homebrew slice 5A, D1.
#
# This file is DATA, not a recipe. The leading underscore is what stops
# `build.sh _php-pins <ver>` from ever being a thing: build.sh's package-name
# validator rejects any name not starting with [a-z0-9] (build.sh:158-160), the
# same guard that protects `_template.sh`. recipes/php.sh sources this file at
# source time, which is allowed — it sets variables and nothing else.
#
# WHY THIS FILE EXISTS
#
# PHP is not built by a configure script we drive; it is built by
# static-php-cli ("spc"), which resolves and downloads ~40 third-party sources
# and then compiles them. spc verifies NOTHING: no hash, no signature, anywhere
# in its config. Measured against the tree this pin set was derived from:
#
#   * `curl -sfSL --retry 2` with no --proto/--proto-redir, and no digest
#     compared before or after (src/SPC/store/Downloader.php).
#   * Its `.lock.json` records a SHA-1 computed AFTER the bytes are on disk
#     (LockFile.php:127) and does not record the URL at all. That is
#     trust-on-first-use with no provenance, not a pin.
#   * php.net's release API hands spc a `sha256` for every tarball and spc
#     reads only `$info['version']` from the response, discarding it
#     (store/source/PhpSource.php:47).
#   * Only 44 of its 131 sources are `type: url`; the other 87 resolve
#     "latest" at download time, and 20 of those are git checkouts of a
#     BRANCH. Even "type: url" is not automatically a version pin — spc's
#     `ext-zip` is `https://pecl.php.net/get/zip`, which serves whatever PECL
#     released most recently (it resolved to 1.22.8 here).
#   * bzip2's PRIMARY source is `dl.static-php.dev`, spc's own CDN, not
#     upstream. During the discovery run that host 404'd and spc silently fell
#     back to sourceware.org. On a good day a core build input would have come
#     from spc's mirror, unverified. This is not hypothetical. Six source
#     entries reference that CDN — four as the PRIMARY source (`bzip2`,
#     `jbig`, `pkg-config`, `icu-static-win`) and two as the `alt` an upstream
#     outage silently falls THROUGH to (`gmp`, `re2c`).
#
# So every byte is pinned here instead, and `spc build` runs with no network.
#
# HOW THESE DIGESTS WERE PRODUCED
#
# `spc download` was run ONCE as a discovery step — to learn what spc would
# fetch, not to trust what it fetched — and every SHA-256 below was then
# computed by us from the bytes on disk. Nothing was copied out of spc's lock
# file. Nothing was copied out of spc's published documentation.
#
# The `<spc-filename>` column is load-bearing, not decoration: spc finds a
# pre-downloaded archive by exactly that name in its downloads/ directory. A
# renamed file is a re-download attempt, which under `spc build`'s no-network
# rule is a failed build. `spc extract` was measured running with zero curl
# invocations against a pre-populated downloads/ + .lock.json, which is what
# makes the offline path work at all.

# ------------------------------------------------------------- the anchor --
#
# D4: pinning that rots silently is worse than no pinning. This is the spc
# tree the pin set was derived FROM. recipes/php.sh must compare it against
# the spc checkout it is about to drive and refuse to build on a mismatch —
# an obligation with no trigger is an intention.
#
# 2.8.5 is the newest STABLE tag. `main` at the time of derivation was
# 3.0.0-alpha1 (commit 9daf9dab, 2026-08-05) with the whole config tree
# migrated from config/*.json to config/pkg/**/*.yml. Deriving pins from an
# alpha whose manifest format is mid-rewrite would put the drift alarm on a
# moving target.
PHP_PINS_SPC_TAG="2.8.5"
PHP_PINS_SPC_COMMIT="4318ef8fa32a02460ec1554746674a7bc42b49fa"
PHP_PINS_SPC_URL="https://github.com/crazywhalecc/static-php-cli"
PHP_PINS_DERIVED_ON="2026-08-07"

# ------------------------------------------------------- the extension set --
#
# Justified in docs/superpowers/specs/2026-08-07-p2-php-pin-set.md §2. The
# short version: the app installs `brew install php@X.Y` today, so Homebrew's
# `php` formula is the REGRESSION BASELINE — an extension it has that we lack
# is a capability the user loses by switching to our package on their own
# machine. Everything below is in that baseline or is opcache/xdebug.
PHP_PINS_EXT_STATIC="bcmath,bz2,calendar,ctype,curl,dom,exif,fileinfo,filter,ftp,gd,gettext,gmp,iconv,intl,ldap,mbstring,mysqli,mysqlnd,openssl,pcntl,pdo,pdo_mysql,pdo_pgsql,pdo_sqlite,pgsql,phar,posix,readline,session,shmop,simplexml,soap,sockets,sodium,sqlite3,sysvmsg,sysvsem,sysvshm,tidy,tokenizer,xml,xmlreader,xmlwriter,xsl,zip,zlib"

# Loaded at runtime from the package tree's own modules/ via `-d` pairs (D6),
# never a php.ini. The spike proved both load under `-n` through a real HTTP
# request. opcache is builtin to php-src and costs no pin; xdebug is a source.
PHP_PINS_EXT_SHARED="opcache,xdebug"

# ------------------------------------------------------------- php-src --
#
# PHP gets the full treatment, not just a hash: php.net publishes a .asc for
# every release and bp_gpg_verify_signature checks it through --status-fd.
#
# The signing key is PER RELEASE, not per major — whichever release manager
# cut that tarball signed it — so the fingerprint belongs in this table
# alongside the digest rather than in a single RECIPE_SIGNING_KEY_FPR scalar.
#
# The pinned fingerprint is the PRIMARY key's, which is what
# bp_gpg_verify_signature compares (VALIDSIG's LAST field). 8.3.33 is a live
# example of why that distinction is not folklore: it is signed by Eric Mann's
# rsa4096 SIGNING SUBKEY 4B1FC0D9DF92321CED9F615DBEC555E22A143553, and only
# VALIDSIG's last field carries the primary AFD8691F… that this table pins.
#
# Each fingerprint was confirmed from two hosts that share no infrastructure:
#
#   https://www.php.net/gpg-keys.php          (php.net, over HTTPS)
#   php/web-php include/gpg-keys.inc @ GitHub (php.net's own source, with
#                                              history — the 8.5 key arrived in
#                                              9de9a81b "Add new PHP 8.5 rms
#                                              gpg keys", 2025-06-10; the 8.4
#                                              key in 3814d0ba, 2024-06-04;
#                                              the 8.3 key in 8b97274b,
#                                              2023-06-03)
#
# and every SHA-256 below independently matches the `sha256` php.net's own
# releases API publishes for the same file. None of the three keys expires.
#
# Fields: <version> <sha256> <primary-fpr> <url> <signature-url>
PHP_PINS_PHP_SRC=(
	"8.3.33 e293ed620cec74651bb4a071317892a478aa6840fab22db45c72d77cd42f9676 AFD8691FDAEDF03BDF6E460563F15A9B715376CA https://www.php.net/distributions/php-8.3.33.tar.xz https://www.php.net/distributions/php-8.3.33.tar.xz.asc"
	"8.4.24 e127be09a8506f4327c5cfa78a614b00d210714484ec215ce0011b4a03c00731 9D7F99A0CB8F05C8A6958D6256A97AF7600A39A6 https://www.php.net/distributions/php-8.4.24.tar.xz https://www.php.net/distributions/php-8.4.24.tar.xz.asc"
	"8.5.9 0db7855f25bcd0ab1d592cdb35e284d6f6a5d2ae0f6f621122e364cc39b708f4 49D9AF6BC72A80D6691719C8AA23F5BE9C7097D4 https://www.php.net/distributions/php-8.5.9.tar.xz https://www.php.net/distributions/php-8.5.9.tar.xz.asc"
)

# The reference target for slice 5A's proof. 8.3 and 8.5 are pinned and
# buildable but unproven until someone builds them.
PHP_PINS_PHP_REFERENCE="8.4.24"

# php.net publishes fingerprints but not key MATERIAL, so the keyservers are
# where the bytes come from — which is safe because the fingerprint is the
# trust anchor and a substituted key cannot produce it. `%s` is the
# fingerprint. keys.openpgp.org does not serve Calvin Buckley's key (404 at
# derivation time); bp_gpg_import_key logs a miss and continues, requiring only
# that ONE host serve a key with the pinned primary fingerprint, so listing it
# costs nothing and buys a second host for the other two.
PHP_PINS_KEY_URL_TEMPLATES=(
	"https://keys.openpgp.org/vks/v1/by-fingerprint/%s"
	"https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x%s&options=mr"
)

# php.net's release dates for the pinned tarballs. §14's tripwire: from here
# on a PHP CVE is ours to notice.
PHP_PINS_UPSTREAM_RELEASE_DATE="2026-07-30"
PHP_PINS_LAST_CHECKED="2026-08-07"

# ---------------------------------------------------- libraries and exts --
#
# Fields: <spc-source-name> <version> <sha256> <spc-filename> <url>
#
# URLs are the STABLE public form. Where spc fetched through
# `api.github.com/repos/<r>/releases/assets/<numeric-id>` the numeric id was
# resolved to its browser_download_url and that is what is pinned; the bytes
# are the same, the URL is readable and diffable. Where spc fetched an
# unversioned URL (`pecl.php.net/get/zip`) the VERSIONED equivalent is pinned
# instead — `pecl.php.net/get/zip-1.22.8.tgz`, confirmed byte-identical.
#
# The 11 `api.github.com/.../tarball/<tag>` and `.../zipball/<sha>` entries are
# archives GitHub generates on demand. Three fetches minutes apart produced
# identical bytes, so they are reproducible today — but that is a measurement
# over minutes, not a guarantee from GitHub, and a digest mismatch on one of
# those 11 is the expected first symptom if GitHub ever changes archive
# generation. Treat such a failure as a question ("did GitHub change?") before
# assuming compromise.
PHP_PINS_LIBS=(
	"brotli 1.2.0 eb5f7dadf215d0670665fd81566e1fe2dfdc154d983f09142de7299df4c182e6 google-brotli-v1.2.0-0-g028fb5a.tar.gz https://api.github.com/repos/google/brotli/tarball/refs/tags/v1.2.0"
	"bzip2 1.0.8 ab5a03176ee106d3f0fa90e381da478ddae405918153cca248e682cd0c4a2269 bzip2-1.0.8.tar.gz https://sourceware.org/pub/bzip2/bzip2-1.0.8.tar.gz"
	"curl 8.21.0 aa1b66a70eace83dc624508745646c08ae561de512ab403adffb93ac87fc72e6 curl-8.21.0.tar.xz https://github.com/curl/curl/releases/download/curl-8_21_0/curl-8.21.0.tar.xz"
	"ext-zip 1.22.8 9fab1f8653d40249bd433ad2c3ca02431f9a5ab06d50f988c8ec53fc6a846eef ext-zip.tgz https://pecl.php.net/get/zip-1.22.8.tgz"
	"freetype 2.14.3 1cc149d9dce64e02f92713a777588d0551a8334d63c3d3e73e955269dc57a89a freetype-freetype-VER-2-14-3-0-g0a0221a.tar.gz https://api.github.com/repos/freetype/freetype/tarball/refs/tags/VER-2-14-3"
	"gettext 1.0 71132a3fb71e68245b8f2ac4e9e97137d3e5c02f415636eb508ae607bc01add7 gettext-1.0.tar.xz https://ftp.gnu.org/pub/gnu/gettext/gettext-1.0.tar.xz"
	"gmp 6.3.0 a3c2b80201b89e68616f4ad30bc66aee4927c3ce50e33929ca819d5c43538898 gmp-6.3.0.tar.xz https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz"
	"icu 77.1 588e431f77327c39031ffbb8843c0e3bc122c211374485fa87dc5f3faff24061 icu4c-77_1-src.tgz https://github.com/unicode-org/icu/releases/download/release-77-1/icu4c-77_1-src.tgz"
	"krb5 1.22.2 3243ffbc8ea4d4ac22ddc7dd2a1dc54c57874c40648b60ff97009763554eaf13 krb5-1.22.2.tar.gz https://web.mit.edu/kerberos/dist/krb5/1.22/krb5-1.22.2.tar.gz"
	"ldap 2.7.0 9e86f37da375aa948a1b478dd76fe87b02090e47c21facae19223588e3407922 openldap-2.7.0.tgz https://www.openldap.org/software/download/OpenLDAP/openldap-release/openldap-2.7.0.tgz"
	"libcares 1.34.8 c222b6d681096f9444d2c4863d2c1174019e27cacca0a4a5c114d36dd7d7bf78 c-ares-1.34.8.tar.gz https://github.com/c-ares/c-ares/releases/download/v1.34.8/c-ares-1.34.8.tar.gz"
	"libedit 20260512-3.1 432d5e7ea8b0116dd39f2eca7bc11d0eed77faa6b77ea526ace89907c23ea4a0 libedit-20260512-3.1.tar.gz https://thrysoee.dk/editline/libedit-20260512-3.1.tar.gz"
	"libiconv 1.19 88dd96a8c0464eca144fc791ae60cd31cd8ee78321e67397e25fc095c4a19aa6 libiconv-1.19.tar.gz https://ftp.gnu.org/gnu/libiconv/libiconv-1.19.tar.gz"
	"libidn2 2.3.8 f557911bf6171621e1f72ff35f5b1825bb35b52ed45325dcdee931e5d3c0787a libidn2-2.3.8.tar.gz https://ftp.gnu.org/gnu/libidn/libidn2-2.3.8.tar.gz"
	"libjpeg 3.2.0 d31968b44f4bf962948a97232459e03658806f608e06ca25e72c72b42347b02c libjpeg-turbo-libjpeg-turbo-3.2.0-0-gc85e6b9.tar.gz https://api.github.com/repos/libjpeg-turbo/libjpeg-turbo/tarball/3.2.0"
	"libpng 1.6.58 6052d0d9f03cb9bf611133b10e0ef211687e13f9f629a63af44c9960659f0e3f pnggroup-libpng-v1.6.58-0-g3061454.tar.gz https://api.github.com/repos/pnggroup/libpng/tarball/refs/tags/v1.6.58"
	"libsodium 1.0.22 adbdd8f16149e81ac6078a03aca6fc03b592b89ef7b5ed83841c086191be3349 libsodium-1.0.22.tar.gz https://github.com/jedisct1/libsodium/releases/download/1.0.22-RELEASE/libsodium-1.0.22.tar.gz"
	"libssh2 1.11.1 d9ec76cbe34db98eec3539fe2c899d26b0c837cb3eb466a56b0f109cabf658f7 libssh2-1.11.1.tar.gz https://github.com/libssh2/libssh2/releases/download/libssh2-1.11.1/libssh2-1.11.1.tar.gz"
	"libunistring 1.4.2 e82664b170064e62331962126b259d452d53b227bb4a93ab20040d846fec01d8 libunistring-1.4.2.tar.gz https://ftp.gnu.org/gnu/libunistring/libunistring-1.4.2.tar.gz"
	"libwebp 1.6.0 48c7b41fc22d53c5a8dd969fd0c5b302987a654df8c9f0e7e353c896703ff81c webmproject-libwebp-v1.6.0-0-g4fa2191.tar.gz https://api.github.com/repos/webmproject/libwebp/tarball/refs/tags/v1.6.0"
	"libxml2 2.15.3 cbe9ba025247c7da13cd64bc2145ecbb2e6f181dfe61cdd8afeda4acfb7619b6 GNOME-libxml2-v2.15.3-0-gc94eb02.tar.gz https://api.github.com/repos/GNOME/libxml2/tarball/refs/tags/v2.15.3"
	"libxslt 1.1.45 9acfe68419c4d06a45c550321b3212762d92f41465062ca4ea19e632ee5d216e libxslt-1.1.45.tar.xz https://download.gnome.org/sources/libxslt/1.1/libxslt-1.1.45.tar.xz"
	"libzip 1.11.4 8a247f57d1e3e6f6d11413b12a6f28a9d388de110adc0ec608d893180ed7097b libzip-1.11.4.tar.xz https://github.com/nih-at/libzip/releases/download/v1.11.4/libzip-1.11.4.tar.xz"
	"ncurses 6.6 355b4cbbed880b0381a04c46617b7656e362585d52e9cf84a67e2009b749ff11 ncurses-6.6.tar.gz https://ftp.gnu.org/pub/gnu/ncurses/ncurses-6.6.tar.gz"
	"nghttp2 1.70.0 e05cb1388eaca3830aded4ccf20044b6e1ac1a61411dcca11b0437c4285c8bc2 nghttp2-1.70.0.tar.xz https://github.com/nghttp2/nghttp2/releases/download/v1.70.0/nghttp2-1.70.0.tar.xz"
	"openssl 3.6.3 243a86649cf6f23eeb6a2ff2456e09e5d77dd9018a54d3d96b0c6bdd6ba6c7f1 openssl-3.6.3.tar.gz https://github.com/openssl/openssl/releases/download/openssl-3.6.3/openssl-3.6.3.tar.gz"
	"postgresql 18.4 f519178b848b54df90f38bafcf4d13b3547e0e896e4851a7f7acc104f52105d1 postgres-postgres-REL_18_4-0-gf5cc817.tar.gz https://api.github.com/repos/postgres/postgres/tarball/refs/tags/REL_18_4"
	"sqlite 3.45.2 bc9067442eedf3dd39989b5c5cfbfff37ae66cc9c99274e0c3052dc4d4a8f6ae sqlite-autoconf-3450200.tar.gz https://www.sqlite.org/2024/sqlite-autoconf-3450200.tar.gz"
	"tidy 5.8.0 85fe03682c870b1c83d4b22e9165333fd8adb5474ac469fba8da13e77a4caa33 htacg-tidy-html5-5.8.0-0-g1ca3747.tar.gz https://api.github.com/repos/htacg/tidy-html5/tarball/5.8.0"
	"xdebug 3.5.3 781cf03aee443c317c20d0299fd298d2d8ac9394cfa22b912a61d02464941a55 xdebug_xdebug-3.5.3.zip https://api.github.com/repos/xdebug/xdebug/zipball/127bbcb980400752221cfaa54bdc1420e6ef3c12"
	"xz 5.8.3 fff1ffcf2b0da84d308a14de513a1aa23d4e9aa3464d17e64b9714bfdd0bbfb6 xz-5.8.3.tar.xz https://github.com/tukaani-project/xz/releases/download/v5.8.3/xz-5.8.3.tar.xz"
	"zlib 1.3.2 bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16 zlib-1.3.2.tar.gz https://github.com/madler/zlib/releases/download/v1.3.2/zlib-1.3.2.tar.gz"
	"zstd 1.5.7 eb33e51f49a15e023950cd7825ca74a4a2b43db8354825ac24fc1b7ee09e6fa3 zstd-1.5.7.tar.gz https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz"
)

# ---------------------------------------------------------- git sources --
#
# A git source has no archive to hash, so the pin is the COMMIT — which is a
# cryptographic pin in its own right, and a stronger one than a tarball digest
# because it also fixes every submodule.
#
# `micro` is the phpmicro SAPI. Nothing in this app wants it: we build
# php-fpm. It is here because spc's own graph makes the `php` pseudo-lib
# hard-depend on `micro` and `frankenphp` (config/lib.json), so the closure
# drags them in regardless of target. Upstream tracks `master` — an unpinned
# branch of a third party's fork of a PHP SAPI.
#
# MEASURED, because "in the closure" and "actually needed" are not the same
# question and task 2a could not separate them:
#
#   micro       REQUIRED. spc extracts it unconditionally for any PHP >= 8.0,
#               whatever the SAPI target — BuilderBase::proveExts() calls
#               SourceManager::initSource(sources: ['micro']) with no target
#               test at all, and BuildPHPCommand always calls proveExts. A
#               --build-fpm build that lacks it dies before it compiles
#               anything. Kept, and it stays a git pin: a commit fixes the
#               tree more tightly than a tarball digest and phpmicro has no
#               submodules to complicate that (checked).
#   frankenphp  DEAD, deleted from PHP_PINS_LIBS. Its source is only ever
#               extracted inside the FrankenPHP SAPI build
#               (UnixBuilderBase::processFrankenphpApp), which needs
#               --build-frankenphp, ZTS and a Go toolchain and which this
#               recipe never asks for. The LIBRARY stays in spc's resolved
#               closure — nothing can remove a hard lib-depend — but its
#               `type: target` makes proveLibs() skip instantiating it and
#               LicenseDumper skip reading it, so no code path ever asks
#               where its bytes are. Proven by building without the pin.
#   watcher     DEAD, deleted from PHP_PINS_LIBS. It only entered the closure
#               through spc's `--with-suggested-libs`, which recipes/php.sh
#               does not use (it passes an explicit --with-libs so that
#               libaom/libavif stay excluded), and the only thing that
#               consumes it is the FrankenPHP Go build's `-tags=...nowatcher`
#               switch. Proven by building without the pin.
#
# Fields: <spc-source-name> <commit> <url>
PHP_PINS_GIT=(
	"micro fb6d497b6f4cf138ee3851a30c905d64b7b19aed https://github.com/static-php/phpmicro"
)

# --------------------------------------------------- deliberately EXCLUDED --
#
# Recorded, not silently dropped: a gap nobody wrote down is a gap nobody
# fixes. See the findings doc §3 for the full reasoning.
#
# Fields: <spc-source-name> <version-spc-would-have-taken> <why>
PHP_PINS_EXCLUDED=(
	"libaom git-rev-main type=git rev=main from aomedia.googlesource.com — an UNRELEASED development branch of a video codec, compiled into the binary that runs users' web code. Not pinnable as a release, and pinning a main-branch commit means shipping whatever landed that day."
	"libavif 1.4.2 the sole reason libaom enters the closure (config/lib.json: libavif lib-depends libaom). Dropping it costs gd its AVIF encoder and nothing else; JPEG, PNG, WebP and FreeType all stay. A missing imageavif() fails loudly at the call site, which is the side that fails LOUDLY."
	"net-snmp - ext/snmp is network-device management, not web development. ServBay omits it; Homebrew ships it. Large pin, no OpenVHost feature behind it."
	"unixodbc - ext/odbc + ext/pdo_odbc are a generic DB bridge. The app ships MySQL and MariaDB, and pdo_pgsql/pdo_sqlite cover the rest. ServBay omits both."
	"qdbm - ext/dba is Berkeley-DB-style key/value storage, effectively unused in modern PHP. ServBay omits it."
	"libffi - ext/FFI loads arbitrary dylibs by path at runtime. In a STATIC php-fpm launched with -n it is the sharpest foot-gun in the catalogue, and it buys a local dev environment nothing. ServBay omits it. This exclusion is a security call, not a cost call."
	"ngtcp2 1.25.0 curl's HTTP/3. Removed 2026-08-07 because it does not BUILD on macOS under spc 2.8.5, not because it was unwanted — recipes/php.sh's _PHP_SPC_LIBS carries the full reproduction. In one line: spc hands ngtcp2's configure OPENSSL_LIBS as absolute .a paths, libtool on macOS then treats each as a convenience archive and emits a 96-byte (empty) libngtcp2_crypto_ossl.a, and php fails to link with 15 undefined _ngtcp2_crypto_ossl_* symbols. Re-running the same make with libngtcp2_crypto_ossl_la_LIBADD= produces a correct 49 KB archive, which isolates the cause. Restoring HTTP/3 means carrying a patch against ngtcp2's Makefile.in."
	"nghttp3 1.18.0 the HTTP/3 framing layer; useful only alongside ngtcp2, so it leaves with it."
)

# Capability gaps against the Homebrew-php baseline. Recorded here because
# `brew install php@X.Y` is what this package replaces, so a gap is something a
# user loses by switching, not merely something we did not build.
#
#   pdo_dblib  Microsoft SQL Server via FreeTDS. Absent from spc's catalogue
#              entirely, so there is no source to pin even if we wanted one.
#              Homebrew's php formula has it. Nothing in OpenVHost uses it.
#   http3      curl's HTTP/3, via ngtcp2 + nghttp3. Homebrew's php DOES have
#              it (curl_version()'s feature_list carries HTTP3, checked
#              2026-08-07). Ours does not, for the upstream build reason in
#              PHP_PINS_EXCLUDED above. curl negotiates down to HTTP/2 on its
#              own, so the failure mode is slower, not broken.
PHP_PINS_KNOWN_GAPS="pdo_dblib,http3"
