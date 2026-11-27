#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Finds the Tauri commands the front end never calls: neither Rust's dead_code nor knip sees them,
# since the command is registered (so used, for Rust) and `commands` is one exported object (so
# used, for knip). A command counts as called when its name appears as a word in src/, outside
# the generated bindings.ts and the tests.
set -euo pipefail

cd "$(dirname "$0")/../.."
bindings=src/lib/bindings.ts

names=$(sed -n '/^export const commands = {/,/^}/p' "$bindings" | sed -nE 's/^\t([A-Za-z0-9_]+): \(.*/\1/p')
if [ -z "$names" ]; then
  echo "No command found in $bindings: has its format changed?" >&2
  exit 1
fi

unused=()
for name in $names; do
  if ! grep -rqw --include='*.ts' --include='*.svelte' --exclude=bindings.ts --exclude='*.test.ts' \
    -e "$name" src; then
    unused+=("$name")
  fi
done

if [ ${#unused[@]} -gt 0 ]; then
  echo "Tauri commands the front end never calls (remove them from the Rust side):" >&2
  printf '  %s\n' "${unused[@]}" >&2
  exit 1
fi
