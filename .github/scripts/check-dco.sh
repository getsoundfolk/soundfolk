#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Developer Certificate of Origin check: every non-merge commit in BASE..HEAD must carry a
# `Signed-off-by: Name <email>` line whose email matches the commit author's email.
#
# Usage: check-dco.sh <base-sha> <head-sha>
# Exit code: 0 when all commits are signed off, 1 otherwise.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <base-sha> <head-sha>" >&2
  exit 2
fi
base="$1"
head="$2"

failed=0
checked=0

# Merge commits are not authored contributions; bot accounts (e.g. Dependabot) are exempt.
for sha in $(git rev-list --no-merges "$base..$head"); do
  short="${sha:0:7}"
  author_email="$(git show -s --format='%ae' "$sha")"

  case "$author_email" in
    *'[bot]@users.noreply.github.com')
      echo "skip   $short (bot: $author_email)"
      continue
      ;;
  esac

  checked=$((checked + 1))
  signoffs="$(git show -s --format='%B' "$sha" | grep -iE '^Signed-off-by: .+ <[^>]+>[[:space:]]*$' || true)"

  if [ -z "$signoffs" ]; then
    echo "::error::Commit $short has no 'Signed-off-by' line. Sign it with 'git commit -s'."
    failed=1
  elif ! printf '%s\n' "$signoffs" | grep -qiF "<$author_email>"; then
    echo "::error::Commit $short: the 'Signed-off-by' email does not match the author email <$author_email>."
    failed=1
  else
    echo "ok     $short"
  fi
done

if [ "$checked" -eq 0 ]; then
  echo "No commits to check."
fi

if [ "$failed" -ne 0 ]; then
  cat <<'EOF'

DCO check failed. See CONTRIBUTING.md. To sign off existing commits of your branch:

  git rebase --signoff <base-branch>
  git push --force-with-lease
EOF
  exit 1
fi
