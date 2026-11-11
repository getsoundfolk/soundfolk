#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Warns before the APT repository's signing subkey expires (docs/17-apt-repository.md §5). Once it
# has expired, APT refuses the repository on every subscribed system, and no release can bring them
# a new key any more: it has to be renewed while the current one still signs.
#
#   more than WARN_DAYS left   nothing
#   less than WARN_DAYS        a warning on every CI run
#   less than FAIL_DAYS        CI fails (and so does the release that needs it)
#
# Reads the expiry from the public keyring the .deb ships, so it follows any key change.
# Usage: check-apt-key.sh            (from anywhere in the repository)
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

KEYRING=src-tauri/packaging/apt/soundfolk-archive-keyring.gpg
WARN_DAYS=180
FAIL_DAYS=30

if [ ! -f "$KEYRING" ]; then
  echo "no APT archive key yet ($KEYRING): nothing to check"
  exit 0
fi

# The latest expiry among the subkeys that can sign and are still valid (field 2: validity, 7:
# expiry as a Unix time, 12: capabilities). Empty: no such subkey, or one that never expires.
expiry=$(gpg --batch --with-colons --show-keys "$KEYRING" 2> /dev/null \
  | awk -F: '$1 == "sub" && $12 ~ /s/ && $2 !~ /[erd]/ { print ($7 == "" ? "never" : $7) }' \
  | sort | tail -n 1)

if [ -z "$expiry" ]; then
  echo "::error file=$KEYRING::no valid signing subkey: APT cannot verify the repository"
  exit 1
fi
if [ "$expiry" = never ]; then
  echo "the APT signing subkey never expires"
  exit 0
fi

left=$(((expiry - $(date +%s)) / 86400))
until=$(date -u -d "@$expiry" +%Y-%m-%d)
message="the APT signing subkey expires on $until ($left days): renew it (docs/17-apt-repository.md §5)"
if [ "$left" -lt "$FAIL_DAYS" ]; then
  echo "::error file=$KEYRING::$message"
  exit 1
elif [ "$left" -lt "$WARN_DAYS" ]; then
  echo "::warning file=$KEYRING::$message"
else
  echo "the APT signing subkey expires on $until ($left days)"
fi
