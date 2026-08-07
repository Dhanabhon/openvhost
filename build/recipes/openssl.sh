# SPDX-License-Identifier: GPL-3.0-or-later
# shellcheck shell=bash
# shellcheck disable=SC2034  # the RECIPE_* variables are read by build.sh and
#                              audit.sh, which source this file
#
# OpenSSL — a build INPUT, never a product.
#
# D3/§13: the owner chose static OpenSSL. MariaDB consumes this tree through
# RECIPE_DEPENDS and links libcrypto.a/libssl.a into its own binaries, so that
# the shipped package has no OpenSSL dylib to relocate, re-sign, or fail to find
# at runtime. Nothing built here is ever packed or published — which is why this
# recipe refuses to run without --stage-only. It could not satisfy the artifact
# contract if it tried: contract check 1 wants bin/ *and* share/, and a static
# OpenSSL install has no share/.
#
# Consequently there is no recipe_serve_probe and no RECIPE_SERVER_BIN here:
# --stage-only stops after normalize and never reaches the audit at all.

# ---------------------------------------------------------------- provenance --

# 3.5 is the LTS series (supported to 2030-04-08); 3.5.7 is its current patch
# release. §13's "11.4 LTS only" reasoning applies with equal force to the
# library we compile into it: every extra series is another tree to rebuild when
# a CVE lands.
RECIPE_PINNED_VERSION="3.5.7"

RECIPE_SOURCE_URL="https://github.com/openssl/openssl/releases/download/openssl-$RECIPE_VERSION/openssl-$RECIPE_VERSION.tar.gz"
RECIPE_SOURCE_SHA256="a8c0d28a529ca480f9f36cf5792e2cd21984552a3c8e4aa11a24aa31aeac98e8"
RECIPE_SIGNATURE_URL="$RECIPE_SOURCE_URL.asc"

# The OpenSSL release key. Verified on 2026-08-02 by fetching it from two
# independent hosts that share no infrastructure with the download host and
# comparing primary fingerprints:
#
#   keys.openpgp.org        -> BA5473A2B0587B07FB27CF2D216094DFD0CB81EF
#   keyserver.ubuntu.com    -> BA5473A2B0587B07FB27CF2D216094DFD0CB81EF
#
# The fingerprint below is the trust anchor, so the key MATERIAL may be fetched
# from anywhere: a substituted key cannot produce this fingerprint. What the
# fetch has to get right is freshness, not authenticity — see the expiry note.
RECIPE_SIGNING_KEY_FPR="BA5473A2B0587B07FB27CF2D216094DFD0CB81EF"
RECIPE_SIGNING_KEY_EXPIRY="2026-09-08"
RECIPE_SIGNING_KEY_VERIFIED_ON="2026-08-02"

# This key expires, and upstream extends it rather than rotating it — on
# 2026-08-02 keyserver.ubuntu.com still served a copy that expired 2026-06-14
# while keys.openpgp.org served the extended one expiring 2026-09-08, and the
# 3.5.7 signature was made on 2026-06-09, five days before the stale copy's
# expiry. Both copies are therefore authentic and only one verifies cleanly.
#
# So: every candidate whose primary fingerprint matches the pin is imported into
# one scratch keyring, and gpg merges them — the newest self-signature wins, and
# a stale mirror cannot hold the build back. Freshest source first regardless.
# When this expiry passes and no host yet serves an extended copy, the build
# fails loudly. That is the correct failure: it means go and look.
RECIPE_SIGNING_KEY_URLS=(
	"https://keys.openpgp.org/vks/v1/by-fingerprint/BA5473A2B0587B07FB27CF2D216094DFD0CB81EF"
	"https://keyserver.ubuntu.com/pks/lookup?op=get&search=0xBA5473A2B0587B07FB27CF2D216094DFD0CB81EF&options=mr"
)

# §14's tripwire. Leaving Homebrew makes an OpenSSL CVE ours to notice.
RECIPE_UPSTREAM_RELEASE_DATE="2026-06-09"
RECIPE_LAST_CHECKED="2026-08-02"

# ------------------------------------------------------------------- inputs --

# gpg only. OpenSSL's build needs perl, make and cc, and all three are in the
# scrubbed PATH already — resolving them BEFORE the scrub, as bp_tool does,
# would let a Homebrew perl or gmake in through the front door for no gain.
RECIPE_BUILD_TOOLS=(gpg)

# Never audited (--stage-only), but stated rather than left to the default so
# that a reader does not have to infer it.
RECIPE_SERVER_BIN=""

# ------------------------------------------------------------------ helpers --

# Every path below is derived from $BUILD_WORK, which build.sh owns.
_openssl_tarball() { printf '%s/openssl-%s.tar.gz\n' "$BUILD_DOWNLOADS" "$RECIPE_VERSION"; }
_openssl_signature() { printf '%s.asc\n' "$(_openssl_tarball)"; }

# ------------------------------------------------------------------- stages --

recipe_fetch() {
	# A build input is not a product. build.sh sets STAGE_ONLY from --stage-only
	# before it sources a recipe; reading it here rather than at source time keeps
	# sourcing free of side effects, which audit.sh relies on.
	[ "${STAGE_ONLY:-0}" -eq 1 ] ||
		bp_die "openssl is a build input, not a product: build it with --stage-only (it is consumed through RECIPE_DEPENDS)"
	[ "$RECIPE_VERSION" = "$RECIPE_PINNED_VERSION" ] ||
		bp_die "this recipe pins OpenSSL $RECIPE_PINNED_VERSION; $RECIPE_VERSION would need a new sha256 and a re-verified signature"
	# §12: no signature-checked x86_64 pin exists and this slice does not add one.
	[ "$BUILD_ARCH" = "arm64" ] ||
		bp_die "this pipeline is Apple Silicon only (spec §12); got $BUILD_ARCH"

	bp_download "$RECIPE_SOURCE_URL" "$(_openssl_tarball)"
	bp_download "$RECIPE_SIGNATURE_URL" "$(_openssl_signature)"

	bp_gpg_init_home
	bp_gpg_import_key "$RECIPE_SIGNING_KEY_FPR" "" \
		${RECIPE_SIGNING_KEY_URLS[@]+"${RECIPE_SIGNING_KEY_URLS[@]}"}
}

recipe_verify_source() {
	# The signature first: it is the statement about who produced these bytes.
	# The pinned digest that follows says they are the same bytes we reviewed.
	bp_gpg_verify_signature "$(_openssl_tarball)" "$(_openssl_signature)" "$RECIPE_SIGNING_KEY_FPR"
	bp_verify_sha256 "$(_openssl_tarball)" "$RECIPE_SOURCE_SHA256"
}

recipe_extract() {
	tar -xzf "$(_openssl_tarball)" -C "$BUILD_SRC" --strip-components 1
}

recipe_configure() {
	local args=(
		"--prefix=$BUILD_PREFIX"
		"--openssldir=$BUILD_PREFIX/ssl"
		--libdir=lib
		# The whole point of this recipe. no-shared gives .a only; no-module stops
		# the providers being built as loadable .dylib bundles that would have to
		# ship, be signed, and be found at runtime — exactly the class of failure
		# D3 chose static to delete.
		no-shared
		no-module
		# Neither is used by anything we link, and both cost minutes.
		no-tests
		no-docs
		# Explicit rather than auto-detected. macOS/arm64 is the only target this
		# pipeline has (§12), so naming it removes a guess.
		darwin64-arm64-cc
	)
	bp_record_flags "openssl:Configure" "${args[@]}"
	# Out of tree: $BUILD_SRC stays pristine and re-configuring cannot inherit a
	# previous run's generated makefile.
	(cd "$BUILD_OBJ" && /usr/bin/perl "$BUILD_SRC/Configure" "${args[@]}")
}

recipe_build() {
	(cd "$BUILD_OBJ" && make -j"$BUILD_JOBS")
}

recipe_install() {
	# install_sw, not install: the docs target needs pod2man and installs nothing
	# MariaDB links against.
	(cd "$BUILD_OBJ" && make install_sw)

	# Plan Task 2 step 1: assert static, do not assume it. A single .dylib here
	# would silently become a MariaDB dependency, and the first place anyone would
	# notice is contract check 2 — after a twenty-minute build.
	local shared lib
	shared="$(find "$BUILD_PREFIX" \( -name '*.dylib' -o -name '*.so' \) -print | sort || true)"
	[ -z "$shared" ] ||
		bp_die "OpenSSL produced shared objects despite no-shared/no-module: $(printf '%s' "$shared" | tr '\n' ' ')"
	for lib in libcrypto.a libssl.a; do
		[ -f "$BUILD_PREFIX/lib/$lib" ] ||
			bp_die "static build did not produce lib/$lib"
	done
	bp_log "static OpenSSL staged at $BUILD_PREFIX (libcrypto.a, libssl.a, no shared objects)"
}
