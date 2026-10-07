#!/usr/bin/env bash
# Fix failing GitHub checks. Optional RECUR_PROMPT_ARG_CHECK / RECUR_PROMPT_ARGS
# is a check name to focus; empty or "all" means every failing check.
# RunPromptShebang copies this to a temp file — do not source $0.
#
# Fast-fail (empty prompt, toast display) when there is no GitHub PR, this
# cwd is a PR chat host (Ask/Plan/Review only), or gh reports nothing failing.
set -euo pipefail

emit() {
  python3 -c 'import json,sys; print(json.dumps({"prompt":"","display":sys.argv[1]}, ensure_ascii=False))' "$1"
}

if [ "${RECUR_PR_HOST:-}" = "1" ]; then
  emit "Create a workspace from this pull request to edit files"
  exit 0
fi

num="${RECUR_PR_NUMBER:-}"
if [ "${RECUR_PR_SOURCE:-}" != "gh" ] || [ -z "$num" ] || [ "$num" = "0" ]; then
  emit "No GitHub pull request on this workspace"
  exit 0
fi

set +e
checks_json="$(gh pr checks "$num" --json name,state,bucket,link,description 2>/dev/null)"
gh_ok=$?
set -e

export _RECUR_CHECKS_JSON="$checks_json"
export _RECUR_CHECKS_OK="$gh_ok"
python3 - <<'PY'
import json, os, secrets

num = (os.environ.get("RECUR_PR_NUMBER") or "").strip()
url = (os.environ.get("RECUR_PR_URL") or "").strip()
title = (os.environ.get("RECUR_PR_TITLE") or "").strip()
branch = (os.environ.get("RECUR_PR_BRANCH") or "").strip()
focus = (os.environ.get("RECUR_PROMPT_ARG_CHECK") or os.environ.get("RECUR_PROMPT_ARGS") or "").strip()
if focus.lower() in ("all", "*"):
    focus = ""
raw = os.environ.get("_RECUR_CHECKS_JSON") or ""
gh_ok = os.environ.get("_RECUR_CHECKS_OK") == "0"

nonce = secrets.token_hex(4)
open_m, close_m = f"⟦raw:{nonce}⟧", f"⟦/raw:{nonce}⟧"


def scrub(s: str) -> str:
    return s.replace(open_m, "⟦raw\u200b:" + nonce + "⟧").replace(close_m, "⟦/raw\u200b:" + nonce + "⟧")


def bucket(row: dict) -> str:
    return (row.get("bucket") or "").strip().lower()


def is_fail(row: dict) -> bool:
    b = bucket(row)
    if b in ("fail", "cancel"):
        return True
    state = (row.get("state") or "").strip().upper()
    return state in ("FAILURE", "ERROR", "CANCELLED", "TIMED_OUT")


def is_pending(row: dict) -> bool:
    b = bucket(row)
    if b == "pending":
        return True
    state = (row.get("state") or "").strip().upper()
    return state in ("PENDING", "QUEUED", "IN_PROGRESS", "WAITING", "EXPECTED")


rows = []
if raw.strip():
    try:
        parsed = json.loads(raw)
        if isinstance(parsed, list):
            rows = [r for r in parsed if isinstance(r, dict)]
    except json.JSONDecodeError:
        rows = []

fail = [r for r in rows if is_fail(r)]
pending = [r for r in rows if is_pending(r)]

if focus:
    key = focus.lower()
    named = [r for r in rows if (r.get("name") or "").lower() == key]
    named_fail = [r for r in named if is_fail(r)]
    named_pending = [r for r in named if is_pending(r)]
    if named and not named_fail and not named_pending:
        print(json.dumps({"prompt": "", "display": f"{focus} is passing"}, ensure_ascii=False))
        raise SystemExit(0)
    if named_pending and not named_fail:
        print(json.dumps({"prompt": "", "display": f"{focus} is still running"}, ensure_ascii=False))
        raise SystemExit(0)
    if named_fail:
        fail = named_fail
elif not fail:
    if pending:
        print(json.dumps({"prompt": "", "display": "Checks still running"}, ensure_ascii=False))
        raise SystemExit(0)
    if gh_ok or rows:
        print(json.dumps({"prompt": "", "display": "All checks passed"}, ensure_ascii=False))
        raise SystemExit(0)

pr = f"GitHub pull request #{num}"
if url:
    pr += f" ({url})"

embed = json.dumps(fail, ensure_ascii=False) if fail else (raw.strip() or "(gh pr checks failed; run it yourself)")

raw_lines = []
if title:
    raw_lines.append(f"PR title: {scrub(title)}")
if focus:
    raw_lines.append(f'Focus on the check named "{scrub(focus)}". Other failures are omitted — run `gh pr checks` if you need them.')
else:
    raw_lines.append("Fix every failing check below.")
raw_lines += ["", "## failing gh pr checks", scrub(embed)]

parts = [
    f"The user asked you to fix failing GitHub checks on {pr}.",
    "",
    f"The PR head branch is {branch or 'unknown'}.",
    "",
    "Follow these steps:",
    "",
    "- If you have skills related to CI failures, invoke them now. Instructions there should take precedence.",
    "- Failing checks are listed below (passing/pending omitted). Run `gh pr checks` yourself if you need the full rollup.",
    "- For each failing check, open its job and read the failed logs (`gh run view --log-failed`, or the check URL). Investigate one check at a time.",
    "- Fix the root cause in this worktree. Do not paper over it in CI YAML unless the workflow itself is wrong.",
    "- After the fix, commit with a message that names the check. If this PR is already on GitHub, push (do not force-push) so checks re-run.",
    "- If any of these steps fail, call mcp__recur__ask_user for help.",
    "",
    open_m,
    *raw_lines,
    close_m,
]

if fail:
    display = f"Fix failing check: {focus}" if focus else (
        f"Fix {len(fail)} failing GitHub check" + ("s" if len(fail) != 1 else "")
    )
else:
    display = f"Fix failing check: {focus}" if focus else "Fix failing GitHub checks"

print(json.dumps({"prompt": "\n".join(parts), "display": display}, ensure_ascii=False))
PY
