#!/usr/bin/env bash
# bash -n on stdin; toast OK or fail with stderr.
tmp=$(mktemp)
err=$(mktemp)
trap 'rm -f "$tmp" "$err"' EXIT
cat >"$tmp"
if bash -n "$tmp" 2>"$err"; then
  printf 'Shell syntax OK\n'
else
  cat "$err" >&2
  exit 1
fi
