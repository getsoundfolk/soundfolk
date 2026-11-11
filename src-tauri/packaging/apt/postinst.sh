#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
#
# After the soundfolk package is installed or upgraded: subscribes the system to Soundfolk's APT
# repository (docs/17-apt-repository.md), so that Software Updater installs new versions. Done
# once: a listener who deletes the source file is not subscribed again, and a system that already
# lists the repository elsewhere keeps its own entry.
set -e

SOURCE=/etc/apt/sources.list.d/soundfolk.sources
TEMPLATE=/usr/share/soundfolk/apt/soundfolk.sources
ADDED=/var/lib/soundfolk/apt-source-added

if [ "$1" = configure ] && [ ! -e "$ADDED" ]; then
  if [ ! -e "$SOURCE" ] &&
    ! grep -rqs 'getsoundfolk.github.io/apt' /etc/apt/sources.list /etc/apt/sources.list.d/; then
    install -m 644 "$TEMPLATE" "$SOURCE"
  fi
  mkdir -p "${ADDED%/*}"
  touch "$ADDED"
fi

exit 0
