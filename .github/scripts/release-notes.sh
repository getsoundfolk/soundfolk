#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# The notes of a tagged release (release.yml, publish job): "What's new", a summary for listeners
# written by hand in the annotated tag's message, then .github/release-notes.md (which file to
# download, the known limitations) for this version, then the link to every commit since the
# previous release.
#
# Usage: release-notes.sh draft <tag>             before tagging: prints a draft of the tag
#                                                 message, the title then the commits that may
#                                                 matter to listeners, to rewrite in their words:
#                                                   release-notes.sh draft v0.4.0 > notes.md
#                                                   git tag -a v0.4.0 -F notes.md --cleanup=verbatim
#        release-notes.sh notes <tag> <summary>   prints the notes of the release, around the
#                                                 summary file (empty when the tag has none).
# The release follows the previous `v*` tag. <tag> may not exist yet: the draft then covers the
# commits up to HEAD.
set -euo pipefail

usage() {
  echo "usage: $0 draft <tag> | notes <tag> <summary file>" >&2
  exit 2
}
[ $# -ge 2 ] || usage
mode=$1 tag=$2
case $mode in
  draft) [ $# -eq 2 ] || usage ;;
  notes) [ $# -eq 3 ] || usage ;;
  *) usage ;;
esac

cd "$(git rev-parse --show-toplevel)"

if git rev-parse -q --verify "refs/tags/$tag" > /dev/null; then
  end=$tag
  previous=$(git describe --tags --abbrev=0 --match 'v[0-9]*' "$tag^" 2> /dev/null || true)
else
  end=HEAD
  previous=$(git describe --tags --abbrev=0 --match 'v[0-9]*' HEAD 2> /dev/null || true)
fi

case $mode in
  draft)
    echo "Soundfolk ${tag#v}"
    echo
    # Without merges, version bumps ("Release x.y.z") and the areas that never change what a
    # listener sees; the rest is a starting point, to rewrite.
    git log --reverse --no-merges --format='%s' "${previous:+$previous..}$end" \
      | grep -Eiv '^(release [0-9]|(ci|deps|todo|release|tooling|stylelint|readme|docs|repo|dco|claude|build)(:| ))' \
      | sed 's/^/- /' || true
    ;;
  notes)
    summary=$3
    repo=${GITHUB_REPOSITORY:-getsoundfolk/soundfolk}
    if grep -q '[^[:space:]]' "$summary"; then
      # A summary that brings its own heading keeps it.
      if ! grep -m1 '[^[:space:]]' "$summary" | grep -q '^#'; then
        echo "## What's new"
        echo
      fi
      cat "$summary"
      echo
    fi
    sed "s/{{version}}/${tag#v}/g" .github/release-notes.md
    if [ -n "$previous" ]; then
      echo
      echo "**Full Changelog**: https://github.com/$repo/compare/$previous...$tag"
    fi
    ;;
esac
