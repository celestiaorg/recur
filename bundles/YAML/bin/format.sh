#!/usr/bin/env bash
# Format stdin as YAML (prettier, then PyYAML); pass through if missing/failing.
tmp=$(mktemp "${TMPDIR:-/tmp}/recur-yaml.XXXXXX")
trap 'rm -f "$tmp"' EXIT
cat >"$tmp"
ext="${RECUR_FILENAME##*.}"
case "$ext" in
  yml|yaml) ;;
  *) ext="yaml" ;;
esac
named="${tmp}.${ext}"
mv "$tmp" "$named"
trap 'rm -f "$named" "$named.out"' EXIT
if command -v prettier >/dev/null 2>&1; then
  if prettier --parser yaml "$named" >"$named.out" 2>/dev/null; then
    cat "$named.out"
    exit 0
  fi
fi
if command -v python3 >/dev/null 2>&1; then
  if python3 - "$named" >"$named.out" 2>/dev/null <<'PY'
import sys
try:
    import yaml
except ImportError:
    sys.exit(1)
path = sys.argv[1]
with open(path, encoding="utf-8") as f:
    data = yaml.safe_load(f)
yaml.dump(data, sys.stdout, sort_keys=False, allow_unicode=True, default_flow_style=False)
PY
  then
    cat "$named.out"
    exit 0
  fi
fi
cat "$named"
