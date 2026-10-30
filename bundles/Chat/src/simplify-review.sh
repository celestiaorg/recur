#!/usr/bin/env bash
# Over-engineering review. Same surface git embed as /review; files findings.
# RunPromptShebang copies this to a temp file: source via RECUR_BUNDLE_SUPPORT, not $0.
set -euo pipefail
. "${RECUR_BUNDLE_SUPPORT}/src/git-context.sh"
export RECUR_REVIEW_PASS=initial
export RECUR_REVIEW_KIND=simplify
python3 "${RECUR_BUNDLE_SUPPORT}/src/review-prompt.py"
