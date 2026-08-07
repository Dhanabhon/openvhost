# SPDX-License-Identifier: GPL-3.0-or-later
# shellcheck shell=bash
# shellcheck disable=SC2034  # the RECIPE_* variables are read by build.sh and
#                              audit.sh, which source this file
#
# MariaDB 11.4 LTS — the package this pipeline exists to prove.
#
# §13: 11.4 LTS only. No 10.x, no 11.7. Every extra major is another tree to
# build, verify and patch, and §14's obligation scales with that count.
#
# The configure flags below are not a starting point to tune; they are the
# configuration measured on 2026-08-02, where four separate build failures each
# turned into one of them (spec §2). Dropping any one of them reproduces a
# failure that has already been paid for:
#
#   1. cmake and bison are resolved by absolute path. Scrubbing PATH first
#      removed cmake itself; and the bison first on PATH is ServBay's, which
#      reports "GNU Bison 3.8.2" and then cannot generate a parser at all
#      because its compiled-in data directory was never packaged.
#   2. CMAKE_IGNORE_PREFIX_PATH, because the host leaked in from two package
#      managers at once — GNUTLS/HOGWEED from /opt/homebrew, GSSAPI and
#      KRB5_CONFIG from /Applications/ServBay.
#   3. WITH_SSL is an overloaded name. The server accepts bundled|system|<path>,
#      and `bundled` — wolfSSL for the server — is what hands the bundled
#      Connector/C to GnuTLS: cmake/mariadb_connector_c.cmake derives
#      CONC_WITH_SSL from the top-level choice and, having seen wolfSSL, sets it
#      to GNUTLS on every non-Windows platform. The connector then does
#      FIND_PACKAGE(GnuTLS REQUIRED) and finds Homebrew's, which is how GNUTLS
#      and HOGWEED got into the reference build. One concrete prefix — ours — is
#      the only value that gives both readers OpenSSL.
#      (An earlier version of this comment claimed the connector "falls into its
#      GnuTLS branch on anything else"; it does not. Its GnuTLS branch is an
#      exact STREQUAL match and its catch-all is FATAL_ERROR "Invalid TLS/SSL
#      option". The flag was right; the reason written next to it was not.)
#   4. `auto` is the enemy. It is how WITH_PCRE found Homebrew's pcre2.

# ---------------------------------------------------------------- provenance --

RECIPE_PINNED_VERSION="11.4.9"

RECIPE_SOURCE_URL="https://archive.mariadb.org/mariadb-$RECIPE_VERSION/source/mariadb-$RECIPE_VERSION.tar.gz"
RECIPE_SOURCE_SHA256="8e481ca29b5a740444d45451c8ea2d93711cf525d6fa5d27bc9512cf8973b075"
RECIPE_SIGNATURE_URL="$RECIPE_SOURCE_URL.asc"

# The MariaDB signing key. Verified on 2026-08-02 by fetching it from three
# hosts that do not share infrastructure with each other or with the download
# host, and comparing primary fingerprints — all three agreed:
#
#   mariadb.org             -> 177F4010FE56CA3336300305F1656F24C74CD1D8
#   supplychain.mariadb.com -> 177F4010FE56CA3336300305F1656F24C74CD1D8
#   keyserver.ubuntu.com    -> 177F4010FE56CA3336300305F1656F24C74CD1D8
#
# The same key signs upstream's sha256sums.txt, and the digest pinned above is
# the one it attests to. This key does not expire; the release was signed on
# 2025-11-05.
RECIPE_SIGNING_KEY_FPR="177F4010FE56CA3336300305F1656F24C74CD1D8"
RECIPE_SIGNING_KEY_EXPIRY="none"
RECIPE_SIGNING_KEY_VERIFIED_ON="2026-08-02"

# The fingerprint above is the trust anchor, so key material may come from any
# host: a substituted key cannot produce it. Upstream first, keyserver second,
# so a mariadb.org outage does not stop a security rebuild.
RECIPE_SIGNING_KEY_URLS=(
	"https://mariadb.org/mariadb_release_signing_key.asc"
	"https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x177F4010FE56CA3336300305F1656F24C74CD1D8&options=mr"
)

# §14's tripwire. From here on a MariaDB CVE is ours to notice: a user's
# `brew upgrade` no longer reaches what we ship, and they have no other route to
# the fix. A stale date here is the only signal that says so.
RECIPE_UPSTREAM_RELEASE_DATE="2025-11-05"
RECIPE_LAST_CHECKED="2026-08-02"

# ------------------------------------------- what `bundled` actually downloads --
#
# -DWITH_PCRE=bundled and -DWITH_LIBFMT=bundled vendor nothing. Each is an
# ExternalProject_Add that fetches an archive over the network DURING the build
# and checks it with URL_MD5 and nothing else (cmake/pcre.cmake, cmake/libfmt.cmake).
# Both are compiled into bin/mariadbd, so the artifact contract cannot see either
# one: check 2 reads link commands, and a static library that was compiled in
# leaves no entry there. MD5 stopped being collision-resistant in 2004, and
# neither archive is signed by the host that serves it.
#
# The answer is not to trust that check harder. Both archives are fetched here
# and verified here — PCRE2 against its GPG signature, fmt against its digest,
# because fmt publishes no signature — and then placed where ExternalProject
# looks before it decides anything, since its download step returns early when
# the file is already present and its hash matches (_mariadb_seed_archive).
# recipe_build then runs with the network taken away (_mariadb_no_network), which
# is what makes "no unverified fetch" a property of the build rather than a claim
# about the two downloads we happened to think of.
#
# §14 now covers these two as well — but note the shape of that obligation. The
# versions are MariaDB's choice, not ours, because cmake insists on its own
# URL_MD5; so the answer to a pcre2 CVE is a MariaDB release that bumps it, and
# not a number edited here.

RECIPE_VENDORED_LAST_CHECKED="2026-08-03"

RECIPE_PCRE2_VERSION="10.45"
RECIPE_PCRE2_URL="https://github.com/PCRE2Project/pcre2/releases/download/pcre2-$RECIPE_PCRE2_VERSION/pcre2-$RECIPE_PCRE2_VERSION.zip"
RECIPE_PCRE2_SHA256="59c8556fd45e68599897cd5d74efad9c4a43f85e981fe7ac3ac5fd7aa70672ac"
RECIPE_PCRE2_SIGNATURE_URL="$RECIPE_PCRE2_URL.sig"
RECIPE_PCRE2_UPSTREAM_RELEASE_DATE="2025-02-05"

# PCRE2 signs every release asset. The signature over pcre2-10.45.zip was made on
# 2025-02-04 by subkey BACF71F10404D5761C09D392021DE40BFB63B406, whose primary is
# the fingerprint pinned below — Nicholas Wilson, PCRE2's maintainer. Verified on
# 2026-08-03 from two hosts that share no infrastructure with github.com, both of
# which served the same primary and the same subkey set:
#
#   keys.openpgp.org      -> A95536204A3BB489715231282A98E77EB6F24CA8
#   keyserver.ubuntu.com  -> A95536204A3BB489715231282A98E77EB6F24CA8
#
# The PRIMARY is pinned, not the subkey that made the signature, which is why
# build.sh's bp_gpg_verify_signature reads the last field of VALIDSIG rather
# than the first. A maintainer may rotate a signing subkey between releases
# without the identity changing; pinning the subkey would turn that into a
# build failure indistinguishable from an attack.
RECIPE_PCRE2_SIGNING_KEY_FPR="A95536204A3BB489715231282A98E77EB6F24CA8"
RECIPE_PCRE2_SIGNING_KEY_URLS=(
	"https://keys.openpgp.org/vks/v1/by-fingerprint/A95536204A3BB489715231282A98E77EB6F24CA8"
	"https://keyserver.ubuntu.com/pks/lookup?op=get&search=0xA95536204A3BB489715231282A98E77EB6F24CA8&options=mr"
)

RECIPE_FMT_VERSION="12.0.0"
RECIPE_FMT_URL="https://github.com/fmtlib/fmt/releases/download/$RECIPE_FMT_VERSION/fmt-$RECIPE_FMT_VERSION.zip"
RECIPE_FMT_SHA256="1c32293203449792bf8e94c7f6699c643887e826f2d66a80869b4f279fb07d25"
RECIPE_FMT_UPSTREAM_RELEASE_DATE="2025-09-17"

# fmt publishes NO signature: no detached .sig, no signed digest list. Said out
# loud rather than left to inference, because "verified" has to mean one thing
# everywhere in this file. What upstream does publish is fmt-12.0.0.intoto.jsonl,
# a SLSA provenance attestation whose subject digest is the sha256 pinned above —
# but checking its Sigstore envelope needs cosign or slsa-verifier, which this
# pipeline does not have and will not grow for one archive, and reading the
# digest out of an unverified attestation served by the same host proves nothing.
#
# So the pin above is the guarantee, with one genuinely independent corroboration:
# MariaDB's GPG-signed source declares the MD5 of these same bytes, and
# _mariadb_seed_archive asserts our copy matches it. Two parties, two algorithms,
# one artifact.
RECIPE_FMT_SIGNATURE="none published upstream; sha256 pin only"

# ------------------------------------------------------------------- inputs --

# D3/§13: static OpenSSL, built by build/recipes/openssl.sh with --stage-only
# and consumed from its staged prefix. Never `bundled` — see failure 3 above.
RECIPE_OPENSSL_VERSION="3.5.7"
RECIPE_DEPENDS=("openssl:$RECIPE_OPENSSL_VERSION")

# cmake and gpg only. bison is deliberately absent: RECIPE_BUILD_TOOLS resolves
# by `command -v`, and on the machine this was written on that finds ServBay's
# broken one. A tool whose failure mode is "on PATH, right version, cannot run"
# has to be pinned by path and then proved, which _mariadb_bison does below.
RECIPE_BUILD_TOOLS=(cmake gpg)

# The driver's default omits /Applications/ServBay/package/common, and
# CMAKE_IGNORE_PREFIX_PATH matches prefixes exactly rather than by ancestry —
# ignoring /Applications/ServBay does not ignore the directory below it, which
# is the one that actually holds their headers and libraries.
RECIPE_IGNORE_PREFIXES=(
	/opt/homebrew
	/usr/local
	/Applications/ServBay/package/common
	/Applications/ServBay
)

# ------------------------------------------------------- the artifact contract --

RECIPE_SERVER_BIN="bin/mariadbd"
# --no-defaults first: contract check 5 runs this binary from two scratch paths,
# and a my.cnf on the builder's machine must not be able to decide whether the
# package passes.
RECIPE_SERVER_VERSION_ARGS=(--no-defaults --version)

# Contract check 7: no absolute path embedded in the tree may have a
# world-writable ancestor. MariaDB's own corpus does not satisfy that literally,
# so the exceptions are declared here — visible in every audit run, next to a
# reason — rather than built into the checker where nobody would read them.
#
# Two subtrees are documentation and fixtures, not things the product resolves:
RECIPE_INERT_PATHS=(
	# 234 MB of MariaDB's own regression suite. Its .test/.sql files name ~500
	# scratch files under /tmp because that is what a test harness does with a
	# scratch file; nothing in the server reads them. Checks 1-3 still cover
	# every Mach-O in here, and check 6 proves the server works without it.
	mariadb-test
	# The sql-bench harness, same shape: Perl scripts with /tmp workspaces.
	sql-bench
	# Manual pages. `man mariadbd` documents MariaDB's historical defaults,
	# including /tmp/mysql.sock; documenting a default is not adopting one.
	man
	# One file rather than a subtree, which is the whole reason a single file is
	# allowed here: bin/mariadb-client-test is upstream's client regression
	# binary, and its proxy-header fixture has the literal string
	# "/tmp/mysql.sock" compiled in as test DATA — it is not that binary's
	# socket default, which -DMYSQL_UNIX_ADDR moved with everything else's.
	# Declining to scan one test binary is narrow; allowing the string
	# /tmp/mysql.sock tree-wide would also have waved it through in mariadbd,
	# which is the file the whole check exists for.
	bin/mariadb-client-test
)
# And these paths survive in files the product does ship. Each was traced to the
# file that carries it before it was written down; none is a path the server
# resolves while serving. The one that WAS — /tmp/mysql.sock, upstream's
# MYSQL_UNIX_ADDR, in 27 files including mariadbd and libmariadb.3.dylib — is
# absent from this list on purpose: it is fixed in recipe_configure instead,
# because a socket a client will actually open is not something to wave through.
RECIPE_ALLOWED_WRITABLE_PATHS=(
	# DBUG's default trace target, in the client tools only. Written to only
	# when --debug is passed with no explicit target, which no OpenVHost code
	# path does. bin/mariadb, bin/mariadb-dump, bin/mariadb-slap,
	# bin/my_print_defaults, bin/mariadb-client-test.
	/tmp/mariadb.trace
	/tmp/mariadb-dump.trace
	/tmp/mariadb-slap.trace
	/tmp/my_print_defaults.trace
	/tmp/mysql_client_test.trace
	# The example my.cnf printed inside bin/mariadbd-multi's own --example
	# output. Documentation that happens to live in a script.
	/tmp/mysql.sock2
	/tmp/mysql.sock3
	/tmp/mysql.sock4
	/tmp/mysql.sock6
	# bin/mariadb-access (a Perl script) writes a debug log here when run with
	# its own --debug. Not on any path OpenVHost invokes.
	/tmp/mysqlaccess.log
	# mktemp template in bin/mariadbd-safe, reached only by the Galera recovery
	# branch — and -DWITH_WSREP=OFF means there is no Galera here to recover.
	/tmp/wsrep_recovery.XXXXXX
	# Worked examples in the built-in HELP corpus (share/fill_help_tables.sql):
	# the manual's LOAD DATA and LOAD_FILE entries use /tmp paths as prose.
	/tmp/loaddata7.dat
	/tmp/picture
	/tmp/world
	/tmp/skr3
	# Worked examples in share/mariadb_sys_schema.sql (and its copy embedded in
	# bin/mariadb-upgrade): the format_path()/ps_trace_thread() docs show
	# /tmp/stack_*.dot output filenames.
	/tmp/stack-
	/tmp/stack-2014-02-16-21
	/tmp/stack_
	/tmp/stack_25.pdf
	/tmp/stack_25.png
)

# Filled in by recipe_configure, reported by recipe_manifest_extra. Which bison
# built the parser is an audit fact, not a detail.
RECIPE_BISON_PATH=""
RECIPE_BISON_VERSION=""

# ------------------------------------------------------------------ helpers --

_mariadb_tarball() { printf '%s/mariadb-%s.tar.gz\n' "$BUILD_DOWNLOADS" "$RECIPE_VERSION"; }
_mariadb_signature() { printf '%s.asc\n' "$(_mariadb_tarball)"; }
_mariadb_openssl_prefix() { bp_dep_prefix openssl "$RECIPE_OPENSSL_VERSION"; }

# The basenames are not free choices: ExternalProject looks for the last path
# component of its own URL, so these must be what upstream's URL ends with.
_mariadb_pcre2_archive() { printf '%s/pcre2-%s.zip\n' "$BUILD_DOWNLOADS" "$RECIPE_PCRE2_VERSION"; }
_mariadb_pcre2_signature() { printf '%s.sig\n' "$(_mariadb_pcre2_archive)"; }
_mariadb_fmt_archive() { printf '%s/fmt-%s.zip\n' "$BUILD_DOWNLOADS" "$RECIPE_FMT_VERSION"; }

# Take the network away from one command. cmake's file(DOWNLOAD) — which is what
# every ExternalProject download step ends up in — is libcurl, and libcurl honours
# these variables for the schemes upstream uses, so a step that decides to fetch
# gets a connection refused instead of an archive. no_proxy is cleared because a
# single entry in it would route around the whole thing.
#
# This is the load-bearing half of the arrangement. Seeding two archives stops
# the two downloads we know about; this is what makes a third one, added by some
# future MariaDB release, fail loudly instead of quietly succeeding.
_mariadb_no_network() {
	http_proxy="http://127.0.0.1:9" https_proxy="http://127.0.0.1:9" \
		all_proxy="http://127.0.0.1:9" ftp_proxy="http://127.0.0.1:9" \
		HTTP_PROXY="http://127.0.0.1:9" HTTPS_PROXY="http://127.0.0.1:9" \
		ALL_PROXY="http://127.0.0.1:9" FTP_PROXY="http://127.0.0.1:9" \
		no_proxy="" NO_PROXY="" \
		"$@"
}

# Put a verified archive where MariaDB's ExternalProject step will look, and prove
# that step is still asking for exactly these bytes.
#
# ExternalProject downloads into <PREFIX>/src and its download step returns early
# when the file is already there and its hash matches. On a MISMATCH it deletes
# the file and fetches. So a stale seed does not fail — it silently becomes the
# network fetch this exists to remove, and that is the failure mode worth
# engineering against, not a wrong digest. The URL and URL_MD5 are therefore read
# back out of the source whose signature we just checked, and compared with what
# we fetched: a MariaDB release that bumps either one stops the build here,
# before anything is compiled, rather than downloading its own copy.
_mariadb_seed_archive() {
	local module="$1" download_dir="$2" archive="$3" want_url="$4"
	local declared_url declared_md5 actual_md5

	[ -f "$module" ] ||
		bp_die "$module is missing; MariaDB no longer bundles this the way the recipe seeds it, and the seed cannot be verified"

	# $1 rather than a regex: URL_MD5 also begins with "URL", and a pattern that
	# confuses the two would silently compare a digest against a URL.
	declared_url="$(awk '$1 == "URL" { gsub(/"/, "", $2); print $2; exit }' "$module")"
	declared_md5="$(awk '$1 == "URL_MD5" { print $2; exit }' "$module")"
	[ -n "$declared_url" ] && [ -n "$declared_md5" ] ||
		bp_die "could not read URL/URL_MD5 out of $module; it no longer has the ExternalProject shape this recipe seeds, so the seed would be ignored"

	[ "$declared_url" = "$want_url" ] ||
		bp_die "$(basename -- "$module") now fetches $declared_url, not $want_url; re-pin it in this recipe with a fresh digest and, where upstream signs, a re-verified signature"

	# /sbin/md5, absolute: this is a verification step, and the one thing this
	# pipeline has already been bitten by is a tool found on PATH (spec §2).
	actual_md5="$(/sbin/md5 -q -- "$archive")"
	[ "$actual_md5" = "$declared_md5" ] ||
		bp_die "$(basename -- "$archive") has md5 $actual_md5 but $(basename -- "$module") expects $declared_md5; cmake would delete our verified copy and fetch its own"

	mkdir -p "$download_dir"
	cp -- "$archive" "$download_dir/$(basename -- "$archive")"
	bp_log "seeded $(basename -- "$archive") into $download_dir (md5 agrees with $(basename -- "$module"), so no download step runs)"
}

# True only if this bison can actually generate a parser. Version is not a test:
# ServBay's answers "3.8.2" to --version and then writes nothing at all, which is
# how it broke the reference build while looking correct.
_mariadb_bison_works() {
	local bison="$1" dir="$BUILD_WORK/bison-probe" major
	[ -x "$bison" ] || return 1
	major="$("$bison" --version 2>/dev/null |
		awk 'NR == 1 { split($NF, v, "."); print v[1]; exit }')"
	case "$major" in '' | *[!0-9]*) return 1 ;; esac
	[ "$major" -ge 3 ] || return 1
	bp_rm_tree "$dir"
	mkdir -p "$dir"
	printf '%%%%\nstart: ;\n' >"$dir/probe.y"
	(cd "$dir" && "$bison" -o probe.c probe.y) >/dev/null 2>&1 || return 1
	[ -s "$dir/probe.c" ]
}

# Absolute path of a bison that works. Candidates are absolute on purpose:
# `command -v bison` is the thing that went wrong.
_mariadb_bison() {
	local candidate
	for candidate in \
		${OPENVHOST_BISON:+"$OPENVHOST_BISON"} \
		/opt/homebrew/opt/bison/bin/bison \
		/usr/local/opt/bison/bin/bison \
		/opt/homebrew/bin/bison \
		/usr/local/bin/bison; do
		if _mariadb_bison_works "$candidate"; then
			printf '%s\n' "$candidate"
			return 0
		fi
	done
	bp_die "no working bison >= 3.0 found (macOS ships 2.3, which is too old). Install one — \`brew install bison\` — or point OPENVHOST_BISON at it. Note that a bison can be the right version, be first on PATH, and still be unable to generate a parser."
}

# ------------------------------------------------------------------- stages --

recipe_fetch() {
	[ "$RECIPE_VERSION" = "$RECIPE_PINNED_VERSION" ] ||
		bp_die "this recipe pins MariaDB $RECIPE_PINNED_VERSION; $RECIPE_VERSION would need a new sha256 and a re-verified signature. §13 is 11.4 LTS only — a new major is a decision with a cost, not a version argument."
	# §12: no signature-checked x86_64 pin exists and this slice does not add one.
	[ "$BUILD_ARCH" = "arm64" ] ||
		bp_die "this pipeline is Apple Silicon only (spec §12); got $BUILD_ARCH"

	bp_download "$RECIPE_SOURCE_URL" "$(_mariadb_tarball)"
	bp_download "$RECIPE_SIGNATURE_URL" "$(_mariadb_signature)"
	# The two archives -DWITH_PCRE=bundled and -DWITH_LIBFMT=bundled would
	# otherwise fetch mid-build behind an MD5. Fetched here so that they pass
	# through the same verification as everything else.
	bp_download "$RECIPE_PCRE2_URL" "$(_mariadb_pcre2_archive)"
	bp_download "$RECIPE_PCRE2_SIGNATURE_URL" "$(_mariadb_pcre2_signature)"
	bp_download "$RECIPE_FMT_URL" "$(_mariadb_fmt_archive)"

	bp_gpg_init_home
	# One keyring for both. Each verification names the fingerprint it demands, so
	# a key in here can only vouch for the release it actually signed.
	bp_gpg_import_key "$RECIPE_SIGNING_KEY_FPR" mariadb \
		${RECIPE_SIGNING_KEY_URLS[@]+"${RECIPE_SIGNING_KEY_URLS[@]}"}
	bp_gpg_import_key "$RECIPE_PCRE2_SIGNING_KEY_FPR" pcre2 \
		${RECIPE_PCRE2_SIGNING_KEY_URLS[@]+"${RECIPE_PCRE2_SIGNING_KEY_URLS[@]}"}
}

recipe_verify_source() {
	# The signature says who produced these bytes; the pinned digest says they
	# are the same bytes we reviewed. Both, in that order.
	bp_gpg_verify_signature "$(_mariadb_tarball)" "$(_mariadb_signature)" \
		"$RECIPE_SIGNING_KEY_FPR"
	bp_verify_sha256 "$(_mariadb_tarball)" "$RECIPE_SOURCE_SHA256"

	bp_gpg_verify_signature "$(_mariadb_pcre2_archive)" "$(_mariadb_pcre2_signature)" \
		"$RECIPE_PCRE2_SIGNING_KEY_FPR"
	bp_verify_sha256 "$(_mariadb_pcre2_archive)" "$RECIPE_PCRE2_SHA256"

	# Logged, not buried in a comment: a reader of the build output should not have
	# to infer which inputs were signature-checked and which were not.
	bp_log "fmt $RECIPE_FMT_VERSION signature: $RECIPE_FMT_SIGNATURE"
	bp_verify_sha256 "$(_mariadb_fmt_archive)" "$RECIPE_FMT_SHA256"
}

recipe_extract() {
	tar -xzf "$(_mariadb_tarball)" -C "$BUILD_SRC" --strip-components 1
}

recipe_configure() {
	local ssl_prefix bison lib
	ssl_prefix="$(_mariadb_openssl_prefix)"

	# build.sh skips a dependency whose prefix directory already exists, so a
	# half-built or hand-edited OpenSSL tree would be reused in silence. If the
	# static libraries are not there, cmake's FindOpenSSL would go looking
	# elsewhere and the leak would only surface at the audit, twenty minutes
	# later — so it is caught here, before anything is compiled.
	for lib in libcrypto.a libssl.a; do
		[ -f "$ssl_prefix/lib/$lib" ] ||
			bp_die "static OpenSSL $RECIPE_OPENSSL_VERSION is not staged at $ssl_prefix (missing lib/$lib); remove that directory and re-run so the dependency is rebuilt"
	done
	[ -z "$(find "$ssl_prefix" -name '*.dylib' -print | sed -n 1p)" ] ||
		bp_die "the staged OpenSSL at $ssl_prefix contains a dylib; D3 requires static, and a dylib here would become a shipped dependency"

	# Before cmake runs, so that a pin that has drifted stops the build while
	# nothing has been compiled yet. The archives only have to be in place before
	# the BUILD step reaches ExternalProject, but the assertions inside are worth
	# more the earlier they fire.
	_mariadb_seed_archive "$BUILD_SRC/cmake/pcre.cmake" "$BUILD_OBJ/extra/pcre2/src" \
		"$(_mariadb_pcre2_archive)" "$RECIPE_PCRE2_URL"
	_mariadb_seed_archive "$BUILD_SRC/cmake/libfmt.cmake" "$BUILD_OBJ/extra/libfmt/src" \
		"$(_mariadb_fmt_archive)" "$RECIPE_FMT_URL"

	bison="$(_mariadb_bison)"
	RECIPE_BISON_PATH="$bison"
	RECIPE_BISON_VERSION="$("$bison" --version 2>/dev/null |
		awk 'NR == 1 { print }' | tr -cd 'A-Za-z0-9 ().-')"
	bp_log "bison $RECIPE_BISON_VERSION at $bison (generated a parser, not just a version string)"

	local args=(
		"-DCMAKE_BUILD_TYPE=RelWithDebInfo"
		"-DCMAKE_INSTALL_PREFIX=$BUILD_PREFIX"
		# STANDALONE is what makes the tree self-contained; the rpath trio is what
		# makes it work from a path it was never built for. CMAKE_INSTALL_RPATH is
		# applied at BUILD time rather than at install time
		# (CMAKE_BUILD_WITH_INSTALL_RPATH), so no binary ever carries the build
		# tree's absolute rpath, not even briefly.
		"-DINSTALL_LAYOUT=STANDALONE"
		# Upstream's default MYSQL_UNIX_ADDR is /tmp/mysql.sock, and it is
		# compiled into mariadbd, every client, libmariadb.3.dylib, mysql_config
		# and mariadb.pc — 27 shipped files. /tmp is mode 1777, so anything on
		# the machine can bind that name first and collect the credentials of
		# every client that connects to "localhost" without an explicit
		# --socket. Contract check 7 rejects it; this is the fix, not an
		# allowance. The value lands inside the prefix, which check 7 then
		# proves is un-plantable.
		"-DMYSQL_UNIX_ADDR=$BUILD_PREFIX/run/mariadb.sock"
		"-DCMAKE_INSTALL_RPATH=@loader_path/../lib"
		"-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON"
		"-DCMAKE_MACOSX_RPATH=ON"
		"-DBISON_EXECUTABLE=$bison"
		"-DCMAKE_IGNORE_PREFIX_PATH=$(bp_ignore_prefix_path)"
		# One concrete prefix. Never `bundled`, never `system`, never `auto`.
		"-DWITH_SSL=$ssl_prefix"
		# `bundled` here means "download it during the build and check the MD5",
		# not "vendored in the tarball". Both archives were fetched, signature- or
		# digest-verified, and seeded above; the value stays `bundled` because the
		# alternatives are worse — see the provenance block.
		"-DWITH_PCRE=bundled"
		"-DWITH_LIBFMT=bundled"
		"-DWITH_ZLIB=bundled"
		"-DPLUGIN_AUTH_GSSAPI=NO"
		"-DWITH_UNIT_TESTS=OFF"
		"-DWITH_WSREP=OFF"
		# The heavy engines. Disabled deliberately: each pulls host libraries of
		# its own, and the ~3 min compile / 3.4 GB build tree in §11 was measured
		# with exactly this set off. Re-enabling one moves those numbers.
		"-DPLUGIN_ROCKSDB=NO"
		"-DPLUGIN_MROONGA=NO"
		"-DPLUGIN_SPIDER=NO"
		"-DPLUGIN_CONNECT=NO"
		"-DPLUGIN_OQGRAPH=NO"
		"-DPLUGIN_SPHINX=NO"
		"-DPLUGIN_COLUMNSTORE=NO"
		"-DPLUGIN_S3=NO"
	)
	bp_record_flags "${args[@]}"
	(cd "$BUILD_OBJ" && _mariadb_no_network "$(bp_tool cmake)" "$BUILD_SRC" "${args[@]}")
}

recipe_build() {
	# This is where ExternalProject's download steps run, and where the network
	# block earns its place: with pcre2 and fmt already seeded, nothing here has
	# any business reaching a remote host, so anything that tries is a supply-chain
	# input nobody reviewed and the build should stop rather than acquire it.
	_mariadb_no_network "$(bp_tool cmake)" --build "$BUILD_OBJ" --parallel "$BUILD_JOBS"
}

recipe_install() {
	# Straight into $BUILD_PREFIX (D8). Roughly fifty files in a finished tree
	# embed the install prefix; a DESTDIR staging directory that is moved
	# afterwards puts the staging path into every one of them, which is exactly
	# the defect contract check 4 exists to reject.
	_mariadb_no_network "$(bp_tool cmake)" --install "$BUILD_OBJ"
}

# ----------------------------------------------------------------- optional --

# recipe_normalize rewrites NO Mach-O. Static OpenSSL is why: with no bundled
# dylib there is nothing to rewrite, and D4's rule that signing must follow the
# last Mach-O edit stays trivially satisfied — this function touches one text
# file. MariaDB's own libmariadb.3.dylib carries LC_ID_DYLIB =
# @rpath/libmariadb.3.dylib with LC_RPATH = @loader_path/../lib, which is the
# idiomatic self-contained layout and is what the contract admits.
recipe_normalize() {
	# The bundled Connector/C generates mariadb.pc from its own socket default
	# rather than from MYSQL_UNIX_ADDR, so this one file still advertised
	# /tmp/mysql.sock after the flag had moved every binary — including
	# libmariadb.3.dylib itself — off it. It is metadata, but metadata that
	# anyone compiling a client against this tree would compile in, and contract
	# check 7 found it. Rewritten to agree with the library it describes.
	local pc="$BUILD_PREFIX/lib/pkgconfig/mariadb.pc" want="$BUILD_PREFIX/run/mariadb.sock"
	[ -f "$pc" ] || bp_die "lib/pkgconfig/mariadb.pc is missing; the install layout changed"
	LC_ALL=C sed -e "s|^socket=.*|socket=$want|" "$pc" >"$pc.new"
	# Verified, not assumed: a template that stops emitting `socket=` must fail
	# the build rather than quietly leave check 7 to catch it on some later day.
	grep -qxF "socket=$want" "$pc.new" ||
		bp_die "could not rewrite the socket default in mariadb.pc"
	mv -- "$pc.new" "$pc"
	bp_log "mariadb.pc socket default set to $want"
}

# --------------------------------------------------- contract check 6 (serve) --

# Everything below runs inside audit.sh, where errexit is suspended because the
# probe is invoked from an `if`. Nothing may therefore rely on set -e: every step
# is checked, and every exit path goes through _mariadb_probe_stop.

_MARIADB_PROBE_PID=""

# Has this pid exited? A child we started stays visible to `kill -0` as a zombie
# until it is waited for, so polling with `kill -0` alone would spin for the full
# timeout on a perfectly clean shutdown.
_mariadb_pid_gone() {
	local pid="$1" state
	state="$(ps -p "$pid" -o state= 2>/dev/null | tr -d ' ')"
	[ -z "$state" ] && return 0
	case "$state" in Z*) return 0 ;; esac
	return 1
}

_mariadb_probe_start() {
	local tree="$1" scratch="$2" waited=0
	"$tree/bin/mariadbd" --no-defaults \
		--basedir="$tree" \
		--datadir="$scratch/data" \
		--socket="$scratch/mariadb.sock" \
		--pid-file="$scratch/mariadbd.pid" \
		--log-error="$scratch/mariadbd.err" \
		--skip-networking \
		--skip-name-resolve \
		>>"$scratch/mariadbd.out" 2>&1 &
	_MARIADB_PROBE_PID=$!
	# 120 s. A cold InnoDB bootstrap on a busy machine is slow; a server that is
	# never going to come up is detected by the liveness check, not by the clock.
	while [ "$waited" -lt 240 ]; do
		if [ -S "$scratch/mariadb.sock" ] &&
			"$tree/bin/mariadb-admin" --no-defaults --socket="$scratch/mariadb.sock" \
				--user=root ping >/dev/null 2>&1; then
			return 0
		fi
		if _mariadb_pid_gone "$_MARIADB_PROBE_PID"; then
			printf 'mariadbd exited before it accepted a connection\n'
			return 1
		fi
		sleep 0.5
		waited=$((waited + 1))
	done
	printf 'mariadbd did not accept a connection within 120s\n'
	return 1
}

_mariadb_probe_stop() {
	local waited=0 pid="$_MARIADB_PROBE_PID"
	[ -n "$pid" ] || return 0
	_MARIADB_PROBE_PID=""
	kill -TERM "$pid" 2>/dev/null || true
	while [ "$waited" -lt 240 ]; do
		if _mariadb_pid_gone "$pid"; then
			wait "$pid" 2>/dev/null || true
			return 0
		fi
		sleep 0.5
		waited=$((waited + 1))
	done
	# The contract says the probe leaves no process running, on either path.
	kill -KILL "$pid" 2>/dev/null || true
	wait "$pid" 2>/dev/null || true
	printf 'mariadbd did not stop on SIGTERM within 120s and was killed\n'
	return 1
}

_mariadb_sql() {
	local tree="$1" scratch="$2" sql="$3"
	"$tree/bin/mariadb" --no-defaults --socket="$scratch/mariadb.sock" \
		--user=root --batch --skip-column-names --execute="$sql"
}

recipe_serve_probe() {
	local tree="$1" scratch="$2" note engine

	mkdir -p "$scratch/data"

	if ! "$tree/scripts/mariadb-install-db" --no-defaults \
		--basedir="$tree" --datadir="$scratch/data" \
		--skip-test-db --auth-root-authentication-method=normal \
		>"$scratch/install-db.log" 2>&1; then
		printf 'mariadb-install-db failed:\n'
		tail -n 20 "$scratch/install-db.log"
		return 1
	fi

	if ! _mariadb_probe_start "$tree" "$scratch"; then
		tail -n 20 "$scratch/mariadbd.err" 2>/dev/null || true
		_mariadb_probe_stop >/dev/null 2>&1 || true
		return 1
	fi

	# ENGINE=InnoDB is not decoration. If InnoDB fails to initialise MariaDB
	# falls back to the default engine with a warning, and a MyISAM table would
	# then "survive a restart" while proving nothing about the storage engine
	# every OpenVHost site will actually use — so the engine is read back too.
	if ! _mariadb_sql "$tree" "$scratch" "
		CREATE DATABASE openvhost_probe;
		CREATE TABLE openvhost_probe.relocation (
			id INT PRIMARY KEY, note VARCHAR(64)) ENGINE=InnoDB;
		INSERT INTO openvhost_probe.relocation VALUES (1, 'survives-a-restart');
	" >"$scratch/create.log" 2>&1; then
		printf 'create/insert failed:\n'
		tail -n 20 "$scratch/create.log"
		_mariadb_probe_stop >/dev/null 2>&1 || true
		return 1
	fi

	if ! _mariadb_probe_stop; then
		return 1
	fi
	if ! _mariadb_probe_start "$tree" "$scratch"; then
		tail -n 20 "$scratch/mariadbd.err" 2>/dev/null || true
		_mariadb_probe_stop >/dev/null 2>&1 || true
		return 1
	fi

	note="$(_mariadb_sql "$tree" "$scratch" \
		"SELECT note FROM openvhost_probe.relocation WHERE id = 1;" 2>>"$scratch/read.log" || true)"
	engine="$(_mariadb_sql "$tree" "$scratch" \
		"SELECT engine FROM information_schema.tables
		 WHERE table_schema = 'openvhost_probe' AND table_name = 'relocation';" \
		2>>"$scratch/read.log" || true)"

	_mariadb_probe_stop >/dev/null 2>&1 || true

	if [ "$note" != "survives-a-restart" ]; then
		printf 'the row did not come back after a restart: got %s\n' "${note:-<nothing>}"
		tail -n 20 "$scratch/read.log" 2>/dev/null || true
		return 1
	fi
	if [ "$engine" != "InnoDB" ]; then
		printf 'the table is on %s, not InnoDB\n' "${engine:-<unknown>}"
		return 1
	fi
	printf 'created on InnoDB, inserted, stopped, restarted, read back: %s\n' "$note"
	return 0
}

# ---------------------------------------------------------------- manifest ----

recipe_manifest_extra() {
	# Which OpenSSL, how it is linked, which bison built the parser, and what went
	# into the two libraries that are compiled in and therefore invisible to every
	# contract check. All of it is audit fact, not detail.
	printf '{"openssl": {"version": "%s", "linkage": "static"}, "bison": {"path": "%s", "version": "%s"}, "vendored_last_checked": "%s", "vendored": %s, "vendored_on_disk": %s}' \
		"$RECIPE_OPENSSL_VERSION" \
		"$(printf '%s' "$RECIPE_BISON_PATH" | tr -cd 'A-Za-z0-9@_/.+-')" \
		"$RECIPE_BISON_VERSION" \
		"$RECIPE_VENDORED_LAST_CHECKED" \
		"$(_mariadb_vendored)" \
		"$(_mariadb_vendored_on_disk)"
}

# What we pinned, and how far the verification of each actually goes. "verified"
# is spelled out per entry precisely because it differs: PCRE2 publishes a
# detached signature and fmt publishes none, and a manifest that flattened both
# to "verified" would be worse than one that said nothing.
_mariadb_vendored() {
	printf '['
	printf '{"name": "pcre2", "version": "%s", "url": "%s", "sha256": "%s", "release_date": "%s", "verified": "gpg+sha256", "signing_key_fingerprint": "%s"}, ' \
		"$RECIPE_PCRE2_VERSION" "$RECIPE_PCRE2_URL" "$RECIPE_PCRE2_SHA256" \
		"$RECIPE_PCRE2_UPSTREAM_RELEASE_DATE" "$RECIPE_PCRE2_SIGNING_KEY_FPR"
	printf '{"name": "fmt", "version": "%s", "url": "%s", "sha256": "%s", "release_date": "%s", "verified": "sha256", "signature": "%s"}' \
		"$RECIPE_FMT_VERSION" "$RECIPE_FMT_URL" "$RECIPE_FMT_SHA256" \
		"$RECIPE_FMT_UPSTREAM_RELEASE_DATE" "$RECIPE_FMT_SIGNATURE"
	printf ']'
}

# And what was actually on disk when the build read it. The block above records
# intent; this records the bytes, which is the only one of the two that a later
# auditor can check against the artifact. They should agree — the seeding is what
# makes them agree — and the point of printing both is that a disagreement is
# visible rather than assumed away.
_mariadb_vendored_on_disk() {
	local first=1 archive name digest
	printf '['
	while IFS= read -r archive; do
		[ -n "$archive" ] || continue
		[ -f "$archive" ] || continue
		name="$(basename -- "$archive" | tr -cd 'A-Za-z0-9._+-')"
		digest="$(shasum -a 256 -- "$archive" | cut -d' ' -f1)"
		if [ "$first" -eq 1 ]; then first=0; else printf ', '; fi
		printf '{"file": "%s", "sha256": "%s"}' "$name" "$digest"
	done < <(find "$BUILD_OBJ/extra" -maxdepth 3 -type f \
		\( -name '*.zip' -o -name '*.tar.gz' \) -print 2>/dev/null | sort)
	printf ']'
}
