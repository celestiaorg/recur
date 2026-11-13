#!/bin/sh
# One place to manage the release version. The workspace version in Cargo.toml
# is the source of truth; this keeps Cargo.lock and the package.json files in
# step with it.
#   build/set-version.sh X.Y.Z   bump everything
#   build/set-version.sh --check fail if anything disagrees with Cargo.toml
set -eu
PKGS="src-web/package.json src-web/demo/package.json src-docs/package.json bundles/sdk/package.json"
want=$(build/version.sh --base)
case "${1:-}" in
  --check)
    bad=0
    for f in $PKGS; do
      got=$(sed -n 's/^  "version": "\(.*\)",*$/\1/p' "$f")
      [ "$got" = "$want" ] || { echo "$f: $got, want $want" >&2; bad=1; }
    done
    got=$(grep -A1 '^name = "recur-cli"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')
    [ "$got" = "$want" ] || { echo "Cargo.lock: $got, want $want" >&2; bad=1; }
    [ "$bad" = 0 ] || { echo "run: mise run version:set $want" >&2; exit 1; }
    ;;
  [0-9]*.[0-9]*.[0-9]*)
    sed -i.bak '/^\[workspace\.package\]/,/^\[/s/^version = ".*"/version = "'"$1"'"/' Cargo.toml && rm Cargo.toml.bak
    for f in $PKGS; do sed -i.bak 's/^  "version": ".*"/  "version": "'"$1"'"/' "$f" && rm "$f.bak"; done
    cargo update --workspace
    ;;
  *) echo "usage: $0 X.Y.Z | --check" >&2; exit 2 ;;
esac
