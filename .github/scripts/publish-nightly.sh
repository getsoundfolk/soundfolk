#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Publishes the nightly pre-release: one rolling release, tag `nightly`, moved to each new build.
# The packages are uploaded first, to a draft, while the previous nightly stays online; only then is
# the previous one deleted and the draft published in its place, a few seconds apart. Deleting
# first, as before, left no nightly at all whenever an upload failed. Each step is tried again after
# a server error (retry.sh) and starts by cleaning up what a failed try or run left: a draft, or a
# `nightly` tag without its release.
#
# Usage: publish-nightly.sh <version> <commit SHA> <package>...
# Needs gh, with GH_TOKEN (contents: write) and GH_REPO set.
set -euo pipefail

if [ $# -lt 3 ]; then
  echo "usage: $0 <version> <commit SHA> <package>..." >&2
  exit 2
fi
version=$1 sha=$2
shift 2

# shellcheck source=SCRIPTDIR/retry.sh
source "$(dirname "$0")/retry.sh"

# The draft's tag name. A draft does not create its tag, so this tag never exists; it only tells
# this build's draft from any other.
draft_tag=nightly-next

draft_ids() {
  gh api --paginate "repos/$GH_REPO/releases" \
    --jq ".[] | select(.draft and .tag_name == \"$draft_tag\") | .id"
}

upload_draft() {
  local ids id
  ids=$(draft_ids)
  for id in $ids; do
    gh api -X DELETE "repos/$GH_REPO/releases/$id"
  done
  gh release create "$draft_tag" "$@" --draft --prerelease --target "$sha" \
    --title "Nightly $version" \
    --notes "Automatic build of main at ${sha:0:7}. Not a stable release."
}

# Deletes the previous nightly and its tag; what is already gone is not an error.
remove_previous() {
  local answer
  if answer=$(gh api "repos/$GH_REPO/releases/tags/nightly" --jq .id 2>&1); then
    gh api -X DELETE "repos/$GH_REPO/releases/$answer"
  elif ! grep -q 'HTTP 404' <<< "$answer"; then
    echo "$answer" >&2
    return 1
  fi
  # 422 "Reference does not exist": the tag went with an earlier try.
  if ! answer=$(gh api -X DELETE "repos/$GH_REPO/git/refs/tags/nightly" 2>&1) \
    && ! grep -qE 'HTTP (404|422)' <<< "$answer"; then
    echo "$answer" >&2
    return 1
  fi
}

# Publishing a draft whose tag does not exist creates the tag, at the target commit.
publish_draft() {
  gh api -X PATCH "repos/$GH_REPO/releases/$1" --silent \
    -f tag_name=nightly -f target_commitish="$sha" -F draft=false -F prerelease=true
}

retry upload_draft "$@"
id=$(retry draft_ids)
if [ "$(wc -w <<< "$id")" -ne 1 ]; then
  echo "::error::expected one draft tagged $draft_tag, found: ${id:-none}" >&2
  exit 1
fi
retry remove_previous
retry publish_draft "$id"
echo "Nightly $version published at ${sha:0:7}."
