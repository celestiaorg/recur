#!/usr/bin/env bash
# Sample shebang command: prefix each stdin line with "> ".
# Prefer this interpreter when you need PATH tools or the real filesystem.
set -euo pipefail
while IFS= read -r line || [ -n "${line}" ]; do
  printf '> %s\n' "${line}"
done
