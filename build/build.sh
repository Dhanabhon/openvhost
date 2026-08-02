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
  OPENVHOST_BUILD_ROOT   the neutral build root (default /tmp/openvhost-build).
                         D8: the install prefix is deliberately meaningless, so
                         that the paths every build embeds are inert.

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

BUILD_ROOT="${OPENVHOST_BUILD_ROOT:-/tmp/openvhost-build}"
case "$BUILD_ROOT" in
/?*) ;;
*) bp_usage_die "OPENVHOST_BUILD_ROOT must be an absolute path: $BUILD_ROOT" ;;
esac

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
bp_assert_under() {
	local path="$1" root="$2" what="$3"
	case "$path" in
	"$root" | "$root"/*) return 0 ;;
	esac
	bp_die "refusing to $what outside $root: $path"
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
RECIPE_IGNORE_PREFIXES=(/opt/homebrew /usr/local /Applications/ServBay)
RECIPE_SIGNING_KEY_FPR=""
RECIPE_SIGNING_KEY_EXPIRY=""
RECIPE_SIGNING_KEY_VERIFIED_ON=""
RECIPE_UPSTREAM_RELEASE_DATE=""
RECIPE_LAST_CHECKED=""
RECIPE_SOURCE_URL=""
RECIPE_SOURCE_SHA256=""
RECIPE_CONFIGURE_FLAGS=()

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
	curl --fail --location --silent --show-error \
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

stage_build() { recipe_build; }

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

stage_audit() {
	"$BUILD_DIR/audit.sh" --recipe "$RECIPE_FILE" --version "$BUILD_VERSION" \
		"$BUILD_PREFIX"
}

stage_pack() {
	bp_assert_under "$TARBALL" "$OUT_DIR" "write"
	rm -f -- "$TARBALL" "$TARBALL.sha256"
	# COPYFILE_DISABLE keeps bsdtar from adding ._AppleDouble members for
	# extended attributes. An ad-hoc signature lives inside the Mach-O, not in an
	# xattr, so nothing of consequence is dropped — and verify-artifact re-checks
	# every signature after the round trip rather than taking that on trust.
	COPYFILE_DISABLE=1 tar -czf "$TARBALL" -C "$BUILD_ROOT" "$BUILD_NAME-$BUILD_VERSION"
	TARBALL_SHA="$(shasum -a 256 -- "$TARBALL" | cut -d' ' -f1)"
	printf '%s  %s\n' "$TARBALL_SHA" "$(basename -- "$TARBALL")" >"$TARBALL.sha256"
	bp_log "packed $(basename -- "$TARBALL") ($TARBALL_SHA)"
}

stage_verify_artifact() {
	"$BUILD_DIR/audit.sh" --recipe "$RECIPE_FILE" --version "$BUILD_VERSION" \
		"$TARBALL"
}

json_string() {
	printf '%s' "$1" | LC_ALL=C sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/	/\\t/g' |
		LC_ALL=C tr -d '\000-\010\013\014\016-\037'
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
	local versions=() tool line
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
