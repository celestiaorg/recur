#!/usr/bin/env bash
# Run stdin as a bash script; stdout replaces the input.
tmp=$(mktemp)
err=$(mktemp)
trap 'rm -f "$tmp" "$err"' EXIT
cat >"$tmp"
if ! bash "$tmp" 2>"$err"; then
  cat "$err" >&2
  exit 1
fi
