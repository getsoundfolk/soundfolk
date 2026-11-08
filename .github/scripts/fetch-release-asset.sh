#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Downloads a GitHub release asset and checks its SHA-256.
# First from its usual address, github.com/<repo>/releases/download/<tag>/<asset>. When that
# address answers 504 (it did for every repository for hours on 2026-11-08, while the API and the
# storage behind it worked), the same file is fetched through the API, which redirects to the same
# storage. The checksum is the same either way.
#
# Usage: fetch-release-asset.sh <owner/repo> <tag> <asset> <sha256> <destination file>
# GITHUB_TOKEN, when set, is sent to the API only (it raises its rate limit).
set -euo pipefail

repo=$1 tag=$2 asset=$3 sha256=$4 dest=$5

status=$(curl -sSL --retry 2 --connect-timeout 20 --max-time 120 -o "$dest" -w '%{http_code}' \
  "https://github.com/$repo/releases/download/$tag/$asset" || true)

if [ "$status" = 504 ]; then
  echo "github.com answered 504 for $asset: fetching it through the API" >&2
  auth=()
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
  fi
  id=$(curl -fsSL "${auth[@]}" -H 'Accept: application/vnd.github+json' \
    "https://api.github.com/repos/$repo/releases/tags/$tag" |
    jq -er --arg name "$asset" '.assets[] | select(.name == $name) | .id')
  # The API answers with a redirect to the storage: the token must not follow it.
  location=$(curl -fsS "${auth[@]}" -H 'Accept: application/octet-stream' -o /dev/null \
    -w '%{redirect_url}' "https://api.github.com/repos/$repo/releases/assets/$id")
  curl -fsSL --retry 2 --connect-timeout 20 --max-time 120 -o "$dest" "$location"
elif [ "$status" != 200 ]; then
  echo "github.com answered $status for $asset" >&2
  exit 1
fi

echo "$sha256  $dest" | sha256sum -c -
