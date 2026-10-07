#!/usr/bin/env bash
# xmllint --format stdin; pass through if missing/failing.
if ! command -v xmllint >/dev/null 2>&1; then
  cat
  exit 0
fi
tmp=$(mktemp)
trap 'rm -f "$tmp" "$tmp.out"' EXIT
cat >"$tmp"
if xmllint --format "$tmp" >"$tmp.out" 2>/dev/null; then
  cat "$tmp.out"
else
  cat "$tmp"
fi
