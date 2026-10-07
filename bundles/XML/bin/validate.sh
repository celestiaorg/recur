#!/usr/bin/env bash
# Well-formedness check (xmllint --noout). Does not mutate the buffer.
if ! command -v xmllint >/dev/null 2>&1; then
  echo "xmllint not found" >&2
  exit 1
fi
tmp=$(mktemp)
err=$(mktemp)
trap 'rm -f "$tmp" "$err"' EXIT
cat >"$tmp"
if xmllint --noout "$tmp" 2>"$err"; then
  printf 'Valid XML\n'
else
  cat "$err" >&2
  exit 1
fi
