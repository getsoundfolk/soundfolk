#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Keeps files small enough to read (docs/99-analysis.md §3): a Rust file has at most 800 lines
# before its `mod tests`, a Svelte component at most 350 lines. A file still over its limit is
# listed below with its size when it was listed: it may shrink, not grow. Split it rather than
# raise a number.
#
# Usage: check-file-sizes.sh       (from anywhere in the repository)
# Exit code: 0 when every file is within its limit, 1 otherwise.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

RUST_MAX=800
SVELTE_MAX=350

# Files over the limit, and the most lines each may have.
declare -A CEILING=(
  [src-tauri/src/db/mod.rs]=896
  [src-tauri/src/now_playing.rs]=979
  [src/lib/components/StationSheet.svelte]=449
  [src/lib/components/StatsPanel.svelte]=381
)

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
  lines=$(awk '/^mod tests/ { exit } { n++ } END { print n + 0 }' "$file")
  check "$file" "$lines" "$RUST_MAX"
done < <(git ls-files -z -- 'src-tauri/*.rs')

while IFS= read -r -d '' file; do
  [ -f "$file" ] || continue
  check "$file" "$(wc -l <"$file")" "$SVELTE_MAX"
done < <(git ls-files -z -- '*.svelte')

exit "$over"
