#!/usr/bin/env bash
# Whole-repo over-engineering audit. Files findings; do not apply cuts.
# RunPromptShebang copies this to a temp file — source via RECUR_BUNDLE_SUPPORT, not $0.
set -euo pipefail
export RECUR_REVIEW_PASS=initial
export RECUR_REVIEW_KIND=audit
# Do not source git-context.sh: that embeds up to ~200KB of diffs and tells
# the agent to prefer them. Audit scans the tree. GitHub PR cwd is not the
# PR head — review-prompt.py refuses without a gh round-trip.
if [ "${RECUR_SURFACE:-}" != "github_pr" ]; then
  RECUR_GIT_BRANCH="$(git -C "${RECUR_WORKTREE:-.}" rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
  export RECUR_GIT_BRANCH
fi
python3 "${RECUR_BUNDLE_SUPPORT}/src/review-prompt.py"
