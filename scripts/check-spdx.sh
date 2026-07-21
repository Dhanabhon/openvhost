#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Fail if any tracked source file is missing the SPDX header (spec §6 invariant).
set -euo pipefail

offenders=()
while IFS= read -r f; do
  case "$f" in
    apps/desktop/src/lib/ipc/bindings.ts|apps/desktop/src/lib/ipc/gen/*|apps/desktop/src-tauri/gen/*) continue ;;
  esac
  if ! head -n 3 "$f" | grep -q 'SPDX-License-Identifier: GPL-3.0-or-later'; then
    offenders+=("$f")
  fi
done < <(git ls-files '*.rs' '*.ts' '*.js' '*.svelte' '*.css' '*.sh' '.github/workflows/*.yml')

if [ "${#offenders[@]}" -gt 0 ]; then
  printf 'missing SPDX header:\n'
  printf '  %s\n' "${offenders[@]}"
  exit 1
fi
echo "SPDX headers OK"
