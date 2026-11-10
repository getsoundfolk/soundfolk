#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Keeps files small enough to read (docs/99-analysis.md §3): a Rust file has at most 800 lines
# before its `mod tests`, a Svelte component at most 350 lines. Split a file rather than raise a
# number.
#
# Usage: check-file-sizes.sh       (from anywhere in the repository)
# Exit code: 0 when every file is within its limit, 1 otherwise.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

RUST_MAX=800
SVELTE_MAX=350

# Files over the limit, and the most lines each may have: none today. A file that cannot be split
# at once goes here with its current size, so that it may shrink but not grow.
declare -A CEILING=()

over=0
check() {
  local file=$1 lines=$2 max=$3
  local ceiling=${CEILING[$file]:-$max}
  if [ "$lines" -gt "$ceiling" ]; then
    echo "$file: $lines lines, more than $ceiling: split it" >&2
    over=1
  fi
}

while IFS= read -r -d '' file; do
  [ -f "$file" ] || continue # deleted in the working tree
  # The lines before the test module.
  lines=$(awk '/^(pub(\([a-z]+\))? )?mod tests/ { exit } { n++ } END { print n + 0 }' "$file")
  check "$file" "$lines" "$RUST_MAX"
done < <(git ls-files -z -- 'src-tauri/*.rs')

while IFS= read -r -d '' file; do
  [ -f "$file" ] || continue
  check "$file" "$(wc -l <"$file")" "$SVELTE_MAX"
done < <(git ls-files -z -- '*.svelte')

exit "$over"
