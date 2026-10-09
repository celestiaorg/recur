#!/usr/bin/env bash
# Work-on-GitHub-issue prompt (/github and New Task paste-expand). Auth is the
# gh CLI's own login. cwd is the workspace worktree, so bare #12 resolves
# against that repo.
set -euo pipefail

args="${RECUR_PROMPT_ARG_ISSUE:-${RECUR_PROMPT_ARGS:-}}"
if [ -z "$(printf '%s' "$args" | tr -d '[:space:]')" ]; then
  echo 'usage: /github #12, owner/repo#12, or paste an issue URL' >&2
  exit 1
fi
if ! command -v gh >/dev/null 2>&1; then
  echo 'GitHub CLI (gh) not found. Install it and run gh auth login.' >&2
  exit 1
fi

export RECUR_GH_ARGS="$args"
python3 -c '
import json, os, re, subprocess, sys

USAGE = "usage: /github #12, owner/repo#12, or paste an issue URL"
URL = re.compile(r"^https?://github\.com/([^/\s]+/[^/\s]+)/(issues|pull)/(\d+)\S*", re.I)
FULL = re.compile(r"^([\w.-]+/[\w.-]+)#(\d+)$")
NUM = re.compile(r"^#?(\d+)$")


def fail(msg):
    sys.stderr.write(msg + "\n")
    raise SystemExit(1)


def parse(s):
    t = s.strip()
    if t.lower().startswith("/github") and (len(t) == 7 or t[7].isspace()):
        t = t[7:].strip()
    m = URL.match(t)
    if m:
        if m.group(2).lower() == "pull":
            fail("That is a pull request. Open it from Pull requests to create a workspace.")
        return m.group(1), m.group(3), t[m.end():].strip()
    first = t.split()[0] if t.split() else ""
    notes = t[len(first):].strip()
    m = FULL.match(first)
    if m:
        return m.group(1), m.group(2), notes
    m = NUM.match(first)
    if m:
        return "", m.group(1), notes
    fail(USAGE)


repo, num, notes = parse(os.environ.get("RECUR_GH_ARGS") or "")
cmd = ["gh", "issue", "view", num, "--json", "id,number,title,body,url,state,labels,assignees,comments"]
if repo:
    cmd += ["--repo", repo]
p = subprocess.run(cmd, capture_output=True, text=True, timeout=12)
if p.returncode != 0:
    fail((p.stderr or "").strip() or f"Could not load GitHub issue #{num}")
issue = json.loads(p.stdout)

url = issue.get("url") or ""
m = URL.match(url)
if m and m.group(2).lower() == "pull":
    fail("That is a pull request. Open it from Pull requests to create a workspace.")
number = issue.get("number") or num
key = f"{m.group(1)}#{number}" if m else f"#{number}"
title = (issue.get("title") or "").strip()
body = (issue.get("body") or "").strip() or "(no description)"
state = (issue.get("state") or "").strip()
labels = [l.get("name") for l in (issue.get("labels") or []) if l.get("name")]
assignees = [a.get("login") for a in (issue.get("assignees") or []) if a.get("login")]
comments = [c for c in (issue.get("comments") or []) if (c.get("body") or "").strip()][-80:]

nonce = "gh"
open_m, close_m = f"⟦raw:{nonce}⟧", f"⟦/raw:{nonce}⟧"


def scrub(s):
    return s.replace(open_m, "⟦raw​:" + nonce + "⟧").replace(close_m, "⟦/raw​:" + nonce + "⟧")


lines = [
    f"Work on GitHub issue {key} in this workspace. Implement what the issue asks; do not expand scope. Rename the git branch with mcp__recur__rename_branch so the topic includes the issue number (e.g. {num}-short-slug). If you open a pull request, put \"Fixes {key}\" in its body so merging closes the issue.",
    "",
]
if notes:
    lines += ["Additional instructions from the user:", notes, ""]
lines += [open_m, f"Issue: {scrub(key)}"]
if title:
    lines.append(f"Title: {scrub(title)}")
if url:
    lines.append(f"URL: {scrub(url)}")
if state:
    lines.append(f"State: {scrub(state)}")
if assignees:
    lines.append("Assignees: " + scrub(", ".join(assignees)))
if labels:
    lines.append("Labels: " + scrub(", ".join(labels)))
lines += ["", "## Description", scrub(body)]
if comments:
    lines += ["", "## Comments"]
    for c in comments:
        head = ((c.get("author") or {}).get("login") or "unknown").strip()
        if (c.get("createdAt") or "").strip():
            head += " · " + c["createdAt"].strip()
        lines += [scrub(head), scrub(c["body"].strip()), ""]
lines += [close_m, ""]

out = {"prompt": "\n".join(lines), "display": f"GitHub {key}" + (f" · {title}" if title else "")}
if issue.get("id"):
    out["issue"] = {"source": "github", "id": issue["id"], "key": key}
print(json.dumps(out, ensure_ascii=False))
'
