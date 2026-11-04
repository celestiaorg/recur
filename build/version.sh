#!/bin/sh
# The version stamped into builds: $RECUR_VERSION, else the workspace version in
# Cargo.toml (the single source of truth, so cargo's output and `recur --version`
# agree). An exact vX.Y.Z tag must match it; any other commit gets +commit.<sha>.
set -eu
if [ -n "${RECUR_VERSION:-}" ]; then echo "$RECUR_VERSION"; exit; fi
v=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p;}' Cargo.toml)
if tag=$(git describe --tags --match 'v[0-9]*' --exact-match 2>/dev/null); then
  [ "${tag#v}" = "$v" ] || { echo "tag $tag does not match Cargo.toml version $v" >&2; exit 1; }
  echo "$v"; exit
fi
echo "$v+commit.$(git rev-parse --short=8 HEAD)"
