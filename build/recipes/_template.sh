# SPDX-License-Identifier: GPL-3.0-or-later
# shellcheck shell=bash
# shellcheck disable=SC2034  # the RECIPE_* variables are read by build.sh and
#                              audit.sh, which source this file
#
# Template for a build/build.sh recipe. Copy to <name>.sh and fill in.
# The interface, and the reasoning behind each rule, is in README.md.
#
# This file is sourced, never executed. At source time set variables and define
# functions — nothing else. Sourcing happens before the environment is made
# hermetic, and audit.sh sources recipes too, where there is no build at all.
#
# A name starting with "_" is not a valid package name, so this template cannot
# be built by accident.

# ---------------------------------------------------------------- provenance --

# Upstream's archive and the digest we pin it to. Both are required.
RECIPE_SOURCE_URL="https://example.invalid/$RECIPE_NAME-$RECIPE_VERSION.tar.gz"
RECIPE_SOURCE_SHA256="0000000000000000000000000000000000000000000000000000000000000000"

# Upstream's signing key, and when its fingerprint was last cross-checked
# against a second host. Provenance is verified, not assumed (spec §7).
RECIPE_SIGNING_KEY_FPR=""
RECIPE_SIGNING_KEY_EXPIRY=""
RECIPE_SIGNING_KEY_VERIFIED_ON=""

# §14's tripwire: leaving Homebrew makes security updates ours to notice. A
# stale check has to be visible in source rather than remembered.
RECIPE_UPSTREAM_RELEASE_DATE=""
RECIPE_LAST_CHECKED=""

# ------------------------------------------------------------------- inputs --

# Every tool not in /usr/bin or /bin. These are resolved to absolute paths
# before PATH is scrubbed, and reached through bp_tool — never by bare name.
RECIPE_BUILD_TOOLS=(gpg)

# name:version entries built first with --stage-only. Locate one with
# bp_dep_prefix <name> <version>.
RECIPE_DEPENDS=()

# ------------------------------------------------------- the artifact contract --

# The binary contract checks 5 and 6 exercise. Leave empty for a package with no
# server: both checks then report SKIPPED (no server binary), never silently.
RECIPE_SERVER_BIN=""
RECIPE_SERVER_VERSION_ARGS=(--version)

# ------------------------------------------------------------------- stages --

recipe_fetch() {
	bp_download "$RECIPE_SOURCE_URL" "$BUILD_DOWNLOADS/$RECIPE_NAME-$RECIPE_VERSION.tar.gz"
	# Fetch upstream's signature and its signed checksum file here too.
}

recipe_verify_source() {
	# Verify upstream's GPG signature FIRST, then the pinned digest. A failure
	# here must abort the build — that is the whole point of the stage.
	bp_verify_sha256 "$BUILD_DOWNLOADS/$RECIPE_NAME-$RECIPE_VERSION.tar.gz" \
		"$RECIPE_SOURCE_SHA256"
	bp_die "recipe_verify_source is not implemented: upstream's signature is unchecked"
}

recipe_extract() {
	tar -xzf "$BUILD_DOWNLOADS/$RECIPE_NAME-$RECIPE_VERSION.tar.gz" \
		-C "$BUILD_SRC" --strip-components 1
}

recipe_configure() {
	# Pin every option explicitly. `auto` is the enemy: it is how the reference
	# build picked up Homebrew's pcre2 (spec §2).
	bp_record_flags \
		"-DCMAKE_INSTALL_PREFIX=$BUILD_PREFIX" \
		"-DCMAKE_IGNORE_PREFIX_PATH=$(bp_ignore_prefix_path)"
	bp_die "recipe_configure is not implemented"
}

recipe_build() {
	bp_die "recipe_build is not implemented"
}

recipe_install() {
	# Install directly to $BUILD_PREFIX. A DESTDIR staging directory that is
	# moved afterwards embeds the staging path in the tree, which is exactly
	# what contract check 4 exists to reject.
	bp_die "recipe_install is not implemented"
}

# ----------------------------------------------------------------- optional --

# Rewrite install names so every reference is @loader_path/... Runs before
# signing, because install_name_tool invalidates a signature (D4). Delete this
# function entirely if static linking makes it unnecessary — which is why D3
# chose static.
#
# recipe_normalize() {
# 	install_name_tool -id "@loader_path/../lib/libfoo.1.dylib" \
# 		"$BUILD_PREFIX/lib/libfoo.1.dylib"
# }

# Contract check 6. Runs against <tree> in place; may write only inside
# <scratch>; must leave no process behind.
#
# recipe_serve_probe() {
# 	local tree="$1" scratch="$2"
# 	...start, create a table, insert, restart, read the row back...
# }

# One JSON value recorded under "recipe" in the build manifest.
#
# recipe_manifest_extra() {
# 	printf '{"note": "anything worth auditing later"}'
# }
