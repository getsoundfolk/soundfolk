#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Re-runs the failed jobs of a CI or Release run when none of them ever got a runner: GitHub gave up
# queuing them ("The job was not acquired by Runner of type hosted even after multiple attempts",
# macOS arm64 in CI runs 42651821949 and 43246302663, Release run 45408119818). Such a job has no
# runner name and no step. If any failed job did run, nothing is re-run: a real failure stays
# visible. It waits first, longer after each attempt (20 minutes, 1 hour, 2 hours): a re-run on the
# spot lands in the same shortage, and there is no other pool to fall back on (GitHub's Intel macOS
# runners are going away; larger runners are paid). It leaves alone a run that a newer run of the same workflow and
# branch replaced: re-running an old nightly would publish it over the newer one. Called by
# ci-retry.yml.
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

case $attempt in
  1) wait=20 ;;
  2) wait=60 ;;
  *) wait=120 ;;
esac
if [ "$dry_run" = --dry-run ]; then
  echo "(dry run: would wait $wait minutes first)"
else
  echo "Waiting $wait minutes for runners to free up"
  sleep $((wait * 60))
fi

# Checked after the wait: a newer run may have started meanwhile.
read -r workflow branch < <(gh api "repos/$repo/actions/runs/$run_id" --jq '"\(.workflow_id) \(.head_branch)"')
newest=$(gh api "repos/$repo/actions/workflows/$workflow/runs?branch=$branch&per_page=1" \
  --jq '.workflow_runs[0].id')
if [ "$newest" != "$run_id" ]; then
  summary "Run $run_id, attempt $attempt: run $newest of the same workflow on $branch replaced it, nothing re-run."
  exit 0
fi

summary "Run $run_id, attempt $attempt: these jobs never got a runner, re-running them:"
summary "$unacquired"
if [ "$dry_run" = --dry-run ]; then
  echo "(dry run: nothing re-run)"
else
  gh run rerun "$run_id" --failed --repo "$repo"
fi
