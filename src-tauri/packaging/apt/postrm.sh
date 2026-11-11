#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
#
# After the soundfolk package is removed: its repository goes with it, since the key it is
# checked with was part of the package (APT would fail on every update otherwise). Installing the
# package again subscribes again (postinst.sh).
set -e

SOURCE=/etc/apt/sources.list.d/soundfolk.sources

case "$1" in
  remove | purge)
    # Only the file the package wrote: one a listener rewrote by hand is theirs.
    if grep -qs 'Added by the soundfolk package' "$SOURCE"; then
      rm -f "$SOURCE"
    fi
    rm -rf /var/lib/soundfolk
    ;;
esac

exit 0
