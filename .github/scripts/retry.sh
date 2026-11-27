#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Runs a command up to three times, 1 then 5 minutes apart, for the release steps that talk to
# GitHub: a server error there is often gone minutes later (the site's Pages deployment, run
# 45651066588 of getsoundfolk.github.io: "Failed to create deployment (status: 500)"). The last
# failure is the command's: a lasting one still fails the step. Only for a command that can be
# run again without harm, or that first cleans up what a failed try left.
#
# Usage: retry.sh <command> [<argument>...]
#    or: source retry.sh, then retry <command or shell function> [<argument>...]
# A shell function runs with `set -e`, in a subshell: its first failing command ends the try, and
# the variables it sets do not outlive it (print what the caller needs instead). Call retry as a
# command of its own, never in an `if` or after `&&` or `||`: there, bash ignores `set -e` down to
# the function, which would carry on past a failure.
# RETRY_DELAYS replaces the waits, in seconds ("60 300"; "0 0" to test).

retry() {
  local delays delay try=1 code errexit=
  read -r -a delays <<< "${RETRY_DELAYS:-60 300}"
  case $- in *e*) errexit=1 ;; esac
  for delay in "${delays[@]}" ''; do
    # Not in an `if` or after `||`: there, bash ignores `set -e`, also inside the function.
    set +e
    (
      set -e
      "$@"
    )
    code=$?
    if [ -n "$errexit" ]; then set -e; fi
    if [ "$code" -eq 0 ]; then
      return 0
    fi
    if [ -z "$delay" ]; then
      echo "::error::try $try of $1 failed (exit $code), giving up" >&2
      return "$code"
    fi
    echo "::warning::try $try of $1 failed (exit $code), trying again in ${delay}s" >&2
    sleep "$delay"
    try=$((try + 1))
  done
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -euo pipefail
  if [ $# -eq 0 ]; then
    echo "usage: $0 <command> [<argument>...]" >&2
    exit 2
  fi
  retry "$@"
fi
