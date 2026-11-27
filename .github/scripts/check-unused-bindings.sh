#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Finds the Tauri commands the front end never calls and the events it never listens to. Neither
# Rust's dead_code nor knip sees them: a command is registered and an event is emitted (so used,
# for Rust), and `commands` and `EventPayloads` are each one export (so used, for knip).
# Searched in src/, outside the generated bindings.ts and the tests:
#   - a command counts as called when its name appears as a word;
#   - an event counts as listened to when its name appears as a quoted string.
set -euo pipefail

cd "$(dirname "$0")/../.."
bindings=src/lib/bindings.ts

# The lines of a block of bindings.ts, from its opening line to the closing brace.
block() { sed -n "/^$1/,/^}/p" "$bindings"; }

commands=$(block 'export const commands = {' | sed -nE 's/^\t([A-Za-z0-9_]+): \(.*/\1/p')
events=$(block 'export type EventPayloads = {' | sed -nE 's/^\t"([^"]+)": .*/\1/p')
if [ -z "$commands" ] || [ -z "$events" ]; then
  echo "No command or no event found in $bindings: has its format changed?" >&2
  exit 1
fi

search() {
  grep -rq --include='*.ts' --include='*.svelte' --exclude=bindings.ts --exclude='*.test.ts' "$@" src
}

failed=0
report() {
  echo "$1" >&2
  printf '  %s\n' "${@:2}" >&2
  failed=1
}

unused=()
for name in $commands; do
  search -w -e "$name" || unused+=("$name")
done
[ ${#unused[@]} -eq 0 ] || report "Tauri commands the front end never calls (remove them):" "${unused[@]}"

unheard=()
for name in $events; do
  search -F -e "'$name'" -e "\"$name\"" || unheard+=("$name")
done
[ ${#unheard[@]} -eq 0 ] || report "Events the front end never listens to (stop sending them):" "${unheard[@]}"

exit "$failed"
