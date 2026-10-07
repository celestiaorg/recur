#!/usr/bin/env bash
# Arg complete for /linear and [[search]] ⌘K. Key: RECUR_SETTING_API_KEY.
# Composer: RECUR_PROMPT_COMPLETE_ARG + RECUR_PROMPT_ARGS. ⌘K: RECUR_SEARCH_QUERY.
set -euo pipefail

slot="${RECUR_PROMPT_COMPLETE_ARG:-issue}"
term="${RECUR_SEARCH_QUERY:-}"
if [ -z "$term" ]; then
  if [ "$slot" != "issue" ]; then
    printf '%s\n' '{"hits":[]}'
    exit 0
  fi
  term="${RECUR_PROMPT_ARG_ISSUE:-${RECUR_PROMPT_ARGS:-}}"
fi
term="$(printf '%s' "$term" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"

key="${RECUR_SETTING_API_KEY:-}"
if [ -z "$key" ]; then
  printf '%s\n' '{"hits":[{"label":"Connect Linear…","detail":"Settings → Integrations","action":"settings:integrations"}],"message":"Linear API key missing"}'
  exit 0
fi

export RECUR_LINEAR_KEY="$key"
export RECUR_LINEAR_TERM="$term"
python3 -c '
import json, os, urllib.request

term = (os.environ.get("RECUR_LINEAR_TERM") or "").strip()
key = os.environ.get("RECUR_LINEAR_KEY") or ""
if len(term) < 2:
    print(json.dumps({"hits": []}))
    raise SystemExit(0)

body = json.dumps({
    "query": """query Search($term: String!, $first: Int!) {
      searchIssues(term: $term, first: $first) {
        nodes { identifier title state { name } }
      }
    }""",
    "variables": {"term": term, "first": 15},
}).encode()
req = urllib.request.Request(
    "https://api.linear.app/graphql",
    data=body,
    headers={"Content-Type": "application/json", "Authorization": key},
    method="POST",
)
try:
    with urllib.request.urlopen(req, timeout=3) as resp:
        payload = json.loads(resp.read().decode())
except Exception as e:
    print(json.dumps({"hits": [], "message": str(e)}))
    raise SystemExit(0)

if payload.get("errors"):
    msg = (payload["errors"][0] or {}).get("message") or "Linear GraphQL error"
    low = msg.lower()
    if "auth" in low or "unauthorized" in low:
        print(json.dumps({
            "hits": [{"label": "Connect Linear…", "detail": "Settings → Integrations", "action": "settings:integrations"}],
            "message": "Linear API key missing or invalid",
        }))
        raise SystemExit(0)
    print(json.dumps({"hits": [], "message": msg}))
    raise SystemExit(0)

nodes = (((payload.get("data") or {}).get("searchIssues") or {}).get("nodes")) or []
hits = []
for n in nodes:
    ident = (n.get("identifier") or "").strip()
    if not ident:
        continue
    title = (n.get("title") or "").strip()
    state = ((n.get("state") or {}).get("name") or "").strip()
    hits.append({
        "insert": ident,
        "label": title or ident,
        "psub": ident,
        "detail": " · ".join(x for x in (ident, state) if x),
        "badge": "Issue",
    })
print(json.dumps({"hits": hits}))
'
