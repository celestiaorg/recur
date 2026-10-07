#!/usr/bin/env bash
# Work-on-Linear-issue prompt. API key from [[setting]] api_key
# (RECUR_SETTING_API_KEY / RECUR_BUNDLE_SETTINGS). RunPromptShebang copies
# this to a temp file — use RECUR_BUNDLE_SUPPORT, not $0.
set -euo pipefail

args="${RECUR_PROMPT_ARG_ISSUE:-${RECUR_PROMPT_ARGS:-}}"
args="$(printf '%s' "$args" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [ -z "$args" ]; then
  echo 'usage: /linear TEAM-123 or paste a Linear URL' >&2
  exit 1
fi

key="${RECUR_SETTING_API_KEY:-}"
if [ -z "$key" ]; then
  echo 'Linear API key is missing or invalid — Settings → Integrations' >&2
  exit 1
fi

export RECUR_LINEAR_KEY="$key"
export RECUR_LINEAR_TEAM="${RECUR_SETTING_TEAM_KEY:-}"
export RECUR_LINEAR_ARGS="$args"
python3 -c '
import json, os, re, sys, urllib.error, urllib.parse, urllib.request

USAGE = "usage: /linear TEAM-123 or paste a Linear URL"
IDENT = re.compile(r"^[A-Z][A-Z0-9]+-\d+$")
IDENT_FIND = re.compile(r"[A-Z][A-Z0-9]+-\d+")
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.I)
BARE = re.compile(r"^\d+$")
ISSUE_QUERY = """query Issue($id: String!) {
  issue(id: $id) {
    identifier
    title
    description
    url
    state { name type }
    assignee { name displayName }
    team { key }
    labels { nodes { name } }
    comments(first: 80) {
      nodes {
        body
        createdAt
        user { name displayName }
        externalUser { name }
        parent { id }
      }
    }
    attachments(first: 20) {
      nodes { title url }
    }
  }
}"""


def fail(msg: str, code: int = 1) -> None:
    sys.stderr.write(msg + "\n")
    raise SystemExit(code)


def identifier_from_url(raw: str):
    u = urllib.parse.urlparse(raw)
    host = (u.hostname or "").lower()
    if host != "linear.app" and not host.endswith(".linear.app"):
        return None
    parts = [p for p in u.path.split("/") if p]
    for i, p in enumerate(parts):
        if p == "issue" and i + 1 < len(parts):
            cand = parts[i + 1]
            if IDENT.match(cand) or UUID.match(cand):
                return cand
    return None


def strip_slash_linear(s: str) -> str:
    t = s.strip()
    if not t.lower().startswith("/linear"):
        return t
    rest = t[7:]
    if rest == "" or rest[:1].isspace():
        return rest.strip()
    return t


def split_url(s: str):
    t = s.strip()
    low = t.lower()
    if not (low.startswith("http://") or low.startswith("https://")):
        return None, ""
    i = 0
    while i < len(t) and not t[i].isspace():
        i += 1
    return t[:i], t[i:].strip()


def split_ref(s: str, team_key: str):
    s = strip_slash_linear(s)
    if not s:
        return None, None
    raw, rest = split_url(s)
    if raw:
        ident = identifier_from_url(raw)
        if ident:
            return ident, rest
    fields = s.split()
    if not fields:
        return None, None
    first = fields[0]
    notes = s[len(first):].strip()
    if IDENT.match(first) or UUID.match(first):
        return first, notes
    tk = team_key.strip()
    if BARE.match(first) and tk:
        return tk.upper() + "-" + first, notes
    m = IDENT_FIND.search(s)
    if m and IDENT.match(m.group(0)):
        leftover = (s[: m.start()] + s[m.end() :]).strip()
        return m.group(0), leftover
    return None, None


args = (os.environ.get("RECUR_LINEAR_ARGS") or "").strip()
team = os.environ.get("RECUR_LINEAR_TEAM") or ""
ident, notes = split_ref(args, team)
if not ident:
    fail(USAGE)

body = json.dumps({"query": ISSUE_QUERY, "variables": {"id": ident}}).encode()
req = urllib.request.Request(
    "https://api.linear.app/graphql",
    data=body,
    headers={
        "Content-Type": "application/json",
        "Authorization": os.environ.get("RECUR_LINEAR_KEY") or "",
        # JWT query param on uploads.linear.app URLs so the agent can fetch
        # images without a Linear login. 7d; bump if a long-running session
        # starts hitting 403s on stale signatures.
        "public-file-urls-expire-in": "604800",
    },
    method="POST",
)
try:
    with urllib.request.urlopen(req, timeout=10) as resp:
        payload = json.loads(resp.read().decode())
except urllib.error.HTTPError as e:
    fail(f"Could not load Linear issue {ident}: HTTP {e.code}")
except Exception as e:
    fail(f"Could not load Linear issue {ident}: {e}")

if payload.get("errors"):
    msg = ((payload["errors"][0] or {}).get("message")) or "Linear GraphQL error"
    low = msg.lower()
    if "auth" in low or "unauthorized" in low:
        fail("Linear API key is missing or invalid — Settings → Integrations")
    fail(msg)

issue = ((payload.get("data") or {}).get("issue")) or None
if not issue:
    fail(f"Linear issue {ident} not found")

title = (issue.get("title") or "").strip()
ident = (issue.get("identifier") or ident).strip() or "issue"
url = issue.get("url") or ""
desc = (issue.get("description") or "").strip() or "(no description)"
state = ((issue.get("state") or {}).get("name") or "").strip()
state_type = ((issue.get("state") or {}).get("type") or "").strip()
assignee = ""
if issue.get("assignee"):
    assignee = (issue["assignee"].get("displayName") or "").strip() or (issue["assignee"].get("name") or "").strip()
team_key = ((issue.get("team") or {}).get("key") or "").strip()
labels = [n.get("name") for n in ((issue.get("labels") or {}).get("nodes") or []) if n.get("name")]


def comment_author(n):
    u = n.get("user") or {}
    name = (u.get("displayName") or "").strip() or (u.get("name") or "").strip()
    if name:
        return name
    ext = n.get("externalUser") or {}
    return (ext.get("name") or "").strip() or "Unknown"


raw_desc = issue.get("description") or ""
atts = []
for n in ((issue.get("attachments") or {}).get("nodes") or []):
    u = (n.get("url") or "").strip()
    if u and u not in raw_desc:
        atts.append(((n.get("title") or "").strip(), u))
comments = []
for n in ((issue.get("comments") or {}).get("nodes") or [])[:80]:
    if (n.get("body") or "").strip():
        comments.append(n)

display = "Linear " + ident
if title:
    display += " · " + title

nonce = "lin"
open_m, close_m = f"⟦raw:{nonce}⟧", f"⟦/raw:{nonce}⟧"


def scrub(s: str) -> str:
    return s.replace(open_m, "⟦raw\u200b:" + nonce + "⟧").replace(close_m, "⟦/raw\u200b:" + nonce + "⟧")


slug = ident.lower()
lines = [
    f"Work on Linear issue {ident} in this workspace. Implement what the issue asks; do not expand scope. Rename the git branch with mcp__recur__rename_branch so the topic includes {ident} (e.g. {slug}-short-slug) — Linear attaches that branch and later PRs to the issue. Mention {ident} in the PR body if you open a pull request.",
    "",
]
if notes:
    lines += ["Additional instructions from the user:", notes, ""]
lines += [open_m, f"Identifier: {scrub(ident)}"]
if title:
    lines.append(f"Title: {scrub(title)}")
if url:
    lines.append(f"URL: {scrub(url)}")
if state:
    st = scrub(state) + (f" ({scrub(state_type)})" if state_type else "")
    lines.append(f"State: {st}")
if assignee:
    lines.append(f"Assignee: {scrub(assignee)}")
if team_key:
    lines.append(f"Team: {scrub(team_key)}")
if labels:
    lines.append("Labels: " + scrub(", ".join(labels)))
lines += ["", "## Description", scrub(desc)]
if atts:
    lines += ["", "## Attachments"]
    for title, u in atts:
        lines.append(scrub(f"{title} — {u}" if title else u))
if comments:
    lines += ["", "## Comments"]
    for n in comments:
        head = comment_author(n)
        created = (n.get("createdAt") or "").strip()
        if created:
            head += " · " + created
        if n.get("parent"):
            head += " (reply)"
        lines += [scrub(head), scrub((n.get("body") or "").strip()), ""]
lines += [close_m, ""]
print(json.dumps({"prompt": "\n".join(lines), "display": display}, ensure_ascii=False))
'
