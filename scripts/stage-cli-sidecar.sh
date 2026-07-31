#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the `openvhost` CLI and stage it under the target-triple name Tauri's
# `bundle.externalBin` requires, so the bundler drops it into
# `OpenVHost.app/Contents/MacOS/openvhost` beside the app binary (install
# design D1).
#
# Run by `tauri.bundle.conf.json`'s `beforeBuildCommand`, so it is inseparable
# from the `externalBin` entry that needs its output. Also runnable on its own.
#
# Usage: ./scripts/stage-cli-sidecar.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found — install Rust via https://rustup.rs (rust-toolchain.toml pins the version automatically)" >&2
  exit 1
fi

# Tauri exports the triple it is bundling for; fall back to the host when the
# script is run by hand. `rustc -vV` is the same source Tauri derives it from.
triple="${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}"
if [ -z "$triple" ]; then
  echo "could not determine the target triple (rustc -vV printed no host line)" >&2
  exit 1
fi

# A universal build needs TWO architectures lipo'd together. Staging a
# single-arch binary under that name would produce a bundle that silently fails
# on half the Macs it claims to support, so refuse rather than guess. Phase 2
# owns the multi-target build pipeline; this is the marker for it.
if [ "$triple" = "universal-apple-darwin" ]; then
  echo "universal builds are not supported yet: the sidecar would have to be a lipo'd fat binary." >&2
  echo "Build per-arch for now (see docs/superpowers/specs/2026-07-31-p1-cli-install-design.md D1)." >&2
  exit 1
fi

host="$(rustc -vV | sed -n 's/^host: //p')"
if [ "$triple" = "$host" ]; then
  cargo build --manifest-path "$root/Cargo.toml" -p openvhost --release
  built="$root/target/release/openvhost"
else
  cargo build --manifest-path "$root/Cargo.toml" -p openvhost --release --target "$triple"
  built="$root/target/$triple/release/openvhost"
fi

if [ ! -f "$built" ]; then
  echo "cargo reported success but $built is missing" >&2
  exit 1
fi

# The release gate, enforced on the path that actually ships (install design
# D7). `tests/release_gate.rs` proves the same thing, but only when someone
# runs `cargo test --release`; this runs every single time a bundle is built.
# `--probe-state` is the fixture's sharpest edge: two unconfined writes at a
# path the caller names. If the binary still answers it, the file appears.
probe="$(mktemp -d)"
trap 'rm -rf "$probe"' EXIT
"$built" __testchild --probe-state "$probe/state" --probe-succeed-after 1 >/dev/null 2>&1 || true
if [ -e "$probe/state" ] || [ -e "$probe/state.pid" ]; then
  echo "REFUSING TO STAGE: $built still answers the hidden __testchild fixture." >&2
  echo "It wrote to a caller-named path. That must not ship on a user's PATH — see design D7." >&2
  exit 1
fi

staged_dir="$root/target/sidecar"
staged="$staged_dir/openvhost-$triple"
mkdir -p "$staged_dir"

# Copy-then-rename: the bundler must never see a half-written binary, and
# `cp` onto a live path is not atomic. Same directory, so `mv` is a rename.
tmp="$staged.tmp.$$"
cp "$built" "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$staged"

echo "staged $staged"
