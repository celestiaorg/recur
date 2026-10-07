---
name: split-to-prs
description: >-
  Split current work into small reviewable PRs. Use when the user asks to split
  a chat, set of changes, branch, or PR.
---
# Split to PRs

Turn one pile of work into a few small PRs.

## Hard rules

- Do not create branches, commit, push, or open PRs until the user approves the split plan (`mcp__recur__ask_user`).
- Never discard user work. No destructive git commands without explicit approval.
- Always save a recoverable snapshot before moving work around.
- Stage only named files or hunks. No `git add .` / `git add -A`.

## 1. Check the state

Compare the current work to the repo's default branch, including committed and uncommitted changes. Summarize real slices. Use ownership signals (`CODEOWNERS`, etc.) when present.

## 2. Propose the split

Usually PR titles are enough. Show a Mermaid diagram when there are multiple slices. Prefer independent PRs off the default branch; stack only when dependency is real.

Ask for approval with `ask_user` before starting.

## 3. Execute the split

- If there is uncommitted work, save a recoverable snapshot (`git stash create` + backup ref) without changing the working tree.
- For each approved slice: branch from the right base, stage/commit only planned files, push, open the PR.

## 4. Report back

PR titles and URLs, plus anything left on the starting branch or working tree.
