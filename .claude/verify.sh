#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Fast check run by the Claude Code Stop hook when tracked files changed (from the repository root).
# Same checks as `npm run check` and `npm run lint`, with ESLint and Stylelint caches kept in
# node_modules/.cache, and the type check running in parallel. Clippy and the tests stay in CI.
set -euo pipefail

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
npm run --silent check > "$log" 2>&1 &
check_pid=$!

npx eslint --cache --cache-location node_modules/.cache/eslint/ .
npx stylelint --cache --cache-location node_modules/.cache/stylelint/ "src/**/*.{css,svelte}"
npx knip
.github/scripts/check-unused-bindings.sh
.github/scripts/check-file-sizes.sh

if ! wait "$check_pid"; then
  cat "$log" >&2
  exit 1
fi
