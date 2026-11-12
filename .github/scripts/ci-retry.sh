#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Re-runs the failed jobs of a CI run when none of them ever got a runner: GitHub gave up queuing
# them ("The job was not acquired by Runner of type hosted even after multiple attempts", macOS
# arm64 in runs 42651821949 and 43246302663). Such a job has no runner name and no step. If any
# failed job did run, nothing is re-run: a real failure stays visible. Called by ci-retry.yml.
#
# Usage: ci-retry.sh REPO RUN_ID ATTEMPT [--dry-run]
#   --dry-run   says what it would do, re-runs nothing (to try it on past runs)
# Needs gh (GH_TOKEN with actions: write unless --dry-run) and jq. Writes a summary to
# $GITHUB_STEP_SUMMARY when set.
set -euo pipefail

if [ $# -lt 3 ]; then
  echo "usage: $0 REPO RUN_ID ATTEMPT [--dry-run]" >&2
  exit 2
fi
repo=$1 run_id=$2 attempt=$3 dry_run=${4:-}

summary() {
  echo "$1"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    echo "$1" >> "$GITHUB_STEP_SUMMARY"
  fi
}

# One line per job that neither succeeded nor was skipped: "true<TAB>name" when it never got a
# runner, "false<TAB>name" when it ran.
jobs=$(gh api --paginate "repos/$repo/actions/runs/$run_id/attempts/$attempt/jobs" \
  --jq '.jobs[] | select(.conclusion != "success" and .conclusion != "skipped")
        | "\((.runner_name // "") == "" and (.steps | length) == 0)\t\(.name)"')

if [ -z "$jobs" ]; then
  summary "Run $run_id, attempt $attempt: no failed job, nothing to re-run."
  exit 0
fi

ran=$(awk -F'\t' '$1 == "false" { print "- " $2 }' <<< "$jobs")
unacquired=$(awk -F'\t' '$1 == "true" { print "- " $2 }' <<< "$jobs")

if [ -n "$ran" ]; then
  summary "Run $run_id, attempt $attempt: these jobs ran and failed, so nothing is re-run:"
  summary "$ran"
  if [ -n "$unacquired" ]; then
    summary "These never got a runner (re-run them by hand once the failure is fixed):"
    summary "$unacquired"
  fi
  exit 0
fi

summary "Run $run_id, attempt $attempt: these jobs never got a runner, re-running them:"
summary "$unacquired"
if [ "$dry_run" = --dry-run ]; then
  echo "(dry run: nothing re-run)"
else
  gh run rerun "$run_id" --failed --repo "$repo"
fi
