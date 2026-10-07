#!/usr/bin/env bash
# Initial Review-mode prompt. Surface (workspace / local PR / GitHub PR) selects
# worktree git diff vs `gh pr diff`. RunPromptShebang copies this to a temp file
# — source via RECUR_BUNDLE_SUPPORT, not $0.
set -euo pipefail
. "${RECUR_BUNDLE_SUPPORT}/src/git-context.sh"
export RECUR_REVIEW_PASS=initial
python3 "${RECUR_BUNDLE_SUPPORT}/src/review-prompt.py"
