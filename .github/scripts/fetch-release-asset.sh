#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Downloads a GitHub release asset and checks its SHA-256.
# First from its usual address, github.com/<repo>/releases/download/<tag>/<asset>. When that
# address fails on GitHub's side (a 5xx, or a connection cut or timed out: on 2026-11-08 it gave
# 504, 503 and cut connections for every repository for hours, while the API and the storage
# behind it worked), the same file is fetched through the API, which redirects to the same
# storage. A 4xx is ours (a wrong version or name) and fails at once. The checksum is checked
# either way.
#
# Usage: fetch-release-asset.sh <owner/repo> <tag> <asset> <sha256> <destination file>
# GITHUB_TOKEN, when set, is sent to the API only (it raises its rate limit).
# RELEASE_DOWNLOAD_BASE replaces https://github.com for the usual address (to test the fallback).
set -euo pipefail

repo=$1 tag=$2 asset=$3 sha256=$4 dest=$5

base=${RELEASE_DOWNLOAD_BASE:-https://github.com}
code=0
status=$(curl -sSL --retry 2 --connect-timeout 20 --max-time 120 -o "$dest" -w '%{http_code}' \
  "$base/$repo/releases/download/$tag/$asset") || code=$?

# 000: no answer (connection refused, cut or timed out); 200 with a curl error: cut mid-file.
if [[ $status == 5?? || $status == 000 || ($status == 200 && $code != 0) ]]; then
  echo "github.com failed for $asset (HTTP $status, curl exit $code): fetching it through the API" >&2
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
elif [[ $status != 200 ]]; then
  echo "github.com answered $status for $asset" >&2
  exit 1
fi

echo "$sha256  $dest" | sha256sum -c -
