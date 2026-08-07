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
# THIS FILE, RIGHT NOW: fetch and verify only (task 2b). recipe_configure,
# recipe_build, recipe_install, recipe_normalize and recipe_serve_probe are
# explicit "not yet implemented" stubs — task 2c's job. recipe_extract is
# ALSO stubbed here, one function beyond task 2b's brief: build.sh requires
# every one of recipe_fetch/verify_source/extract/configure/build/install to
# already be a defined function before it runs ANY stage (build/build.sh
# ~L371), so leaving recipe_extract undefined would stop the real driver
# before recipe_fetch ever ran and this file's fetch/verify stages could
# never be proved against it.
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
# php-src itself gets the full treatment beyond the other 37 archives: a
# GPG-verified signature, not just a digest (D1). Its signing key is PER
# RELEASE, not per PHP major — whichever release manager cut that tarball
# signed it — so RECIPE_SOURCE_URL/SHA256/SIGNING_KEY_FPR are resolved below
# from _php-pins.sh's PHP_PINS_PHP_SRC table, keyed on RECIPE_VERSION, rather
# than hardcoded the way nginx.sh's single pinned version is.

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
# Not called anywhere in THIS file: pkg-config is a recipe_configure input
# (task 2c), and recipe_configure is a stub here. Defined now because task
# 2b's brief is to pin every build input this recipe needs, this one
# included, and proving the resolution doesn't require configure to exist —
# see this task's report for the live proof.

# spc builds its own OpenSSL from the pinned `openssl` entry in
# PHP_PINS_LIBS (3.6.3) as part of its own closure — unlike nginx.sh/
# mariadb.sh, which reuse a separately staged, shared static OpenSSL via
# RECIPE_DEPENDS. PHP has no use for that shared one: D5's finding is that
# php-src's own statically-linked OpenSSL bakes MODULESDIR/ENGINESDIR paths
# into bin/php and bin/php-fpm, so it has to be the copy spc itself compiles
# and controls, not a prefix borrowed from elsewhere.
RECIPE_DEPENDS=()

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

	# Every one of the 37 pinned third-party archives (D1) — ours, through
	# bp_download, never spc's own (unverified, `curl -sfSL`, no --proto)
	# fetch. The <spc-filename> column is load-bearing (_php-pins.sh's own
	# header): spc locates a pre-downloaded archive by exactly that name.
	local row filename url
	for row in "${PHP_PINS_LIBS[@]}"; do
		read -r _ _ _ filename url <<<"$row"
		bp_download "$url" "$BUILD_DOWNLOADS/$filename"
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

	local row sha256 filename
	for row in "${PHP_PINS_LIBS[@]}"; do
		read -r _ _ sha256 filename _ <<<"$row"
		bp_verify_sha256 "$BUILD_DOWNLOADS/$filename" "$sha256"
	done
}

recipe_extract() {
	bp_die "not yet implemented (task 2c)"
}

recipe_configure() {
	bp_die "not yet implemented (task 2c)"
}

recipe_build() {
	bp_die "not yet implemented (task 2c)"
}

recipe_install() {
	bp_die "not yet implemented (task 2c)"
}

recipe_normalize() {
	bp_die "not yet implemented (task 2c)"
}

recipe_serve_probe() {
	bp_die "not yet implemented (task 2c)"
}
