#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# audit.sh — the artifact contract for an OpenVHost package.
#
# Implements the six points of the build-pipeline design (D6, spec §8). A
# package is acceptable only if ALL of them hold:
#
#   1. layout      a single root containing bin/ and share/
#   2. linkage     every otool -L entry of every Mach-O is /usr/lib/*,
#                  /System/*, @loader_path/... or @rpath/..., AND every
#                  LC_RPATH is @loader_path-relative — nothing else
#   3. signature   every Mach-O is signed and codesign --verify passes
#   4. identity    the tree carries no trace of the machine that built it
#   5. relocation  copy to path A, run; move to path B, run again
#   6. service     start the server, create a table, insert, restart, read back
#
# Points 5 and 6 need a server binary and a package-specific probe. A package
# with neither reports them as SKIPPED, with the reason — never silently. See
# build/recipes/README.md for how a recipe supplies them.
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

BUILD_ROOT="${OPENVHOST_BUILD_ROOT:-/tmp/openvhost-build}"

usage() {
	cat <<'EOF'
Usage: build/audit.sh [options] <tree-or-tarball>

Verifies one built package tree, or one packed tarball, against the artifact
contract in docs/superpowers/specs/2026-08-02-p2-build-pipeline-design.md §8.

Options
  --recipe <name>      source build/recipes/<name>.sh for RECIPE_SERVER_BIN and
                       recipe_serve_probe, which drive checks 5 and 6. A value
                       containing a slash is taken as a path to a recipe.
  --version <version>  value of RECIPE_VERSION while the recipe is sourced
  --server-bin <path>  server binary, relative to the tree root, for checks 5
                       and 6; overrides the recipe. Empty disables both.
  --forbid <string>    add one forbidden string to check 4 (repeatable)
  --max-report <n>     cap the offending lines printed per check (default 20)
  --keep-scratch       do not delete this run's scratch directory
  -h, --help           this text

Environment
  OPENVHOST_AUDIT_FORBIDDEN  colon-separated extra forbidden strings for check
                             4 — session directories, scratchpads, staging
                             roots. Check 4 takes the builder's identity from
                             the environment; it hardcodes no machine's paths.
  OPENVHOST_BUILD_ROOT       the neutral build root (default
                             /tmp/openvhost-build). Meaningless by design (D8),
                             so it is exempt from check 4, and it is where this
                             script keeps its scratch.

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
CHECK_TOTAL=6
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

# Every byte this script writes lands under here, and here is under the neutral
# build root by construction.
SCRATCH="$BUILD_ROOT/_audit/$$"
mkdir -p "$SCRATCH"

# shellcheck disable=SC2329  # invoked by the EXIT trap installed below
cleanup() {
	local status=$?
	if [ "$KEEP_SCRATCH" -eq 1 ]; then
		say "scratch kept at $SCRATCH"
	else
		# Never rm -rf an unvalidated variable. This path is BUILD_ROOT plus
		# two fixed components, and the shape is re-checked here rather than
		# trusted.
		case "$SCRATCH" in
		/?*/_audit/[0-9]*) rm -rf -- "$SCRATCH" ;;
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

for want in bin share; do
	if [ ! -d "$TREE/$want" ]; then
		printf 'missing directory: %s/\n' "$want" >>"$layout_problems"
	fi
done

if [ -s "$layout_problems" ]; then
	check_fail "the tree is not a valid package root"
	report_lines "$layout_problems"
else
	check_pass "$SINGLE_ROOT_NOTE"
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

# The neutral build prefix (D8) is meaningless by design and is *expected* to be
# embedded in the tree, so it and everything under it are exempt.
is_exempt() {
	local p="$1" root
	for root in "$BUILD_ROOT" "/private${BUILD_ROOT}" "${BUILD_ROOT#/private}"; do
		[ -n "$root" ] || continue
		case "$p" in "$root" | "$root"/*) return 0 ;; esac
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

# Check 4's strength depends on where the build actually ran, and D8 is what
# makes that safe rather than lucky: build.sh forces every build under the
# neutral root, so the paths a package embeds are inert by construction. Auditing
# a tree built somewhere else — a session scratchpad, say — can therefore pass
# here and still carry that path, which is correct behaviour for a contract about
# the builder's IDENTITY, and is why --forbid exists for the cases it is not.
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
if [ -n "$RECIPE_SERVER_BIN" ]; then
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
if [ -n "$RECIPE_SERVER_BIN" ]; then
	if [ "$probe_kind" != "function" ]; then
		service_note="the recipe defines no recipe_serve_probe"
	else
		probe_scratch="$SCRATCH/serve"
		mkdir -p "$probe_scratch"
		# The probe runs against the tree in place and writes only into its own
		# scratch directory — see build/recipes/README.md.
		if recipe_serve_probe "$TREE" "$probe_scratch" >"$probe_log" 2>&1; then
			service_state="pass"
			service_note="started, created, inserted, restarted, read back"
		else
			service_state="fail"
			service_note="the serve-and-survive probe failed"
		fi
	fi
fi

check_start "6 service"
case "$service_state" in
pass)
	check_pass "$service_note"
	if [ -s "$probe_log" ]; then detail "$(tail -n 1 "$probe_log")"; fi
	;;
skip) check_skip "$service_note" ;;
*)
	check_fail "$service_note"
	report_lines "$probe_log"
	;;
esac

# --------------------------------------------------------------- verdict -----

say ""
if [ -n "$FAILED_CHECKS" ]; then
	say "AUDIT FAILED — check(s)${FAILED_CHECKS} did not pass for $TARGET"
	exit 1
fi
say "AUDIT PASSED — $TARGET satisfies the artifact contract"
exit 0
