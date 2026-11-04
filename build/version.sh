#!/bin/sh
# The version stamped into builds: $RECUR_VERSION, else the workspace version in
# Cargo.toml (the single source of truth; set it with `mise run version:set`).
# An exact vX.Y.Z tag must match it. Any other commit is the last release plus
# +commit.<sha>, so a dev build is never mistaken for a release.
# `--base` prints just the Cargo.toml version.
set -eu
v=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p;}' Cargo.toml)
if [ "${1:-}" = --base ]; then echo "$v"; exit; fi
if [ -n "${RECUR_VERSION:-}" ]; then echo "$RECUR_VERSION"; exit; fi
if tag=$(git describe --tags --match 'v[0-9]*' --exact-match 2>/dev/null); then
  [ "${tag#v}" = "$v" ] || { echo "tag $tag does not match Cargo.toml version $v" >&2; exit 1; }
  echo "$v"; exit
fi
echo "$v+commit.$(git rev-parse --short=8 HEAD)"
