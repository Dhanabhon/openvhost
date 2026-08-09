#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# build.sh — the one driver behind every OpenVHost package (D7).
#
# The sequence is fixed and lives here. A recipe supplies only the parts that
# are genuinely package-specific; it never gets to reorder or skip a stage. If a
# package cannot be built without changing this file, that is a finding to
# report, not a change to make quietly (D7).
#
#   fetch → verify → extract → configure → build → install → normalize
#         → sign → audit → pack → verify-artifact → manifest
#
# Two deliberate departures from the sequence as written in the design, both
# recorded rather than assumed:
#
#   * sign runs BEFORE audit. Contract check 3 is "every Mach-O is signed and
#     codesign --verify passes", which cannot hold on an unsigned staging tree;
#     and D4 requires signing to come after the last Mach-O modification, which
#     normalize is. Auditing first would mean either a failing gate on every
#     build or a gate with check 3 switched off — and a gate with an off switch
#     is not a gate.
#   * verify-artifact re-runs the whole contract against the packed tarball,
#     which is the thing users actually receive. Auditing only the staging tree
#     leaves tar itself unverified.
#
# See build/recipes/README.md for the recipe interface.
#
# Requires bash 3.2 (what macOS ships).

set -euo pipefail

BUILD_SELF="${BASH_SOURCE[0]}"
BUILD_DIR="$(cd -- "$(dirname -- "$BUILD_SELF")" && pwd -P)"

usage() {
	cat <<'EOF'
Usage: build/build.sh [options] <name> <version>

Builds one package from build/recipes/<name>.sh into a relocatable, signed,
audited macOS tarball.

Options
  --out <dir>       where the tarball, its .sha256 and the manifest land
                    (default: build/out, which .gitignore excludes)
  --recipe <path>   use this recipe file instead of build/recipes/<name>.sh
  --stage-only      stop after normalize. For a build input (static OpenSSL,
                    say) that is consumed by another recipe and never shipped:
                    the artifact contract describes products, not inputs.
  --from <stage>    resume at a stage. A development aid for iterating on a
                    recipe; it is recorded in the manifest, because an artifact
                    built from partly stale state must not look like a clean one
  --jobs <n>        parallelism offered to the recipe (default: hw.ncpu)
  --keep-work       keep the work tree (sources, objects) after a successful run
  -h, --help        this text

Environment
  OPENVHOST_BUILD_ROOT   the build root (default /opt/openvhost-build). D8: the
                         install prefix is deliberately meaningless, AND no
                         ancestor of it may be world-writable — a package
                         embeds its prefix, and a prefix an unprivileged
                         process can create is a prefix it can plant a plugin
                         in. The driver refuses to run otherwise.

Preparing a build machine
  The build root must live where unprivileged code cannot create it, which is
  the same thing as saying you need one privileged mkdir, once:

    sudo mkdir -p /opt/openvhost-build && sudo chown "$(id -u):$(id -g)" /opt/openvhost-build

Exit status
  0  the artifact was produced and passes the contract
  1  a stage failed, or the artifact does not satisfy the contract
  2  the build could not start (bad usage, missing recipe, missing tool)
EOF
}

# ---------------------------------------------------------------- reporting --

BP_STAGE=""
bp_log() { printf '==> [%s] %s\n' "${BP_STAGE:-driver}" "$*"; }
bp_die() {
	printf 'build: %s\n' "$*" >&2
	exit "${2:-1}"
}
bp_usage_die() {
	printf 'build: %s\n' "$*" >&2
	exit 2
}

# ------------------------------------------------------------------ options --

OUT_DIR=""
RECIPE_FILE=""
STAGE_ONLY=0
FROM_STAGE=""
BUILD_JOBS=""
KEEP_WORK=0
BUILD_NAME=""
BUILD_VERSION=""

while [ $# -gt 0 ]; do
	case "$1" in
	--out)
		[ $# -ge 2 ] || bp_usage_die "--out needs a value"
		OUT_DIR="$2"
		shift 2
		;;
	--recipe)
		[ $# -ge 2 ] || bp_usage_die "--recipe needs a value"
		RECIPE_FILE="$2"
		shift 2
		;;
	--stage-only)
		STAGE_ONLY=1
		shift
		;;
	--from)
		[ $# -ge 2 ] || bp_usage_die "--from needs a value"
		FROM_STAGE="$2"
		shift 2
		;;
	--jobs)
		[ $# -ge 2 ] || bp_usage_die "--jobs needs a value"
		BUILD_JOBS="$2"
		shift 2
		;;
	--keep-work)
		KEEP_WORK=1
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	-*) bp_usage_die "unknown option: $1 (try --help)" ;;
	*)
		if [ -z "$BUILD_NAME" ]; then
			BUILD_NAME="$1"
		elif [ -z "$BUILD_VERSION" ]; then
			BUILD_VERSION="$1"
		else
			bp_usage_die "unexpected argument: $1"
		fi
		shift
		;;
	esac
done

if [ -z "$BUILD_NAME" ] || [ -z "$BUILD_VERSION" ]; then
	usage >&2
	exit 2
fi

# Both values become path components under the build root and the output
# directory, so they are validated rather than trusted. A leading underscore is
# reserved for the driver's own directories (_work, _audit).
case "$BUILD_NAME" in
[a-z0-9]*[!a-z0-9._+-]* | [!a-z0-9]*) bp_usage_die "invalid package name: $BUILD_NAME" ;;
esac
case "$BUILD_VERSION" in
[0-9]*[!a-zA-Z0-9._+-]* | [!0-9]*) bp_usage_die "invalid version: $BUILD_VERSION" ;;
esac

# -------------------------------------------------------------- environment --

BUILD_ROOT="${OPENVHOST_BUILD_ROOT:-/opt/openvhost-build}"
case "$BUILD_ROOT" in
/?*) ;;
*) bp_usage_die "OPENVHOST_BUILD_ROOT must be an absolute path: $BUILD_ROOT" ;;
esac
BUILD_ROOT="${BUILD_ROOT%/}"

# The deepest ancestor of $1 that exists, resolved physically. Everything below
# it does not exist yet, so who is allowed to create those components is decided
# by this directory's mode — which is the question every caller here is asking.
# Resolving matters because /tmp is a symlink to /private/tmp and `stat` reads
# the link's own mode, which is not the directory's.
bp_deepest_existing() {
	local p="$1"
	while [ ! -d "$p" ]; do p="$(dirname -- "$p")"; done
	(cd -- "$p" && pwd -P)
}

bp_mode() { stat -L -f '%OLp' -- "$1" 2>/dev/null || true; }

bp_is_world_writable() {
	case "$(bp_mode "$1")" in
	'') return 1 ;;
	*[2367]) return 0 ;;
	esac
	return 1
}

# Takes the root to SUGGEST, which is not always the root that was asked for: a
# root with a world-writable ancestor cannot be fixed by creating it, so telling
# the operator to mkdir the very path just rejected would be advice that does not
# work.
bp_build_root_help() {
	printf 'build: prepare a root whose every ancestor is root-owned, once:\n' >&2
	printf 'build:   sudo mkdir -p %s && sudo chown %s:%s %s\n' \
		"$1" "$(id -u)" "$(id -g)" "$1" >&2
}

# D8, corrected. A neutral prefix is NOT an inert one. Fifty-odd files in a
# finished tree embed the install prefix, and mariadbd resolves basedir,
# plugin-dir and character-sets-dir from it — so on a user's machine, where that
# tree does not exist, the prefix is a name something will follow. If any
# ancestor of it is world-writable (/tmp is mode 1777) then any unprivileged
# local process can create the tree we named and plant a plugin dylib, a charset
# Index.xml or an option file for the server to load: CWE-426 / CWE-427.
#
# So the prefix must be un-plantable, not merely meaningless, and that is
# enforced here rather than assumed. The corollary is not a wart to work around:
# a directory unprivileged code cannot create is a directory unprivileged code
# cannot create, so preparing a build machine costs one privileged mkdir, once.
bp_assert_unplantable() {
	local root="$1" node
	node="$(bp_deepest_existing "$root")"
	while :; do
		if bp_is_world_writable "$node"; then
			printf 'build: refusing to build under %s\n' "$root" >&2
			printf 'build: its ancestor %s is world-writable (mode %s), so on a machine\n' \
				"$node" "$(bp_mode "$node")" >&2
			printf 'build: where this tree does not exist anything could create the path the\n' >&2
			printf 'build: package embeds and plant a plugin, a charset index or an option file.\n' >&2
			bp_build_root_help /opt/openvhost-build
			exit 1
		fi
		[ "$node" != "/" ] || break
		node="$(dirname -- "$node")"
	done
}

bp_assert_unplantable "$BUILD_ROOT"

# Owner and mode are verified, not hoped for: a root somebody else owns, or one
# left group- or world-readable by an earlier run, is not a root this build may
# treat as its own.
if [ ! -d "$BUILD_ROOT" ]; then
	if ! mkdir -p -- "$BUILD_ROOT" 2>/dev/null; then
		printf 'build: cannot create the build root: %s\n' "$BUILD_ROOT" >&2
		bp_build_root_help "$BUILD_ROOT"
		exit 2
	fi
fi
chmod 0700 -- "$BUILD_ROOT" 2>/dev/null || true
bp_root_owner="$(stat -L -f '%u' -- "$BUILD_ROOT" 2>/dev/null || true)"
[ "$bp_root_owner" = "$(id -u)" ] ||
	bp_die "the build root is owned by uid ${bp_root_owner:-?}, not $(id -u): $BUILD_ROOT"
bp_root_mode="$(bp_mode "$BUILD_ROOT")"
[ "$bp_root_mode" = "700" ] ||
	bp_die "the build root is mode ${bp_root_mode:-?}, not 700: $BUILD_ROOT"

BUILD_PREFIX="$BUILD_ROOT/$BUILD_NAME-$BUILD_VERSION"
BUILD_WORK="$BUILD_ROOT/_work/$BUILD_NAME-$BUILD_VERSION"
BUILD_DOWNLOADS="$BUILD_WORK/downloads"
BUILD_SRC="$BUILD_WORK/src"
BUILD_OBJ="$BUILD_WORK/obj"
BUILD_ARCH="$(uname -m)"

if [ -z "$OUT_DIR" ]; then OUT_DIR="$BUILD_DIR/out"; fi
case "$OUT_DIR" in
/*) ;;
*) OUT_DIR="$PWD/$OUT_DIR" ;;
esac
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd -- "$OUT_DIR" && pwd -P)"

if [ -z "$BUILD_JOBS" ]; then BUILD_JOBS="$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"; fi
case "$BUILD_JOBS" in
'' | *[!0-9]*) bp_usage_die "--jobs must be a positive integer" ;;
esac

# Everything the driver writes goes to one of exactly three places, and nothing
# below ever writes anywhere else: the neutral build root, the output directory,
# and (read-only) the repo. Nothing here touches ~/.openvhost, a datadir, or
# Homebrew.
#
# Lexical containment is not containment. /tmp/openvhost-build/../../../etc
# begins with the root and names none of it, so `..` is refused outright rather
# than reasoned about; and because a component of the root may be a symlink
# (/tmp is one), both ends are then resolved physically and compared again. The
# comment above is a guarantee, so both halves have to hold, not just the cheap
# one.
bp_assert_under() {
	local path="$1" root="$2" what="$3" real_path real_root
	case "$path" in
	/*) ;;
	*) bp_die "refusing to $what a relative path: $path" ;;
	esac
	case "/$path/" in
	*/../*) bp_die "refusing to $what a path containing '..': $path" ;;
	esac
	case "$path" in
	"$root" | "$root"/*) ;;
	*) bp_die "refusing to $what outside $root: $path" ;;
	esac
	real_root="$(bp_deepest_existing "$root")"
	real_path="$(bp_deepest_existing "$path")"
	case "$real_path" in
	"$real_root" | "$real_root"/*) return 0 ;;
	esac
	bp_die "refusing to $what outside $root: $path resolves to $real_path"
}

# rm -rf is never handed an unvalidated variable: a path must be absolute, deep
# enough not to be a system directory, and inside the build root.
bp_rm_tree() {
	local path="$1"
	[ -n "$path" ] || bp_die "bp_rm_tree called with an empty path"
	case "$path" in
	/*) ;;
	*) bp_die "bp_rm_tree needs an absolute path: $path" ;;
	esac
	case "$path" in
	*/*/*) ;;
	*) bp_die "bp_rm_tree refuses a shallow path: $path" ;;
	esac
	bp_assert_under "$path" "$BUILD_ROOT" "remove"
	rm -rf -- "$path"
}

# ------------------------------------------------------------ recipe loading --

if [ -z "$RECIPE_FILE" ]; then RECIPE_FILE="$BUILD_DIR/recipes/$BUILD_NAME.sh"; fi
[ -f "$RECIPE_FILE" ] || bp_usage_die "no such recipe: $RECIPE_FILE"
# Absolute, because audit.sh is handed this path from a different directory and
# resolves a bare name against build/recipes/ instead.
RECIPE_FILE="$(cd -- "$(dirname -- "$RECIPE_FILE")" && pwd -P)/$(basename -- "$RECIPE_FILE")"

# Values a recipe may read while it is being sourced.
RECIPE_NAME="$BUILD_NAME"
RECIPE_VERSION="$BUILD_VERSION"

# Optional recipe declarations, defaulted here so a recipe only states what it
# actually needs. Everything else is required and is checked below.
RECIPE_BUILD_TOOLS=()
RECIPE_DEPENDS=()
# CMAKE_IGNORE_PREFIX_PATH matches a prefix EXACTLY; it does not ignore
# descendants. Listing /Applications/ServBay therefore does nothing about
# /Applications/ServBay/package/common — which is the directory that actually
# holds their headers and libraries, and one of the two routes by which the host
# leaked into the reference build (spec §2). A default that names the parent and
# misses the child reads like a defence and is not one, so both are listed.
RECIPE_IGNORE_PREFIXES=(
	/opt/homebrew
	/usr/local
	/Applications/ServBay
	/Applications/ServBay/package/common
)
RECIPE_SIGNING_KEY_FPR=""
RECIPE_SIGNING_KEY_EXPIRY=""
RECIPE_SIGNING_KEY_VERIFIED_ON=""
RECIPE_UPSTREAM_RELEASE_DATE=""
RECIPE_LAST_CHECKED=""
RECIPE_SOURCE_URL=""
RECIPE_SOURCE_SHA256=""
RECIPE_CONFIGURE_FLAGS=()
# Every file this recipe is assembled from, digested into the manifest's
# "pipeline" block. Defaulted to the entry file and APPENDED to by a recipe that
# sources another one: php.sh sources _php-pins.sh, where all 41 of its pins
# live, so a digest over the entry file alone would name none of them — the
# single most important thing this block records. Absolute paths only; the
# default already is one, and a relative path would resolve against whatever
# directory the manifest stage happens to be standing in.
RECIPE_SOURCE_FILES=("$RECIPE_FILE")

# A recipe is sourced, not executed: at source time it may only set variables
# and define functions. Anything it runs would run before the environment has
# been made hermetic.
# shellcheck source=/dev/null
. "$RECIPE_FILE"

for var in RECIPE_SOURCE_URL RECIPE_SOURCE_SHA256; do
	eval "value=\${$var:-}"
	[ -n "$value" ] || bp_die "recipe $RECIPE_FILE does not set $var"
done
for fn in recipe_fetch recipe_verify_source recipe_extract recipe_configure \
	recipe_build recipe_install; do
	[ "$(type -t "$fn" 2>/dev/null || true)" = "function" ] ||
		bp_die "recipe $RECIPE_FILE does not define $fn()"
done
if [ "$(type -t recipe_normalize 2>/dev/null || true)" != "function" ]; then
	recipe_normalize() { :; }
fi
# A recipe that ASSIGNS RECIPE_SOURCE_FILES instead of appending to it drops the
# entry file, and the manifest then records a set of inputs missing the one file
# that is certainly an input. Nothing downstream could tell: the block would be
# present, well-formed and short by one. Checked rather than left to the
# convention the line above states.
recipe_source_seen=0
for src in ${RECIPE_SOURCE_FILES[@]+"${RECIPE_SOURCE_FILES[@]}"}; do
	if [ "$src" = "$RECIPE_FILE" ]; then recipe_source_seen=1; fi
done
[ "$recipe_source_seen" -eq 1 ] ||
	bp_die "recipe $RECIPE_FILE removed itself from RECIPE_SOURCE_FILES: append to that array, never assign it"

# ---------------------------------------------------------- recipe helpers ---

# Absolute path of a build tool, resolved before PATH was scrubbed. A recipe
# must call tools through this and never by bare name (D2: ServBay's bison was
# on PATH, could not run at all, and broke the reference build).
bp_tool() {
	local key value
	key="$(printf '%s' "$1" | tr -c 'A-Za-z0-9_' '_')"
	eval "value=\${BP_TOOL_$key:-}"
	[ -n "$value" ] || bp_die "build tool '$1' was not declared in RECIPE_BUILD_TOOLS"
	printf '%s\n' "$value"
}

# Where a dependency built with --stage-only put its tree.
bp_dep_prefix() { printf '%s/%s-%s\n' "$BUILD_ROOT" "$1" "$2"; }

# The prefixes a configure step must be told to ignore (D2), already joined for
# -DCMAKE_IGNORE_PREFIX_PATH. Assembled here so every recipe ignores the same
# set, and so adding a fourth package manager is one edit rather than N.
bp_ignore_prefix_path() {
	local joined="" p
	for p in ${RECIPE_IGNORE_PREFIXES[@]+"${RECIPE_IGNORE_PREFIXES[@]}"}; do
		if [ -z "$joined" ]; then joined="$p"; else joined="$joined;$p"; fi
	done
	printf '%s\n' "$joined"
}

bp_download() {
	local url="$1" dest="$2"
	bp_assert_under "$dest" "$BUILD_ROOT" "download to"
	if [ -f "$dest" ]; then
		bp_log "already fetched: $(basename -- "$dest")"
		return 0
	fi
	mkdir -p "$(dirname -- "$dest")"
	bp_log "fetching $url"
	# --location without --proto/--proto-redir is a downgrade waiting to happen:
	# one 302 to http:// and a signature-verified source arrives over a channel
	# anyone on the path can rewrite. Both are needed — --proto pins the request
	# we make, --proto-redir pins the ones curl is told to make next.
	curl --fail --location --silent --show-error \
		--proto '=https' --proto-redir '=https' \
		--connect-timeout 30 --speed-time 60 --speed-limit 1024 \
		--output "$dest.part" -- "$url"
	mv -- "$dest.part" "$dest"
}

bp_verify_sha256() {
	local file="$1" want="$2" got
	got="$(shasum -a 256 -- "$file" | cut -d' ' -f1)"
	[ "$got" = "$want" ] ||
		bp_die "sha256 mismatch for $file: expected $want, got $got"
	bp_log "sha256 verified: $(basename -- "$file")"
}

# --------------------------------------------------- GPG import-and-verify ---
#
# Written three times before this extraction (openssl.sh, mariadb.sh,
# nginx.sh), identical down to the awk that reads the primary fingerprint out
# of `--show-keys --with-colons`. Package-agnostic from the start; only now
# shared.
#
# `gpg --verify` exits 0 on an EXPIRED signing key — measured 2026-08-02
# against OpenSSL's, whose keyserver copy had lapsed — so
# bp_gpg_verify_signature never trusts that exit status; --status-fd is
# parsed instead. The fingerprint compared is the LAST field of VALIDSIG (the
# signature's PRIMARY key), never the first (whichever subkey actually made
# the signature): MariaDB and nginx both sign directly with their primary
# key, but PCRE2 — a dependency of both — signs with a subkey, so the primary
# is the only stable thing to pin.
#
# Key material may come from any host, because the fingerprint is the trust
# anchor and a substituted key cannot produce it — but freshness (expiry,
# revocation) only travels with the key, which is why bp_gpg_import_key never
# reuses a candidate from a previous run. The keyring itself lives in one
# fresh, mode-700 homedir per build, under $BUILD_WORK, and bp_gpg always
# passes --homedir explicitly so no ambient GNUPGHOME or ~/.gnupg/gpg.conf can
# steer it.

bp_gnupg_home() { printf '%s/gnupg\n' "$BUILD_WORK"; }

bp_gpg_init_home() {
	local home
	home="$(bp_gnupg_home)"
	bp_rm_tree "$home"
	mkdir -p "$home"
	chmod 700 "$home"
}

bp_gpg() {
	"$(bp_tool gpg)" --batch --no-tty --quiet --homedir "$(bp_gnupg_home)" "$@"
}

# Import the fetched file once it is confirmed to contain a PRIMARY key with
# fingerprint <fpr>, from the URL(s) given after <label>. `gpg --import` does
# not filter — every key in the file lands in the keyring, not just the one
# matching <fpr> — but that is harmless: verification insists on the same
# fingerprint, so an extra key riding along in the file could not vouch for
# anything, and there is no reason to keep one out of the keyring either.
#
# <label> distinguishes concurrent imports into the same keyring (a recipe
# verifying its own release plus a dependency's, say PCRE2's) in the
# downloaded file's name; a recipe importing only one key may pass "".
bp_gpg_import_key() {
	local fpr="$1" label="$2"
	shift 2
	local index=0 url dest imported=0 primary
	for url in "$@"; do
		index=$((index + 1))
		dest="$BUILD_DOWNLOADS/signing-key${label:+-$label}-$index.asc"
		rm -f -- "$dest"
		# Never reused from a previous run: a stale mirror copy is precisely the
		# failure mode this list exists to route around.
		if ! bp_download "$url" "$dest" >/dev/null 2>&1; then
			bp_log "signing key not available from $url"
			continue
		fi
		primary="$(bp_gpg --show-keys --with-colons "$dest" 2>/dev/null |
			awk -F: '$1 == "pub" { want = 1; next } $1 == "fpr" && want { print $10; want = 0 }' |
			grep -Fx "$fpr" || true)"
		if [ -z "$primary" ]; then
			bp_log "ignoring key from $url: no primary key with fingerprint $fpr"
			continue
		fi
		bp_gpg --import "$dest" >/dev/null 2>&1 || continue
		imported=$((imported + 1))
		bp_log "imported signing key $fpr from $url"
	done
	[ "$imported" -gt 0 ] ||
		bp_die "no host served a key with fingerprint $fpr; cannot verify provenance"
}

# Insist on a good signature over <file> by the primary key <fpr>. See the
# block comment above for why the exit status is ignored and why the
# fingerprint compared is VALIDSIG's last field rather than its first.
bp_gpg_verify_signature() {
	local file="$1" sig="$2" fpr="$3" what status errors bad
	what="$(basename -- "$file")"
	status="$BUILD_WORK/gpg-status-$what.txt"
	errors="$BUILD_WORK/gpg-stderr-$what.txt"

	bp_gpg --status-fd 1 --verify "$sig" "$file" >"$status" 2>"$errors" || true

	awk -v fpr="$fpr" \
		'$1 == "[GNUPG:]" && $2 == "VALIDSIG" && $NF == fpr { found = 1 }
		 END { exit found ? 0 : 1 }' "$status" ||
		bp_die "no valid signature by $fpr over $what; gpg said: $(tr '\n' ' ' <"$errors")"
	for bad in EXPKEYSIG REVKEYSIG BADSIG ERRSIG EXPSIG; do
		# An `if`, not `grep ... && bp_die`: under set -e the AND-list's failure
		# becomes the loop's exit status, and a loop that "fails" because nothing
		# was wrong would abort the build on the happy path.
		if grep -q "^\[GNUPG:\] $bad " "$status"; then
			bp_die "signature over $what is $bad; refusing to build from it"
		fi
	done
	bp_log "GPG: good signature by $fpr over $what"
}

# Record a configure flag so it reaches the build manifest (§7). Intent that is
# not recorded is not auditable.
bp_record_flags() {
	local flag
	for flag in "$@"; do
		RECIPE_CONFIGURE_FLAGS[${#RECIPE_CONFIGURE_FLAGS[@]}]="$flag"
	done
}

bp_machos() {
	local tree="$1"
	find "$tree" -type f -print0 |
		xargs -0 file -h -F $'\001' -- 2>/dev/null |
		grep -F 'Mach-O' |
		cut -d$'\001' -f1 || true
}

# --------------------------------------------------------------- dependencies --

# Dependencies are built first and with the caller's PATH intact, because their
# own driver run has to resolve cmake, make and friends before it scrubs.
BP_HOST_PATH="$PATH"
for dep in ${RECIPE_DEPENDS[@]+"${RECIPE_DEPENDS[@]}"}; do
	dep_name="${dep%%:*}"
	dep_version="${dep#*:}"
	[ -n "$dep_name" ] && [ "$dep_name" != "$dep" ] ||
		bp_die "RECIPE_DEPENDS entry must be name:version, got: $dep"
	dep_prefix="$(bp_dep_prefix "$dep_name" "$dep_version")"
	if [ -d "$dep_prefix" ]; then
		# Existence is the whole test, and it cannot tell one build of a version
		# from another — which is how a rebuilt OpenSSL silently changed what
		# nginx linked against. Reusing a prefix is still the right default here;
		# what was missing is that nothing recorded WHICH build got reused, so
		# stage_manifest now digests this prefix either way.
		bp_log "dependency already built: $dep_name $dep_version"
		continue
	fi
	bp_log "building dependency $dep_name $dep_version"
	PATH="$BP_HOST_PATH" OPENVHOST_BUILD_ROOT="$BUILD_ROOT" \
		"$BUILD_SELF" --stage-only --jobs "$BUILD_JOBS" "$dep_name" "$dep_version"
done

# ------------------------------------------------- tool resolution, then scrub --

for tool in ${RECIPE_BUILD_TOOLS[@]+"${RECIPE_BUILD_TOOLS[@]}"}; do
	path="$(command -v "$tool" 2>/dev/null || true)"
	[ -n "$path" ] || bp_die "required build tool not on PATH: $tool"
	case "$path" in
	/*) ;;
	*) bp_die "build tool '$tool' resolved to a shell builtin or function, not a binary" ;;
	esac
	key="$(printf '%s' "$tool" | tr -c 'A-Za-z0-9_' '_')"
	eval "BP_TOOL_$key=\$path"
	eval "export BP_TOOL_$key"
	bp_log "tool $tool -> $path"
done

# D2. Scrubbing is necessary and not sufficient — the audit is the gate — but it
# removes the routes leakage took on 2026-08-02: two package managers at once,
# through include and library search paths nobody passed deliberately.
PATH="/usr/bin:/bin:/usr/sbin:/sbin"
export PATH
unset PKG_CONFIG_PATH CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH OBJC_INCLUDE_PATH \
	LIBRARY_PATH LD_LIBRARY_PATH DYLD_LIBRARY_PATH DYLD_FALLBACK_LIBRARY_PATH \
	LDFLAGS CPPFLAGS CFLAGS CXXFLAGS OBJCFLAGS \
	CMAKE_PREFIX_PATH CMAKE_FRAMEWORK_PATH CMAKE_INCLUDE_PATH CMAKE_LIBRARY_PATH \
	ACLOCAL_PATH M4PATH PERL5LIB PYTHONPATH || true
LC_ALL=C
LANG=C
export LC_ALL LANG
export BUILD_NAME BUILD_VERSION BUILD_ROOT BUILD_PREFIX BUILD_WORK \
	BUILD_DOWNLOADS BUILD_SRC BUILD_OBJ BUILD_ARCH BUILD_JOBS OUT_DIR \
	RECIPE_NAME RECIPE_VERSION

# ------------------------------------------------------------------- stages ---

STAGES="fetch verify extract configure build install normalize sign audit pack verify-artifact manifest"
if [ "$STAGE_ONLY" -eq 1 ]; then STAGES="fetch verify extract configure build install normalize"; fi

if [ -n "$FROM_STAGE" ]; then
	found=0
	for s in $STAGES; do
		if [ "$s" = "$FROM_STAGE" ]; then found=1; fi
	done
	[ "$found" -eq 1 ] || bp_usage_die "--from: no such stage in this run: $FROM_STAGE"
fi

should_run() {
	if [ -z "$FROM_STAGE" ]; then return 0; fi
	if [ "$1" = "$FROM_STAGE" ]; then
		FROM_STAGE=""
		return 0
	fi
	return 1
}

# Whether THIS invocation ran the build stage — the one that compiles and links
# the artifact's own binaries. It is the difference between a manifest that
# reports a dependency digest and one that admits it cannot: see
# json_dependencies. A run
# resumed past `build` (--from pack, say) repacks bytes some earlier run
# produced, so the dependency prefixes sitting on disk now are not necessarily
# the ones those bytes were built against, however confidently they hash.
BUILD_STAGE_RAN=0

# Named up front rather than inside stage_pack, so that --from verify-artifact
# and --from manifest still know what they are talking about.
TARBALL="$OUT_DIR/$BUILD_NAME-$BUILD_VERSION-macos-$BUILD_ARCH.tar.gz"
TARBALL_SHA=""

stage_fetch() {
	mkdir -p "$BUILD_DOWNLOADS"
	recipe_fetch
}

stage_verify() { recipe_verify_source; }

stage_extract() {
	bp_rm_tree "$BUILD_SRC"
	mkdir -p "$BUILD_SRC"
	recipe_extract
}

stage_configure() {
	mkdir -p "$BUILD_OBJ"
	recipe_configure
}

stage_build() {
	recipe_build
	# Recorded here rather than in the dispatch below, so the fact lives next to
	# the thing it is a fact about. After recipe_build, not before: what this
	# licenses json_dependencies to claim is that this run COMPLETED producing
	# the artifact's bytes against the prefixes on disk. (Under set -e a failed
	# build never reaches the manifest anyway, so the ordering costs nothing and
	# cannot mislead.)
	BUILD_STAGE_RAN=1
}

stage_install() {
	# The staged tree IS the neutral prefix (D8). Installing elsewhere and moving
	# would put the mover's path into the 50-odd files that embed the prefix.
	bp_rm_tree "$BUILD_PREFIX"
	mkdir -p "$BUILD_PREFIX"
	recipe_install
}

stage_normalize() { recipe_normalize; }

stage_sign() {
	# D4, and it is structural: Apple Silicon will not execute unsigned code, and
	# every install_name_tool edit invalidates a signature — so this is last, and
	# nothing may modify a Mach-O after it.
	local count=0 macho
	while IFS= read -r macho; do
		[ -n "$macho" ] || continue
		codesign --force --sign - --timestamp=none -- "$macho" >/dev/null 2>&1 ||
			bp_die "codesign failed for $macho"
		count=$((count + 1))
	done < <(bp_machos "$BUILD_PREFIX")
	bp_log "ad-hoc signed $count Mach-O files"
}

# --execute-artifact is passed deliberately and only here: checks 5 and 6 run the
# server binary this driver has just produced, which the driver is entitled to do
# because it built it. Someone auditing a tarball they were handed is not in that
# position, so audit.sh does not assume it.
stage_audit() {
	"$BUILD_DIR/audit.sh" --recipe "$RECIPE_FILE" --version "$BUILD_VERSION" \
		--execute-artifact "$BUILD_PREFIX"
}

stage_pack() {
	bp_assert_under "$TARBALL" "$OUT_DIR" "write"
	rm -f -- "$TARBALL" "$TARBALL.sha256"
	# COPYFILE_DISABLE keeps bsdtar from adding ._AppleDouble members for
	# extended attributes. An ad-hoc signature lives inside the Mach-O, not in an
	# xattr, so nothing of consequence is dropped — and verify-artifact re-checks
	# every signature after the round trip rather than taking that on trust.
	#
	# gzip:!timestamp is what makes this stage REPRODUCIBLE, and it was measured
	# before it was written: packing one staged prefix twice used to produce two
	# different tarballs, but `gunzip -c` on both gave the SAME raw tar — same
	# entries, same modes, same mtimes. The whole difference was the four-byte
	# MTIME field gzip writes into its header from the clock. Everything this
	# pipeline does up to and including tar was already deterministic; four bytes
	# undid it.
	#
	# The option NAMES THE INTENT, which is why it is preferred over piping to
	# `gzip -n`: `-n` means "no name", and suppressing the timestamp is a side
	# effect of the same flag that a reader has to already know about to see why
	# it is there. Both were measured on this toolchain (bsdtar 3.5.3 /
	# libarchive 3.7.4) and both produce identical bytes across runs, so this is
	# a choice about legibility, not correctness. bsdtar-only syntax is fine
	# here: this pipeline is macOS-only by construction (codesign, otool, the
	# artifact contract's Mach-O checks).
	#
	# What this buys, stated exactly: from a GIVEN STAGED PREFIX, pack produces
	# identical bytes every time. It does NOT claim a full build from source
	# reproduces — that can fail for reasons far below gzip, and nothing here
	# has measured it.
	COPYFILE_DISABLE=1 tar --options gzip:'!timestamp' -czf "$TARBALL" \
		-C "$BUILD_ROOT" "$BUILD_NAME-$BUILD_VERSION"
	TARBALL_SHA="$(shasum -a 256 -- "$TARBALL" | cut -d' ' -f1)"
	printf '%s  %s\n' "$TARBALL_SHA" "$(basename -- "$TARBALL")" >"$TARBALL.sha256"
	bp_log "packed $(basename -- "$TARBALL") ($TARBALL_SHA)"
}

stage_verify_artifact() {
	"$BUILD_DIR/audit.sh" --recipe "$RECIPE_FILE" --version "$BUILD_VERSION" \
		--execute-artifact "$TARBALL"
}

# A manifest that is not valid JSON is a manifest nothing reads, so every
# character JSON forbids raw in a string is escaped here rather than left to a
# caller to remember. `tr` drops the control characters that have no short
# escape; the rest — backslash, quote, tab, CR and LF — are escaped. LF matters
# most: a tool version line is one `head -n 1` away from being multi-line, and
# the old code passed both LF and CR through untouched.
# sed escapes each line; awk joins them. NOT `sed -e :a -e N -e '$!ba'` to slurp
# first: on macOS's sed, `N` with no next line quits WITHOUT printing, so every
# single-line value — which is nearly all of them — came out EMPTY. That wrote a
# manifest of empty strings, and it was still valid JSON, so nothing downstream
# would have complained.
json_string() {
	printf '%s' "$1" |
		LC_ALL=C tr -d '\000-\010\013\014\016-\037' |
		LC_ALL=C sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' \
			-e "s/$(printf '\t')/\\\\t/g" -e "s/$(printf '\r')/\\\\r/g" |
		LC_ALL=C awk 'NR > 1 { printf "%s", "\\n" } { printf "%s", $0 }'
}

json_array() {
	local first=1 item
	printf '['
	for item in "$@"; do
		if [ "$first" -eq 1 ]; then first=0; else printf ', '; fi
		printf '"%s"' "$(json_string "$item")"
	done
	printf ']'
}

# One value standing for the whole content of a staged dependency prefix.
#
# A dependency is satisfied by directory existence alone, and a consumer used to
# record only the dependency's VERSION — so two different builds of openssl
# 3.5.7 wrote the identical line, and rebuilding OpenSSL underneath changed what
# nginx, PHP and MariaDB all link against with no artifact anywhere carrying a
# value that differed. nginx's prefix drifted 611 bytes from its pin that way,
# with every gate green. This is the value that differs.
#
# The rule every stream below obeys, because breaking it is how this function
# was first got wrong: NO FIELD MAY CONTAIN THE BYTE THAT SEPARATES ITS OWN
# RECORDS. A field is either bounded (octal digits) or separated by NUL, which
# is the one byte a path or a symlink target cannot hold.
#
# What it covers, and it is four streams into one sha256:
#
#   1. every path under the prefix, relative to it, byte-sorted and
#      NUL-separated. Relative so the digest describes the tree and not where
#      the build root happens to be.
#   2. each of those entries' st_mode in octal — which carries the file type,
#      the permission bits and setuid/setgid/sticky — one per line, in the same
#      order as (1), which is what associates the two. Octal digits and nothing
#      else, so the newline that ends a record cannot occur inside one.
#   3. every symlink's own path AND its target, both NUL-terminated, byte-
#      sorted. A stream of its own rather than a `%SY` suffix on (2), because a
#      target is unbounded text the tree's author chooses: riding in a
#      newline-separated stream it can forge a record. That is measured, not
#      supposed. Tree A `p1 -> "a\n120755 -> b"`, `p2 -> "c"` and tree B
#      `p1 -> "a"`, `p2 -> "b\n120755 -> c"` are two materially different trees
#      that hashed to the same b145ad8eab60… while the target rode in (2) —
#      identical path sets, so stream (1) could not tell them apart either.
#      Carrying its own path makes this stream self-describing, so it needs no
#      positional agreement with (1) to be read; that agreement was the bug.
#   4. the SHA-256 of every regular file's content, in that same order. Left on
#      `shasum`'s own output format, which escapes a newline or a backslash in a
#      file name (`\<digest>  ./n\nl`) rather than emitting it raw — checked,
#      not assumed, since the rule above would otherwise fail here too.
#
# One `stat` per SYMLINK, but still one batched `stat` for the modes of
# everything else. Per-entry would be simpler and is what a first reading
# suggests, but measured on the largest real prefix here — mariadb-11.4.9,
# 17245 entries, 39 of them symlinks — per-entry `sh -c` takes 59s against 1.6s
# for this, and openssl-3.5.7 (160 entries, no symlinks at all) is unchanged at
# 0.05s. The cost now scales with symlink count, not with tree size.
#
# What it deliberately does NOT cover is time: no mtime, no atime, no ctime. A
# digest that moved every time the tree was touched or copied would record
# nothing at all, and this pipeline has already been bitten once by four bytes
# of timestamp (see stage_pack). Nor uid/gid, which say who unpacked a tree
# rather than what is in it; nor extended attributes, which COPYFILE_DISABLE=1
# keeps out of the artifact as well.
#
# `stat -f` here is BSD stat's output format — GNU stat's -f is "file system
# status" and would print something else entirely. Safe because PATH was
# scrubbed to the system directories before any stage runs, so these are the
# system tools whatever the caller had on PATH.
prefix_digest() {
	local tree="$1" link target
	(
		cd -- "$tree" || exit 1
		find . -mindepth 1 -print0 | LC_ALL=C sort -z
		find . -mindepth 1 -print0 | LC_ALL=C sort -z | xargs -0 stat -f '%Op'
		find . -mindepth 1 -type l -print0 | LC_ALL=C sort -z |
			while IFS= read -r -d '' link; do
				# `%Y.` and then drop the `.`: command substitution strips EVERY
				# trailing newline, so a target ending in one would otherwise
				# record the same bytes as the same target without it — the
				# defect above in miniature, reintroduced by the fix. The
				# sentinel makes that newline interior, and interior bytes
				# survive. `%Y` alone, not `%SY`: the ` -> ` decoration is for
				# people reading `stat`, and this stream is read by sha256.
				target="$(stat -f '%Y.' -- "$link")"
				printf '%s\0%s\0' "$link" "${target%.}"
			done
		find . -mindepth 1 -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256
	) | shasum -a 256 | cut -d' ' -f1
}

# The "dependencies" object: which build of each RECIPE_DEPENDS entry this
# artifact was made against. Computed here, centrally, and never in a recipe —
# three recipes reimplementing one digest is three chances to disagree about it,
# and this pipeline has already shipped thrice-duplicated GPG code where one
# copy compared the wrong field.
#
# Driven off RECIPE_DEPENDS rather than off what the dependency loop did, so a
# BUILD that reused an already-staged prefix records what it built against just
# as one that staged the prefix itself does. That is the case this exists for —
# reuse during a build is exactly how nginx drifted. The name:version shape is not
# re-validated: the loop above runs on every invocation, --from resumes
# included, and has already rejected anything else.
#
# A digest is only evidence when THIS run built (BUILD_STAGE_RAN). Hashing a
# prefix is possible in any run; it means something in a run that produced the
# artifact's bytes, because then the prefixes on disk ARE the ones it was built
# against. In a run resumed past `build` it means nothing — those bytes were
# produced days ago, possibly against a different build of the same version, so
# digesting today's prefix would write a precise, confident, WRONG claim. That
# is the failure this whole block exists to remove, so such a run says so
# instead. Not by leaving the field out: `resumed_from` would let a reader
# infer it, and "a reader could infer it" is the standard this pipeline keeps
# rejecting. An absent value is also ambiguous between "no dependencies" and
# "not looked at", where an empty object for a recipe with no RECIPE_DEPENDS is
# honest in both kinds of run.
#
# "Built against" and not "linked against", deliberately: php.sh's one
# RECIPE_DEPENDS entry is nginx, which nothing links and no byte of which
# reaches the artifact — it is contract check 6's FastCGI client. A sentence
# about linkage would be false there, and a provenance record that is false in
# one of its three cases is the thing this file keeps being fixed for.
#
# BUILD_STAGE_RAN is a fact about this INVOCATION, not about every byte of the
# artifact, and `--from build`/`--from configure` are where the two part
# company: they set it, but `make` is incremental, so a consumer whose link step
# decides it has nothing to do can leave the manifest naming a dependency digest
# beside bytes linked against the previous one. A complete run cannot reach that
# — it always re-runs `configure` — so it is confined to the `--from`
# development aid, and `resumed_from` sits in the same manifest as the
# disambiguator. Recorded here rather than fixed: the honest alternative is to
# refuse a digest on any resumed run at all, which would strip the field from
# precisely the runs a recipe author iterates with.
#
# Every shape this can emit, so a consumer can be written against the set rather
# than against the case it happened to see first:
#
#   {}                                             recipe with no RECIPE_DEPENDS
#   {"<name>": {version, prefix, tree_sha256: "<64 hex>"}}          observed
#   {"<name>": {version, prefix, tree_sha256: null, not_observed: "…"}}
#   {"<name>": {version, prefix, tree_sha256: null, prefix_missing: "…"}}
#
# `tree_sha256` is therefore either a 64-character hex string or `null`, never a
# word standing in for one, and `null` always arrives with exactly one sentence
# beside it saying which way the run failed to produce a digest.
json_dependencies() {
	local first=1 dep name version prefix digest reason_key reason
	# Fixed text, never interpolated, so every manifest that cannot vouch for
	# its dependencies says it the same way and the string can be grepped for.
	local unobserved='this run did not execute the build stage: this prefix is as it stands now, which is not necessarily what the artifact was built against'
	local vanished='this prefix was present when the build began and is gone now: it was removed mid-run, so there is nothing left to digest'
	printf '{'
	for dep in ${RECIPE_DEPENDS[@]+"${RECIPE_DEPENDS[@]}"}; do
		name="${dep%%:*}"
		version="${dep#*:}"
		prefix="$(bp_dep_prefix "$name" "$version")"
		if [ "$first" -eq 1 ]; then first=0; else printf ', '; fi
		# version and prefix are what the RECIPE declares, so they are true of
		# this run either way; only the digest is a claim about the artifact.
		printf '"%s": {"version": "%s", "prefix": "%s"' \
			"$(json_string "$name")" "$(json_string "$version")" \
			"$(json_string "$prefix")"
		# Both no-digest cases fall through to ONE printf below, so that `null`
		# means exactly "no digest" on every path and the differing reason rides
		# in its own key. It used to be two shapes: the vanished-prefix arm wrote
		# the STRING "unknown" into tree_sha256, which is the sentinel the arm
		# below argues against — a consumer testing `tree_sha256 is not None`
		# takes it for a digest, and two runs that both hit it record the
		# identical line and compare equal, which is the failure this whole
		# feature exists to remove, one layer down.
		if [ "$BUILD_STAGE_RAN" -eq 0 ]; then
			reason_key='not_observed'
			reason="$unobserved"
		elif [ ! -d "$prefix" ]; then
			# Only reachable if the prefix went away mid-run, since the loop
			# above built or found it before any stage ran. Recorded rather than
			# fatal: by now the artifact is packed and audited, and a manifest
			# that does not exist is worse than one that says it does not know.
			reason_key='prefix_missing'
			reason="$vanished"
		else
			# Assigned, never inlined into printf's argument list: under `set -e`
			# a failed command substitution aborts an assignment, but as an
			# argument it is printf's own success that decides, and an empty
			# digest would be written instead.
			digest="$(prefix_digest "$prefix")"
			printf ', "tree_sha256": "%s"}' "$(json_string "$digest")"
			continue
		fi
		# null rather than a sentinel string: no hex digest can equal it, and a
		# reader that expects one gets a type error instead of a false match. The
		# sentence beside it is the explicit part.
		printf ', "tree_sha256": null, "%s": "%s"}' \
			"$reason_key" "$(json_string "$reason")"
	done
	printf '}'
}

# Where the manifest names a file of this pipeline. Relative to the directory
# that holds build/ — the repository checkout in every real invocation — so two
# builds of the same bytes from two checkouts record the same path, and a
# manifest committed under build/manifests/ does not carry the builder's home
# directory. A file outside that tree (`--recipe` pointing elsewhere) is
# recorded absolute, as what it is, rather than mangled into looking local.
pipeline_path() {
	local file="$1" root
	root="$(dirname -- "$BUILD_DIR")"
	case "$file" in
	"$root"/*) printf '%s' "${file#"$root"/}" ;;
	*) printf '%s' "$file" ;;
	esac
}

# `[{"path": …, "sha256": …}, …]` over the files named, in the order named.
#
# There is no delimited stream anywhere in here, which is how it meets
# prefix_digest's rule that no field may contain the byte separating its own
# records: each path goes straight into its own JSON string with its digest
# beside it, and is never accumulated into a `path<sep>digest` list that
# something later splits. A path may hold any byte but NUL — including the
# newline that forged a collision in prefix_digest's symlink stream — so the
# cheapest way to keep one out of a separator is to have no separator.
#
# `$what` names the caller's category so a failure says which declaration is
# wrong, since one of the two lists is the recipe's and the other is not.
json_file_digests() {
	local what="$1" first=1 file digest
	shift
	printf '['
	for file in "$@"; do
		case "$file" in
		/*) ;;
		*) bp_die "$what must be an absolute path, got: $file" ;;
		esac
		# Fails the whole run rather than recording a sentinel. A block that says
		# "these are the inputs" with one entry standing in for a file it could not
		# read is the shape this pipeline keeps rejecting — see json_dependencies
		# on the string "unknown". Because json_pipeline is called into a variable
		# BEFORE stage_manifest's redirect opens the file, this abort leaves the
		# previous manifest whole instead of truncating a new one mid-write.
		[ -f "$file" ] && [ -r "$file" ] ||
			bp_die "$what is not a readable file: $file"
		digest="$(shasum -a 256 -- "$file" | cut -d' ' -f1)"
		if [ "$first" -eq 1 ]; then first=0; else printf ', '; fi
		printf '{"path": "%s", "sha256": "%s"}' \
			"$(json_string "$(pipeline_path "$file")")" "$(json_string "$digest")"
	done
	printf ']'
}

# The "pipeline" object: the digest of every file the recipe was assembled from
# ("sources", recipe-declared) and of the driver and audit gate that ran it
# ("driver"). Two lists rather than one, so which entries a recipe chose and
# which the driver added is stated rather than inferred from the paths.
#
# RECORDED, NEVER ENFORCED. Nothing compares these digests against anything —
# not here, not in audit.sh, not in the Rust catalogues — and that is the
# design's central decision rather than an omission. If you are here to add the
# comparison, read this first:
#
#   nginx.sh mixes ~30 declarable pins with ~600 lines of stage code and prose,
#   so editing a COMMENT moves its whole-file digest. An alarm that fires on
#   comment edits is one people learn to override, and an overridden alarm is
#   worse than none. This project refused a gate that could not fail (PR #68);
#   a gate that cries wolf fails the same standard from the other side.
#
# What it is for is a human reading a diff that changes a pin: the manifest of
# record beside the artifact says which bytes of which files that artifact was
# made from, so the question "was this recipe edited after these bytes were
# cut?" has an answer in committed evidence instead of in memory. The
# mechanically enforceable version — pins split into their own file, whose
# digest a catalogue test may hard-assert because it changes only when a pin
# changes — is designed and deferred (design §6, D4).
#
# So: editing a comment in a recipe changes what this records and fails no test.
# That is the property. It is not an oversight, and turning it into an alarm
# undoes the decision.
json_pipeline() {
	printf '{"driver": '
	# $BUILD_SELF may be relative to the caller's cwd; $BUILD_DIR is this file's
	# own physical directory, resolved before anything could chdir.
	json_file_digests 'a driver file' \
		"$BUILD_DIR/$(basename -- "$BUILD_SELF")" "$BUILD_DIR/audit.sh"
	printf ', "sources": '
	json_file_digests 'a RECIPE_SOURCE_FILES entry' \
		${RECIPE_SOURCE_FILES[@]+"${RECIPE_SOURCE_FILES[@]}"}
	printf '}'
}

tool_version() {
	local tool path
	tool="$1"
	path="$(bp_tool "$tool" 2>/dev/null || true)"
	[ -n "$path" ] || return 0
	printf '%s: %s' "$tool" "$("$path" --version 2>&1 | head -n 1 || true)"
}

stage_manifest() {
	# §7. Single-builder trust (D1) is only acceptable because the inputs are
	# recorded; this file is the whole of that acceptability.
	local manifest="$OUT_DIR/$BUILD_NAME-$BUILD_VERSION-macos-$BUILD_ARCH.manifest.json"
	local versions=() tool line dependencies pipeline
	bp_assert_under "$manifest" "$OUT_DIR" "write"
	if [ -z "$TARBALL_SHA" ] && [ -f "$TARBALL" ]; then
		TARBALL_SHA="$(shasum -a 256 -- "$TARBALL" | cut -d' ' -f1)"
	fi
	for tool in ${RECIPE_BUILD_TOOLS[@]+"${RECIPE_BUILD_TOOLS[@]}"}; do
		line="$(tool_version "$tool")"
		if [ -n "$line" ]; then versions[${#versions[@]}]="$line"; fi
	done
	versions[${#versions[@]}]="clang: $(clang --version 2>&1 | head -n 1 || true)"
	versions[${#versions[@]}]="macos-sdk: $(xcrun --show-sdk-version 2>/dev/null || echo unknown)"
	versions[${#versions[@]}]="macos: $(sw_vers -productVersion 2>/dev/null || echo unknown)"

	# Hashing trees and files is what can fail on its own, so it happens BEFORE
	# the redirect below opens the manifest. Inside that group an abort under
	# set -e truncates the file mid-write, which would leave a half-written
	# manifest next to a finished tarball; out here it leaves the previous one
	# untouched. Both assignments are plain, never `local x="$(...)"`, whose exit
	# status is `local`'s and would swallow the failure this ordering exists for.
	dependencies="$(json_dependencies)"
	pipeline="$(json_pipeline)"

	{
		printf '{\n'
		printf '  "name": "%s",\n' "$(json_string "$BUILD_NAME")"
		printf '  "version": "%s",\n' "$(json_string "$BUILD_VERSION")"
		printf '  "arch": "%s",\n' "$(json_string "$BUILD_ARCH")"
		printf '  "built_at": "%s",\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
		printf '  "upstream": {\n'
		printf '    "url": "%s",\n' "$(json_string "$RECIPE_SOURCE_URL")"
		printf '    "sha256": "%s",\n' "$(json_string "$RECIPE_SOURCE_SHA256")"
		printf '    "release_date": "%s",\n' "$(json_string "$RECIPE_UPSTREAM_RELEASE_DATE")"
		printf '    "last_checked": "%s",\n' "$(json_string "$RECIPE_LAST_CHECKED")"
		printf '    "signing_key_fingerprint": "%s",\n' "$(json_string "$RECIPE_SIGNING_KEY_FPR")"
		printf '    "signing_key_expiry": "%s",\n' "$(json_string "$RECIPE_SIGNING_KEY_EXPIRY")"
		printf '    "signing_key_verified_on": "%s"\n' "$(json_string "$RECIPE_SIGNING_KEY_VERIFIED_ON")"
		printf '  },\n'
		printf '  "build_prefix": "%s",\n' "$(json_string "$BUILD_PREFIX")"
		printf '  "resumed_from": "%s",\n' "$(json_string "$RESUMED_FROM")"
		printf '  "configure_flags": %s,\n' \
			"$(json_array ${RECIPE_CONFIGURE_FLAGS[@]+"${RECIPE_CONFIGURE_FLAGS[@]}"})"
		printf '  "toolchain": %s,\n' "$(json_array ${versions[@]+"${versions[@]}"})"
		printf '  "dependencies": %s,\n' "$dependencies"
		printf '  "pipeline": %s,\n' "$pipeline"
		printf '  "output": {\n'
		printf '    "file": "%s",\n' "$(json_string "$(basename -- "$TARBALL")")"
		printf '    "sha256": "%s"\n' "$(json_string "$TARBALL_SHA")"
		printf '  }'
		if [ "$(type -t recipe_manifest_extra 2>/dev/null || true)" = "function" ]; then
			printf ',\n  "recipe": '
			recipe_manifest_extra
		fi
		printf '\n}\n'
	} >"$manifest"
	bp_log "manifest written to $manifest"
}

RESUMED_FROM="${FROM_STAGE:-}"

for stage in $STAGES; do
	if ! should_run "$stage"; then
		printf '==> [%s] skipped (--from %s)\n' "$stage" "$RESUMED_FROM"
		continue
	fi
	BP_STAGE="$stage"
	# One line per stage even when the recipe is silent, so a build log shows the
	# sequence that actually ran rather than only the stages that had something
	# to say.
	bp_log "start"
	case "$stage" in
	fetch) stage_fetch ;;
	verify) stage_verify ;;
	extract) stage_extract ;;
	configure) stage_configure ;;
	build) stage_build ;;
	install) stage_install ;;
	normalize) stage_normalize ;;
	sign) stage_sign ;;
	audit) stage_audit ;;
	pack) stage_pack ;;
	verify-artifact) stage_verify_artifact ;;
	manifest) stage_manifest ;;
	*) bp_die "unknown stage: $stage" ;;
	esac
done

BP_STAGE="driver"
if [ "$KEEP_WORK" -eq 0 ]; then
	bp_rm_tree "$BUILD_WORK"
	bp_log "work tree removed; the staged prefix is kept at $BUILD_PREFIX"
fi

if [ "$STAGE_ONLY" -eq 1 ]; then
	bp_log "staged $BUILD_NAME $BUILD_VERSION at $BUILD_PREFIX (no artifact: --stage-only)"
else
	bp_log "done: $TARBALL"
fi
