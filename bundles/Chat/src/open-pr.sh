#!/usr/bin/env bash
# Open-PR prompt with worktree diff embedded so the agent can write the GitHub
# description without a discovery loop. RunPromptShebang copies this to a temp
# file — source via RECUR_BUNDLE_SUPPORT, not $0.
#
# /open-draft-pr shares this script (RECUR_PR_DRAFT=1 or slash open-draft-pr)
# and asks the agent to pass draft:true on mcp__recur__open_pr.
#
# If Recur already has an open GitHub PR for this workspace, emit an empty
# prompt (host toasts display, no LLM turn) so a second /open-pr does not
# waste tokens rewriting the description. Push new commits from the titlebar
# instead. A Local fallback (gh create failed) is not treated as open — the
# agent must be able to retry mcp__recur__open_pr.
set -euo pipefail

if [ -n "${RECUR_PR_ID:-}" ] && [ "${RECUR_PR_SOURCE:-}" = "gh" ]; then
  num="${RECUR_PR_NUMBER:-}"
  if [ -n "$num" ] && [ "$num" != "0" ]; then
    python3 -c 'import json,sys; print(json.dumps({"prompt":"","display":sys.argv[1]}, ensure_ascii=False))' "PR #${num} is already open"
  else
    python3 -c 'import json,sys; print(json.dumps({"prompt":"","display":sys.argv[1]}, ensure_ascii=False))' "A pull request is already open"
  fi
  exit 0
fi

. "${RECUR_BUNDLE_SUPPORT}/src/git-context.sh"

python3 - <<'PY'
import json, os, secrets

base = os.environ.get("RECUR_GIT_BASE", "")
merge_base = os.environ.get("RECUR_GIT_MERGE_BASE", "")
branch = os.environ.get("RECUR_GIT_BRANCH", "")
status = os.environ.get("_RECUR_STATUS", "")
stat = os.environ.get("_RECUR_STAT", "")
stat_work = os.environ.get("_RECUR_STAT_WORK", "")
diff_commits = os.environ.get("_RECUR_DIFF_COMMITS", "")
diff_work = os.environ.get("_RECUR_DIFF_WORK", "")
untracked = os.environ.get("_RECUR_UNTRACKED", "")
draft = os.environ.get("RECUR_PR_DRAFT") == "1" or os.environ.get("RECUR_PROMPT_SLASH") == "open-draft-pr"

nonce = secrets.token_hex(4)
open_m, close_m = f"⟦raw:{nonce}⟧", f"⟦/raw:{nonce}⟧"

def scrub(s: str) -> str:
    return s.replace(open_m, "⟦raw\u200b:" + nonce + "⟧").replace(close_m, "⟦/raw\u200b:" + nonce + "⟧")

kind = "draft pull request" if draft else "pull request"
open_step = (
    "- Call mcp__recur__open_pr with draft set to true, a title under 80 characters, and a markdown body (keep the description under five sentences unless the change is large). Recur pushes the branch and opens a GitHub draft PR when this project opts in (otherwise a Local PR). Do not run `gh pr create` yourself."
    if draft else
    "- Call mcp__recur__open_pr with a title under 80 characters and a markdown body (keep the description under five sentences unless the change is large). Recur pushes the branch and opens the GitHub PR when this project opts in (otherwise a Local PR). Do not run `gh pr create` yourself."
)

parts = [
    f"The user requested a {kind} for this workspace.",
    "",
    f"The current branch is {branch}.",
    f"The target branch is {base}.",
    "",
    "Follow these steps to create a PR:",
    "",
    "- If you have any skills related to creating PRs, invoke them now. Instructions there should take precedence over these instructions.",
    "- If git status shows uncommitted changes, review them and commit them. Write a clear message from the diff. Do not amend unless the user asked. Uncommitted work is not included in the PR.",
    "- Git context is already gathered below — do NOT re-run git status/diff/log unless something looks incomplete. Describe not just this chat turn but ALL changes versus the base.",
    open_step,
    "- If any of these steps fail, call mcp__recur__ask_user for help.",
    "",
    open_m,
    f"Current branch: {branch}",
    f"Compare base: {base} (merge-base {merge_base})",
    "",
    "## git status --short",
    scrub(status.strip() or "(clean)"),
    "",
    "## Untracked files",
    scrub(untracked.strip() or "(none)"),
    "",
    f"## git diff --stat {merge_base}...HEAD (committed vs base)",
    scrub(stat.strip() or "(empty)"),
    "",
    "## git diff --stat (uncommitted)",
    scrub(stat_work.strip() or "(empty)"),
    "",
    f"## git diff {merge_base}...HEAD",
    "```diff",
    scrub(diff_commits if diff_commits.strip() else "(empty)"),
    "```",
    "",
    "## git diff (uncommitted)",
    "```diff",
    scrub(diff_work if diff_work.strip() else "(empty)"),
    "```",
    close_m,
]
print(json.dumps({
    "prompt": "\n".join(parts),
    "display": "Open a draft pull request" if draft else "Open a pull request",
}, ensure_ascii=False))
PY
