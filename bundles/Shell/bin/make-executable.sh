#!/usr/bin/env bash
# chmod +x the file on disk (RECUR_FILEPATH). No buffer mutation.
path="${RECUR_FILEPATH:-}"
if [[ -z "$path" ]]; then
  echo "No file path" >&2
  exit 1
fi
if [[ ! -e "$path" ]]; then
  echo "Not on disk: $path" >&2
  exit 1
fi
chmod +x "$path"
printf 'Made executable: %s\n' "$(basename "$path")"
