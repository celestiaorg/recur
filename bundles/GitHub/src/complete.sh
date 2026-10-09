#!/usr/bin/env bash
# Arg complete for /github: open issues involving you, filtered by the typed
# term. Composer: RECUR_PROMPT_COMPLETE_ARG + RECUR_PROMPT_ARGS.
set -euo pipefail

slot="${RECUR_PROMPT_COMPLETE_ARG:-issue}"
if [ "$slot" != "issue" ] || ! command -v gh >/dev/null 2>&1; then
  printf '%s\n' '{"hits":[]}'
  exit 0
fi

export RECUR_GH_TERM="${RECUR_PROMPT_ARG_ISSUE:-${RECUR_PROMPT_ARGS:-}}"
python3 -c '
import json, os, subprocess

term = (os.environ.get("RECUR_GH_TERM") or "").strip().lstrip("#")
cmd = ["gh", "search", "issues", "--involves", "@me", "--state", "open", "--limit", "15",
       "--json", "number,title,repository"]
if len(term) >= 2:
    cmd.insert(3, term)
try:
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=3.5)
    rows = json.loads(p.stdout) if p.returncode == 0 else []
except Exception as e:
    print(json.dumps({"hits": [], "message": str(e)}))
    raise SystemExit(0)

hits = []
for r in rows:
    repo = (r.get("repository") or {}).get("nameWithOwner") or ""
    number = r.get("number")
    key = f"{repo}#{number}"
    hits.append({
        "insert": key,
        "label": (r.get("title") or "").strip() or key,
        "psub": key,
        "detail": key,
        "badge": "Issue",
    })
print(json.dumps({"hits": hits}))
'
