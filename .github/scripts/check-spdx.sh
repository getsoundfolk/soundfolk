#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Every source file under version control names its license in its first lines:
# `SPDX-License-Identifier: MPL-2.0`, as a comment (see CONTRIBUTING.md).
# Source files: code, styles, scripts, workflows and git hooks. Manifests, recipes, fixtures and
# docs are not checked.
#
# Usage: check-spdx.sh            (from anywhere in the repository)
# Exit code: 0 when every source file has the line, 1 otherwise.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

missing=0
while IFS= read -r -d '' file; do
  [ -f "$file" ] || continue # deleted in the working tree
  if ! head -n 5 "$file" | grep -q 'SPDX-License-Identifier: MPL-2.0'; then
    echo "$file: no 'SPDX-License-Identifier: MPL-2.0' line in its first 5 lines" >&2
    missing=1
  fi
done < <(git ls-files -z -- \
  '*.rs' '*.ts' '*.js' '*.svelte' '*.css' '*.sh' '*.py' '*.yml' '*.yaml' '.githooks/*')

exit "$missing"
