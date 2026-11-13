#!/usr/bin/env bash
# Update the worktree with its merge base. Clean git merge/rebase emits an
# empty prompt (host toasts display, no LLM turn). Conflicts leave markers
# and emit a resolve prompt. Other git failures (dirty tree, etc.) emit an
# agent prompt with the failure so the model can finish the update.
# RunPromptShebang copies this to a temp file: source siblings via
# RECUR_BUNDLE_SUPPORT, not $0.
set -euo pipefail

root="${RECUR_WORKTREE:-.}"
cd "$root"

base="${RECUR_BASE_BRANCH:-}"
if [ -n "$base" ] && ! git rev-parse -q --verify "$base" >/dev/null 2>&1; then
  echo "base ref ${base} not found" >&2
  exit 1
fi
if [ -z "$base" ]; then
  . "${RECUR_BUNDLE_SUPPORT}/src/git-context.sh"
  base="$RECUR_GIT_BASE"
fi
if [ -z "$base" ] || ! git rev-parse -q --verify "$base" >/dev/null 2>&1; then
  echo "no merge base ref" >&2
  exit 1
fi
method="${RECUR_UPDATE_BASE_METHOD:-merge}"
if [ "$method" != "rebase" ]; then
  method="merge"
fi

in_progress=0
if git rev-parse -q --verify MERGE_HEAD >/dev/null 2>&1; then
  in_progress=1
  method="merge"
elif git rev-parse -q --verify REBASE_HEAD >/dev/null 2>&1; then
  in_progress=1
  method="rebase"
else
  rebase_merge="$(git rev-parse --git-path rebase-merge 2>/dev/null || true)"
  rebase_apply="$(git rev-parse --git-path rebase-apply 2>/dev/null || true)"
  if [ -n "$rebase_merge" ] && [ -d "$rebase_merge" ]; then
    in_progress=1
    method="rebase"
  elif [ -n "$rebase_apply" ] && [ -d "$rebase_apply" ]; then
    in_progress=1
    method="rebase"
  fi
fi

emit_ok() {
  python3 -c 'import json,sys; print(json.dumps({"prompt":"","display":sys.argv[1]}, ensure_ascii=False))' "$1"
}

_upd_cap() {
  local tmp
  tmp="$(mktemp)"
  head -c 100000 >"$tmp" 2>/dev/null || true
  cat "$tmp"
  if [ "$(wc -c <"$tmp" | tr -d ' ')" -ge 100000 ]; then
    printf '\n\n…(truncated; over 100000 bytes)…\n'
  fi
  rm -f "$tmp"
}

# An open GitHub PR gets the finished update pushed, matching the titlebar's
# clean Update with base (force-with-lease only after a rebase). The refspec is
# explicit: workspace branches track the base, so a bare push targets it.
push_line() {
  local num="${RECUR_PR_NUMBER:-0}" head="${RECUR_PR_BRANCH:-}"
  if [ "${RECUR_PR_SOURCE:-}" != "gh" ] || [ "$num" = "0" ] || [ -z "$num" ] || [ -z "$head" ]; then
    echo "Do not push."
    return
  fi
  local fork="If the PR comes from a fork, push to the fork's repository instead of origin."
  if [ "$method" = "rebase" ]; then
    echo "Then push to update GitHub PR #${num}: git push --force-with-lease=refs/heads/${head} origin HEAD:refs/heads/${head}. ${fork}"
  else
    echo "Then push to update GitHub PR #${num}: git push origin HEAD:refs/heads/${head} (do not force-push). ${fork}"
  fi
}

git_out=""
if [ "$in_progress" -eq 0 ]; then
  behind="$(git rev-list --count "HEAD..${base}")"
  if [ "$behind" = "0" ]; then
    emit_ok "Already up to date with ${base}"
    exit 0
  fi
  set +e
  if [ "$method" = "rebase" ]; then
    git_out="$(git rebase "$base" 2>&1)"
    git_ok=$?
  else
    git_out="$(git merge --no-edit "$base" 2>&1)"
    git_ok=$?
  fi
  set -e
  if [ "$git_ok" -eq 0 ]; then
    emit_ok "Updated with latest ${base}"
    exit 0
  fi
  if git rev-parse -q --verify MERGE_HEAD >/dev/null 2>&1; then
    in_progress=1
    method="merge"
  elif git rev-parse -q --verify REBASE_HEAD >/dev/null 2>&1; then
    in_progress=1
    method="rebase"
  else
    export _UPD_BASE="$base"
    export _UPD_METHOD="$method"
    export _UPD_GIT_ERR="$(printf '%s' "$git_out" | _upd_cap || true)"
    export _UPD_STATUS="$(git status --short 2>/dev/null | _upd_cap || true)"
    export _UPD_PUSH="$(push_line)"
    python3 - <<'PY'
import json, os

base = os.environ.get("_UPD_BASE", "")
method = os.environ.get("_UPD_METHOD", "merge")
git_err = os.environ.get("_UPD_GIT_ERR", "").strip()
status = os.environ.get("_UPD_STATUS", "")
prompt = "\n".join([
    f"Updating this worktree with {base} via git {method} failed.",
    "Git did not leave an in-progress merge/rebase: typically uncommitted local "
    "changes would be overwritten, or another git error aborted the operation.",
    "Bring the worktree up to date with "
    f"{base}. Preserve local uncommitted work: git stash it and restore it afterwards, never commit it. "
    f"Then complete the {method}. {os.environ.get('_UPD_PUSH', 'Do not push.')}",
    "",
    "## git output",
    git_err or "(empty)",
    "",
    "## git status --short",
    status.strip() or "(clean)",
])
print(json.dumps({
    "prompt": prompt,
    "display": f"Finish updating with {base}",
}, ensure_ascii=False))
PY
    exit 0
  fi
fi

# Name the incoming side of an in-progress merge/rebase (Pull updates uses
# origin/<pr-branch>, not the workspace merge base). Fall back to $base.
conflict_base="$base"
if [ "$method" = "merge" ] && git rev-parse -q --verify MERGE_HEAD >/dev/null 2>&1; then
  named="$(git name-rev --name-only --refs='refs/remotes/*' MERGE_HEAD 2>/dev/null || true)"
  if [ -n "$named" ] && [ "$named" != "undefined" ]; then
    conflict_base="$named"
  fi
elif [ "$method" = "rebase" ]; then
  onto_file="$(git rev-parse --git-path rebase-merge/onto 2>/dev/null || true)"
  if [ -z "$onto_file" ] || [ ! -f "$onto_file" ]; then
    onto_file="$(git rev-parse --git-path rebase-apply/onto 2>/dev/null || true)"
  fi
  if [ -n "$onto_file" ] && [ -f "$onto_file" ]; then
    onto="$(tr -d '[:space:]' <"$onto_file")"
    if [ -n "$onto" ]; then
      named="$(git name-rev --name-only --refs='refs/remotes/*' "$onto" 2>/dev/null || true)"
      if [ -n "$named" ] && [ "$named" != "undefined" ]; then
        conflict_base="$named"
      fi
    fi
  fi
fi

export _UPD_BASE="$conflict_base"
export _UPD_METHOD="$method"
export _UPD_STATUS="$(git status --short 2>/dev/null | _upd_cap || true)"
export _UPD_UNMERGED="$(git diff --name-only --diff-filter=U 2>/dev/null | _upd_cap || true)"
export _UPD_PUSH="$(push_line)"

python3 - <<'PY'
import json, os

base = os.environ.get("_UPD_BASE", "")
method = os.environ.get("_UPD_METHOD", "merge")
status = os.environ.get("_UPD_STATUS", "")
unmerged = os.environ.get("_UPD_UNMERGED", "")
if method == "rebase":
    finish = (
        "git add the resolved files, then git rebase --continue "
        "(GIT_EDITOR=true is fine). Do not git rebase --abort unless the user asks."
    )
else:
    finish = (
        "git add the resolved files, then git commit --no-edit "
        "(or git merge --continue) to finish the merge. "
        "Do not git merge --abort unless the user asks."
    )
prompt = "\n".join([
    f"This worktree has an in-progress {method} with {base} that produced conflict markers.",
    "Resolve every conflicted file: edit out the conflict markers and keep the correct combined result.",
    "Do not change feature code beyond what is needed to resolve conflicts.",
    finish,
    os.environ.get("_UPD_PUSH", "Do not push."),
    "",
    "## git status --short",
    status.strip() or "(clean)",
    "",
    "## Unmerged files",
    unmerged.strip() or "(none listed: check git status)",
])
print(json.dumps({
    "prompt": prompt,
    "display": f"Resolve conflicts with {base}",
}, ensure_ascii=False))
PY
