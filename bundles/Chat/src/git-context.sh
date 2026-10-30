#!/usr/bin/env bash
# Shared git / PR snapshot for Chat prompt actions. Sourced by review.sh /
# re-review.sh and by update.sh when RECUR_BASE_BRANCH is unset.
# Sets: RECUR_GIT_BASE, RECUR_GIT_MERGE_BASE, RECUR_GIT_BRANCH, and _RECUR_* blobs.
# Blobs are capped before export so the python3 exec stays under ARG_MAX.

root="${RECUR_WORKTREE:-.}"
cd "$root"

_RECUR_BLOB_MAX=100000
# Shared budget for the two embedded diffs (committed + uncommitted) so a huge
# branch can't cost ~50K tokens of prompt before the reviewer reads a file.
_RECUR_DIFF_TOTAL_MAX=120000

# Cap stdin to $1 (default _RECUR_BLOB_MAX) bytes; append a note when truncated.
_recur_cap() {
  local max="${1:-$_RECUR_BLOB_MAX}" tmp
  tmp="$(mktemp)"
  # head closes early → producer may SIGPIPE; ignore.
  head -c "$max" >"$tmp" 2>/dev/null || true
  cat "$tmp"
  # If the stream had more, head stopped at exactly max: peek via wc if we
  # drained only max bytes. We can't know remaining after head closed the pipe,
  # so note truncation whenever output length equals the cap.
  if [ "$(wc -c <"$tmp" | tr -d ' ')" -ge "$max" ]; then
    printf '\n\n…(truncated; over %s bytes)…\n' "$max"
  fi
  rm -f "$tmp"
}

# GitHub PR surface: embed `gh pr diff`, not cwd `git diff` (hosts sit on the
# project's working copy, often a different branch).
if [ "${RECUR_SURFACE:-}" = "github_pr" ] && [ -n "${RECUR_PR_NUMBER:-}" ] && [ "${RECUR_PR_NUMBER}" != "0" ]; then
  num="$RECUR_PR_NUMBER"
  RECUR_GIT_BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)"
  RECUR_GIT_BASE="${RECUR_PR_BASE:-${RECUR_BASE_BRANCH:-}}"
  RECUR_GIT_MERGE_BASE=""
  export RECUR_GIT_BASE RECUR_GIT_MERGE_BASE RECUR_GIT_BRANCH
  view_tmp="$(mktemp)"
  diff_tmp="$(mktemp)"
  gh pr view "$num" --json title,url,baseRefName,headRefName,body,author,reviewDecision,state 2>/dev/null | _recur_cap >"$view_tmp" || true &
  view_pid=$!
  gh pr diff "$num" 2>/dev/null | _recur_cap >"$diff_tmp" || true &
  diff_pid=$!
  wait "$view_pid" "$diff_pid" || true
  export _RECUR_PR_VIEW="$(cat "$view_tmp" 2>/dev/null || true)"
  export _RECUR_PR_DIFF="$(cat "$diff_tmp" 2>/dev/null || true)"
  rm -f "$view_tmp" "$diff_tmp"
  # Derive --stat from the already-fetched diff (no third gh round-trip).
  export _RECUR_PR_STAT="$(printf '%s\n' "$_RECUR_PR_DIFF" | git apply --numstat --summary 2>/dev/null | _recur_cap || true)"
  export _RECUR_STATUS=""
  export _RECUR_STAT=""
  export _RECUR_STAT_WORK=""
  export _RECUR_DIFF_COMMITS=""
  export _RECUR_DIFF_WORK=""
  export _RECUR_UNTRACKED=""
else
  base="${RECUR_BASE_BRANCH:-}"
  if [ -n "$base" ] && ! git rev-parse -q --verify "$base" >/dev/null 2>&1; then
    base=""
  fi
  if [ -z "$base" ]; then
    for cand in main master trunk develop; do
      if git rev-parse --verify "origin/${cand}" >/dev/null 2>&1; then
        base="origin/${cand}"
        break
      fi
      if git rev-parse --verify "$cand" >/dev/null 2>&1; then
        base="$cand"
        break
      fi
    done
    if [ -z "$base" ] && git rev-parse --verify origin/HEAD >/dev/null 2>&1; then
      base="$(git rev-parse --abbrev-ref origin/HEAD 2>/dev/null || true)"
    fi
    if [ -z "$base" ]; then
      base="HEAD~1"
    fi
  fi

  RECUR_GIT_BASE="$base"
  RECUR_GIT_MERGE_BASE="$(git merge-base HEAD "$base" 2>/dev/null || git rev-parse HEAD)"
  RECUR_GIT_BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)"
  export RECUR_GIT_BASE RECUR_GIT_MERGE_BASE RECUR_GIT_BRANCH

  export _RECUR_STATUS="$(git status --short 2>/dev/null | _recur_cap || true)"
  if [ -n "${RECUR_REVIEW_SINCE:-}" ] && [ -n "${RECUR_REVIEW_NOW:-}" ]; then
    # /re-review continuing this chat's earlier review: only what changed
    # since that pass (Recur checkpoint commits: tracked + untracked files).
    # The full branch diff is already in the conversation.
    export _RECUR_STAT=""
    export _RECUR_DIFF_COMMITS=""
    export _RECUR_STAT_WORK="$(git diff --stat "$RECUR_REVIEW_SINCE" "$RECUR_REVIEW_NOW" 2>/dev/null | _recur_cap || true)"
    export _RECUR_DIFF_WORK="$(git diff "$RECUR_REVIEW_SINCE" "$RECUR_REVIEW_NOW" 2>/dev/null | _recur_cap "$_RECUR_DIFF_TOTAL_MAX" || true)"
  else
    export _RECUR_STAT="$(git diff --stat "$RECUR_GIT_MERGE_BASE"...HEAD 2>/dev/null | _recur_cap || true)"
    export _RECUR_STAT_WORK="$(git diff --stat 2>/dev/null | _recur_cap || true)"
    export _RECUR_DIFF_COMMITS="$(git diff "$RECUR_GIT_MERGE_BASE"...HEAD 2>/dev/null | _recur_cap || true)"
    _work_max=$(( _RECUR_DIFF_TOTAL_MAX - ${#_RECUR_DIFF_COMMITS} ))
    [ "$_work_max" -lt 1 ] && _work_max=1
    export _RECUR_DIFF_WORK="$(git diff 2>/dev/null | _recur_cap "$_work_max" || true)"
  fi
  export _RECUR_UNTRACKED="$(git ls-files --others --exclude-standard 2>/dev/null | _recur_cap || true)"
  export _RECUR_PR_VIEW=""
  export _RECUR_PR_DIFF=""
  export _RECUR_PR_STAT=""
fi
