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
#   3. WITH_SSL is an overloaded name: the server accepts bundled|system|<path>,
#      the bundled Connector/C accepts ON|OPENSSL|GNUTLS and silently falls into
#      its GnuTLS branch on anything else (libmariadb/CMakeLists.txt:346). One
#      concrete prefix — ours — is the only value that satisfies both readers.
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

# Filled in by recipe_configure, reported by recipe_manifest_extra. Which bison
# built the parser is an audit fact, not a detail.
RECIPE_BISON_PATH=""
RECIPE_BISON_VERSION=""

# ------------------------------------------------------------------ helpers --

_mariadb_gnupg_home() { printf '%s/gnupg\n' "$BUILD_WORK"; }
_mariadb_tarball() { printf '%s/mariadb-%s.tar.gz\n' "$BUILD_DOWNLOADS" "$RECIPE_VERSION"; }
_mariadb_signature() { printf '%s.asc\n' "$(_mariadb_tarball)"; }
_mariadb_openssl_prefix() { bp_dep_prefix openssl "$RECIPE_OPENSSL_VERSION"; }

_mariadb_gpg() {
	"$(bp_tool gpg)" --batch --no-tty --quiet --homedir "$(_mariadb_gnupg_home)" "$@"
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

	local index=0 url dest imported=0 primary
	bp_rm_tree "$(_mariadb_gnupg_home)"
	mkdir -p "$(_mariadb_gnupg_home)"
	chmod 700 "$(_mariadb_gnupg_home)"
	for url in ${RECIPE_SIGNING_KEY_URLS[@]+"${RECIPE_SIGNING_KEY_URLS[@]}"}; do
		index=$((index + 1))
		dest="$BUILD_DOWNLOADS/signing-key-$index.asc"
		# Never reused from a previous run: key material is the one input whose
		# freshness matters, because expiry and revocation only travel with it.
		rm -f -- "$dest"
		if ! bp_download "$url" "$dest" >/dev/null 2>&1; then
			bp_log "signing key not available from $url"
			continue
		fi
		# Import only if a PRIMARY key in the file carries the pinned fingerprint.
		# Verification below insists on VALIDSIG for that same fingerprint, so an
		# extra key riding along could not vouch for anything — but there is no
		# reason to let one into the keyring either.
		primary="$(_mariadb_gpg --show-keys --with-colons "$dest" 2>/dev/null |
			awk -F: '$1 == "pub" { want = 1; next } $1 == "fpr" && want { print $10; want = 0 }' |
			grep -Fx "$RECIPE_SIGNING_KEY_FPR" || true)"
		if [ -z "$primary" ]; then
			bp_log "ignoring key from $url: no primary key with fingerprint $RECIPE_SIGNING_KEY_FPR"
			continue
		fi
		_mariadb_gpg --import "$dest" >/dev/null 2>&1 || continue
		imported=$((imported + 1))
		bp_log "imported signing key $RECIPE_SIGNING_KEY_FPR from $url"
	done
	[ "$imported" -gt 0 ] ||
		bp_die "no host served a key with fingerprint $RECIPE_SIGNING_KEY_FPR; cannot verify provenance"
}

recipe_verify_source() {
	local status="$BUILD_WORK/gpg-status.txt" errors="$BUILD_WORK/gpg-stderr.txt" bad

	# The signature says who produced these bytes; the pinned digest says they
	# are the same bytes we reviewed. Both, in that order.
	#
	# gpg --verify exits 0 on an EXPIRED signing key — measured on 2026-08-02
	# against OpenSSL's, whose keyserver copy had lapsed — so the exit status
	# proves nothing and the machine-readable status is read instead.
	_mariadb_gpg --status-fd 1 --verify "$(_mariadb_signature)" "$(_mariadb_tarball)" \
		>"$status" 2>"$errors" || true

	grep -q "^\[GNUPG:\] VALIDSIG $RECIPE_SIGNING_KEY_FPR " "$status" ||
		bp_die "no valid signature by $RECIPE_SIGNING_KEY_FPR over $(basename -- "$(_mariadb_tarball)"); gpg said: $(tr '\n' ' ' <"$errors")"
	for bad in EXPKEYSIG REVKEYSIG BADSIG ERRSIG EXPSIG; do
		# An `if`, not `grep ... && bp_die`: under set -e the AND-list's failure
		# becomes the loop's exit status, and a loop that "fails" because nothing
		# was wrong would abort the build on the happy path.
		if grep -q "^\[GNUPG:\] $bad " "$status"; then
			bp_die "signature over $(basename -- "$(_mariadb_tarball)") is $bad; refusing to build from it"
		fi
	done
	bp_log "GPG: good signature by $RECIPE_SIGNING_KEY_FPR"

	bp_verify_sha256 "$(_mariadb_tarball)" "$RECIPE_SOURCE_SHA256"
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
		"-DCMAKE_INSTALL_RPATH=@loader_path/../lib"
		"-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON"
		"-DCMAKE_MACOSX_RPATH=ON"
		"-DBISON_EXECUTABLE=$bison"
		"-DCMAKE_IGNORE_PREFIX_PATH=$(bp_ignore_prefix_path)"
		# One concrete prefix. Never `bundled`, never `system`, never `auto`.
		"-DWITH_SSL=$ssl_prefix"
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
	(cd "$BUILD_OBJ" && "$(bp_tool cmake)" "$BUILD_SRC" "${args[@]}")
}

recipe_build() {
	"$(bp_tool cmake)" --build "$BUILD_OBJ" --parallel "$BUILD_JOBS"
}

recipe_install() {
	# Straight into $BUILD_PREFIX (D8). Roughly fifty files in a finished tree
	# embed the install prefix; a DESTDIR staging directory that is moved
	# afterwards puts the staging path into every one of them, which is exactly
	# the defect contract check 4 exists to reject.
	"$(bp_tool cmake)" --install "$BUILD_OBJ"
}

# ----------------------------------------------------------------- optional --

# No recipe_normalize. Static OpenSSL is why: with no bundled dylib there is
# nothing to rewrite, and D4's rule that signing must follow the last Mach-O
# edit becomes trivially satisfied. MariaDB's own libmariadb.3.dylib carries
# LC_ID_DYLIB = @rpath/libmariadb.3.dylib with LC_RPATH = @loader_path/../lib,
# which is the idiomatic self-contained layout and is what the contract admits.

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
	# Which OpenSSL, how it is linked, and which bison built the parser. All
	# three are audit facts: the first two are what D3 decided, and the third is
	# the tool whose failure looked like success.
	printf '{"openssl": {"version": "%s", "linkage": "static"}, "bison": {"path": "%s", "version": "%s"}, "bundled_downloads": %s}' \
		"$RECIPE_OPENSSL_VERSION" \
		"$(printf '%s' "$RECIPE_BISON_PATH" | tr -cd 'A-Za-z0-9@_/.+-')" \
		"$RECIPE_BISON_VERSION" \
		"$(_mariadb_bundled_downloads)"
}

# WITH_PCRE=bundled and WITH_LIBFMT=bundled do not mean "vendored in the
# tarball" — MariaDB's cmake fetches pcre2 and libfmt over the network at build
# time and checks them with URL_MD5 (cmake/pcre.cmake, cmake/libfmt.cmake).
# Both are compiled into bin/mariadbd, neither is signed, and MD5 is not a
# collision-resistant check. That is upstream's decision, not ours, and no
# linker flag or contract check can see it — but §7 exists so the inputs are at
# least auditable, and an input nobody recorded is an input nobody can audit.
# So the digest of what actually arrived goes in the manifest.
_mariadb_bundled_downloads() {
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
