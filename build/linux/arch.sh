#!/bin/sh
# Prints $ARCH (default: the host's) as a Debian arch name, amd64 or arm64.
case "${ARCH:-$(uname -m)}" in
  x86_64 | amd64) echo amd64 ;;
  aarch64 | arm64) echo arm64 ;;
  *) echo "unsupported arch ${ARCH:-$(uname -m)}" >&2; exit 1 ;;
esac
