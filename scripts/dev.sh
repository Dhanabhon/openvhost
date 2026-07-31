#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# One-command dev run: launches the OpenVHost desktop app in dev mode.
# Works from anywhere inside the repo; installs frontend deps on first run.
# Usage: ./scripts/dev.sh [extra tauri-cli args]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v pnpm >/dev/null 2>&1; then
  echo "pnpm not found — enable it with: corepack enable   (or: npm install -g pnpm)" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found — install Rust via https://rustup.rs (rust-toolchain.toml pins the version automatically)" >&2
  exit 1
fi

if [ ! -d "$root/apps/desktop/node_modules" ]; then
  echo "node_modules missing — running pnpm install first…"
  pnpm -C "$root/apps/desktop" install --frozen-lockfile
fi

# The CLI is NOT a dependency of the desktop crate, so `tauri dev` never builds
# it. "Install Command Line Tool…" resolves the binary as current_exe()'s
# sibling (install design D1), which in a dev build means
# target/debug/openvhost sitting beside target/debug/openvhost-desktop — so on
# a fresh clone that menu item would correctly, and uselessly, report the
# binary as missing. Cheap once built; a no-op on every later run.
echo "building the openvhost CLI so it sits beside the app in target/debug…"
cargo build --manifest-path "$root/Cargo.toml" -p openvhost

exec pnpm -C "$root/apps/desktop" run tauri dev "$@"
