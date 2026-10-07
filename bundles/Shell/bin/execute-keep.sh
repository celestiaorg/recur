#!/usr/bin/env bash
# Run stdin as bash; keep the source and append stdout below it.
tmp=$(mktemp)
out=$(mktemp)
err=$(mktemp)
trap 'rm -f "$tmp" "$out" "$err"' EXIT
cat >"$tmp"
if ! bash "$tmp" >"$out" 2>"$err"; then
  cat "$err" >&2
  exit 1
fi
cat "$tmp"
last=$(tail -c 1 "$tmp" 2>/dev/null || true)
if [[ -s "$tmp" && -n "$last" ]]; then
  printf '\n'
fi
cat "$out"
