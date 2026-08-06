# SPDX-License-Identifier: GPL-3.0-or-later
# shellcheck shell=bash
# shellcheck disable=SC2034  # the RECIPE_* variables are read by build.sh and
#                              audit.sh, which source this file
#
# nginx 1.30 stable — off-Homebrew slice 4A. The cheapest build target this
# pipeline has: one static binary, no plugins, no message catalogues.
#
# Facts that became flags, each paid for once so they do not have to be
# rediscovered:
#
#   1. nginx has no system PCRE2 headers to build against even though macOS
#      ships the RUNTIME library. `/usr/lib/libpcre2-8.dylib` (and the SDK's
#      matching `libpcre2-8.tbd` stub) are present on every Mac, but
#      `pcre2.h` is not shipped anywhere under the Xcode/CLT SDK. So this
#      recipe fetches and GPG-verifies upstream PCRE2 source for its header
#      ONLY — never compiled, never staged, never shipped — and links the
#      binary against Apple's own system library. That is a materially
#      smaller obligation than MariaDB's vendored PCRE2 (§14 there is a
#      CVE-patching promise, because that source is compiled into
#      mariadbd; here the header only has to keep describing an API PCRE2
#      has never broken across a 10.x release, and the actual code Apple
#      ships is Apple's own patching responsibility). Verified independently
#      of `recipes/mariadb.sh`'s pin even though it lands on the same
#      version — see the provenance block below.
#   2. zlib needs nothing at all: the SDK ships both `zlib.h` and a matching
#      `.tbd` stub for `/usr/lib/libz.1.dylib`, so nginx's own default
#      "system zlib" probe (`-lz`, no extra flags) already succeeds. No
#      RECIPE_DEPENDS, no fetch, nothing to verify — there is no third party
#      here for a signature to be over.
#   3. `--with-pcre` (bare, no `=`) is not decoration. Without it, a
#      configure that already has a working PCRE2 header+lib available will
#      still silently build WITHOUT regex support the instant
#      `--without-http_rewrite_module` is also passed (measured: dropping
#      just that one module made an otherwise-identical configure print
#      "PCRE library is not used" and link clean). This app's own generated
#      configs use regex in `location ~` and in `map`'s capture groups
#      (main.conf.tera), so "builds fine" is not the bar — "loads our
#      config" is, and only `--with-pcre` makes a missing/broken PCRE2 a
#      configure-time failure instead of a 3 a.m. `nginx -t` failure on a
#      user's machine.
#   4. `--sbin-path=$BUILD_PREFIX/bin/nginx`, not `sbin/`, is D1: every
#      discovery path in this app already speaks `bin/`, and a package tree
#      where one member is shaped differently is a trap for the code that
#      walks it. share/ has no use here at all — see RECIPE_REQUIRED_LAYOUT
#      below and the driver fix in build/audit.sh.
#
# Static OpenSSL and system PCRE2/zlib together mean this binary's ENTIRE
# dynamic dependency list is /usr/lib/* and /System/* — nothing this recipe
# stages or ships needs relocating at runtime, which is a stronger property
# than MariaDB's (whose libmariadb.3.dylib is a real, if @rpath-relative,
# shipped dylib).

# ---------------------------------------------------------------- provenance --

RECIPE_PINNED_VERSION="1.30.4"

RECIPE_SOURCE_URL="https://nginx.org/download/nginx-$RECIPE_VERSION.tar.gz"
RECIPE_SOURCE_SHA256="4261dc90e9e47c1c4041276e9aaa3d48ebe2e664f728e14fa95ae6c67d57a08b"
RECIPE_SIGNATURE_URL="$RECIPE_SOURCE_URL.asc"

# Verified 2026-08-06 against https://nginx.org/en/download.html: "Stable
# version" is 1.30.4 (1.28.3 — this design doc's own arithmetic — is now
# under "Legacy versions"; the doc said to verify rather than trust it, and
# this is why). 1.30.4 is itself a security release: CVE-2026-42533 (heap
# overflow via `map` + regex capture — directly relevant, this app's own
# generated config uses exactly that combination),  CVE-2026-60005 (uninit
# memory via unnamed regex captures) and CVE-2026-56434 (use-after-free in
# ngx_http_ssi_filter_module, not compiled into this build at all — see the
# module list in recipe_configure). Released 2026-07-15 per both
# nginx.org/en/CHANGES-1.30 and the tarball's Last-Modified header, which
# agree with the signature timestamp below to the minute once timezones are
# accounted for.
#
# The signing key. nginx.org/en/pgp_keys.html lists four maintainers' keys
# plus a separate "nginx public key (used for signing packages and
# repositories)" — release TARBALLS are signed by whichever maintainer cut
# that release, not the repo key, so the fingerprint was read out of the
# .asc itself (`gpg --list-packets`: issuer fpr
# 43387825DDB1BB97EC36BA5D007C8D7C15D87369, made directly by that primary
# key, not a subkey) and then INDEPENDENTLY confirmed — not just trusted —
# from three hosts that share no infrastructure with each other or with the
# download host, all of which agreed on the same primary fingerprint for
# "Roman Arutyunyan <r.arutyunyan@f5.com>" / "Roman Arutyunyan
# <arut@nginx.com>":
#
#   nginx.org (arut.key)  -> 43387825DDB1BB97EC36BA5D007C8D7C15D87369
#   keys.openpgp.org      -> 43387825DDB1BB97EC36BA5D007C8D7C15D87369
#   keyserver.ubuntu.com  -> 43387825DDB1BB97EC36BA5D007C8D7C15D87369
#
# This key does not expire.
RECIPE_SIGNING_KEY_FPR="43387825DDB1BB97EC36BA5D007C8D7C15D87369"
RECIPE_SIGNING_KEY_EXPIRY="none"
RECIPE_SIGNING_KEY_VERIFIED_ON="2026-08-06"

# Upstream first, keyserver second, so an nginx.org outage does not stop a
# security rebuild — same ordering rationale as recipes/mariadb.sh.
RECIPE_SIGNING_KEY_URLS=(
	"https://nginx.org/keys/arut.key"
	"https://keys.openpgp.org/vks/v1/by-fingerprint/43387825DDB1BB97EC36BA5D007C8D7C15D87369"
	"https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x43387825DDB1BB97EC36BA5D007C8D7C15D87369&options=mr"
)

# §14's tripwire. From here on an nginx CVE is ours to notice.
RECIPE_UPSTREAM_RELEASE_DATE="2026-07-15"
RECIPE_LAST_CHECKED="2026-08-06"

# ------------------------------------------------ PCRE2, header only (note 1) --
#
# Not a RECIPE_DEPENDS entry: there is nothing to stage. recipe_extract below
# pulls exactly one file — src/pcre2.h.generic, PCRE2's own build systems
# copy this to pcre2.h verbatim, and it carries no unsubstituted @…@ or
# #cmakedefine tokens, confirmed by reading it — out of a verified archive,
# into $BUILD_WORK, and nothing else from this archive is ever touched
# again. The compiled, linked, SHIPPED pcre2 is Apple's
# /usr/lib/libpcre2-8.dylib, covered by check 2's unconditional allowance for
# /usr/lib/*.
#
# Same version recipes/mariadb.sh already pinned (10.45) — deliberately: one
# fewer PCRE2 release to track under §14 across the whole build fleet — but
# verified again here, independently, rather than cited, because this file
# has to stand on its own the same way that one does.
RECIPE_PCRE2_VERSION="10.45"
RECIPE_PCRE2_URL="https://github.com/PCRE2Project/pcre2/releases/download/pcre2-$RECIPE_PCRE2_VERSION/pcre2-$RECIPE_PCRE2_VERSION.zip"
RECIPE_PCRE2_SHA256="59c8556fd45e68599897cd5d74efad9c4a43f85e981fe7ac3ac5fd7aa70672ac"
RECIPE_PCRE2_SIGNATURE_URL="$RECIPE_PCRE2_URL.sig"
RECIPE_PCRE2_UPSTREAM_RELEASE_DATE="2025-02-05"
RECIPE_PCRE2_LAST_CHECKED="2026-08-06"

# PCRE2 signs every release asset. Verified 2026-08-06 from two hosts sharing
# no infrastructure with github.com, both serving the same primary and the
# same signing subkey:
#
#   keys.openpgp.org      -> A95536204A3BB489715231282A98E77EB6F24CA8
#   keyserver.ubuntu.com  -> A95536204A3BB489715231282A98E77EB6F24CA8
#
# The PRIMARY is pinned, not the subkey that made the signature (Nicholas
# Wilson, PCRE2's maintainer, signs with a subkey) — see
# recipes/mariadb.sh's identical note on why the primary, not the signing
# subkey, is the stable thing to pin.
RECIPE_PCRE2_SIGNING_KEY_FPR="A95536204A3BB489715231282A98E77EB6F24CA8"
RECIPE_PCRE2_SIGNING_KEY_URLS=(
	"https://keys.openpgp.org/vks/v1/by-fingerprint/A95536204A3BB489715231282A98E77EB6F24CA8"
	"https://keyserver.ubuntu.com/pks/lookup?op=get&search=0xA95536204A3BB489715231282A98E77EB6F24CA8&options=mr"
)

# ------------------------------------------------------------------- inputs --

# D6: reuse the staged static OpenSSL, never build a second one. Consumed
# through --with-cc-opt/--with-ld-opt in recipe_configure, NOT
# --with-openssl=<dir> — that flag makes nginx's own build compile OpenSSL
# again from source, which is exactly the "second one" D6 forbids.
RECIPE_OPENSSL_VERSION="3.5.7"
RECIPE_DEPENDS=("openssl:$RECIPE_OPENSSL_VERSION")

RECIPE_BUILD_TOOLS=(gpg)

# No RECIPE_IGNORE_PREFIXES: that mechanism only exists for
# -DCMAKE_IGNORE_PREFIX_PATH, and nginx's shell configure has no cmake-style
# prefix-search to poison in the first place. Safety here comes from
# supplying the staged OpenSSL's -I/-L explicitly (so the FIRST, unqualified
# feature test succeeds before configure ever tries its own
# /opt/homebrew-or-/opt/local fallbacks) and from the driver's own PATH
# scrub, which is already in effect by the time this function runs.

# ------------------------------------------------------- the artifact contract --

RECIPE_SERVER_BIN="bin/nginx"
# nginx has no --version; -v prints the banner to stderr and exits 0.
RECIPE_SERVER_VERSION_ARGS=(-v)

# D1: nginx's `make install` produces bin/ conf/ html/ logs/ — never share/.
# Declaring the layout here (rather than the audit.sh default of `bin
# share`) is the fix for that collision; see build/audit.sh and
# build/recipes/README.md. conf/ and html/ are nginx's own stock sample
# files (mime.types, fastcgi_params, index.html, …) — left in place rather
# than pruned by name, which would be a second, version-fragile source of
# truth about nginx's install layout; check 7 already proves none of them
# embeds a plantable path, and nothing this app runs ever reads them, since
# every generated site inlines its own mime map and fastcgi_param list
# instead of `include`-ing these (spec §3 "Measured" table).
RECIPE_REQUIRED_LAYOUT=(bin)

# ------------------------------------------------------------------ helpers --

_nginx_gnupg_home() { printf '%s/gnupg\n' "$BUILD_WORK"; }
_nginx_tarball() { printf '%s/nginx-%s.tar.gz\n' "$BUILD_DOWNLOADS" "$RECIPE_VERSION"; }
_nginx_signature() { printf '%s.asc\n' "$(_nginx_tarball)"; }
_nginx_pcre2_archive() { printf '%s/pcre2-%s.zip\n' "$BUILD_DOWNLOADS" "$RECIPE_PCRE2_VERSION"; }
_nginx_pcre2_signature() { printf '%s.sig\n' "$(_nginx_pcre2_archive)"; }
# Sibling of $BUILD_DOWNLOADS/$BUILD_SRC/$BUILD_OBJ under $BUILD_WORK — same
# pattern recipes/mariadb.sh uses for its gnupg home, not one of the three
# names the interface calls out by name but equally scratch (README.md
# "Paths a recipe may write to").
_nginx_pcre2_header_dir() { printf '%s/pcre2-include\n' "$BUILD_WORK"; }
_nginx_openssl_prefix() { bp_dep_prefix openssl "$RECIPE_OPENSSL_VERSION"; }

_nginx_gpg() {
	"$(bp_tool gpg)" --batch --no-tty --quiet --homedir "$(_nginx_gnupg_home)" "$@"
}

# Import every candidate key whose PRIMARY fingerprint is $1, from the URLs
# given after the label. Identical shape to recipes/mariadb.sh's
# _mariadb_import_key — the fingerprint is the trust anchor so key MATERIAL
# may come from anywhere, but freshness (expiry, revocation) only travels
# with the key, which is why nothing is ever reused from a previous run.
_nginx_import_key() {
	local fpr="$1" label="$2"
	shift 2
	local index=0 url dest imported=0 primary
	for url in "$@"; do
		index=$((index + 1))
		dest="$BUILD_DOWNLOADS/signing-key-$label-$index.asc"
		rm -f -- "$dest"
		if ! bp_download "$url" "$dest" >/dev/null 2>&1; then
			bp_log "signing key not available from $url"
			continue
		fi
		primary="$(_nginx_gpg --show-keys --with-colons "$dest" 2>/dev/null |
			awk -F: '$1 == "pub" { want = 1; next } $1 == "fpr" && want { print $10; want = 0 }' |
			grep -Fx "$fpr" || true)"
		if [ -z "$primary" ]; then
			bp_log "ignoring key from $url: no primary key with fingerprint $fpr"
			continue
		fi
		_nginx_gpg --import "$dest" >/dev/null 2>&1 || continue
		imported=$((imported + 1))
		bp_log "imported signing key $fpr from $url"
	done
	[ "$imported" -gt 0 ] ||
		bp_die "no host served a key with fingerprint $fpr; cannot verify provenance"
}

# Insist on a good signature over <file> by the primary key <fpr>.
#
# gpg --verify exits 0 on an EXPIRED signing key (measured 2026-08-02 against
# OpenSSL's) so the exit status proves nothing here either; the
# machine-readable status is read instead, exactly as recipes/mariadb.sh and
# recipes/openssl.sh already do. Neither key pinned in this file expires, but
# the check does not get to assume that of itself.
_nginx_verify_signature() {
	local file="$1" sig="$2" fpr="$3" what status errors bad
	what="$(basename -- "$file")"
	status="$BUILD_WORK/gpg-status-$what.txt"
	errors="$BUILD_WORK/gpg-stderr-$what.txt"

	_nginx_gpg --status-fd 1 --verify "$sig" "$file" >"$status" 2>"$errors" || true

	awk -v fpr="$fpr" \
		'$1 == "[GNUPG:]" && $2 == "VALIDSIG" && $NF == fpr { found = 1 }
		 END { exit found ? 0 : 1 }' "$status" ||
		bp_die "no valid signature by $fpr over $what; gpg said: $(tr '\n' ' ' <"$errors")"
	for bad in EXPKEYSIG REVKEYSIG BADSIG ERRSIG EXPSIG; do
		# An `if`, not `grep ... && bp_die`: under set -e the AND-list's failure
		# becomes the loop's exit status, and a loop that "fails" because
		# nothing was wrong would abort the build on the happy path.
		if grep -q "^\[GNUPG:\] $bad " "$status"; then
			bp_die "signature over $what is $bad; refusing to build from it"
		fi
	done
	bp_log "GPG: good signature by $fpr over $what"
}

# ------------------------------------------------------------------- stages --

recipe_fetch() {
	[ "$RECIPE_VERSION" = "$RECIPE_PINNED_VERSION" ] ||
		bp_die "this recipe pins nginx $RECIPE_PINNED_VERSION; $RECIPE_VERSION would need a new sha256 and a re-verified signature."
	# §12: no signature-checked x86_64 pin exists and this slice does not add
	# one. Also transitively true of RECIPE_DEPENDS' openssl recipe, which
	# hardcodes darwin64-arm64-cc — checked here too so this fails before
	# that dependency build even starts, not partway through it.
	[ "$BUILD_ARCH" = "arm64" ] ||
		bp_die "this pipeline is Apple Silicon only (spec §12); got $BUILD_ARCH"

	bp_download "$RECIPE_SOURCE_URL" "$(_nginx_tarball)"
	bp_download "$RECIPE_SIGNATURE_URL" "$(_nginx_signature)"
	bp_download "$RECIPE_PCRE2_URL" "$(_nginx_pcre2_archive)"
	bp_download "$RECIPE_PCRE2_SIGNATURE_URL" "$(_nginx_pcre2_signature)"

	bp_rm_tree "$(_nginx_gnupg_home)"
	mkdir -p "$(_nginx_gnupg_home)"
	chmod 700 "$(_nginx_gnupg_home)"
	# One keyring for both keys. Each verification names the fingerprint it
	# demands, so a key in here can only vouch for the release it actually
	# signed.
	_nginx_import_key "$RECIPE_SIGNING_KEY_FPR" nginx \
		${RECIPE_SIGNING_KEY_URLS[@]+"${RECIPE_SIGNING_KEY_URLS[@]}"}
	_nginx_import_key "$RECIPE_PCRE2_SIGNING_KEY_FPR" pcre2 \
		${RECIPE_PCRE2_SIGNING_KEY_URLS[@]+"${RECIPE_PCRE2_SIGNING_KEY_URLS[@]}"}
}

recipe_verify_source() {
	_nginx_verify_signature "$(_nginx_tarball)" "$(_nginx_signature)" "$RECIPE_SIGNING_KEY_FPR"
	bp_verify_sha256 "$(_nginx_tarball)" "$RECIPE_SOURCE_SHA256"

	_nginx_verify_signature "$(_nginx_pcre2_archive)" "$(_nginx_pcre2_signature)" \
		"$RECIPE_PCRE2_SIGNING_KEY_FPR"
	bp_verify_sha256 "$(_nginx_pcre2_archive)" "$RECIPE_PCRE2_SHA256"
}

recipe_extract() {
	tar -xzf "$(_nginx_tarball)" -C "$BUILD_SRC" --strip-components 1

	# The header only (note 1 above) — never the rest of the archive.
	local hdr_dir hdr
	hdr_dir="$(_nginx_pcre2_header_dir)"
	mkdir -p "$hdr_dir"
	hdr="$hdr_dir/pcre2.h"
	unzip -p "$(_nginx_pcre2_archive)" "pcre2-$RECIPE_PCRE2_VERSION/src/pcre2.h.generic" \
		>"$hdr" ||
		bp_die "could not extract src/pcre2.h.generic from the verified PCRE2 archive; the internal layout may have changed"
	# Verified, not assumed: a truncated or wrong-entry extraction would
	# otherwise surface only as a confusing configure-time PCRE2 failure two
	# stages later. An `if`, not `A && B || bp_die`, so a `grep` no-match
	# (exit 1) cannot be misread as "C may run when A is true" — it always
	# should here.
	if [ ! -s "$hdr" ] || ! grep -q 'pcre2_compile' "$hdr"; then
		bp_die "extracted pcre2.h does not look like a PCRE2 header (empty, or missing pcre2_compile)"
	fi
}

recipe_configure() {
	local ssl_prefix pcre2_inc lib

	ssl_prefix="$(_nginx_openssl_prefix)"
	# Same defensive check recipes/mariadb.sh runs before trusting a staged
	# dependency: build.sh skips a dependency whose prefix directory already
	# exists, so a half-built or hand-edited tree would otherwise be reused in
	# silence and only surface at the audit, twenty minutes later.
	for lib in libcrypto.a libssl.a; do
		[ -f "$ssl_prefix/lib/$lib" ] ||
			bp_die "static OpenSSL $RECIPE_OPENSSL_VERSION is not staged at $ssl_prefix (missing lib/$lib); remove that directory and re-run so the dependency is rebuilt"
	done
	[ -z "$(find "$ssl_prefix" -name '*.dylib' -print | sed -n 1p)" ] ||
		bp_die "the staged OpenSSL at $ssl_prefix contains a dylib; D6 requires static, and a dylib here would become a shipped dependency"

	pcre2_inc="$(_nginx_pcre2_header_dir)"
	[ -s "$pcre2_inc/pcre2.h" ] ||
		bp_die "pcre2.h is missing at $pcre2_inc; recipe_extract should have produced it"

	# --with-cc-opt puts both -I paths into CFLAGS, and --with-ld-opt puts
	# OpenSSL's -L into LDFLAGS, BEFORE auto/lib/{pcre,openssl,zlib}/conf run
	# (configure sources auto/cc/conf, which folds these in, at position 50;
	# auto/lib/conf runs at position 64) — confirmed by reading configure's
	# own stage order, not assumed. That ordering is what makes the very
	# FIRST, unqualified feature test inside each of those three succeed
	# against exactly the library this recipe intends, before configure ever
	# tries its own /usr/local, /opt/local or /opt/homebrew fallbacks. PCRE2
	# needs no -L: the SDK's libpcre2-8.tbd stub is already on the default
	# linker search path (confirmed by a standalone link test against
	# /usr/lib/libpcre2-8.dylib with zero extra -L). zlib needs neither flag:
	# the SDK ships zlib.h and a libz.tbd stub, so nginx's own default "system
	# zlib" probe already succeeds unassisted.
	local args=(
		"--prefix=$BUILD_PREFIX"
		# D1: the binary at bin/, not sbin/.
		"--sbin-path=$BUILD_PREFIX/bin/nginx"
		# These compiled-in defaults are what a bare `nginx -V`/no-flags
		# invocation would fall back to; OpenVHost itself always passes -c and
		# -e explicitly (design doc §10, and inspect.rs's -e-is-mandatory
		# rule), so none of these are live paths for this app. They still have
		# to be SOMETHING, and un-plantable-by-construction ($BUILD_PREFIX,
		# D8) beats nginx's own /usr/local/nginx default, which is one of
		# RECIPE_IGNORE_PREFIXES for exactly this reason elsewhere in this
		# pipeline.
		"--conf-path=$BUILD_PREFIX/conf/nginx.conf"
		"--error-log-path=$BUILD_PREFIX/logs/error.log"
		"--pid-path=$BUILD_PREFIX/logs/nginx.pid"
		"--lock-path=$BUILD_PREFIX/logs/nginx.lock"
		"--http-log-path=$BUILD_PREFIX/logs/access.log"
		"--http-client-body-temp-path=$BUILD_PREFIX/client_body_temp"
		"--http-proxy-temp-path=$BUILD_PREFIX/proxy_temp"
		"--http-fastcgi-temp-path=$BUILD_PREFIX/fastcgi_temp"
		"--http-uwsgi-temp-path=$BUILD_PREFIX/uwsgi_temp"
		"--http-scgi-temp-path=$BUILD_PREFIX/scgi_temp"
		"--builddir=$BUILD_OBJ"
		"--with-cc-opt=-I$ssl_prefix/include -I$pcre2_inc"
		"--with-ld-opt=-L$ssl_prefix/lib"
		# D6: static OpenSSL, one SSL story, no dynamic modules anywhere below.
		"--with-http_ssl_module"
		# Force PCRE2 usage rather than leave it optional — note 3 above: with
		# rewrite disabled, an otherwise-identical configure silently built
		# WITHOUT regex support at all, which this app's generated configs
		# (location ~, map capture groups) cannot tolerate.
		"--with-pcre"
		#
		# Everything below is read off crates/openvhost-conf/templates/nginx/
		# {main,site,default-site,php-location}.conf.tera, plus a repo-wide
		# grep for every remaining optional module's directive name
		# (autoindex, auth_basic, charset, ssi, userid, mirror, geo,
		# split_clients, referer/$http_referer, rewrite/if/return/set/break,
		# the upstream hash/ip_hash/least_conn/random/zone/keepalive/sticky
		# balancing methods, sub_filter, xslt, image_filter, dav,
		# auth_request, slice, secure_link, stub_status, realip,
		# random_index, perl, geoip, grpc_pass, proxy_pass) confirming none of
		# them appear anywhere the app generates — no guessing either
		# direction. ngx_http_log_module, ngx_http_upstream_module,
		# ngx_http_core_module and the mandatory filter chain have no
		# --without- flag at all (auto/modules: they are compiled
		# unconditionally), so they are not listed here either way.
		#
		# What stays ON, and what in the config needs it:
		#   http_map      main.conf.tera's `map $request_uri $request_path`
		#                 (the querystring-stripping trick the P1
		#                 live-log-viewer design settled on) — a regex
		#                 capture, which is also why --with-pcre (above) is
		#                 forced rather than left optional.
		#   http_access   `location ~ /\. { deny all; }`, in every site
		#                 (site.conf.tera and default-site.conf.tera).
		#   http_fastcgi  php-location.conf.tera's fastcgi_pass/fastcgi_param
		#                 block; main.conf.tera's fastcgi_*_timeout and
		#                 fastcgi_temp_path; webserver.rs's
		#                 PhpUpstream::TcpPorts arm, which emits a plain
		#                 upstream{} (core, unconditional) fronted by
		#                 `fastcgi_pass <name>;`.
		#   http_gzip     main.conf.tera's `gzip`/`gzip_comp_level`/
		#                 `gzip_types` (webserver.rs fn gzip_extra).
		#   http_proxy    main.conf.tera unconditionally emits
		#                 `proxy_temp_path` even though no site proxies
		#                 today — the directive itself would not PARSE
		#                 without the module, so this is load-bearing today,
		#                 not forward-looking.
		#   http_uwsgi    same reasoning, for `uwsgi_temp_path`.
		#   http_scgi     same reasoning, for `scgi_temp_path`.
		#   http_ssl      the one deliberate exception to "only what the
		#                 config uses": not referenced by any generated
		#                 directive today, but required by D6 — link the
		#                 already-staged static OpenSSL now, while it costs
		#                 nothing extra, ahead of a TLS slice.
		"--without-http_charset_module"
		"--without-http_ssi_module"
		"--without-http_userid_module"
		"--without-http_mirror_module"
		"--without-http_autoindex_module"
		"--without-http_geo_module"
		"--without-http_split_clients_module"
		"--without-http_referer_module"
		"--without-http_rewrite_module"
		"--without-http_auth_basic_module"
		"--without-http_memcached_module"
		"--without-http_limit_conn_module"
		"--without-http_limit_req_module"
		"--without-http_empty_gif_module"
		"--without-http_browser_module"
		"--without-http_upstream_hash_module"
		"--without-http_upstream_ip_hash_module"
		"--without-http_upstream_least_conn_module"
		"--without-http_upstream_random_module"
		"--without-http_upstream_keepalive_module"
		"--without-http_upstream_zone_module"
		"--without-http_upstream_sticky"
		"--without-http_grpc_module"
	)
	bp_record_flags "${args[@]}"
	# CWD matters: configure sources auto/options etc. via relative `.`
	# paths, so it must run from the extracted source root. `sh`, not relying
	# on the extracted file's own shebang+exec bit — same explicitness
	# openssl.sh uses for its own Configure invocation.
	(cd "$BUILD_SRC" && sh ./configure "${args[@]}")
}

recipe_build() {
	# nginx's configure writes the tree's real Makefile to
	# $BUILD_OBJ/Makefile (its own $NGX_OBJS, set by --builddir above), but
	# that Makefile's OWN rules name sources relative to the source root
	# (`src/core/nginx.h`, not an absolute or $BUILD_OBJ-relative path) —
	# measured: `make -f "$BUILD_OBJ/Makefile"` from outside $BUILD_SRC fails
	# immediately with "No rule to make target `src/core/nginx.h'". `cd` is
	# therefore not optional the way it looks; `--builddir` relocates the
	# object/output tree, not the source-relative-path assumption baked into
	# every rule.
	(cd "$BUILD_SRC" && make -f "$BUILD_OBJ/Makefile" -j"$BUILD_JOBS" build)
}

recipe_install() {
	# Same CWD requirement as recipe_build, and for the same reason: the
	# install rules also name sources relative to $BUILD_SRC.
	#
	# Installs straight to $BUILD_PREFIX (D8): every path flag in
	# recipe_configure was already absolute and under $BUILD_PREFIX, so there
	# is no DESTDIR/staging directory here to embed a mover's path into.
	(cd "$BUILD_SRC" && make -f "$BUILD_OBJ/Makefile" install)
}

# recipe_normalize is not defined: nothing this recipe ships carries a
# custom LC_ID_DYLIB to rewrite. The only dynamic dependencies bin/nginx
# has are /usr/lib/libpcre2-8.dylib, /usr/lib/libz.1.dylib and
# /usr/lib/libSystem.B.dylib (confirmed via otool -L on a real build) —
# already @loader_path-agnostic system paths — and OpenSSL is linked
# statically, so there is no dylib of our own to touch, same reasoning
# recipes/openssl.sh gives for skipping this stage.

# --------------------------------------------------- contract check 6 (serve) --
#
# Everything below runs inside audit.sh, where errexit is suspended because
# the probe is invoked from an `if`. Nothing may therefore rely on set -e:
# every step is checked, and every exit path goes through _nginx_probe_stop.

_NGINX_PROBE_PID=""

# Has this pid exited? A child we started stays visible to `kill -0` as a
# zombie until it is waited for, so polling with `kill -0` alone would spin
# for the full timeout on a perfectly clean shutdown. Identical to
# recipes/mariadb.sh's _mariadb_pid_gone.
_nginx_pid_gone() {
	local pid="$1" state
	state="$(ps -p "$pid" -o state= 2>/dev/null | tr -d ' ')"
	[ -z "$state" ] && return 0
	case "$state" in Z*) return 0 ;; esac
	return 1
}

# A free TCP port on 127.0.0.1, PROVEN free by a real connection attempt —
# never assumed, and never 80 or 8080. This is still check-then-bind, a
# textbook TOCTOU: RANDOM only lowers the odds that two audits running at
# once start looking at the same candidate, it does not remove the race
# between the `nc -z` check here and nginx's own bind a few lines later.
# Low impact when it does happen — the loser fails loudly as a probe
# failure (nginx cannot bind the port), never a false pass — but that is a
# mitigation, not a proof, and is not claimed as one.
_nginx_free_port() {
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

# A config that exercises the same directive shapes the real templates do —
# regex map capture, a dotfile deny, a PHP location with fastcgi_param — so
# a serve failure here is a proof the module selection above is wrong, not
# just that SOME config loads. The fastcgi_pass target does not exist:
# nothing here depends on PHP running, only on nginx accepting the directive
# and serving the static path.
_nginx_write_conf() {
	local scratch="$1" port="$2" conf
	conf="$scratch/probe.conf"
	cat >"$conf" <<EOF
daemon off;
worker_processes 1;
pid "$scratch/run/nginx.pid";
error_log "$scratch/run/error.log" warn;

events {
    worker_connections 64;
}

http {
    map \$request_uri \$request_path {
        ~^(?<p>[^?]*)  \$p;
    }
    log_format probe '\$remote_addr - [\$time_local] "\$request_method \$request_path \$server_protocol" \$status \$body_bytes_sent';
    access_log "$scratch/run/access.log" probe;
    gzip on;
    gzip_comp_level 6;
    default_type application/octet-stream;
    client_body_temp_path "$scratch/run/client_body";
    proxy_temp_path "$scratch/run/proxy";
    fastcgi_temp_path "$scratch/run/fastcgi";
    uwsgi_temp_path "$scratch/run/uwsgi";
    scgi_temp_path "$scratch/run/scgi";

    server {
        listen 127.0.0.1:$port;
        server_name _;
        root "$scratch/docroot";
        index index.html;

        location / {
            try_files \$uri \$uri/ =404;
        }

        location ~ /\. {
            deny all;
        }

        location ~ \.php\$ {
            try_files \$uri =404;
            fastcgi_pass "unix:$scratch/run/no-such-php.sock";
            fastcgi_param SCRIPT_FILENAME \$document_root\$fastcgi_script_name;
            fastcgi_param QUERY_STRING \$query_string;
        }
    }
}
EOF
	printf '%s\n' "$conf"
}

_nginx_probe_start() {
	local tree="$1" conf="$2" scratch="$3" port="$4" waited=0
	"$tree/bin/nginx" -c "$conf" >>"$scratch/run/nginx.out" 2>&1 &
	_NGINX_PROBE_PID=$!
	# 30s: nginx has no slow bootstrap phase (no InnoDB-style warmup), so this
	# is already generous. A server that is never coming up is caught by the
	# liveness check below, not by the clock.
	while [ "$waited" -lt 60 ]; do
		if curl -fsS -m 2 -o /dev/null "http://127.0.0.1:$port/index.html" 2>/dev/null; then
			return 0
		fi
		if _nginx_pid_gone "$_NGINX_PROBE_PID"; then
			printf 'nginx exited before it accepted a connection\n'
			return 1
		fi
		sleep 0.5
		waited=$((waited + 1))
	done
	printf 'nginx did not accept a connection within 30s\n'
	return 1
}

_nginx_probe_stop() {
	local waited=0 pid="$_NGINX_PROBE_PID"
	[ -n "$pid" ] || return 0
	_NGINX_PROBE_PID=""
	# SIGTERM is nginx's fast-shutdown signal (distinct from SIGQUIT's
	# graceful drain); the master cascades it to the worker before the
	# master itself exits, so this brings down both with nothing orphaned —
	# proven live, not assumed: a bare stop/start cycle against a build from
	# this exact recipe left zero processes behind before this probe was
	# written into the recipe.
	kill -TERM "$pid" 2>/dev/null || true
	while [ "$waited" -lt 60 ]; do
		if _nginx_pid_gone "$pid"; then
			wait "$pid" 2>/dev/null || true
			return 0
		fi
		sleep 0.5
		waited=$((waited + 1))
	done
	# The contract says the probe leaves no process running, on either path.
	kill -KILL "$pid" 2>/dev/null || true
	wait "$pid" 2>/dev/null || true
	printf 'nginx did not stop on SIGTERM within 30s and was killed\n'
	return 1
}

# GET <path> into <dest> and cmp(1) it against <want-file> — a real byte
# comparison of files on disk, not a shell-string compare: command
# substitution strips trailing newlines, which would quietly turn "compare
# the bytes" into "compare the bytes modulo a trailing newline".
_nginx_get_and_compare() {
	local port="$1" path="$2" want_file="$3" dest="$4" err
	err="$dest.err"
	if ! curl -fsS -m 5 "http://127.0.0.1:$port/$path" -o "$dest" 2>"$err"; then
		printf 'GET /%s failed:\n' "$path"
		cat "$err" 2>/dev/null || true
		return 1
	fi
	if ! cmp -s "$want_file" "$dest"; then
		printf 'GET /%s returned bytes that do not match what was written (want %s bytes, got %s)\n' \
			"$path" "$(wc -c <"$want_file" | tr -d ' ')" "$(wc -c <"$dest" | tr -d ' ')"
		return 1
	fi
	return 0
}

recipe_serve_probe() {
	local tree="$1" scratch="$2" port conf docfile stop_note

	port="$(_nginx_free_port)" || {
		printf 'could not find a free TCP port on 127.0.0.1\n'
		return 1
	}
	mkdir -p "$scratch/run" "$scratch/docroot"
	docfile="$scratch/docroot/index.html"
	printf 'openvhost-nginx-probe-%s\n' "$RECIPE_VERSION" >"$docfile"
	conf="$(_nginx_write_conf "$scratch" "$port")"

	if ! _nginx_probe_start "$tree" "$conf" "$scratch" "$port"; then
		tail -n 20 "$scratch/run/error.log" 2>/dev/null || true
		_nginx_probe_stop >/dev/null 2>&1 || true
		return 1
	fi

	if ! _nginx_get_and_compare "$port" "index.html" "$docfile" "$scratch/got1.html"; then
		_nginx_probe_stop >/dev/null 2>&1 || true
		return 1
	fi

	if ! _nginx_probe_stop; then
		return 1
	fi
	if ! _nginx_probe_start "$tree" "$conf" "$scratch" "$port"; then
		tail -n 20 "$scratch/run/error.log" 2>/dev/null || true
		_nginx_probe_stop >/dev/null 2>&1 || true
		return 1
	fi

	if ! _nginx_get_and_compare "$port" "index.html" "$docfile" "$scratch/got2.html"; then
		_nginx_probe_stop >/dev/null 2>&1 || true
		return 1
	fi

	# Status captured, not discarded: nothing is orphaned either way (SIGKILL +
	# wait below), so this is reporting only — but a server that needed
	# SIGKILL to stop is exactly what this check exists to notice, and a
	# summary that stays silent about it would hide the one thing check 6
	# cares about most.
	stop_note=""
	_nginx_probe_stop >/dev/null 2>&1 || stop_note=" (note: SIGTERM alone did not stop it within 30s; needed SIGKILL)"
	# PROBE-SUMMARY: marks the line audit.sh's check 6 takes as its PASS note
	# (build/recipes/README.md) — the first line carrying this prefix, so
	# nothing printed afterward (including the stop note above, on the
	# happy path there is none) can displace it.
	printf 'PROBE-SUMMARY: served on 127.0.0.1:%s, GET matched byte-for-byte (cmp), restarted, GET matched again%s\n' \
		"$port" "$stop_note"
	return 0
}

# ---------------------------------------------------------------- manifest ----

recipe_manifest_extra() {
	local pcre2_actual
	pcre2_actual="$(shasum -a 256 -- "$(_nginx_pcre2_archive)" 2>/dev/null | cut -d' ' -f1)"
	printf '{"openssl": {"version": "%s", "linkage": "static"}, "pcre2": {"version": "%s", "sha256": "%s", "sha256_on_disk": "%s", "release_date": "%s", "last_checked": "%s", "verified": "gpg+sha256", "signing_key_fingerprint": "%s", "usage": "header only (src/pcre2.h.generic); linked against the system /usr/lib/libpcre2-8.dylib, never compiled or shipped"}, "zlib": {"source": "system (/usr/lib/libz.dylib via the Xcode/CLT SDK)", "note": "no fetch: nginx'"'"'s own default probe already succeeds against the SDK"}}' \
		"$RECIPE_OPENSSL_VERSION" \
		"$RECIPE_PCRE2_VERSION" "$RECIPE_PCRE2_SHA256" "${pcre2_actual:-unknown}" \
		"$RECIPE_PCRE2_UPSTREAM_RELEASE_DATE" "$RECIPE_PCRE2_LAST_CHECKED" "$RECIPE_PCRE2_SIGNING_KEY_FPR"
}
