#!/usr/bin/env bash
# Re-review after fixes. Same surface-aware embed as review.sh; short directive only.
# RunPromptShebang copies this to a temp file — source via RECUR_BUNDLE_SUPPORT, not $0.
set -euo pipefail
. "${RECUR_BUNDLE_SUPPORT}/src/git-context.sh"
export RECUR_REVIEW_PASS=again
python3 "${RECUR_BUNDLE_SUPPORT}/src/review-prompt.py"
