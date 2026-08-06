#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# audit.sh — the artifact contract for an OpenVHost package.
#
# Implements the seven points of the build-pipeline design (D6, spec §8). A
# package is acceptable only if ALL of them hold:
#
#   1. layout      a single root containing the recipe's required directories
#                  (RECIPE_REQUIRED_LAYOUT, default bin share)
#   2. linkage     every otool -L entry of every Mach-O is /usr/lib/*,
#                  /System/*, @loader_path/... or @rpath/..., AND every
#                  LC_RPATH is @loader_path-relative — nothing else
#   3. signature   every Mach-O is signed and codesign --verify passes
#   4. identity    the tree carries no trace of the machine that built it
#   5. relocation  copy to path A, run; move to path B, run again
#   6. service     the recipe's own serve-and-survive probe (recipe_serve_probe)
#                  passes — what that proves is package-specific and printed
#                  by the recipe, not this script
#   7. plantable   no absolute path embedded anywhere in the tree has a
#                  world-writable ancestor
#
# Check 7 exists because check 4 asks the wrong question about the install
# prefix. Check 4 asks "does this path identify the builder"; /tmp/openvhost-build
# identifies nobody, and the tree was signed off on that basis. But the prefix is
# not decoration — mariadbd resolves basedir, plugin-dir and character-sets-dir
# from it — and /tmp is mode 1777, so on a user's machine any unprivileged
# process could create the tree the package names and plant a plugin dylib in it
# (CWE-426 / CWE-427). Neutral is not inert. Check 7 asks the question that
# actually decides it: can anything unprivileged create the paths this tree
# names?
#
# Points 5 and 6 need a server binary and a package-specific probe, AND they
# execute the artifact, so they additionally need --execute-artifact. A package
# with neither, or a run without the flag, reports them as SKIPPED with the
# reason — never silently. See build/recipes/README.md for how a recipe supplies
# them.
#
# This script is the gate, not a warning: any failed check exits non-zero. Every
# check runs even after an earlier one fails, so one invocation tells you
# everything that is wrong rather than only the first thing.
#
# Requires bash 3.2 (what macOS ships) and nothing outside /usr/bin.

set -euo pipefail

# The gate must not be at the mercy of whatever sits early on the caller's PATH.
# The reference build was broken by a ServBay binary shadowing a system one
# (spec §2), and ServBay's alias directory precedes /usr/bin on the machine this
# pipeline was written on. Everything below is a system tool, so the system
# toolchain wins; nothing is removed, so a recipe's serve probe keeps its own.
PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
export PATH

AUDIT_SELF="${BASH_SOURCE[0]}"
AUDIT_DIR="$(cd -- "$(dirname -- "$AUDIT_SELF")" && pwd -P)"
TAB=$'\t'
SOH=$'\001'

BUILD_ROOT="${OPENVHOST_BUILD_ROOT:-/opt/openvhost-build}"

usage() {
	cat <<'EOF'
Usage: build/audit.sh [options] <tree-or-tarball>

Verifies one built package tree, or one packed tarball, against the artifact
contract in docs/superpowers/specs/2026-08-02-p2-build-pipeline-design.md §8.

THIS SCRIPT EXECUTES CODE, and says so here rather than in a comment nobody
reads:

  * --recipe SOURCES the named file into this shell. Whatever it does at source
    time, this script does. It is a repo file by default; a value containing a
    slash sources that path instead.
  * --execute-artifact RUNS THE ARTIFACT: check 5 runs the server binary from
    two directories, and check 6 hands the tree to the recipe's serve probe,
    which starts a real server against it. Without the flag, both are skipped.
  * the caller's PATH is APPENDED to the system directories (a recipe's probe
    needs its own tools), so a recipe may reach binaries this script did not
    choose. Nothing is prepended: a stale entry cannot shadow /usr/bin.

Audit a tarball you were handed WITHOUT --execute-artifact. build.sh passes it
because it built the thing it is auditing.

Options
  --recipe <name>      source build/recipes/<name>.sh for RECIPE_SERVER_BIN,
                       recipe_serve_probe (checks 5 and 6) and
                       RECIPE_ALLOWED_WRITABLE_PATHS / RECIPE_INERT_PATHS
                       (check 7). A value containing a slash is a path.
  --version <version>  value of RECIPE_VERSION while the recipe is sourced
  --execute-artifact   permit checks 5 and 6, which run code from the tree
  --server-bin <path>  server binary, relative to the tree root, for checks 5
                       and 6; overrides the recipe. Empty disables both.
  --forbid <string>    add one forbidden string to check 4 (repeatable)
  --allow-writable <p> allow one embedded path prefix in check 7 (repeatable)
  --max-report <n>     cap the offending lines printed per check (default 20)
  --keep-scratch       do not delete this run's scratch directory
  -h, --help           this text

Environment
  OPENVHOST_AUDIT_FORBIDDEN  colon-separated extra forbidden strings for check
                             4 — session directories, scratchpads, staging
                             roots. Check 4 takes the builder's identity from
                             the environment; it hardcodes no machine's paths.
  OPENVHOST_BUILD_ROOT       the build root (default /opt/openvhost-build). The
                             staged tree IS the install prefix (D8), so a build
                             audited in place sits inside the one directory it
                             is expected to name, and check 4's ancestor walk
                             stops there. It is check 7, not check 4, that makes
                             that safe. Only the _work subtree — compiler debug
                             info — is exempt from check 4 outright.

Exit status
  0  every check passed, or was explicitly skipped
  1  at least one check failed
  2  the audit could not be run (bad usage, missing tree, missing tool)
EOF
}

# ---------------------------------------------------------------- reporting --

say() { printf '%s\n' "$*"; }
detail() { printf '        %s\n' "$*"; }
die() {
	printf 'audit: %s\n' "$*" >&2
	exit 2
}

CHECK_INDEX=0
CHECK_TOTAL=7
FAILED_CHECKS=""

check_start() {
	CHECK_INDEX=$((CHECK_INDEX + 1))
	printf '[%d/%d] %-22s ' "$CHECK_INDEX" "$CHECK_TOTAL" "$1"
}
check_pass() { printf 'PASS%s\n' "${1:+ — $1}"; }
check_skip() { printf 'SKIPPED (%s)\n' "$1"; }
check_fail() {
	printf 'FAIL — %s\n' "$1"
	FAILED_CHECKS="$FAILED_CHECKS $CHECK_INDEX"
}

count_lines() { wc -l <"$1" | tr -d ' '; }

# Print at most MAX_REPORT lines of a file, then say how many were withheld.
report_lines() {
	local file="$1" total shown
	total="$(count_lines "$file")"
	shown="$total"
	if [ "$total" -gt "$MAX_REPORT" ]; then shown="$MAX_REPORT"; fi
	if [ "$shown" -gt 0 ]; then
		head -n "$shown" "$file" | while IFS= read -r line; do detail "$line"; done
	fi
	if [ "$total" -gt "$shown" ]; then detail "... and $((total - shown)) more"; fi
	return 0
}

# ------------------------------------------------------------------- options --

RECIPE_NAME_ARG=""
RECIPE_VERSION="${RECIPE_VERSION:-}"
SERVER_BIN_ARG=""
SERVER_BIN_SET=0
MAX_REPORT=20
KEEP_SCRATCH=0
EXTRA_FORBIDDEN=()
EXTRA_ALLOWED=()
EXECUTE_ARTIFACT=0
TARGET=""

while [ $# -gt 0 ]; do
	case "$1" in
	--recipe)
		[ $# -ge 2 ] || die "--recipe needs a value"
		RECIPE_NAME_ARG="$2"
		shift 2
		;;
	--version)
		[ $# -ge 2 ] || die "--version needs a value"
		RECIPE_VERSION="$2"
		shift 2
		;;
	--server-bin)
		[ $# -ge 2 ] || die "--server-bin needs a value (which may be empty)"
		SERVER_BIN_ARG="$2"
		SERVER_BIN_SET=1
		shift 2
		;;
	--forbid)
		[ $# -ge 2 ] || die "--forbid needs a value"
		EXTRA_FORBIDDEN[${#EXTRA_FORBIDDEN[@]}]="$2"
		shift 2
		;;
	--allow-writable)
		[ $# -ge 2 ] || die "--allow-writable needs a value"
		EXTRA_ALLOWED[${#EXTRA_ALLOWED[@]}]="$2"
		shift 2
		;;
	--execute-artifact)
		EXECUTE_ARTIFACT=1
		shift
		;;
	--max-report)
		[ $# -ge 2 ] || die "--max-report needs a value"
		MAX_REPORT="$2"
		shift 2
		;;
	--keep-scratch)
		KEEP_SCRATCH=1
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	--)
		shift
		while [ $# -gt 0 ]; do
			[ -z "$TARGET" ] || die "expected exactly one tree or tarball"
			TARGET="$1"
			shift
		done
		;;
	-*) die "unknown option: $1 (try --help)" ;;
	*)
		[ -z "$TARGET" ] || die "expected exactly one tree or tarball, got a second: $1"
		TARGET="$1"
		shift
		;;
	esac
done

if [ -z "$TARGET" ]; then
	usage >&2
	exit 2
fi
case "$MAX_REPORT" in
'' | *[!0-9]*) die "--max-report must be a non-negative integer" ;;
esac

for tool in otool codesign file find grep tar ditto xargs; do
	command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
done

# ------------------------------------------------------------------- scratch --

# Every byte this script writes lands under here. mktemp, not $BUILD_ROOT/_audit/$$:
# a pid is guessable and the old path could be pre-created — with a symlink where
# a directory was expected — by anything that could write the parent. mktemp
# picks a name nothing can predict and creates it 0700, so neither trick works
# even when TMPDIR is unset and this falls back to a world-writable /tmp.
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/openvhost-audit.XXXXXXXX")" ||
	die "could not create a scratch directory"

# shellcheck disable=SC2329  # invoked by the EXIT trap installed below
cleanup() {
	local status=$?
	if [ "$KEEP_SCRATCH" -eq 1 ]; then
		say "scratch kept at $SCRATCH"
	else
		# Never rm -rf an unvalidated variable. This path came from mktemp
		# with a fixed template, and the shape is re-checked here rather
		# than trusted.
		case "$SCRATCH" in
		/?*/openvhost-audit.????????) rm -rf -- "$SCRATCH" ;;
		*) printf 'audit: refusing to remove suspicious scratch path: %s\n' "$SCRATCH" >&2 ;;
		esac
	fi
	return "$status"
}
trap cleanup EXIT

# ------------------------------------------------------------ resolve target --

abspath() {
	local p="$1"
	case "$p" in
	/*) ;;
	*) p="$PWD/$p" ;;
	esac
	if [ -d "$p" ]; then
		(cd -- "$p" && pwd -P)
	else
		printf '%s/%s\n' "$(cd -- "$(dirname -- "$p")" && pwd -P)" "$(basename -- "$p")"
	fi
}

TARGET="$(abspath "$TARGET")"
TREE=""
TARBALL=""
SINGLE_ROOT_NOTE=""

if [ -d "$TARGET" ]; then
	TREE="$TARGET"
elif [ -f "$TARGET" ]; then
	case "$TARGET" in
	*.tar.gz | *.tgz | *.tar.xz | *.tar)
		TARBALL="$TARGET"
		mkdir -p "$SCRATCH/artifact"
		tar -xf "$TARBALL" -C "$SCRATCH/artifact" || die "could not extract $TARBALL"
		;;
	*) die "not a directory and not a recognised tarball: $TARGET" ;;
	esac
else
	die "no such tree or tarball: $TARGET"
fi

# -------------------------------------------------------------------- recipe --

RECIPE_SERVER_BIN="${RECIPE_SERVER_BIN:-}"
RECIPE_SERVER_VERSION_ARGS=(--version)
# Check 1's required layout. `bin share` was the whole contract while MariaDB
# was the only package; nginx's `make install` has no use for `share/` at all
# (D1, docs/superpowers/specs/2026-08-06-p2-nginx-recipe-design.md) and a
# recipe declares what its own `install` stage actually produces instead of
# the driver guessing one shape for every package. Defaulted here, ahead of
# the recipe being sourced, so a recipe that says nothing keeps today's
# behaviour exactly.
RECIPE_REQUIRED_LAYOUT=(bin share)
# Check 7 allowances. Both are empty by default, both are printed on every run,
# and both belong to the recipe rather than to this script: an upstream that
# writes /tmp into its own documentation is a fact about that upstream, and the
# place to record it — with a reason — is next to everything else known about
# that package. An allowance nobody can see is how check 4 went blind.
RECIPE_ALLOWED_WRITABLE_PATHS=()
RECIPE_INERT_PATHS=()

if [ -n "$RECIPE_NAME_ARG" ]; then
	# A bare name resolves inside build/recipes/; anything containing a slash is
	# taken as a path, so a fixture or an out-of-tree recipe can drive checks 5
	# and 6 without being added to the repo.
	case "$RECIPE_NAME_ARG" in
	*/*) recipe_file="$RECIPE_NAME_ARG" ;;
	*) recipe_file="$AUDIT_DIR/recipes/$RECIPE_NAME_ARG.sh" ;;
	esac
	[ -f "$recipe_file" ] || die "no such recipe: $recipe_file"
	export RECIPE_VERSION
	# shellcheck source=/dev/null
	. "$recipe_file"
fi
if [ "$SERVER_BIN_SET" -eq 1 ]; then
	RECIPE_SERVER_BIN="$SERVER_BIN_ARG"
fi

# No package can legitimately require zero directories under its root; an
# empty RECIPE_REQUIRED_LAYOUT is a mistake in the recipe, not a declaration.
# Refused here, before any check runs, for the same reason a floor is refused
# where check 7's allowances are read below: a malformed declaration is a
# question about the recipe, not a verdict on the artifact.
[ "${#RECIPE_REQUIRED_LAYOUT[@]}" -gt 0 ] ||
	die "RECIPE_REQUIRED_LAYOUT is empty; no package can legitimately require zero directories under its root. Name at least one, or leave the variable unset to keep the default (bin share)."

say "auditing $TARGET"
say ""

# --------------------------------------------------- 1. layout / single root --

check_start "1 layout"
layout_problems="$SCRATCH/layout.txt"
: >"$layout_problems"

if [ -n "$TARBALL" ]; then
	roots="$(find "$SCRATCH/artifact" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')"
	if [ "$roots" -ne 1 ]; then
		printf 'the tarball expands to %s top-level entries; the contract requires exactly one\n' \
			"$roots" >>"$layout_problems"
		TREE="$SCRATCH/artifact"
	else
		TREE="$(find "$SCRATCH/artifact" -mindepth 1 -maxdepth 1)"
		SINGLE_ROOT_NOTE="single root $(basename -- "$TREE")"
		if [ ! -d "$TREE" ]; then
			printf 'the single top-level entry is not a directory: %s\n' "$TREE" >>"$layout_problems"
		fi
	fi
fi

layout_required_note=""
for want in ${RECIPE_REQUIRED_LAYOUT[@]+"${RECIPE_REQUIRED_LAYOUT[@]}"}; do
	if [ ! -d "$TREE/$want" ]; then
		printf 'missing directory: %s/\n' "$want" >>"$layout_problems"
	fi
	# Printed whether or not the check passes, same rule check 7's allowances
	# already follow: an allowance (or here, a requirement) nobody can see is
	# how a later reader cannot tell the check ran for real.
	layout_required_note="$layout_required_note${layout_required_note:+, }$want"
done

if [ -s "$layout_problems" ]; then
	check_fail "the tree is not a valid package root"
	report_lines "$layout_problems"
else
	layout_pass_note="required: $layout_required_note"
	if [ -n "$SINGLE_ROOT_NOTE" ]; then layout_pass_note="$SINGLE_ROOT_NOTE; $layout_pass_note"; fi
	check_pass "$layout_pass_note"
fi

# ------------------------------------------------------------ Mach-O inventory --

# `file` separates the path from its description with SOH, which cannot sensibly
# occur in a filename — but "cannot sensibly" is not "cannot", so a path holding
# SOH or a newline is refused outright rather than mis-parsed into a pass.
FILE_LIST="$SCRATCH/files.txt"
MACHO_LIST="$SCRATCH/macho.txt"
find "$TREE" -type f -print >"$FILE_LIST"
lines="$(count_lines "$FILE_LIST")"
entries="$(find "$TREE" -type f -print0 | tr -dc '\0' | wc -c | tr -d ' ')"
[ "$lines" -eq "$entries" ] || die "a path under $TREE contains a newline; refusing to audit it"
if LC_ALL=C grep -q "$SOH" "$FILE_LIST"; then
	die "a path under $TREE contains a control character; refusing to audit it"
fi

find "$TREE" -type f -print0 |
	xargs -0 file -h -F "$SOH" -- 2>/dev/null |
	grep -F 'Mach-O' |
	cut -d"$SOH" -f1 >"$MACHO_LIST" || true
MACHO_COUNT="$(count_lines "$MACHO_LIST")"

# --------------------------------------------------------------- 2. linkage --

check_start "2 linkage"
link_problems="$SCRATCH/linkage.txt"
: >"$link_problems"
otool_out="$SCRATCH/otool.txt"

while IFS= read -r macho; do
	[ -n "$macho" ] || continue
	if ! otool -L "$macho" >"$otool_out" 2>&1; then
		printf '%s: otool -L failed: %s\n' "${macho#"$TREE"/}" "$(head -n 1 "$otool_out")" \
			>>"$link_problems"
		continue
	fi
	while IFS= read -r line; do
		case "$line" in
		"$TAB"*) ;;
		*) continue ;;
		esac
		entry="${line#"$TAB"}"
		entry="${entry%% (*}"
		[ -n "$entry" ] || continue
		case "$entry" in
		/usr/lib/?* | /System/?* | @loader_path/?* | @rpath/?*) ;;
		*) printf '%s -> %s\n' "${macho#"$TREE"/}" "$entry" >>"$link_problems" ;;
		esac
	done <"$otool_out"

	# `@rpath` is only as relocatable as the LC_RPATH entries that resolve it, so
	# admitting it above without this would be a hole rather than a widening: an
	# absolute LC_RPATH defeats relocation exactly the way an absolute dependency
	# does, and `otool -L` never shows it. Real MariaDB needs this — it ships
	# `LC_ID_DYLIB = @rpath/libmariadb.3.dylib` with `LC_RPATH = @loader_path/../lib`,
	# which is the idiomatic macOS pattern for a self-contained tree, not a defect.
	while IFS= read -r rpath; do
		[ -n "$rpath" ] || continue
		case "$rpath" in
		@loader_path/?* | @loader_path) ;;
		*) printf '%s -> LC_RPATH %s\n' "${macho#"$TREE"/}" "$rpath" >>"$link_problems" ;;
		esac
	done < <(otool -l "$macho" 2>/dev/null |
		awk '/^ *cmd LC_RPATH$/{want=1} want && /^ *path /{print $2; want=0}')
done <"$MACHO_LIST"

if [ -s "$link_problems" ]; then
	check_fail "$(count_lines "$link_problems") load-command entries outside the contract"
	report_lines "$link_problems"
else
	check_pass "$MACHO_COUNT Mach-O files; deps /usr/lib, /System, @loader_path or @rpath; every LC_RPATH @loader_path-relative"
fi

# ------------------------------------------------------------- 3. signature --

check_start "3 signature"
sign_problems="$SCRATCH/signature.txt"
: >"$sign_problems"
sign_out="$SCRATCH/codesign.txt"

while IFS= read -r macho; do
	[ -n "$macho" ] || continue
	if ! codesign --verify --strict -- "$macho" >"$sign_out" 2>&1; then
		msg="$(head -n 1 "$sign_out")"
		msg="${msg#"$macho": }"
		printf '%s: %s\n' "${macho#"$TREE"/}" "$msg" >>"$sign_problems"
	fi
done <"$MACHO_LIST"

if [ -s "$sign_problems" ]; then
	check_fail "$(count_lines "$sign_problems") of $MACHO_COUNT Mach-O files fail codesign --verify"
	report_lines "$sign_problems"
else
	check_pass "$MACHO_COUNT Mach-O files verify"
fi

# ------------------------------------------------------ 4. builder identity --

# The forbidden strings are derived from the environment. Nothing below names a
# particular machine, so the check is exactly as strong on a second builder's
# Mac as on the first one's.
FORBID_FILE="$SCRATCH/forbidden.txt"
: >"$FORBID_FILE"

# Roots too broad to forbid: every tree on the machine lives under one of them.
is_floor() {
	case "$1" in
	/ | /Users | /home | /tmp | /private | /private/tmp | /var | /private/var | \
		/opt | /usr | /System | /Volumes | /Applications | /Library | /etc | /net) return 0 ;;
	esac
	return 1
}

# Exempt from check 4: the WORK tree, and nothing else under the build root.
# Compiler debug info records the source and object paths the compiler saw; they
# are not runtime paths — nothing resolves them — and the tree they name is
# deleted when the build finishes.
#
# This used to exempt the entire build root, which is how the contract came to be
# structurally blind to the install prefix: /tmp/openvhost-build was exempt, so
# no check ever looked at it, and mariadbd shipped resolving plugin-dir out of a
# mode-1777 tree. What makes the prefix acceptable is check 7 proving nothing can
# create it, not an exemption asserting it does not matter.
is_exempt() {
	local p="$1" root
	for root in "$BUILD_ROOT" "/private${BUILD_ROOT}" "${BUILD_ROOT#/private}"; do
		[ -n "$root" ] || continue
		case "$p" in "$root"/_work | "$root"/_work/*) return 0 ;; esac
	done
	return 1
}

# The staged tree IS the install prefix (D8), so a build audited in place sits
# inside the single directory it is expected to name, and (c) below stops the
# ancestor walk here. That is a named allowance for one directory, not a blanket
# over a path family: check 7 is what makes it safe, by proving no absolute path
# in the tree — the prefix included — can be created by unprivileged code.
is_build_root() {
	local p="$1" root
	for root in "$BUILD_ROOT" "/private${BUILD_ROOT}" "${BUILD_ROOT#/private}"; do
		[ -n "$root" ] || continue
		if [ "$p" = "${root%/}" ]; then return 0; fi
	done
	return 1
}

add_forbidden_path() {
	local p="${1%/}" twin
	[ -n "$p" ] || return 0
	case "$p" in /*) ;; *) return 0 ;; esac
	if is_floor "$p" || is_exempt "$p"; then return 0; fi
	printf '%s\n' "$p" >>"$FORBID_FILE"
	# /tmp and /var are symlinks into /private on macOS, so the same directory
	# can be embedded under either spelling.
	case "$p" in
	/private/*) twin="${p#/private}" ;;
	*) twin="/private$p" ;;
	esac
	if is_floor "$twin" || is_exempt "$twin"; then return 0; fi
	printf '%s\n' "$twin" >>"$FORBID_FILE"
}

# Check 4's strength depends on where the build actually ran. Auditing a tree
# built somewhere else — a session scratchpad, say — can pass here and still
# carry that path, which is correct behaviour for a contract about the builder's
# IDENTITY, and is why --forbid exists for the cases it is not.
#
# (a) the builder's own directories, as the environment reports them
add_forbidden_path "${HOME:-}"
add_forbidden_path "$PWD"
add_forbidden_path "${OLDPWD:-}"
for var in TMPDIR TMP TEMP XDG_RUNTIME_DIR CLAUDE_SCRATCHPAD; do
	add_forbidden_path "${!var:-}"
done
if command -v git >/dev/null 2>&1; then
	add_forbidden_path "$(git -C "$AUDIT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
fi

# (b) whatever the caller declares — session directories, staging roots
while IFS= read -r entry; do
	add_forbidden_path "$entry"
	# printf '%s\n', not '%s': `read` discards a final line with no newline, so
	# the last entry of the list would silently never be forbidden.
done < <(printf '%s\n' "${OPENVHOST_AUDIT_FORBIDDEN:-}" | tr ':' '\n')
for entry in ${EXTRA_FORBIDDEN[@]+"${EXTRA_FORBIDDEN[@]}"}; do
	add_forbidden_path "$entry"
done

# (c) the tree's own ancestors. A package that names the directory it happens to
# be sitting in is by definition not relocatable, and this is what catches a
# staging path nobody remembered to declare.
ancestor="$TREE"
while :; do
	ancestor="$(dirname -- "$ancestor")"
	[ "$ancestor" != "/" ] || break
	if is_floor "$ancestor"; then break; fi
	if is_build_root "$ancestor"; then break; fi
	add_forbidden_path "$ancestor"
done

# (d) the builder's username, in path position only. The bare name is a common
# substring of ordinary words and would fire on almost anything.
builder="$(id -un 2>/dev/null || printf '%s' "${USER:-}")"
if [ -n "$builder" ]; then printf '/%s/\n' "$builder" >>"$FORBID_FILE"; fi

sort -u -o "$FORBID_FILE" "$FORBID_FILE"

check_start "4 builder identity"
id_problems="$SCRATCH/identity.txt"
: >"$id_problems"
id_hits="$SCRATCH/identity-hits.txt"
: >"$id_hits"
hit_count=0

if [ ! -s "$FORBID_FILE" ]; then
	check_fail "no forbidden string could be derived, so the check would be vacuous"
	detail "set OPENVHOST_AUDIT_FORBIDDEN, or pass --forbid"
else
	find "$TREE" -type f -print0 |
		xargs -0 grep -laF -f "$FORBID_FILE" -- 2>/dev/null >"$id_hits" || true
	hit_count="$(count_lines "$id_hits")"
	# One line per offending file, naming the first forbidden string it carries
	# and how many others it also carries. Itemising every (file, string) pair
	# buries the file names, which are the actionable part.
	detailed=0
	while IFS= read -r hit; do
		[ -n "$hit" ] || continue
		if [ "$detailed" -ge "$MAX_REPORT" ]; then break; fi
		detailed=$((detailed + 1))
		first_needle=""
		extra=0
		while IFS= read -r needle; do
			[ -n "$needle" ] || continue
			if grep -qaF -e "$needle" -- "$hit" 2>/dev/null; then
				if [ -z "$first_needle" ]; then first_needle="$needle"; else extra=$((extra + 1)); fi
			fi
		done <"$FORBID_FILE"
		if [ "$extra" -gt 0 ]; then
			printf '%s embeds %s (and %d other forbidden string(s))\n' \
				"${hit#"$TREE"/}" "$first_needle" "$extra" >>"$id_problems"
		else
			printf '%s embeds %s\n' "${hit#"$TREE"/}" "$first_needle" >>"$id_problems"
		fi
	done <"$id_hits"

	if [ "$hit_count" -gt 0 ]; then
		check_fail "$hit_count files carry the builder's identity"
		report_lines "$id_problems"
		if [ "$hit_count" -gt "$detailed" ]; then
			detail "(itemised the first $detailed of $hit_count files; raise --max-report for the rest)"
		fi
	else
		check_pass "$(count_lines "$FORBID_FILE") forbidden strings, no occurrence of any"
	fi
fi

# ---------------------------------------------------------- 5. relocation ----

# The work happens before check_start, so that a binary dying on a signal —
# which the shell announces on its own stderr, and which is exactly what a
# corrupt signature looks like — cannot land in the middle of the report line.
run_server() {
	local where="$1" log="$2" status=0
	"$where/$RECIPE_SERVER_BIN" \
		${RECIPE_SERVER_VERSION_ARGS[@]+"${RECIPE_SERVER_VERSION_ARGS[@]}"} \
		>"$log" 2>&1 || status=$?
	if [ "$status" -ne 0 ]; then
		if [ -s "$log" ]; then
			printf 'FAILED (exit %d): %s\n' "$status" "$(head -n 1 "$log")"
		else
			printf 'FAILED (exit %d), no output\n' "$status"
		fi
		return 1
	fi
	head -n 1 "$log"
	return 0
}

relocation_state="skip"
relocation_note="no server binary"
if [ "$EXECUTE_ARTIFACT" -eq 0 ]; then
	relocation_note="running the artifact was not requested (--execute-artifact)"
elif [ -n "$RECIPE_SERVER_BIN" ]; then
	if [ ! -x "$TREE/$RECIPE_SERVER_BIN" ]; then
		relocation_state="fail"
		relocation_note="the declared server binary is missing or not executable: $RECIPE_SERVER_BIN"
	else
		reloc_a="$SCRATCH/reloc/a"
		reloc_b="$SCRATCH/reloc/b"
		reloc_log="$SCRATCH/reloc.txt"
		mkdir -p "$SCRATCH/reloc"
		# ditto, not cp: it preserves the extended attributes a code signature
		# can live in, so check 5 cannot quietly invalidate what check 3 proved.
		ditto "$TREE" "$reloc_a"
		relocation_state="pass"
		first_line_a="$(run_server "$reloc_a" "$reloc_log")" || relocation_state="fail"
		mv -- "$reloc_a" "$reloc_b"
		first_line_b="$(run_server "$reloc_b" "$reloc_log")" || relocation_state="fail"
		relocation_note="$RECIPE_SERVER_BIN runs from two different paths"
		[ "$relocation_state" = "pass" ] ||
			relocation_note="$RECIPE_SERVER_BIN does not run from both paths"
	fi
fi

check_start "5 relocation"
case "$relocation_state" in
pass) check_pass "$relocation_note" ;;
skip) check_skip "$relocation_note" ;;
*) check_fail "$relocation_note" ;;
esac
if [ -n "${reloc_a:-}" ]; then
	detail "A $reloc_a: $first_line_a"
	detail "B $reloc_b: $first_line_b"
fi

# ------------------------------------------------------------- 6. service ----

probe_kind="$(type -t recipe_serve_probe 2>/dev/null || true)"
service_state="skip"
service_note="no server binary"
probe_log="$SCRATCH/serve.txt"
: >"$probe_log"
if [ "$EXECUTE_ARTIFACT" -eq 0 ]; then
	service_note="running the artifact was not requested (--execute-artifact)"
elif [ -n "$RECIPE_SERVER_BIN" ]; then
	if [ "$probe_kind" != "function" ]; then
		service_note="the recipe defines no recipe_serve_probe"
	else
		probe_scratch="$SCRATCH/serve"
		mkdir -p "$probe_scratch"
		# The probe runs against the tree in place and writes only into its own
		# scratch directory — see build/recipes/README.md.
		if recipe_serve_probe "$TREE" "$probe_scratch" >"$probe_log" 2>&1; then
			service_state="pass"
			# Recipe-supplied, not hardcoded: "started, created, inserted,
			# restarted, read back" is a database's probe, and printing it for
			# every package would be a lie about whatever check 6 actually did
			# for a package that is not one (D4,
			# docs/superpowers/specs/2026-08-06-p2-nginx-recipe-design.md).
			#
			# The FIRST line beginning with "PROBE-SUMMARY: " is the note, with
			# the prefix stripped — not simply the log's last line. A probe that
			# is careful to send everything after its summary to /dev/null (as
			# recipes/nginx.sh's is) makes "last line" safe today, but a less
			# careful probe would publish a stray warning as the note instead of
			# what the recipe actually proved; a marker line cannot be displaced
			# by output that comes after it. A probe with no marked line falls
			# back to the log's own last line — today's behaviour, unchanged —
			# which is what recipes/mariadb.sh's probe still gets, since it
			# predates the marker and is not touched here.
			service_note="$(LC_ALL=C awk -v p='PROBE-SUMMARY: ' \
				'index($0, p) == 1 { print substr($0, length(p) + 1); found = 1; exit 0 }
				 END { if (!found) exit 1 }' "$probe_log")" ||
				service_note="$(tail -n 1 "$probe_log")"
			[ -n "$service_note" ] || service_note="the probe passed but printed nothing"
		else
			service_state="fail"
			service_note="the serve-and-survive probe failed"
		fi
	fi
fi

check_start "6 service"
case "$service_state" in
pass) check_pass "$service_note" ;;
skip) check_skip "$service_note" ;;
*)
	check_fail "$service_note"
	report_lines "$probe_log"
	;;
esac

# --------------------------------------------- 7. plantable embedded paths ----

# The question is not "is this path pretty" (check 4) but "can anything create
# it". A path is offending when some PROPER ancestor of it is a world-writable
# directory on this machine: that ancestor is where an unprivileged process gets
# to invent the rest of the chain and put a file at the end of it. The path's own
# mode is not the test — a bare /tmp in a man page names a directory that already
# exists and is nobody's load path; /tmp/openvhost-build/mariadb-11.4.9/lib/plugin
# is a directory the loader will happily be handed by whoever creates it first.
#
# Evaluated against the auditing machine's filesystem, which is a proxy for the
# user's; it is a good one, because the ancestors that matter (/tmp, /var/tmp,
# /Users/Shared, /Library/Caches) are macOS's own and are the same everywhere.

check_start "7 embedded paths"
embedded_paths="$SCRATCH/embedded-paths.txt"
embedded_dirs="$SCRATCH/embedded-dirs.txt"
writable_dirs="$SCRATCH/writable-ancestors.txt"
plant_problems="$SCRATCH/plantable.txt"
allow_file="$SCRATCH/allowed-writable.txt"
: >"$plant_problems"
: >"$allow_file"

for entry in ${RECIPE_ALLOWED_WRITABLE_PATHS[@]+"${RECIPE_ALLOWED_WRITABLE_PATHS[@]}"} \
	${EXTRA_ALLOWED[@]+"${EXTRA_ALLOWED[@]}"}; do
	[ -n "$entry" ] || continue
	entry="${entry%/}"
	# An allowance suppresses the path it names AND everything beneath it, so a
	# broad one switches this check off for a whole subtree. `/tmp` would wave
	# through `/tmp/anything` — including the next MYSQL_UNIX_ADDR-class default,
	# which is the one thing check 7 exists to catch. A recipe author silencing a
	# noisy set of test paths is exactly who would reach for it.
	#
	# So a floor is refused at parse time rather than trusted to be avoided. The
	# audit dies instead of failing the check: a malformed declaration is a
	# question about the recipe, not a verdict on the artifact.
	if is_floor "$entry"; then
		die "allowance '$entry' is a floor: it would suppress check 7 for everything beneath it. Name the leaf paths instead."
	fi
	printf '%s\n' "$entry" >>"$allow_file"
done
sort -u -o "$allow_file" "$allow_file"

# Paths the recipe declares inert are skipped, by name, and named in the output.
# An entry may be a subtree or a single file; a file is how you decline to scan
# one binary without declining to scan the STRING it carries everywhere else,
# which is the safer of the two allowances and should be preferred. Checks 1-3
# still cover every Mach-O either way.
scan_prune=()
inert_note=""
for sub in ${RECIPE_INERT_PATHS[@]+"${RECIPE_INERT_PATHS[@]}"}; do
	sub="${sub#/}"
	sub="${sub%/}"
	[ -n "$sub" ] || continue
	case "$sub" in
	*..*) die "RECIPE_INERT_PATHS entry must be a path inside the tree: $sub" ;;
	esac
	[ -e "$TREE/$sub" ] || continue
	scan_prune[${#scan_prune[@]}]="-path"
	scan_prune[${#scan_prune[@]}]="$TREE/$sub"
	scan_prune[${#scan_prune[@]}]="-o"
	scan_prune[${#scan_prune[@]}]="-path"
	scan_prune[${#scan_prune[@]}]="$TREE/$sub/*"
	scan_prune[${#scan_prune[@]}]="-o"
	if [ -d "$TREE/$sub" ]; then sub="$sub/"; fi
	inert_note="$inert_note${inert_note:+, }$sub"
done

if [ "${#scan_prune[@]}" -gt 0 ]; then
	unset 'scan_prune[$((${#scan_prune[@]} - 1))]'
	find "$TREE" \( "${scan_prune[@]}" \) -prune -o -type f -print0
else
	find "$TREE" -type f -print0
fi |
	xargs -0 env LC_ALL=C grep -oahE '/[A-Za-z0-9_.+-]+(/[A-Za-z0-9_.+-]+)*' 2>/dev/null |
	# `.` and `..` are collapsed first. Half of what a build tree embeds looks
	# like obj/client/../mysys/libmysys.a, and comparing prefixes against a
	# spelling the kernel would never resolve to lets an offending path hide
	# behind a `..` — /x/../tmp/evil is /tmp/evil to everything that opens it.
	LC_ALL=C awk '{
		n = split($0, part, "/"); top = 0
		for (i = 2; i <= n; i++) {
			c = part[i]
			if (c == "." || c == "") continue
			if (c == "..") { if (top > 0) top--; continue }
			stack[++top] = c
		}
		out = ""
		for (i = 1; i <= top; i++) out = out "/" stack[i]
		print (out == "" ? "/" : out)
	}' |
	LC_ALL=C sort -u >"$embedded_paths" || true

# Every proper ancestor of every embedded path, deduplicated, so each candidate
# directory is stat'd once instead of once per path that names it.
LC_ALL=C awk -F/ '{ p = ""; for (i = 2; i < NF; i++) { p = p "/" $i; print p } }' \
	"$embedded_paths" | LC_ALL=C sort -u >"$embedded_dirs"

: >"$writable_dirs"
while IFS= read -r dir; do
	[ -d "$dir" ] || continue
	case "$(stat -L -f '%OLp' -- "$dir" 2>/dev/null || true)" in
	'') ;;
	*[2367]) printf '%s\n' "$dir" >>"$writable_dirs" ;;
	esac
done <"$embedded_dirs"

if [ -s "$writable_dirs" ]; then
	LC_ALL=C awk -F/ '
		NR == FNR { writable[$0] = 1; next }
		{
			p = ""
			for (i = 2; i < NF; i++) {
				p = p "/" $i
				if (p in writable) { print $0 "\t" p; next }
			}
		}
	' "$writable_dirs" "$embedded_paths" >"$plant_problems.all"
	# An allowance suppresses the path it names and everything BENEATH it — it is
	# a subtree switch, not an exact match. That breadth is why a floor is refused
	# where the list is read; do not restate a narrowness here that the loop below
	# does not implement. (An earlier version of this comment claimed `/tmp` could
	# never allow `/tmp/anything`. It could, and the recipe leaned on the promise.)
	if [ -s "$allow_file" ]; then
		LC_ALL=C awk -F'\t' '
			NR == FNR { allowed[$0] = 1; next }
			{
				n = split($1, part, "/")
				p = ""
				for (i = 2; i <= n; i++) {
					p = p "/" part[i]
					if (p in allowed) next
				}
				print
			}
		' "$allow_file" "$plant_problems.all" >"$plant_problems"
	else
		cp "$plant_problems.all" "$plant_problems"
	fi
fi

plant_summary="$(count_lines "$embedded_paths") absolute paths"
if [ -n "$inert_note" ]; then plant_summary="$plant_summary; inert: $inert_note"; fi
if [ -s "$allow_file" ]; then
	plant_summary="$plant_summary; $(count_lines "$allow_file") declared allowances"
fi

if [ -s "$plant_problems" ]; then
	check_fail "$(count_lines "$plant_problems") embedded paths have a world-writable ancestor"
	report_lines "$plant_problems"
	detail "world-writable ancestors seen: $(tr '\n' ' ' <"$writable_dirs")"
else
	check_pass "$plant_summary, none rooted in a world-writable directory"
	if [ -n "$inert_note" ]; then detail "declared inert (not scanned): $inert_note"; fi
	while IFS= read -r entry; do
		[ -n "$entry" ] || continue
		detail "declared allowance: $entry"
	done <"$allow_file"
fi

# --------------------------------------------------------------- verdict -----

say ""
if [ -n "$FAILED_CHECKS" ]; then
	say "AUDIT FAILED — check(s)${FAILED_CHECKS} did not pass for $TARGET"
	exit 1
fi
say "AUDIT PASSED — $TARGET satisfies the artifact contract"
exit 0
