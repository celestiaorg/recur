---
name: recur-help
description: >-
  Answer questions about using the Recur app itself: workspaces, chat modes,
  Loops, review, pull requests, inbox, schedules, providers, integrations,
  skills, bundles, themes, keymaps, project.toml, or the recur CLI. Use when
  the user asks how to do something in Recur, what a Recur feature is, or
  where a setting lives.
---
# Recur help

Recur's user docs are in `references/` next to this file (the same pages as
docs.recur.build). Read the page that matches the question before answering.
Do not answer Recur questions from memory: Recur is new, and models have no
training data on it.

| Topic | Page |
|-------|------|
| What Recur is | `references/index.mdx` |
| Install, first run | `references/getting-started/install.mdx` |
| Open a project, pick a mode, start working | `references/getting-started/first-workspace.mdx` |
| Working copy, task worktrees, how work lands | `references/concepts/workspaces.mdx` |
| `.recur/project.toml`: setup, run commands, env | `references/concepts/project-toml.mdx` |
| Attaching files, diff lines, terminal output to chat | `references/concepts/chat-context.mdx` |
| Ask, Plan, Agent, Review, Loop modes | `references/concepts/modes.mdx` |
| Loops, roles, policies | `references/concepts/loop.mdx` |
| Diffs, findings, restoring a chat turn | `references/concepts/review.mdx` |
| Pull requests dashboard, reviewing, merging | `references/concepts/pull-requests.mdx` |
| Inbox | `references/concepts/inbox.mdx` |
| Schedules | `references/concepts/schedules.mdx` |
| Providers and model endpoints | `references/reference/agents.mdx` |
| Linear and MCP servers | `references/reference/integrations.mdx` |
| Enabling and disabling skills | `references/reference/skills.mdx` |
| What each Settings section controls | `references/reference/settings.mdx` |
| Themes, keymaps, shortcuts | `references/reference/themes.mdx` |
| Provider sign-in, missing models, failed turns, `gh` | `references/reference/troubleshooting.mdx` |
| Bundles overview | `references/bundles/index.mdx` |
| Bundle authoring details | `references/bundles/*.mdx` |

Links inside the pages like `/concepts/loop` map to `references/concepts/loop.mdx`.

If the pages do not cover the question, say so instead of guessing. To build or
edit a bundle, use the `create-bundle` skill. To edit project run settings, use
`project-toml`.
