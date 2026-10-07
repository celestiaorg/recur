#!/usr/bin/env bash
# Diff the current selection/document against the clipboard into a unified diff.
set -euo pipefail
left=$(mktemp)
right=$(mktemp)
trap 'rm -f "$left" "$right"' EXIT
# Prefer stdin (resolved input); fall back to RECUR_SELECTED_TEXT.
if [ -t 0 ]; then
  printf '%s' "${RECUR_SELECTED_TEXT-}" >"$left"
else
  cat >"$left"
fi
printf '%s' "${RECUR_CLIPBOARD-}" >"$right"
# diff returns 1 when files differ — treat that as success.
diff -u --label selection --label clipboard "$left" "$right" || {
  code=$?
  if [ "$code" -eq 1 ]; then
    exit 0
  fi
  exit "$code"
}
