#!/bin/sh
# The version stamped into builds: $RECUR_VERSION, else the exact vX.Y.Z tag,
# else the last tag (or 0.0.0) plus +commit.<sha>.
set -eu
if [ -n "${RECUR_VERSION:-}" ]; then echo "$RECUR_VERSION"; exit; fi
if tag=$(git describe --tags --match 'v[0-9]*' --exact-match 2>/dev/null); then echo "${tag#v}"; exit; fi
tag=$(git describe --tags --match 'v[0-9]*' --abbrev=0 2>/dev/null || true)
v=${tag#v}
echo "${v:-0.0.0}+commit.$(git rev-parse --short=8 HEAD)"
