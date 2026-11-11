#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Installs Ubuntu packages on a GitHub runner: apt-install.sh PACKAGE...
#
# The runner images list Azure's mirror first (/etc/apt/apt-mirrors.txt). When its name stops
# resolving, apt does not move on to the next mirror: every source fails, and `apt-get install`
# retried each package for over an hour (CI runs 43032795840 and 43035482870, November 2026),
# while other hosts resolved fine. So the Azure mirror is dropped as soon as it fails, and apt
# gives up on a host after a few short attempts instead of waiting.
set -euo pipefail

azure=azure.archive.ubuntu.com
apt_options=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)
fallen_back=false

# The main archive in place of Azure's mirror, in the mirror list and in any source naming it.
drop_azure_mirror() {
  [ "$fallen_back" = false ] || return 1
  fallen_back=true
  echo "::warning::$azure fails: apt falls back to archive.ubuntu.com"
  if [ -f /etc/apt/apt-mirrors.txt ]; then
    sudo sed -i "/$azure/d" /etc/apt/apt-mirrors.txt
  fi
  local sources
  sources=$(grep -rlF "$azure" /etc/apt/sources.list /etc/apt/sources.list.d 2> /dev/null || true)
  if [ -n "$sources" ]; then
    # shellcheck disable=SC2086 # one path per line, none with spaces
    sudo sed -i "s#http://$azure/#http://archive.ubuntu.com/#g" $sources
  fi
}

# `apt-get update` exits 0 when a source fails: its warnings say so. Only Ubuntu's own sources
# count: a third-party repository the runner lists (Microsoft's) is not needed here.
update() {
  local log
  log=$(mktemp)
  sudo apt-get "${apt_options[@]}" update 2>&1 | tee "$log"
  ! grep -qE '^(W: Failed to fetch .*(ubuntu\.com|apt-mirrors)|E: )' "$log"
}

install() {
  sudo apt-get "${apt_options[@]}" install -y --no-install-recommends "$@"
}

if ! getent hosts "$azure" > /dev/null; then
  drop_azure_mirror
fi
update || { drop_azure_mirror && update; }
install "$@" || { drop_azure_mirror && update && install "$@"; }
