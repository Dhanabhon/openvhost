#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# _php-pins-refresh.sh — the remedy for _php-pins.sh's rot alarm (D4, layer
# 1; docs/superpowers/specs/2026-08-07-p2-php-pin-set.md §7). Task 2a
# recommended landing this alongside the recipe: "an alarm whose remedy is a
# day of manual shasum gets switched off."
#
# What this automates: recomputing ~40 SHA-256 digests by hand and noticing
# whether spc's own manifest (downloads/.lock.json) has grown a source this
# pin table has never seen. That is the tedious, error-prone half of
# "regenerate the pins".
#
# What this deliberately does NOT automate: rewriting _php-pins.sh itself.
# Its prose carries editorial calls this script has no business making
# silently — the extension set (pin-set doc §2), the libaom exclusion (§3),
# which URL form to pin for an unversioned or numeric-asset source (§4). A
# DRIFTED digest or a NEW source below is a decision for a human, reported
# here, not resolved here. Regenerating from zero (a fresh spc checkout, a
# fresh `spc download`, deciding the extension set over again) is what task
# 2a's own discovery run did by hand; this script is the cheap, safe half —
# "did anything I already decided move under me" — not a replacement for the
# expensive, judgement-heavy half.
#
# Usage: build/recipes/_php-pins-refresh.sh [spc-checkout-dir]
#   spc-checkout-dir   defaults to $OPENVHOST_SPC_DIR. Must already have a
#                      populated downloads/ (run `spc download` there first
#                      — never `spc doctor --auto-fix`, which runs `brew
#                      install` unprompted).
#
# Exit status
#   0  every pinned digest matches bytes on disk, and spc's manifest names
#      nothing this pin table doesn't already know about (pinned OR
#      deliberately excluded)
#   1  a digest drifted, a pinned source's bytes are missing, or spc's
#      manifest has a source this table has never triaged — see the report
#   2  usage error

set -euo pipefail

SELF="${BASH_SOURCE[0]}"
HERE="$(cd -- "$(dirname -- "$SELF")" && pwd -P)"
PINS_FILE="$HERE/_php-pins.sh"

usage() {
	cat <<'EOF'
Usage: build/recipes/_php-pins-refresh.sh [spc-checkout-dir]

Re-hashes every _php-pins.sh entry against a static-php-cli checkout's
downloads/ and reports drift, missing bytes, or manifest sources this pin
table has never triaged. See the file header for what it does and does not
automate.

Argument
  spc-checkout-dir   defaults to $OPENVHOST_SPC_DIR
EOF
}

case "${1:-}" in
-h | --help)
	usage
	exit 0
	;;
esac

SPC_DIR="${1:-${OPENVHOST_SPC_DIR:-}}"
if [ -z "$SPC_DIR" ]; then
	printf 'refresh: no spc checkout given and OPENVHOST_SPC_DIR is not set\n' >&2
	usage >&2
	exit 2
fi
[ -d "$SPC_DIR" ] || {
	printf 'refresh: no such directory: %s\n' "$SPC_DIR" >&2
	exit 2
}

DOWNLOADS="$SPC_DIR/downloads"
[ -d "$DOWNLOADS" ] || {
	printf "refresh: %s has no downloads/ — run \`spc download\` there first (never \`spc doctor --auto-fix\`)\n" "$DOWNLOADS" >&2
	exit 2
}

# shellcheck source=/dev/null
. "$PINS_FILE"

log() { printf '%s\n' "$*"; }

DRIFTED=0
MISSING=0
CHECKED=0

# One archive: pinned filename+digest vs. bytes actually under downloads/.
check_archive() {
	local label="$1" filename="$2" pinned_sha="$3" f got
	f="$DOWNLOADS/$filename"
	CHECKED=$((CHECKED + 1))
	if [ ! -f "$f" ]; then
		log "MISSING  $label ($filename not found under $DOWNLOADS)"
		MISSING=$((MISSING + 1))
		return
	fi
	got="$(shasum -a 256 -- "$f" | cut -d' ' -f1)"
	if [ "$got" != "$pinned_sha" ]; then
		log "DRIFTED  $label ($filename): pinned $pinned_sha, spc's tree now has $got"
		DRIFTED=$((DRIFTED + 1))
	else
		log "ok       $label -> $filename"
	fi
}

# One git source: pinned commit vs. the checkout's actual HEAD.
check_git_source() {
	local label="$1" dirname="$2" pinned_commit="$3" d got
	d="$DOWNLOADS/$dirname"
	CHECKED=$((CHECKED + 1))
	if [ ! -d "$d" ]; then
		log "MISSING  $label (no checkout at $d)"
		MISSING=$((MISSING + 1))
		return
	fi
	got="$(git -C "$d" rev-parse HEAD 2>/dev/null || printf 'unreadable')"
	if [ "$got" != "$pinned_commit" ]; then
		log "DRIFTED  $label ($dirname): pinned $pinned_commit, checkout is now at $got"
		DRIFTED=$((DRIFTED + 1))
	else
		log "ok       $label -> $got"
	fi
}

log "== spc checkout: $SPC_DIR =="
spc_head="$(git -C "$SPC_DIR" rev-parse HEAD 2>/dev/null || printf 'unreadable')"
log "HEAD: $spc_head"
if [ "$spc_head" = "$PHP_PINS_SPC_COMMIT" ]; then
	log "matches the commit _php-pins.sh was derived from ($PHP_PINS_SPC_TAG)"
else
	log "does NOT match the derivation commit ($PHP_PINS_SPC_TAG, $PHP_PINS_SPC_COMMIT)"
	log "— recipe_fetch's own rot-alarm check (D4 layer 1) would refuse to build against this checkout as-is; a DRIFTED or NEW finding below may just mean THIS checkout moved, not that upstream did"
fi

log ""
log "== PHP_PINS_LIBS (${#PHP_PINS_LIBS[@]} entries) =="
for row in "${PHP_PINS_LIBS[@]}"; do
	name=""
	sha256=""
	filename=""
	read -r name _ sha256 filename _ <<<"$row"
	check_archive "$name" "$filename" "$sha256"
done

log ""
log "== PHP_PINS_PHP_SRC (${#PHP_PINS_PHP_SRC[@]} entries) =="
for row in "${PHP_PINS_PHP_SRC[@]}"; do
	version=""
	sha256=""
	url=""
	read -r version sha256 _ url _ <<<"$row"
	check_archive "php-src $version" "$(basename -- "$url")" "$sha256"
done

log ""
log "== PHP_PINS_GIT (${#PHP_PINS_GIT[@]} entries) =="
for row in "${PHP_PINS_GIT[@]}"; do
	name=""
	commit=""
	read -r name commit _ <<<"$row"
	check_git_source "$name" "$name" "$commit"
done

log ""
log "== sources in spc's manifest this pin table has never triaged =="
LOCK_JSON="$DOWNLOADS/.lock.json"
NEW=0
if [ -f "$LOCK_JSON" ]; then
	KNOWN=" php-src "
	for row in "${PHP_PINS_LIBS[@]}"; do
		name=""
		read -r name _ <<<"$row"
		KNOWN="$KNOWN$name "
	done
	for row in "${PHP_PINS_GIT[@]}"; do
		name=""
		read -r name _ <<<"$row"
		KNOWN="$KNOWN$name "
	done
	for row in "${PHP_PINS_EXCLUDED[@]}"; do
		name=""
		read -r name _ <<<"$row"
		KNOWN="$KNOWN$name "
	done
	while IFS= read -r key; do
		[ -n "$key" ] || continue
		case "$KNOWN" in
		*" $key "*) ;;
		*)
			log "NEW      $key is in spc's manifest but not pinned OR excluded here — a human call, the shape of the libaom one (pin-set doc §3)"
			NEW=$((NEW + 1))
			;;
		esac
	done < <(awk -F'"' '/^    "[^"]+": \{$/ { print $2 }' "$LOCK_JSON")
	[ "$NEW" -gt 0 ] || log "none"
else
	log "no downloads/.lock.json at $DOWNLOADS — skipping (spc writes this during \`spc download\`)"
fi

log ""
log "checked=$CHECKED drifted=$DRIFTED missing=$MISSING new=$NEW"

if [ "$DRIFTED" -gt 0 ] || [ "$MISSING" -gt 0 ] || [ "$NEW" -gt 0 ]; then
	exit 1
fi
exit 0
