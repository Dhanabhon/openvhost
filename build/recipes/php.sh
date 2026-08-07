# SPDX-License-Identifier: GPL-3.0-or-later
# shellcheck shell=bash
# shellcheck disable=SC2034  # every RECIPE_* variable here is read by
#                              build.sh and audit.sh, which source this file
#
# PHP — off-Homebrew slice 5A. Read
# docs/superpowers/specs/2026-08-07-p2-php-recipe-design.md (D1-D6) and
# docs/superpowers/specs/2026-08-07-p2-php-pin-set.md before touching this
# file; both are the reasoning this recipe is built on, not duplicated here.
#
# WHY THIS ISN'T A SINGLE bp_download + configure THE WAY nginx.sh/mariadb.sh
# ARE: PHP is built by static-php-cli ("spc"), which resolves and compiles
# ~40 third-party sources itself. spc verifies none of them (no hash, no
# signature, anywhere in its config — see the pin-set doc §1). D1's decision
# is that THIS recipe pins and verifies every one of those bytes itself, via
# the shared bp_download/bp_verify_sha256/bp_gpg_* helpers, so that `spc
# build` (task 2c) can run with no network reachable at all, against sources
# this recipe already vouches for.
#
# php-src itself gets the full treatment beyond the other 35 archives: a
# GPG-verified signature, not just a digest (D1). Its signing key is PER
# RELEASE, not per PHP major — whichever release manager cut that tarball
# signed it — so RECIPE_SOURCE_URL/SHA256/SIGNING_KEY_FPR are resolved below
# from _php-pins.sh's PHP_PINS_PHP_SRC table, keyed on RECIPE_VERSION, rather
# than hardcoded the way nginx.sh's single pinned version is.
#
# HOW THE STAGES MAP ONTO spc, WHICH HAS NO configure/build/install SPLIT
#
#   fetch      bp_download every pinned archive + php-src's .asc; clone the
#              one pinned git source (phpmicro) at its pinned commit.
#   verify     GPG + SHA-256 as usual; the git source's pin is its commit,
#              checked with `git rev-parse`.
#   extract    take a private copy of the spc checkout, synthesise the
#              `.lock.json` that tells spc our verified bytes are already
#              downloaded, and run `spc extract` — WITH NO NETWORK.
#   configure  resolve the host tools spc will shell out to (D2) into a
#              private bin/ that is the ONLY non-system directory on the PATH
#              spc sees, and record the exact `spc build` argv.
#   build      run that one `spc build` — WITH NO NETWORK — with spc's
#              BUILD_ROOT_PATH set to $BUILD_PREFIX, because that is the
#              string static OpenSSL bakes into bin/php and bin/php-fpm as
#              MODULESDIR/ENGINESDIR (D5) and it must be the same un-plantable
#              prefix every other package in this fleet embeds (D8), not a
#              scratch path under _work. build.sh's stage_install wipes
#              $BUILD_PREFIX before recipe_install runs, so the finished tree
#              is parked next door and put back — see recipe_build.
#   install    assemble the SHIPPED subset (bin/ + modules/) at $BUILD_PREFIX.
#   normalize  delete any absolute LC_RPATH left on a shipped Mach-O.
#
# "WITH NO NETWORK" is literal, not a claim: every spc invocation runs under
# sandbox-exec with `(deny network*)`. D1's whole value is being able to say
# which bytes we shipped, and a recipe that merely INTENDS not to fetch is one
# upstream change away from fetching. Measured both directions before this was
# written: the full build completes with the network denied, and with the
# network available but one pin removed the build FAILS rather than fetching
# the missing source (spc's build path has no download fallback —
# SourceManager::initSource throws).

# shellcheck source=/dev/null
. "$(dirname -- "${BASH_SOURCE[0]}")/_php-pins.sh"

# ---------------------------------------------------------------- provenance --

RECIPE_SOURCE_URL=""
RECIPE_SOURCE_SHA256=""
RECIPE_SIGNATURE_URL=""
RECIPE_SIGNING_KEY_FPR=""

# This has to run at SOURCE time, not inside recipe_fetch: build.sh checks
# RECIPE_SOURCE_URL/RECIPE_SOURCE_SHA256 are non-empty immediately after
# sourcing the recipe, before any stage — including recipe_fetch — ever runs.
# A recipe may only set variables and define functions at source time
# (README.md); calling this one function, which only assigns globals and
# bp_dies on no-match, stays inside that rule the same way nginx.sh's own
# RECIPE_PINNED_VERSION comparison (there, deferred into recipe_fetch because
# nginx has only one pinned version and RECIPE_SOURCE_URL/SHA256 can be
# assigned unconditionally) stays inside it.
_php_resolve_source_pin() {
	local row v sha256 fpr url sig
	for row in "${PHP_PINS_PHP_SRC[@]}"; do
		read -r v sha256 fpr url sig <<<"$row"
		if [ "$v" = "$RECIPE_VERSION" ]; then
			RECIPE_SOURCE_SHA256="$sha256"
			RECIPE_SIGNING_KEY_FPR="$fpr"
			RECIPE_SOURCE_URL="$url"
			RECIPE_SIGNATURE_URL="$sig"
			return 0
		fi
	done
	return 1
}

_php_pinned_versions() {
	local row v out=""
	for row in "${PHP_PINS_PHP_SRC[@]}"; do
		read -r v _ _ _ _ <<<"$row"
		out="${out:+$out, }$v"
	done
	printf '%s' "$out"
}

_php_resolve_source_pin ||
	bp_die "php.sh has no pin for PHP $RECIPE_VERSION; pinned versions are: $(_php_pinned_versions)"

# Blanket across all three pinned releases (verified independently for each —
# see the pin-set doc §6): none of the three signing keys expires, and all
# three fingerprints were cross-checked against a second host on the same
# day the pin set was derived.
RECIPE_SIGNING_KEY_EXPIRY="none"
RECIPE_SIGNING_KEY_VERIFIED_ON="$PHP_PINS_LAST_CHECKED"

# §14's tripwire. From here a PHP CVE is ours to notice.
RECIPE_UPSTREAM_RELEASE_DATE="$PHP_PINS_UPSTREAM_RELEASE_DATE"
RECIPE_LAST_CHECKED="$PHP_PINS_LAST_CHECKED"

# ------------------------------------------------------------------- inputs --

RECIPE_BUILD_TOOLS=(gpg)

# pkg-config is deliberately absent from RECIPE_BUILD_TOOLS. That mechanism
# resolves by `command -v` against the UNSCRUBBED host PATH (build/build.sh
# ~L577) — exactly the mechanism that is broken here: ServBay ships its own
# pkg-config ahead of Homebrew's on this build host's PATH (0.29.2, the same
# major-version-0 lineage spc itself would fetch unpinned — pin-set doc §8),
# and spc's own README documents nothing that would catch a wrong one
# silently succeeding. The exact precedent is recipes/mariadb.sh's bison:
# ServBay's was on PATH, right-looking name, could not even run. pkg-config's
# failure mode here is milder (ServBay's runs; it's just the wrong major) but
# the fix is the same shape — resolve candidates by absolute path and assert
# the version rather than trust the name (D2). See _php_pkgconfig below.
#
# pkg-config is not the only one. MEASURED on this build host, `command -v`
# resolves bison, xz, curl, bzip2, python3 and go to /Applications/ServBay/bin
# and pkg-config to ServBay's 0.29.2 — six of spc's documented build
# requirements, none of them the one intended. So none of spc's tools go
# through RECIPE_BUILD_TOOLS; _php_toolbin below resolves each by absolute
# path into a private directory which, together with /usr/bin:/bin:/usr/sbin:
# /sbin, is the ENTIRE PATH the spc invocation sees. Neither Homebrew's bin
# nor ServBay's is on it at all, so a tool this recipe did not name cannot be
# picked up by accident — a stronger property than "put the right one first".
#
# gpg stays in RECIPE_BUILD_TOOLS because it is the driver's tool, not spc's:
# it runs in recipe_fetch/recipe_verify_source, through bp_gpg, before any of
# this matters.

# spc builds its own OpenSSL from the pinned `openssl` entry in
# PHP_PINS_LIBS (3.6.3) as part of its own closure — unlike nginx.sh/
# mariadb.sh, which reuse a separately staged, shared static OpenSSL via
# RECIPE_DEPENDS. PHP has no use for that shared one: D5's finding is that
# php-src's own statically-linked OpenSSL bakes MODULESDIR/ENGINESDIR paths
# into bin/php and bin/php-fpm, so it has to be the copy spc itself compiles
# and controls, not a prefix borrowed from elsewhere.
#
# The one entry below is not a build input at all and is deliberately not
# pretending to be: nothing here links against nginx, and no byte of it
# reaches the artifact. It is contract check 6's FastCGI client. Design §9.3
# requires the probe to serve a real .php through a real web server rather
# than settle for `php-fpm -t`, and a probe whose client is "whatever nginx
# happens to be on this machine" would either skip on a clean builder or
# quietly prove something about Homebrew's nginx instead of ours. Declaring it
# here makes build.sh stage it (--stage-only, skipped when already present)
# before PHP's own stages start, so the failure mode "40 minutes of compiling,
# then check 6 has no client" cannot happen. The version is named the way
# nginx.sh names RECIPE_OPENSSL_VERSION: explicitly, so moving nginx's pin is
# a visible one-line edit here rather than a silent mismatch.
_PHP_PROBE_NGINX_VERSION="1.30.4"
RECIPE_DEPENDS=("nginx:$_PHP_PROBE_NGINX_VERSION")

# ------------------------------------------------------- the artifact contract --

# php-fpm, not php: it is the binary the app supervises, so it is the one
# checks 5 and 6 must exercise. `php-fpm --version` prints and exits 0.
RECIPE_SERVER_BIN="bin/php-fpm"

# spc's buildroot also carries lib/*.a, include/, license/, debug/*.dwarf and
# build-*.json; none of those is a RUNTIME input and together they are well
# over a gigabyte, so recipe_install ships two directories and nothing else.
# modules/ is not optional decoration — D6 makes `-d extension_dir=<tree>/
# modules` part of php_fpm_spec's argv, so the app names this directory.
RECIPE_REQUIRED_LAYOUT=(bin modules)

# Check 7's allowances. Every one was traced to the file that carries it
# before being written down — the README's rule is that an allowance is a
# promise nothing resolves the path, and you have to be able to keep it.
#
# The one that mattered most is fixed at CONFIGURE time and only then
# allowed, because reaching for the allowance first would have been the wrong
# move: php-src's default mysql socket is /tmp/mysql.sock, /tmp is mode 1777,
# and mysqli/pdo_mysql CONNECT to it when a script says 'localhost' — the
# same credential-harvesting shape the MariaDB slice found. --with-mysql-sock
# (see recipe_build) moves both extensions' compiled-in defaults to
# $BUILD_PREFIX/run/mysql.sock; MEASURED on the built binary,
# `mysqli.default_socket` and `pdo_mysql.default_socket` both now report that
# path. What survives is ONE literal, in php-src itself:
# ext/mysqlnd/mysqlnd_connection.c:536 falls back to "/tmp/mysql.sock" when
# the socket it was handed is NULL. No configure flag reaches it. So the
# allowance below is deliberately NOT the promise "nothing resolves this" —
# it is the narrower, true statement that after this build both ini defaults
# are non-empty, so the only route left is a script that first blanks
# mysqli.default_socket or pdo_mysql.default_socket (both PHP_INI_ALL) and
# then connects to 'localhost'. Before the flag, every such connection used
# it; after it, none do by default.
#
# The rest split into three kinds, none of which php-fpm can reach on the
# app's own path:
#
#   * A string in a data table, not a path. /tmp/wireshark.TRC000 lives in
#     ext/fileinfo's compiled-in libmagic database (php-src/ext/fileinfo/
#     data_file.c) as part of a magic RULE describing what a Wireshark trace
#     file looks like. Nothing ever opens it.
#   * mkstemp(3) templates reached only by `php -a`. libedit writes the
#     interactive shell's history through /tmp/.historyXXXXXX and
#     /tmp/histedit.XXXXXXXXXX (libedit/src/vi.c). O_EXCL on a random name is
#     the safe pattern for a 1777 directory, and php-fpm has no interactive
#     shell.
#   * Write targets behind an ini directive this app never sets, and cannot
#     set: /tmp/mysqlnd.trace needs mysqlnd.debug, and /tmp/jit- and /tmp/perf-
#     (opcache's perf/JIT dump prefixes) need opcache.jit_debug's perf bits.
#     All three are PHP_INI_SYSTEM, php-fpm runs with -n and no php.ini (D6),
#     and a script cannot ini_set() its way to them.
#
# /tmp/tkt is the odd one out and is called out rather than lumped in: it is
# krb5's FILE credential-cache root (krb5/src/lib/krb5/ccache/cc_file.c),
# which krb5 READS, and a planted ticket cache is a real if narrow concern.
# It is allowed because it is the system-wide Kerberos convention that macOS's
# own Kerberos.framework already follows on the same machine — changing it
# here would make our krb5 disagree with every other one on the box without
# closing anything — and because the only route to it from this artifact is
# curl's SPNEGO/GSSAPI, which nothing in OpenVHost uses.
RECIPE_ALLOWED_WRITABLE_PATHS=(
	/tmp/mysql.sock
	/tmp/wireshark.TRC000
	/tmp/.historyXXXXXX
	/tmp/histedit.XXXXXXXXXX
	/tmp/mysqlnd.trace
	/tmp/jit-
	/tmp/perf-
	/tmp/tkt
)

# ------------------------------------------ D5: the embedded OpenSSL paths --
#
# MEASURED on the built artifact, and written here rather than in a commit
# message because the reason check 7 passes is not visible in its output.
#
# `strings bin/php` and `strings bin/php-fpm` both carry, from the statically
# linked OpenSSL 3.6.3 that spc configures with --prefix=BUILD_ROOT_PATH:
#
#     MODULESDIR: "/opt/openvhost-build/php-8.4.24/lib/ossl-modules"
#     ENGINESDIR: "/opt/openvhost-build/php-8.4.24/lib/engines-3"
#
# and spc's build tree really does produce lib/ossl-modules/legacy.dylib
# (4.4 MB) at the first of them. This is the exact shape of the MariaDB
# finding that a neutral prefix is not an inert one: mariadbd's plugin-dir
# passed every check until someone ran `--verbose --help` and found it
# resolving out of a mode-1777 tree.
#
# Two things make it safe here, and BOTH are load-bearing:
#
#   1. WHERE THE PIPELINE BUILDS. Check 7 asks whether any proper ancestor of
#      an embedded path is world-writable. For this one the answer is no —
#      /opt is root:wheel 0755 and /opt/openvhost-build is 0700 — so check 7
#      passes silently, and it would have FAILED had the build root been
#      under /tmp (mode 1777) the way the reference MariaDB build once was.
#      That is not luck to be enjoyed, it is a property of build.sh's
#      bp_assert_unplantable, which refuses to run under a world-writable
#      ancestor at all. On a USER's machine the same path is unplantable for
#      a stronger reason: /opt is root-owned 0755, so an unprivileged process
#      cannot create /opt/openvhost-build in the first place.
#   2. WHAT recipe_install SHIPS. bin/ and modules/, by name — so
#      lib/ossl-modules/legacy.dylib is NOT in the artifact. The embedded
#      MODULESDIR therefore names a directory that will not exist anywhere on
#      a user's machine, and OpenSSL's legacy provider is simply unavailable.
#      Stated as a consequence rather than left to be discovered: legacy
#      ciphers (RC4, DES, seed-based key derivation) are not loadable from
#      this php. Nothing in OpenVHost wants them, and after relocation they
#      would have been unloadable anyway, since the embedded path is absolute.
#
# --openssldir stays at spc's /etc/ssl, which is right rather than merely
# tolerated: openssl_get_cert_locations() on the built binary reports
# default_cert_file=/etc/ssl/cert.pem and default_cert_dir=/etc/ssl/certs,
# which is where macOS keeps its trust store, and /etc is 0755. That is what
# makes TLS work on a user's Mac at all, and it is asserted by the offline
# openssl sanity check this recipe installs (see
# _php_replace_openssl_ext_test).

# ------------------------------------------------------ what gets built ------

# `-d` pairs at runtime, never a php.ini (D6). opcache and xdebug are the two
# shared extensions; everything else is compiled in.
_PHP_SPC_STATIC_EXTS="$PHP_PINS_EXT_STATIC"
_PHP_SPC_SHARED_EXTS="$PHP_PINS_EXT_SHARED"

# Libraries beyond the ones the extension set already forces, listed here
# rather than obtained with spc's `--with-suggested-libs` (-L). MEASURED: -L
# resolves to a 40-library closure whose last two members are libavif and
# libaom — and libaom is `type: git, rev: main`, an UNRELEASED development
# branch of a video codec, which _php-pins.sh's PHP_PINS_EXCLUDED refuses to
# pin. There is no way to say "suggested, except those two", so the suggestion
# set is spelled out instead. This list is exactly the -L closure minus
# libavif/libaom (which cost gd its AVIF encoder and nothing else) and minus
# `watcher` (see the note on PHP_PINS_GIT in _php-pins.sh: watcher is consumed
# only by the FrankenPHP Go build's `-tags=...nowatcher` switch, which this
# recipe never reaches).
#
# Dropping -L is not a saving, it is the difference between an artifact whose
# every byte is pinned and one that is not: without these, curl loses HTTP/2,
# brotli, zstd, IDN, SFTP, GSSAPI and async DNS, and gd loses JPEG, WebP and
# FreeType — all of which Homebrew's php formula has, and that formula is the
# regression baseline (_php-pins.sh's extension-set note).
#
# ngtcp2 and nghttp3 — curl's HTTP/3 — are the one deliberate omission from
# that list, and Homebrew's php DOES have HTTP3 (checked: curl_version()'s
# feature_list), so this is a real gap and is recorded as one in
# PHP_PINS_KNOWN_GAPS rather than left to be discovered. The cause is upstream
# and was reproduced down to the exact command:
#
#   spc's ngtcp2 builder passes OPENSSL_LIBS as a list of ABSOLUTE .a PATHS
#   (UnixLibraryTrait::getStaticLibFiles, options absolute_libs => true).
#   configure stores that in libngtcp2_crypto_ossl_la_LIBADD, and libtool on
#   macOS then treats each .a as a convenience archive: the link emits
#   "Linking the shared library ... against the static library ... is not
#   portable", ranlib warns "archive member 'libssl.a' not a mach-o file", and
#   the static archive it leaves behind is 96 bytes — a symbol table and
#   nothing else. ossl.o is compiled correctly and simply never gets in. php
#   then fails to link with 15 undefined _ngtcp2_crypto_ossl_* symbols.
#   Re-running the identical `make libngtcp2_crypto_ossl.la` with
#   `libngtcp2_crypto_ossl_la_LIBADD=` produced a correct 49 KB archive
#   containing ossl.o and shared.o, which isolates the cause exactly.
#
# There is no supported way to change what spc passes — the value is built
# inside spc from the resolved dependency graph, not read from the
# environment — so restoring HTTP/3 means carrying a patch against ngtcp2's
# Makefile.in, which is a bigger commitment than this slice should make
# unasked. Both pins were removed from _php-pins.sh with the same reasoning:
# an unused pin is a §14 CVE-tracking obligation for bytes we do not ship.
_PHP_SPC_LIBS="brotli,freetype,idn2,krb5,libcares,libjpeg,libssh2,libunistring,libwebp,nghttp2,xz,zstd"

# ------------------------------------------------------------------ helpers --

_php_src_tarball() { printf '%s/%s\n' "$BUILD_DOWNLOADS" "$(basename -- "$RECIPE_SOURCE_URL")"; }
_php_src_signature() { printf '%s/%s\n' "$BUILD_DOWNLOADS" "$(basename -- "$RECIPE_SIGNATURE_URL")"; }

# PHP_PINS_KEY_URL_TEMPLATES' `%s` filled in with <fpr>, into the global
# array _PHP_KEY_URLS (bash 3.2 has no way to return an array from a
# function except through a global — same constraint bp_record_flags'
# array-append idiom elsewhere in this pipeline works around).
#
# The pattern is `\%s`, not `%s`: inside a `${var/pattern/replacement}`
# pattern, a BARE leading `%` is bash's own "anchor at the end" operator, so
# `${tmpl/%s/$fpr}` parses as "replace a trailing literal s", silently
# corrupting every templated URL (confirmed live against bash 3.2.57 before
# writing this — `${tmpl/%s/X}` on ".../by-fingerprint/%s" produced
# ".../by-fingerprint/%X", not a fingerprint substitution). The backslash
# escapes `%` back to a literal character.
_php_key_urls() {
	local fpr="$1" tmpl
	_PHP_KEY_URLS=()
	for tmpl in "${PHP_PINS_KEY_URL_TEMPLATES[@]}"; do
		_PHP_KEY_URLS[${#_PHP_KEY_URLS[@]}]="${tmpl/\%s/$fpr}"
	done
}

# Where the static-php-cli checkout recipe_fetch is about to drive lives.
# There is no well-known install location to guess the way Homebrew gives
# bison one — spc is a manually maintained checkout, not a formula — so this
# is a required override, not a candidate list with a fallback.
_php_spc_dir() {
	[ -n "${OPENVHOST_SPC_DIR:-}" ] ||
		bp_die "OPENVHOST_SPC_DIR is not set. Point it at a static-php-cli checkout pinned to $PHP_PINS_SPC_TAG ($PHP_PINS_SPC_COMMIT) — clone $PHP_PINS_SPC_URL and check out that tag."
	[ -d "$OPENVHOST_SPC_DIR" ] ||
		bp_die "OPENVHOST_SPC_DIR ($OPENVHOST_SPC_DIR) does not exist"
	printf '%s\n' "$OPENVHOST_SPC_DIR"
}

# A pkg-config that actually is pkg-config-shaped AND new enough, proven by
# running it rather than trusting its path or its name (D2). ServBay's is a
# real, runnable pkg-config — unlike mariadb.sh's ServBay-bison case, this one
# doesn't fail to execute — it is simply the wrong, ancient (0.29.x) lineage,
# so the check that matters here is the version assertion, not an execution
# probe.
_php_pkgconfig_works() {
	local pc="$1" major
	[ -x "$pc" ] || return 1
	major="$("$pc" --version 2>/dev/null | awk -F. 'NR == 1 { print $1; exit }')"
	case "$major" in '' | *[!0-9]*) return 1 ;; esac
	[ "$major" -ge 2 ]
}

# Absolute path of a pkg-config that works. Candidates are absolute on
# purpose: `command -v pkg-config` is the thing that goes wrong on this host
# (ServBay precedes Homebrew on PATH). Homebrew's formula is `pkgconf`, which
# both installs `bin/pkg-config` as the public name and symlinks it under
# `opt/pkgconf/bin` — both listed, Apple Silicon and Intel prefixes, matching
# the breadth of recipes/mariadb.sh's `_mariadb_bison` candidate list.
_php_pkgconfig() {
	local candidate
	for candidate in \
		${OPENVHOST_PKGCONFIG:+"$OPENVHOST_PKGCONFIG"} \
		/opt/homebrew/opt/pkgconf/bin/pkg-config \
		/usr/local/opt/pkgconf/bin/pkg-config \
		/opt/homebrew/bin/pkg-config \
		/usr/local/bin/pkg-config; do
		if _php_pkgconfig_works "$candidate"; then
			printf '%s\n' "$candidate"
			return 0
		fi
	done
	bp_die "no working pkg-config >= 2 found (spc's own — an unpinned, unverified third-party GitHub release, pin-set doc §8 — and ServBay's are both the wrong, ancient lineage and must not be used, D2). Install one — \`brew install pkgconf\` — or point OPENVHOST_PKGCONFIG at it."
}

# The host PHP that runs bin/spc. spc 2.8.5's composer.lock requires >= 8.4
# and its platform check aborts on anything older — and `command -v php` on
# this host is ServBay's 8.2.30, which fails that check, so this is resolved
# by absolute path like everything else (D2). Yes, building our PHP package
# needs a PHP: that is spc's constraint, it applies to the BUILD MACHINE only,
# and the artifact it produces has no such dependency.
_php_host_php() {
	local candidate id
	for candidate in \
		${OPENVHOST_HOST_PHP:+"$OPENVHOST_HOST_PHP"} \
		/opt/homebrew/opt/php/bin/php \
		/opt/homebrew/bin/php \
		/usr/local/opt/php/bin/php \
		/usr/local/bin/php; do
		[ -x "$candidate" ] || continue
		id="$("$candidate" -r 'echo PHP_VERSION_ID;' 2>/dev/null)"
		case "$id" in '' | *[!0-9]*) continue ;; esac
		if [ "$id" -ge 80400 ]; then
			printf '%s\n' "$candidate"
			return 0
		fi
	done
	bp_die "no host PHP >= 8.4 found; static-php-cli $PHP_PINS_SPC_TAG refuses to start on anything older. Install one (\`brew install php\`) or point OPENVHOST_HOST_PHP at it."
}

# First existing executable among <candidates>, or empty. Absolute paths only:
# the whole point is to never ask PATH.
_php_first_exe() {
	local candidate
	for candidate in "$@"; do
		if [ -x "$candidate" ]; then
			printf '%s\n' "$candidate"
			return 0
		fi
	done
	return 1
}

# ------------------------------------------------------- spc's environment --

# Scratch directories. All four are siblings of $BUILD_DOWNLOADS/$BUILD_SRC/
# $BUILD_OBJ under $BUILD_WORK — scratch the interface does not name by name
# but which it explicitly allows (build/recipes/README.md, "Paths a recipe may
# write to"), the same latitude nginx.sh takes for its pcre2-include/.
_php_spc_work() { printf '%s/spc\n' "$BUILD_WORK"; }
_php_toolbin() { printf '%s/toolbin\n' "$BUILD_WORK"; }
_php_pkgroot() { printf '%s/pkgroot\n' "$BUILD_WORK"; }
_php_tmpdir() { printf '%s/tmp\n' "$BUILD_WORK"; }
_php_sandbox_profile() { printf '%s/no-network.sb\n' "$BUILD_WORK"; }
# Where recipe_build parks the finished tree so build.sh's stage_install can
# wipe $BUILD_PREFIX without destroying it. See recipe_build.
_php_built_tree() { printf '%s/built\n' "$BUILD_OBJ"; }

# The tools spc shells out to that are NOT in /usr/bin or /bin, each resolved
# by absolute path and linked into one private directory. Homebrew's
# `opt/<formula>/bin` form is listed before its `bin` form because the former
# is version-stable and cannot be shadowed by another formula's symlink.
#
# Only what is genuinely needed is here, and each entry was established rather
# than guessed:
#   bison       php-src's ./buildconf; /usr/bin/bison is 2.3 and spc requires
#               >= 3, so the system one is not a fallback.
#   re2c        php-src's lexers.
#   cmake       the CMake-built libraries (brotli, libzip, libjpeg, …).
#   autoconf    ./buildconf --force runs it, with autom4te/autoheader beside
#               it and m4 underneath.
#   pkg-config  D2, and _php_pkgconfig asserts the major version.
#   xz          NOT listed: macOS ships no /usr/bin/xz, but bsdtar decompresses
#               .tar.xz through libarchive with no external binary (measured on
#               php-8.4.24.tar.xz), which is the only thing spc needs it for.
#   glibtoolize NOT listed: the only recipes that run libtoolize are attr and
#               libacl, both Linux-only, and krb5 — the one library here whose
#               builder can call autoreconf — ships a pre-generated
#               src/configure in its release tarball, so it never does.
_php_link_tool() {
	local name="$1" resolved
	shift
	resolved="$(_php_first_exe "$@")" ||
		bp_die "build tool '$name' not found at any of: $*. Install it (Homebrew) or the spc build cannot run."
	ln -sf "$resolved" "$(_php_toolbin)/$name"
	bp_log "spc tool $name -> $resolved"
}

_php_prepare_toolbin() {
	local toolbin ac_dir candidate
	toolbin="$(_php_toolbin)"
	bp_rm_tree "$toolbin"
	mkdir -p "$toolbin"

	_php_link_tool pkg-config "$(_php_pkgconfig)"
	_php_link_tool bison \
		/opt/homebrew/opt/bison/bin/bison /usr/local/opt/bison/bin/bison
	_php_link_tool re2c \
		/opt/homebrew/opt/re2c/bin/re2c /usr/local/opt/re2c/bin/re2c \
		/opt/homebrew/bin/re2c /usr/local/bin/re2c
	_php_link_tool cmake \
		/opt/homebrew/opt/cmake/bin/cmake /usr/local/opt/cmake/bin/cmake \
		/opt/homebrew/bin/cmake /usr/local/bin/cmake
	_php_link_tool m4 \
		/opt/homebrew/opt/m4/bin/m4 /usr/local/opt/m4/bin/m4 /usr/bin/m4

	# autoconf's helpers have to come from the SAME installation as autoconf
	# itself — autom4te reads autoconf's own m4 data files by a path baked in
	# at ITS build time, so a mix-and-match of two prefixes silently produces
	# "autom4te: cannot open .../autom4te.cfg".
	ac_dir=""
	for candidate in /opt/homebrew/opt/autoconf/bin /usr/local/opt/autoconf/bin; do
		if [ -x "$candidate/autoconf" ]; then
			ac_dir="$candidate"
			break
		fi
	done
	[ -n "$ac_dir" ] ||
		bp_die "autoconf not found at /opt/homebrew/opt/autoconf/bin or /usr/local/opt/autoconf/bin; php-src's ./buildconf cannot run without it"
	for candidate in autoconf autoheader autom4te autoreconf autoupdate ifnames; do
		[ -x "$ac_dir/$candidate" ] || continue
		ln -sf "$ac_dir/$candidate" "$toolbin/$candidate"
	done
	bp_log "spc tool autoconf -> $ac_dir/autoconf"
}

# D2 again, from the other end: PkgConfigUtil::findPkgConfig() does not look
# at PATH at all — it looks at exactly two places, PKG_ROOT_PATH/bin/pkg-config
# and BUILD_BIN_PATH/pkg-config, in that order — so putting the right one on
# PATH would NOT be enough. PKG_ROOT_PATH is pointed at our own directory and
# the host's verified pkg-config is linked in as the first of those two, which
# also guarantees spc never installs its own (an unpinned, unverified
# third-party GitHub release it would then execute inside the build).
_php_prepare_pkgroot() {
	local pkgroot
	pkgroot="$(_php_pkgroot)"
	bp_rm_tree "$pkgroot"
	mkdir -p "$pkgroot/bin"
	ln -sf "$(_php_pkgconfig)" "$pkgroot/bin/pkg-config"
}

# A private copy of the spc checkout, so the build never writes into the
# operator's tree (spc puts config.log copies and its own spc.output.log in
# `log/` under its working directory, which is not a path this recipe is
# allowed to touch) and so spc's remaining WORKING_DIR-derived defaults land
# under $BUILD_WORK like everything else. downloads/ is excluded deliberately:
# that directory is 655 MB of the operator's own unverified `spc download`
# output, and DOWNLOAD_PATH is pointed at OUR verified one instead.
_php_prepare_spc_copy() {
	local src dst entry name
	src="$(_php_spc_dir)"
	dst="$(_php_spc_work)"
	bp_rm_tree "$dst"
	mkdir -p "$dst"
	# A top-level-entry loop, not `tar --exclude`. bsdtar's exclude patterns
	# are not anchored: `--exclude=./downloads` is normalised to `downloads`
	# and then matches ANY path component, so a first attempt at this silently
	# dropped vendor/psr/log as well as log/ — spc then died on a missing
	# Psr\Log\AbstractLogger. Skipping by exact top-level name has no pattern
	# semantics to get wrong.
	for entry in "$src"/* "$src"/.[!.]*; do
		[ -e "$entry" ] || continue
		name="$(basename -- "$entry")"
		case "$name" in
		# spc's own scratch: downloads/ is 655 MB of the OPERATOR's unverified
		# `spc download` output and DOWNLOAD_PATH is pointed at our verified
		# one instead; the rest are outputs of a previous run of theirs.
		downloads | source | buildroot | pkgroot | log | .git) continue ;;
		esac
		cp -R -- "$entry" "$dst/$name"
	done
	[ -f "$dst/bin/spc" ] ||
		bp_die "the spc copy at $dst has no bin/spc; the checkout at $src does not look like static-php-cli"
	[ -f "$dst/vendor/autoload.php" ] ||
		bp_die "the spc copy at $dst has no vendor/autoload.php; run \`composer install\` in $src"
	mkdir -p "$dst/log"
	_php_replace_openssl_ext_test "$dst"
}

# The ONE thing in `spc build` that genuinely wants the network, replaced in
# our private copy rather than worked around.
#
# MEASURED, not assumed. spc's post-build sanity check runs
# src/globals/ext-tests/<ext>.php against the freshly built bin/php for every
# extension. All 48 of ours pass with the network denied except one:
# ext-tests/openssl.php opens a live TLS connection to captive.apple.com,
# detectportal.firefox.com, static-php.dev or www.example.com and asserts that
# at least one succeeded. Under the sandbox that assertion fails and `spc
# build` exits non-zero AFTER it has already produced correct binaries — so
# the choice is between giving the build a network and replacing the fixture.
# D1 settles that: the network stays off. (ext-tests/curl.php has a similar
# block, but it is gated on `schannel`, i.e. Windows, so on macOS it never
# runs — checked, not guessed.)
#
# The replacement keeps every offline assertion the original made and swaps
# the round trip for a check of what that round trip was really testing: that
# this statically linked OpenSSL looks for CAs where macOS keeps them. It also
# does a real sign/verify against a freshly generated RSA key, which exercises
# more of the library than reaching a web server does. Deliberately NOT
# claimed: nothing here proves the shipped binary can verify a real
# certificate chain end to end. That belongs in a probe against a live TLS
# endpoint, which is a runtime concern, not a build-host one.
#
# No `//` or `#` comments in the heredoc below: spc inlines this file into a
# `php -r "..."` command line after deleting every newline, so a line comment
# would swallow the rest of the test.
_php_replace_openssl_ext_test() {
	local dst="$1" target
	target="$dst/src/globals/ext-tests/openssl.php"
	[ -f "$target" ] ||
		bp_die "spc's ext-tests/openssl.php is not where it was ($target); the sanity-check surface moved and this replacement needs re-deriving"
	cat >"$target" <<'EOF'
<?php

declare(strict_types=1);

assert(function_exists('openssl_digest'));
assert(openssl_digest('123456', 'md5') === 'e10adc3949ba59abbe56e057f20f883e');
$loc = openssl_get_cert_locations();
assert(is_array($loc));
assert($loc['default_cert_file'] === '/etc/ssl/cert.pem');
assert($loc['default_cert_dir'] === '/etc/ssl/certs');
assert(in_array('sha256', openssl_get_md_methods(), true));
assert(count(openssl_get_cipher_methods()) > 0);
$key = openssl_pkey_new(['private_key_bits' => 2048, 'private_key_type' => OPENSSL_KEYTYPE_RSA]);
assert($key !== false);
$sig = '';
assert(openssl_sign('openvhost', $sig, $key, OPENSSL_ALGO_SHA256));
assert(openssl_verify('openvhost', $sig, openssl_pkey_get_details($key)['key'], OPENSSL_ALGO_SHA256) === 1);
if (PHP_VERSION_ID >= 80500 && (!PHP_ZTS || PHP_OS_FAMILY !== 'Windows') && defined('OPENSSL_VERSION_NUMBER') && OPENSSL_VERSION_NUMBER >= 0x30200000) {
    assert(function_exists('openssl_password_hash'));
}
EOF
	bp_log "replaced spc's ext-tests/openssl.php with the offline equivalent (no network during the build)"
}

# `(deny network*)` covers AF_INET and AF_UNIX alike, which is why it also
# blocks DNS: mDNSResponder is reached over a unix socket. Verified live
# before this was written — inside the profile, curl to a HOSTNAME fails to
# resolve AND curl to a bare IP fails to connect, while clang still compiles
# and links. sandbox-exec carries a deprecation warning from Apple; if it ever
# stops working this recipe stops building, which is the failure direction to
# prefer over a build that quietly regains the network.
_php_write_sandbox_profile() {
	cat >"$(_php_sandbox_profile)" <<'EOF'
(version 1)
(allow default)
(deny network*)
EOF
}

# One `spc` invocation, with the network denied, the PATH reduced to our own
# tool directory plus the system ones, and every spc output path redirected
# under the build root.
#
# BUILD_ROOT_PATH is $BUILD_PREFIX and not a scratch directory: spc configures
# its OpenSSL with `--prefix=BUILD_ROOT_PATH` (its macos/library/openssl.php),
# so that string becomes MODULESDIR/ENGINESDIR inside bin/php and bin/php-fpm
# — D5's finding. Making it the real install prefix is what puts that embedded
# path under check 7's guarantee instead of naming a directory that gets
# deleted; see the note above recipe_build.
#
# A real environment variable beats config/env.ini (GlobalEnvManager::init
# only fills in a key when getenv() returns false), which is how
# SPC_CMD_PREFIX_PHP_CONFIGURE below gets `--disable-rpath` added to spc's own
# macOS default. That default is restated here, so it has to be re-read
# whenever the pinned spc commit moves — the same obligation D4's commit
# tripwire already enforces for the pin set.
_php_spc() {
	local spc_dir toolbin
	spc_dir="$(_php_spc_work)"
	toolbin="$(_php_toolbin)"
	mkdir -p "$(_php_tmpdir)"
	(
		cd "$spc_dir" || exit 1
		PATH="$toolbin:/usr/bin:/bin:/usr/sbin:/sbin" \
			TMPDIR="$(_php_tmpdir)" \
			BUILD_ROOT_PATH="$BUILD_PREFIX" \
			SOURCE_PATH="$BUILD_SRC" \
			DOWNLOAD_PATH="$BUILD_DOWNLOADS" \
			PKG_ROOT_PATH="$(_php_pkgroot)" \
			SPC_CONCURRENCY="$BUILD_JOBS" \
			SPC_CMD_PREFIX_PHP_CONFIGURE="./configure --prefix= --with-valgrind=no --enable-shared=no --enable-static=yes --disable-all --disable-phpdbg --disable-rpath --with-mysql-sock=$BUILD_PREFIX/run/mysql.sock" \
			/usr/bin/sandbox-exec -f "$(_php_sandbox_profile)" \
			"$(_php_host_php)" bin/spc "$@"
	)
}

# Everything an spc invocation needs, idempotent, called by every stage that
# drives spc. Cheap (a copy of an 18 MB checkout and a handful of symlinks)
# and it means `--from configure` or `--from install` works without having to
# remember which earlier stage created what.
_php_prepare_spc() {
	_php_prepare_spc_copy
	_php_prepare_toolbin
	_php_prepare_pkgroot
	_php_write_sandbox_profile
}

# ------------------------------------------------------------- the lock file --
#
# spc will not extract a source it has no lock entry for, and it computes lock
# entries only as a side effect of ITS OWN downloader — the one that fetches
# unverified bytes over a redirect-following curl with no digest (D1). So the
# lock file is synthesised here from the pin set instead, and spc's downloader
# is never run at all.
#
# The two fields that cannot be invented are read out of spc's own
# config/source.json rather than duplicated into _php-pins.sh: `source_type`
# (git vs archive) and `move_path` (`micro` extracts into
# php-src/sapi/micro, not source/micro). Duplicating them would be one more
# thing to rot silently, which is the failure D4 exists to prevent.
_php_write_lock_file() {
	local inputs generator row name filename commit
	inputs="$BUILD_WORK/lock-inputs.tsv"
	generator="$BUILD_WORK/write-lock.php"

	: >"$inputs"
	printf 'archive\t%s\t%s\n' php-src "$(basename -- "$RECIPE_SOURCE_URL")" >>"$inputs"
	for row in "${PHP_PINS_LIBS[@]}"; do
		read -r name _ _ filename _ <<<"$row"
		printf 'archive\t%s\t%s\n' "$name" "$filename" >>"$inputs"
	done
	for row in "${PHP_PINS_GIT[@]}"; do
		read -r name commit _ <<<"$row"
		printf 'git\t%s\t%s\n' "$name" "$commit" >>"$inputs"
	done

	cat >"$generator" <<'EOF'
<?php
// Written by build/recipes/php.sh. Emits <download-path>/.lock.json from the
// recipe's pin set, so `spc extract` and `spc build` find every source
// already present and never reach for the network.
//
// Field shapes come from spc's own Downloader::downloadByType()/LockFile::
// lockSource(): an archive entry carries filename + a SHA-1 of the bytes on
// disk, a git entry carries dirname + the checked-out commit, and both carry
// move_path (config/source.json's `path`/`extract`) and lock_as = 1
// (SPC_DOWNLOAD_SOURCE).
declare(strict_types=1);
[$self, $inputs, $downloads, $source_json] = $argv + [null, null, null, null];
if ($inputs === null || $downloads === null || $source_json === null) {
    fwrite(STDERR, "usage: write-lock.php <inputs.tsv> <download-path> <source.json>\n");
    exit(2);
}
$config = json_decode((string) file_get_contents($source_json), true);
if (!is_array($config)) {
    fwrite(STDERR, "cannot read spc's config/source.json at {$source_json}\n");
    exit(1);
}
$lock = [];
foreach (file($inputs, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES) as $line) {
    [$kind, $name, $value] = explode("\t", $line);
    if (!isset($config[$name])) {
        fwrite(STDERR, "pinned source [{$name}] is not in spc's config/source.json; the pin set and the spc checkout disagree\n");
        exit(1);
    }
    $move_path = $config[$name]['path'] ?? $config[$name]['extract'] ?? null;
    if ($kind === 'git') {
        $dir = "{$downloads}/{$name}";
        if (!is_dir("{$dir}/.git")) {
            fwrite(STDERR, "pinned git source [{$name}] is not a checkout at {$dir}\n");
            exit(1);
        }
        $lock[$name] = [
            'source_type' => 'git',
            'dirname' => $name,
            'move_path' => $move_path,
            'lock_as' => 1,
            'hash' => $value,
        ];
        continue;
    }
    $file = "{$downloads}/{$value}";
    if (!is_file($file)) {
        fwrite(STDERR, "pinned source [{$name}] is missing from {$file}\n");
        exit(1);
    }
    $lock[$name] = [
        'source_type' => 'archive',
        'filename' => $value,
        'move_path' => $move_path,
        'lock_as' => 1,
        'hash' => sha1_file($file),
    ];
}
$out = "{$downloads}/.lock.json";
if (file_put_contents($out, json_encode($lock, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES)) === false) {
    fwrite(STDERR, "cannot write {$out}\n");
    exit(1);
}
fwrite(STDOUT, count($lock) . " sources locked from the pin set\n");
EOF

	"$(_php_host_php)" "$generator" "$inputs" "$BUILD_DOWNLOADS" \
		"$(_php_spc_work)/config/source.json"
}

# The source names `spc extract` is asked for, in dependency order where it
# matters: php-src first, then micro, whose move_path is INSIDE php-src
# (php-src/sapi/micro) and which therefore cannot be unpacked before it.
_php_source_list() {
	local row name out="php-src"
	for row in "${PHP_PINS_GIT[@]}"; do
		read -r name _ _ <<<"$row"
		out="$out,$name"
	done
	for row in "${PHP_PINS_LIBS[@]}"; do
		read -r name _ _ _ _ <<<"$row"
		out="$out,$name"
	done
	printf '%s\n' "$out"
}

_php_micro_checkout() { printf '%s/micro\n' "$BUILD_DOWNLOADS"; }

# ------------------------------------------------------------------- stages --

recipe_fetch() {
	# Layer 1 of D4's rot alarm, and first: before this recipe downloads a
	# single byte on the strength of a pin set derived from a specific spc
	# tree, prove that tree is still the one being driven. An spc checkout
	# that moved out from under the pins is exactly the failure D4 names —
	# catching it here means it fails loudly before any network use, not as
	# a confusing `spc build` error partway through task 2c.
	local spc_dir spc_head
	spc_dir="$(_php_spc_dir)"
	spc_head="$(git -C "$spc_dir" rev-parse HEAD 2>/dev/null)" ||
		bp_die "could not read the HEAD commit of the spc checkout at $spc_dir"
	[ "$spc_head" = "$PHP_PINS_SPC_COMMIT" ] ||
		bp_die "_php-pins.sh was derived from spc $PHP_PINS_SPC_TAG ($PHP_PINS_SPC_COMMIT); the checkout at $spc_dir is $spc_head. Regenerate the pins with _php-pins-refresh.sh or check out the pinned tag."

	bp_download "$RECIPE_SOURCE_URL" "$(_php_src_tarball)"
	bp_download "$RECIPE_SIGNATURE_URL" "$(_php_src_signature)"

	# Every one of the 35 pinned third-party archives (D1) — ours, through
	# bp_download, never spc's own (unverified, `curl -sfSL`, no --proto)
	# fetch. The <spc-filename> column is load-bearing (_php-pins.sh's own
	# header): spc locates a pre-downloaded archive by exactly that name.
	local row filename url name commit checkout
	for row in "${PHP_PINS_LIBS[@]}"; do
		read -r _ _ _ filename url <<<"$row"
		bp_download "$url" "$BUILD_DOWNLOADS/$filename"
	done

	# The one git source (phpmicro). A commit is the pin — a stronger one than
	# a tarball digest, because it also fixes every submodule (_php-pins.sh's
	# PHP_PINS_GIT note) — so this is a fetch to a named object, never a
	# branch. `--no-tags` and a bare `git fetch <url> <sha>` rather than
	# `clone`: cloning a branch and then checking the commit out would put
	# whatever master points at today on disk first, which is the thing being
	# avoided. Re-fetching an existing checkout is skipped the way bp_download
	# skips an archive already present; recipe_verify_source re-checks the
	# commit either way, so a stale directory cannot pass unnoticed.
	for row in "${PHP_PINS_GIT[@]}"; do
		read -r name commit url <<<"$row"
		checkout="$BUILD_DOWNLOADS/$name"
		bp_assert_under "$checkout" "$BUILD_ROOT" "clone into"
		if [ -d "$checkout/.git" ]; then
			bp_log "already fetched: $name ($commit)"
			continue
		fi
		bp_rm_tree "$checkout"
		mkdir -p "$checkout"
		git -C "$checkout" init --quiet ||
			bp_die "could not create a git repository at $checkout"
		git -C "$checkout" fetch --quiet --depth 1 --no-tags "$url" "$commit" ||
			bp_die "could not fetch $commit from $url"
		git -C "$checkout" checkout --quiet FETCH_HEAD ||
			bp_die "could not check out $commit in $checkout"
		bp_log "fetched $name at $commit from $url"
	done

	bp_gpg_init_home
	_php_key_urls "$RECIPE_SIGNING_KEY_FPR"
	bp_gpg_import_key "$RECIPE_SIGNING_KEY_FPR" "" \
		${_PHP_KEY_URLS[@]+"${_PHP_KEY_URLS[@]}"}
}

recipe_verify_source() {
	# The signature says php.net's release manager produced these bytes; the
	# pinned digest says they are the same bytes the pin set was reviewed
	# against. Both, in that order — same as nginx.sh and mariadb.sh.
	bp_gpg_verify_signature "$(_php_src_tarball)" "$(_php_src_signature)" "$RECIPE_SIGNING_KEY_FPR"
	bp_verify_sha256 "$(_php_src_tarball)" "$RECIPE_SOURCE_SHA256"

	local row sha256 filename name commit checkout head dirty
	for row in "${PHP_PINS_LIBS[@]}"; do
		read -r _ _ sha256 filename _ <<<"$row"
		bp_verify_sha256 "$BUILD_DOWNLOADS/$filename" "$sha256"
	done

	# The git source's digest IS its commit — git's own object hashing covers
	# every byte of every file at that revision — so this is the same kind of
	# check as the SHA-256s above, not a weaker stand-in. The worktree is
	# checked too: a commit pin says nothing about a file edited after
	# checkout, and the extract stage copies the WORKTREE, not the objects.
	for row in "${PHP_PINS_GIT[@]}"; do
		read -r name commit _ <<<"$row"
		checkout="$BUILD_DOWNLOADS/$name"
		head="$(git -C "$checkout" rev-parse HEAD 2>/dev/null)" ||
			bp_die "pinned git source [$name] is not a checkout at $checkout"
		[ "$head" = "$commit" ] ||
			bp_die "pinned git source [$name] is at $head, not the pinned $commit"
		dirty="$(git -C "$checkout" status --porcelain 2>/dev/null)"
		[ -z "$dirty" ] ||
			bp_die "pinned git source [$name] has local modifications at $checkout; refusing to build from it"
		bp_log "commit verified: $name $commit"
	done
}

recipe_extract() {
	_php_prepare_spc
	_php_write_lock_file

	# From here nothing may reach the network, and nothing does: _php_spc runs
	# every invocation under the sandbox profile. `spc extract` unpacks each
	# locked source into $BUILD_SRC (spc's SOURCE_PATH), which is exactly what
	# this stage is required to do — and it is a cheap, early failure if a
	# pinned archive is truncated or the lock file is wrong, rather than a
	# confusing one forty minutes into `spc build`.
	_php_spc extract "$(_php_source_list)"

	[ -f "$BUILD_SRC/php-src/configure" ] || [ -f "$BUILD_SRC/php-src/buildconf" ] ||
		bp_die "php-src does not look extracted at $BUILD_SRC/php-src"
	[ -f "$BUILD_SRC/php-src/sapi/micro/php_micro.c" ] ||
		bp_die "phpmicro did not land at $BUILD_SRC/php-src/sapi/micro; its move_path is not what spc's config/source.json says"
}

recipe_configure() {
	# spc has no configure step of its own: one `spc build` invocation
	# configures and compiles ~35 libraries and php-src together. What belongs
	# here is therefore everything that FIXES the build's inputs — the tool
	# resolution (D2) and the exact argv — recorded so the manifest carries
	# the intent, per §7's "intent that is not recorded is not auditable".
	_php_prepare_spc

	local args
	args=$(_php_spc_build_args)
	# shellcheck disable=SC2086  # deliberate: one flag per word, no flag here
	#                              contains whitespace (asserted below)
	bp_record_flags $args

	# The assertion behind that disable. _php_spc_build_args emits one flag per
	# LINE, so a space or a tab anywhere in the result means some flag
	# contains one — and that flag would be recorded as two in the manifest
	# and passed to spc as two words. Worth failing on, not papering over.
	case "$args" in
	*" "* | *"	"*) bp_die "an spc build flag contains whitespace; _php_spc_build_args must stay one-flag-per-line" ;;
	esac
}

# The one `spc build` argv, in one place, because recipe_configure records it
# and recipe_build runs it and those two must not be able to drift apart.
#
# --build-cli as well as --build-fpm: the app supervises php-fpm, but `brew
# install php` — the regression baseline — also gives the user a `php` on
# their machine, and 5C routes Install to this package. Shipping php-fpm alone
# would be a capability the user loses by switching.
_php_spc_build_args() {
	printf '%s\n' "build
$_PHP_SPC_STATIC_EXTS
--build-cli
--build-fpm
--build-shared=$_PHP_SPC_SHARED_EXTS
--with-libs=$_PHP_SPC_LIBS
--with-config-file-path=$BUILD_PREFIX/etc/php
--with-config-file-scan-dir=$BUILD_PREFIX/etc/php/conf.d"
}

recipe_build() {
	_php_prepare_spc

	# spc's default compiled-in ini paths are /usr/local/etc/php and
	# /usr/local/etc/php/conf.d. This app always launches php-fpm with `-n`,
	# so neither is a live path for it — but `-n` is one `-c` away from
	# evaporating (design §2) and /usr/local is a directory Homebrew makes
	# group-writable on plenty of Macs, so they are pointed at $BUILD_PREFIX
	# instead: un-plantable by construction (D8), exactly as nginx.sh does
	# with --conf-path and friends.
	#
	# The same treatment, for the same reason, is applied to php's compiled-in
	# mysql socket via --with-mysql-sock in _php_spc: php-src's default is
	# /tmp/mysql.sock, /tmp is mode 1777, and PHP_MYSQL_UNIX_SOCK_ADDR is what
	# mysqli/pdo_mysql CONNECT to when a script says 'localhost'. That is the
	# same defect the MariaDB slice found — a credential-harvesting default
	# any local process can plant — and check 7 catches it here too. It is
	# pointed at $BUILD_PREFIX/run/mysql.sock, which nothing can create on a
	# user's machine, so a 'localhost' connection fails loudly instead of
	# quietly reaching whatever is at /tmp/mysql.sock. OpenVHost's own MySQL
	# and MariaDB listen on 127.0.0.1 and on a socket under the user's
	# ~/.openvhost/run, neither of which is knowable at build time, so there
	# is no correct value to compile in — only a safe one.
	local args
	args=$(_php_spc_build_args)
	# shellcheck disable=SC2086  # one flag per word; recipe_configure asserts it
	_php_spc $args

	# spc reports a shared extension it could not build as a WARNING and exits
	# 0 (BuildPHPCommand: "Shared extension [x] not found, please check!"), so
	# a missing modules/*.so would otherwise reach the audit as a layout
	# failure three stages later. Derived from _PHP_SPC_SHARED_EXTS rather
	# than listed again, so a new shared extension cannot be added without
	# this check covering it.
	local want
	for want in bin/php bin/php-fpm; do
		[ -f "$BUILD_PREFIX/$want" ] ||
			bp_die "spc build finished but $want is missing from $BUILD_PREFIX"
	done
	for want in $(printf '%s' "$_PHP_SPC_SHARED_EXTS" | tr ',' ' '); do
		[ -f "$BUILD_PREFIX/modules/$want.so" ] ||
			bp_die "spc build finished but modules/$want.so is missing from $BUILD_PREFIX"
	done

	# Park the finished tree so build.sh's stage_install — which does
	# `bp_rm_tree "$BUILD_PREFIX"` before calling recipe_install — cannot
	# destroy it.
	#
	# This is NOT the DESTDIR defect build/recipes/README.md warns about. There,
	# the path a package EMBEDS is the staging directory it was later moved out
	# of. Here the embedded path (static OpenSSL's MODULESDIR/ENGINESDIR, D5) is
	# $BUILD_PREFIX — the path the shipped tree is assembled at one stage later
	# — and the parking directory is never named by a single byte of the
	# artifact. A rename within one filesystem, not a copy, so it costs nothing.
	local parked
	parked="$(_php_built_tree)"
	bp_rm_tree "$parked"
	mkdir -p "$(dirname -- "$parked")"
	mv -- "$BUILD_PREFIX" "$parked"
}

recipe_install() {
	local parked file
	parked="$(_php_built_tree)"
	[ -d "$parked" ] ||
		bp_die "no built tree at $parked; run the build stage (or drop --from install)"

	mkdir -p "$BUILD_PREFIX/bin" "$BUILD_PREFIX/modules"

	# Two directories, by name. spc's buildroot is ~1.5 GB — lib/*.a,
	# include/, license/, build-*.json, and debug/*.dwarf which spc produces
	# with dsymutil and never signs — and none of it is a runtime input for a
	# statically linked php-fpm. Naming what ships (rather than pruning what
	# does not) means a new spc output directory arrives OUT of the artifact
	# by default, which is the direction that fails safe. It is also why
	# recipe_normalize has no .dwarf to delete: they are never copied in.
	for file in php php-fpm; do
		cp -p -- "$parked/bin/$file" "$BUILD_PREFIX/bin/$file"
	done
	for file in $(printf '%s' "$_PHP_SPC_SHARED_EXTS" | tr ',' ' '); do
		[ -f "$parked/modules/$file.so" ] ||
			bp_die "shared extension $file.so is missing from $parked/modules"
		cp -p -- "$parked/modules/$file.so" "$BUILD_PREFIX/modules/$file.so"
	done
}

# Runs after install and before signing, which is the only window in which a
# Mach-O may still be edited (build.sh's stage_sign comment).
recipe_normalize() {
	local macho rpath removed=0 kept=0

	# Every LC_RPATH that is not @loader_path-relative, deleted. Contract
	# check 2 rejects them for a real reason: `otool -L` never shows an
	# LC_RPATH, so an absolute one defeats relocation invisibly.
	#
	# BUILD-TIME SUPPRESSION, MEASURED, not assumed: php-src's own configure
	# takes --disable-rpath (it is a PHP_ARG_ENABLE in configure.ac, and
	# without it php's build appends -Wl,-rpath,<libdir> for every
	# PHP_ADD_LIBPATH), and _php_spc passes it through
	# SPC_CMD_PREFIX_PHP_CONFIGURE. That is the right place to fix it — a flag
	# beats a post-hoc edit — and it is what leaves bin/php and bin/php-fpm
	# clean. It does NOT cover modules/*.so: those are built by a separate
	# phpize/configure run inside spc's Extension::buildShared(), which this
	# recipe does not get to pass flags to. So the loop below stays, and it is
	# load-bearing for the shared extensions rather than belt-and-braces.
	while IFS= read -r macho; do
		[ -n "$macho" ] || continue
		while IFS= read -r rpath; do
			[ -n "$rpath" ] || continue
			case "$rpath" in
			@loader_path | @loader_path/?*)
				kept=$((kept + 1))
				continue
				;;
			esac
			install_name_tool -delete_rpath "$rpath" "$macho" >/dev/null 2>&1 ||
				bp_die "could not delete absolute LC_RPATH '$rpath' from ${macho#"$BUILD_PREFIX"/}"
			bp_log "deleted LC_RPATH $rpath from ${macho#"$BUILD_PREFIX"/}"
			removed=$((removed + 1))
		done < <(otool -l "$macho" 2>/dev/null |
			awk '/^ *cmd LC_RPATH$/{want=1} want && /^ *path /{print $2; want=0}')
	done < <(bp_machos "$BUILD_PREFIX")

	bp_log "normalize: deleted $removed absolute LC_RPATH entries, kept $kept @loader_path-relative ones"
}

# --------------------------------------------------- contract check 6 (serve) --
#
# Everything below runs inside audit.sh, where bp_log/bp_die do NOT exist and
# errexit is suspended because the probe is invoked from an `if`. Nothing here
# may therefore rely on set -e or on a driver helper: every step is checked,
# and every exit path goes through _php_probe_stop_all. Same rules
# recipes/nginx.sh's probe already follows.
#
# What this proves, and why nothing weaker would do (design §9.3, §9.5): a
# real HTTP GET for a real .php, through a real nginx, answered by php-fpm
# launched with the app's exact production argv — `-F -O -n -y <pool>` — with
# opcache and xdebug loaded from THIS tree's own modules/ through `-d` pairs
# and no php.ini anywhere (D6). Then a full stop and start, and the same GET
# again. `php-fpm -t` would prove the config parses; `--version` would prove
# the binary runs; neither would have caught an extension that cannot load
# from a relocated tree, which is the thing §9.5 asks about — and the audit
# runs this twice, once against $BUILD_PREFIX and once against the unpacked
# tarball at an unrelated path, so "relocated" is not hypothetical.

_PHP_PROBE_FPM_PID=""
_PHP_PROBE_NGINX_PID=""

# Has this pid exited? A child we started stays visible to `kill -0` as a
# zombie until it is waited for, so polling with `kill -0` alone would spin
# for the full timeout on a perfectly clean shutdown. Identical to
# recipes/nginx.sh's _nginx_pid_gone and recipes/mariadb.sh's _mariadb_pid_gone.
_php_pid_gone() {
	local pid="$1" state
	state="$(ps -p "$pid" -o state= 2>/dev/null | tr -d ' ')"
	[ -z "$state" ] && return 0
	case "$state" in Z*) return 0 ;; esac
	return 1
}

# A free TCP port on 127.0.0.1, PROVEN free by a real connection attempt —
# never assumed, and never 80 or 8080. Still check-then-bind, a textbook
# TOCTOU: RANDOM only lowers the odds that two audits running at once pick the
# same candidate. The loser fails loudly (nginx cannot bind), never a false
# pass — a mitigation, not a proof, and not claimed as one. Same as nginx.sh's.
_php_free_port() {
	local base candidate tries=0
	base=$((20000 + (RANDOM % 20000)))
	while [ "$tries" -lt 50 ]; do
		candidate=$((base + tries))
		if ! nc -z -w1 127.0.0.1 "$candidate" 2>/dev/null; then
			printf '%s\n' "$candidate"
			return 0
		fi
		tries=$((tries + 1))
	done
	return 1
}

# check 6's FastCGI client. Not linked into anything, not shipped, and not
# built here — nginx is a finished package of this same pipeline, staged by
# RECIPE_DEPENDS so that a machine which has never built it still runs a real
# check 6 rather than a skipped one.
_php_probe_nginx() { printf '%s/nginx-%s/bin/nginx\n' "$BUILD_ROOT" "$_PHP_PROBE_NGINX_VERSION"; }

# The pool config, mirroring crates/openvhost-conf/templates/php-fpm/
# pool.conf.tera's shape — an [www] pool on a unix socket with clear_env,
# security.limit_extensions and the log_errors/display_errors pair — so a
# probe failure means the binary cannot run the app's config, not merely that
# it cannot run SOME config.
_php_write_pool_conf() {
	local scratch="$1" conf
	conf="$scratch/php-fpm.conf"
	cat >"$conf" <<EOF
[global]
error_log = $scratch/fpm-error.log
daemonize = no

[www]
listen = $scratch/f.sock
pm = ondemand
pm.max_children = 4
catch_workers_output = yes
clear_env = yes
security.limit_extensions = .php
php_admin_flag[log_errors] = On
php_flag[display_errors] = Off
EOF
	printf '%s\n' "$conf"
}

_php_write_nginx_conf() {
	local scratch="$1" port="$2" conf
	conf="$scratch/nginx.conf"
	cat >"$conf" <<EOF
daemon off;
worker_processes 1;
pid "$scratch/run/nginx.pid";
error_log "$scratch/run/nginx-error.log" warn;

events {
    worker_connections 64;
}

http {
    default_type application/octet-stream;
    access_log off;
    client_body_temp_path "$scratch/run/client_body";
    proxy_temp_path "$scratch/run/proxy";
    fastcgi_temp_path "$scratch/run/fastcgi";
    uwsgi_temp_path "$scratch/run/uwsgi";
    scgi_temp_path "$scratch/run/scgi";

    server {
        listen 127.0.0.1:$port;
        server_name _;
        root "$scratch/docroot";

        location / {
            try_files \$uri =404;
        }

        location ~ \.php\$ {
            try_files \$uri =404;
            fastcgi_pass "unix:$scratch/f.sock";
            fastcgi_param SCRIPT_FILENAME \$document_root\$fastcgi_script_name;
            fastcgi_param SCRIPT_NAME \$fastcgi_script_name;
            fastcgi_param REDIRECT_STATUS 200;
            fastcgi_param QUERY_STRING \$query_string;
            fastcgi_param REQUEST_METHOD \$request_method;
            fastcgi_param CONTENT_TYPE \$content_type;
            fastcgi_param CONTENT_LENGTH \$content_length;
            fastcgi_param REQUEST_URI \$request_uri;
            fastcgi_param DOCUMENT_URI \$document_uri;
            fastcgi_param DOCUMENT_ROOT \$document_root;
            fastcgi_param SERVER_PROTOCOL \$server_protocol;
            fastcgi_param GATEWAY_INTERFACE CGI/1.1;
            fastcgi_param SERVER_SOFTWARE nginx;
            fastcgi_param REMOTE_ADDR \$remote_addr;
            fastcgi_param REMOTE_PORT \$remote_port;
            fastcgi_param SERVER_ADDR \$server_addr;
            fastcgi_param SERVER_PORT \$server_port;
            fastcgi_param SERVER_NAME \$server_name;
        }
    }
}
EOF
	printf '%s\n' "$conf"
}

# The page under test. Every interesting fact is asserted INSIDE PHP so the
# expected response body is one constant line: a byte-for-byte cmp then tells
# the truth about all of them at once, and any failure comes back in the body
# where the probe prints it. A page that merely echoed its findings would
# compare equal to itself no matter what it found.
_php_write_probe_page() {
	local scratch="$1" version="$2" tree="$3"
	mkdir -p "$scratch/docroot"
	cat >"$scratch/docroot/probe.php" <<EOF
<?php
\$fail = [];
if (PHP_VERSION !== '$version') {
    \$fail[] = 'version=' . PHP_VERSION;
}
if (PHP_SAPI !== 'fpm-fcgi') {
    \$fail[] = 'sapi=' . PHP_SAPI;
}
// -n means no php.ini was read. If this ever stops being true the '-d' pairs
// stop being the only source of configuration, which is D6's whole premise.
if (php_ini_loaded_file() !== false) {
    \$fail[] = 'ini=' . php_ini_loaded_file();
}
foreach (['Zend OPcache', 'xdebug'] as \$ext) {
    if (!extension_loaded(\$ext)) {
        \$fail[] = 'not-loaded:' . \$ext;
    }
}
// Loaded is not the same as working: opcache registers its extension entry
// before it decides whether it can actually cache anything.
\$status = function_exists('opcache_get_status') ? opcache_get_status(false) : null;
if (!is_array(\$status) || (\$status['opcache_enabled'] ?? false) !== true) {
    \$fail[] = 'opcache-not-enabled';
}
if (!function_exists('xdebug_info')) {
    \$fail[] = 'xdebug-api-missing';
}
// The extensions must have come from THIS tree, not from some other PHP on
// the machine that happened to be on a default search path. The compiled-in
// extension_dir is /lib/php/extensions/... on a sealed system volume and is
// unusable by design (D6); this asserts the '-d' override actually took.
\$dir = ini_get('extension_dir');
if (\$dir !== '$tree/modules') {
    \$fail[] = 'extension_dir=' . \$dir;
}
echo \$fail === [] ? "openvhost-php-probe OK\n" : ('openvhost-php-probe FAIL ' . implode(' ', \$fail) . "\n");
EOF
	printf 'openvhost-php-probe OK\n' >"$scratch/expected.txt"
}

# The production argv (-F -O -n -y <pool>) plus D6's `-d` pairs, and nothing
# else. -F keeps it in the foreground so the pid we hold is the master; -O
# sends startup errors to stderr rather than the not-yet-open error_log.
# extension_dir comes FIRST because the two zend_extension values are bare
# filenames resolved against it.
_php_probe_start_fpm() {
	local tree="$1" scratch="$2" conf="$3" waited=0
	"$tree/bin/php-fpm" \
		-d "extension_dir=$tree/modules" \
		-d "zend_extension=opcache.so" \
		-d "zend_extension=xdebug.so" \
		-F -O -n -y "$conf" >>"$scratch/run/fpm.out" 2>&1 &
	_PHP_PROBE_FPM_PID=$!
	while [ "$waited" -lt 60 ]; do
		if [ -S "$scratch/f.sock" ]; then
			return 0
		fi
		if _php_pid_gone "$_PHP_PROBE_FPM_PID"; then
			printf 'php-fpm exited before it created its listening socket\n'
			return 1
		fi
		sleep 0.5
		waited=$((waited + 1))
	done
	printf 'php-fpm did not create %s within 30s\n' "$scratch/f.sock"
	return 1
}

_php_probe_start_nginx() {
	local scratch="$1" conf="$2" port="$3" waited=0 nginx
	nginx="$(_php_probe_nginx)"
	"$nginx" -e "$scratch/run/nginx-stderr.log" -c "$conf" >>"$scratch/run/nginx.out" 2>&1 &
	_PHP_PROBE_NGINX_PID=$!
	while [ "$waited" -lt 60 ]; do
		if nc -z -w1 127.0.0.1 "$port" 2>/dev/null; then
			return 0
		fi
		if _php_pid_gone "$_PHP_PROBE_NGINX_PID"; then
			printf 'nginx exited before it accepted a connection\n'
			return 1
		fi
		sleep 0.5
		waited=$((waited + 1))
	done
	printf 'nginx did not accept a connection on 127.0.0.1:%s within 30s\n' "$port"
	return 1
}

# SIGTERM first, SIGKILL as the backstop, and `wait` either way: the contract
# says the probe leaves no process running on EITHER path. Returns 1 if
# anything needed the backstop, which the summary then reports rather than
# hiding — a server that has to be killed is exactly what check 6 exists to
# notice.
_php_probe_stop_one() {
	local var="$1" what="$2" pid waited=0 clean=0
	eval "pid=\$$var"
	[ -n "$pid" ] || return 0
	eval "$var=''"
	kill -TERM "$pid" 2>/dev/null || true
	while [ "$waited" -lt 60 ]; do
		if _php_pid_gone "$pid"; then
			clean=1
			break
		fi
		sleep 0.5
		waited=$((waited + 1))
	done
	if [ "$clean" -ne 1 ]; then
		kill -KILL "$pid" 2>/dev/null || true
	fi
	wait "$pid" 2>/dev/null || true
	[ "$clean" -eq 1 ] && return 0
	printf '%s did not stop on SIGTERM within 30s and was killed\n' "$what"
	return 1
}

# nginx down first: it is the one holding connections to php-fpm's socket.
_php_probe_stop_all() {
	local rc=0
	_php_probe_stop_one _PHP_PROBE_NGINX_PID nginx || rc=1
	_php_probe_stop_one _PHP_PROBE_FPM_PID php-fpm || rc=1
	return "$rc"
}

# GET <path> into <dest> and cmp(1) it against <want-file> — a real byte
# comparison of files on disk, not a shell-string compare: command
# substitution strips trailing newlines, which would quietly turn "compare
# the bytes" into "compare the bytes modulo a trailing newline".
_php_get_and_compare() {
	local port="$1" path="$2" want_file="$3" dest="$4" err
	err="$dest.err"
	if ! curl -fsS -m 15 "http://127.0.0.1:$port/$path" -o "$dest" 2>"$err"; then
		printf 'GET /%s failed:\n' "$path"
		cat "$err" 2>/dev/null || true
		return 1
	fi
	if ! cmp -s "$want_file" "$dest"; then
		printf 'GET /%s returned a body that is not what the page promises:\n' "$path"
		head -c 2000 "$dest" 2>/dev/null || true
		printf '\n'
		return 1
	fi
	return 0
}

_php_probe_dump_logs() {
	local scratch="$1" f
	for f in "$scratch/run/fpm.out" "$scratch/fpm-error.log" \
		"$scratch/run/nginx-error.log" "$scratch/run/nginx.out"; do
		[ -s "$f" ] || continue
		printf -- '--- %s ---\n' "${f##*/}"
		tail -n 20 "$f" 2>/dev/null || true
	done
}

_php_probe_round() {
	local tree="$1" scratch="$2" pool="$3" nginx_conf="$4" port="$5" dest="$6"
	if ! _php_probe_start_fpm "$tree" "$scratch" "$pool"; then
		return 1
	fi
	if ! _php_probe_start_nginx "$scratch" "$nginx_conf" "$port"; then
		return 1
	fi
	_php_get_and_compare "$port" "probe.php" "$scratch/expected.txt" "$dest"
}

recipe_serve_probe() {
	local tree="$1" scratch="$2" port pool nginx_conf nginx_bin stop_note

	nginx_bin="$(_php_probe_nginx)"
	if [ ! -x "$nginx_bin" ]; then
		printf 'check 6 needs a real FastCGI client and there is no nginx at %s.\n' "$nginx_bin"
		printf 'Build it first: build/build.sh nginx %s\n' "$_PHP_PROBE_NGINX_VERSION"
		return 1
	fi

	# sun_path is 104 bytes on macOS and this project has been bitten by it
	# three times. audit.sh's scratch is an mktemp under $TMPDIR, which on
	# macOS is a 49-character /var/folders/… path, so `$scratch/f.sock` lands
	# around 86 — comfortable, but not by so much that a longer TMPDIR could
	# not push it over. Checked here so the failure names the cause instead of
	# arriving as php-fpm's "unable to bind listening socket".
	if [ "${#scratch}" -gt 90 ]; then
		printf 'the audit scratch path is %s bytes (%s); adding /f.sock would approach the 104-byte sun_path limit. Set a shorter TMPDIR and re-run.\n' \
			"${#scratch}" "$scratch"
		return 1
	fi

	port="$(_php_free_port)" || {
		printf 'could not find a free TCP port on 127.0.0.1\n'
		return 1
	}
	mkdir -p "$scratch/run" "$scratch/docroot"
	pool="$(_php_write_pool_conf "$scratch")"
	nginx_conf="$(_php_write_nginx_conf "$scratch" "$port")"
	_php_write_probe_page "$scratch" "$RECIPE_VERSION" "$tree"

	if ! _php_probe_round "$tree" "$scratch" "$pool" "$nginx_conf" "$port" "$scratch/got1.txt"; then
		_php_probe_dump_logs "$scratch"
		_php_probe_stop_all >/dev/null 2>&1 || true
		return 1
	fi

	# The restart half. Both processes down, both back up, same GET — a server
	# that only works on a tree nobody has touched yet is not a server.
	if ! _php_probe_stop_all; then
		return 1
	fi
	rm -f -- "$scratch/f.sock"

	if ! _php_probe_round "$tree" "$scratch" "$pool" "$nginx_conf" "$port" "$scratch/got2.txt"; then
		_php_probe_dump_logs "$scratch"
		_php_probe_stop_all >/dev/null 2>&1 || true
		return 1
	fi

	stop_note=""
	_php_probe_stop_all >/dev/null 2>&1 || stop_note=" (note: SIGTERM alone did not stop everything within 30s; needed SIGKILL)"
	# PROBE-SUMMARY: marks the line audit.sh's check 6 takes as its PASS note
	# (build/recipes/README.md) — the first line carrying this prefix, so
	# nothing printed afterward cannot displace it.
	printf 'PROBE-SUMMARY: php-fpm (-F -O -n -y) served a real .php through nginx on 127.0.0.1:%s with opcache and xdebug loaded from %s/modules and no php.ini, body matched byte-for-byte (cmp), both restarted, body matched again%s\n' \
		"$port" "$tree" "$stop_note"
	return 0
}

# ---------------------------------------------------------------- manifest ----
#
# §7: single-builder trust is only acceptable because the inputs are recorded.
# For every other package in this fleet that is one upstream tarball; here it
# is ~37 of them plus a git commit, and a manifest that recorded only php-src
# would describe a fraction of what the artifact is made of.
recipe_manifest_extra() {
	local row name version sha256 filename url commit first=1
	printf '{"static_php_cli": {"tag": "%s", "commit": "%s", "url": "%s", "pins_derived_on": "%s"}, ' \
		"$PHP_PINS_SPC_TAG" "$PHP_PINS_SPC_COMMIT" "$PHP_PINS_SPC_URL" "$PHP_PINS_DERIVED_ON"
	printf '"extensions": {"static": "%s", "shared": "%s", "known_gaps": "%s"}, ' \
		"$_PHP_SPC_STATIC_EXTS" "$_PHP_SPC_SHARED_EXTS" "$PHP_PINS_KNOWN_GAPS"
	printf '"suggested_libs": "%s", ' "$_PHP_SPC_LIBS"
	printf '"network_during_build": "denied (sandbox-exec, deny network*)", '
	printf '"pinned_sources": ['
	for row in "${PHP_PINS_LIBS[@]}"; do
		read -r name version sha256 filename url <<<"$row"
		if [ "$first" -eq 1 ]; then first=0; else printf ', '; fi
		printf '{"name": "%s", "version": "%s", "sha256": "%s", "file": "%s", "url": "%s", "verified": "sha256"}' \
			"$name" "$version" "$sha256" "$filename" "$url"
	done
	for row in "${PHP_PINS_GIT[@]}"; do
		read -r name commit url <<<"$row"
		if [ "$first" -eq 1 ]; then first=0; else printf ', '; fi
		printf '{"name": "%s", "commit": "%s", "url": "%s", "verified": "git-commit"}' \
			"$name" "$commit" "$url"
	done
	printf ']}'
}
