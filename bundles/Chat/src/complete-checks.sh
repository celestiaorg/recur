#!/usr/bin/env bash
# Arg complete for /fix-checks. Hits: "all" plus failing GitHub check names.
# Composer: RECUR_PROMPT_COMPLETE_ARG + RECUR_PROMPT_ARG_CHECK / RECUR_PROMPT_ARGS.
set -euo pipefail

slot="${RECUR_PROMPT_COMPLETE_ARG:-check}"
term="${RECUR_SEARCH_QUERY:-}"
if [ -z "$term" ]; then
  if [ "$slot" != "check" ]; then
    printf '%s\n' '{"hits":[]}'
    exit 0
  fi
  term="${RECUR_PROMPT_ARG_CHECK:-${RECUR_PROMPT_ARGS:-}}"
fi
term="$(printf '%s' "$term" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"

num="${RECUR_PR_NUMBER:-}"
if [ "${RECUR_PR_SOURCE:-}" != "gh" ] || [ -z "$num" ] || [ "$num" = "0" ]; then
  export RECUR_COMPLETE_TERM="$term"
  python3 - <<'PY'
import json, os

term = (os.environ.get("RECUR_COMPLETE_TERM") or "").strip().lower()
hits = []
if not term or term in "all":
    hits.append({
        "insert": "all",
        "label": "All failing checks",
        "detail": "Every failing GitHub check",
        "badge": "All",
    })
print(json.dumps({"hits": hits, "message": "No GitHub pull request on this workspace"}))
PY
  exit 0
fi

set +e
checks_json="$(gh pr checks "$num" --json name,state,bucket 2>/dev/null)"
set -e

export _RECUR_CHECKS_JSON="$checks_json"
export RECUR_COMPLETE_TERM="$term"
python3 - <<'PY'
import json, os

term = (os.environ.get("RECUR_COMPLETE_TERM") or "").strip().lower()
raw = os.environ.get("_RECUR_CHECKS_JSON") or ""


def bucket(row: dict) -> str:
    return (row.get("bucket") or "").strip().lower()


def is_fail(row: dict) -> bool:
    b = bucket(row)
    if b in ("fail", "cancel"):
        return True
    state = (row.get("state") or "").strip().upper()
    return state in ("FAILURE", "ERROR", "CANCELLED", "TIMED_OUT")


def matches(s: str) -> bool:
    if not term:
        return True
    return term in s.lower()


hits = []
if matches("all"):
    hits.append({
        "insert": "all",
        "label": "All failing checks",
        "detail": "Every failing GitHub check",
        "badge": "All",
    })

rows = []
if raw.strip():
    try:
        parsed = json.loads(raw)
        if isinstance(parsed, list):
            rows = [r for r in parsed if isinstance(r, dict)]
    except json.JSONDecodeError:
        rows = []

for row in rows:
    if not is_fail(row):
        continue
    name = (row.get("name") or "").strip()
    if not name or not matches(name):
        continue
    hits.append({
        "insert": name,
        "label": name,
        "detail": (row.get("bucket") or row.get("state") or "fail").strip(),
        "badge": "Check",
    })
    if len(hits) >= 25:
        break

print(json.dumps({"hits": hits}))
PY
