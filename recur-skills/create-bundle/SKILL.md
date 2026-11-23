---
name: create-bundle
description: >-
  Create or edit a Recur Bundle (bundle.toml). Use when the user wants a
  custom workflow, role, or policy; a To a PR / Converge / Converge-to-PR / Docs / Security
  variant; or a command, snippet, grammar, theme, terminal theme, symbol, language-data,
  or chat prompt action. Use when they ask to author, scaffold, or
  customize a Recur pack.
---
# Create a Recur Bundle

A bundle is a directory with one `bundle.toml`. Workflows, roles, and policies
live in the same pack as editor items. There is no separate “workflow file.”

## Where to write

| Kind | Path |
|------|------|
| Agent install (preferred when you have `install_bundle`) | `<worktree>/<Name>/`, then `install_bundle` |
| User (default) | `~/.recur/bundles/<Name>/` |
| First-party (this repo only) | `bundles/<Name>/` when the worktree *is* Recur |
| Shipped runtime | `<DataDir>/Bundles`: seeded, overwritten. Do not hand-edit. |

User **id** wins over shipped. New capability → new ids. Customize a shipped
item in place → same id in a user pack.

If both a user pack and a Recur-repo change are plausible, ask with
`ask_user` before writing.

## Steps

1. Classify the ask.
2. Read the matching reference **before** writing TOML. Load only what you need.
   - Workflows / roles / policies → [references/workflows.md](references/workflows.md)
   - Shared editor rules (ids, scaffold, keys, lint) → [references/editor.md](references/editor.md)
   - Then the item kind:
     - `[[command]]` → [references/command.md](references/command.md)
     - `[[snippet]]` → [references/snippet.md](references/snippet.md)
     - `[[grammar]]` → [references/grammar.md](references/grammar.md)
     - `[[theme]]` → [references/theme.md](references/theme.md)
     - `[[terminalTheme]]` → [references/terminal-theme.md](references/terminal-theme.md)
     - `[[languageData]]` → [references/language-data.md](references/language-data.md)
     - `[[symbol]]` → [references/symbol.md](references/symbol.md)
     - `[[prompt]]` → [references/prompt.md](references/prompt.md)
3. Prefer reuse: first-party roles and shipped packs as templates. Copy
   shape, do not invent verbs or tool names. When you have the
   `reload_bundles` tool, call it first: it lists every installed workflow,
   role, and policy id (shipped and user) so you reuse real ids.
4. Write `bundle.toml` plus prompts or scripts. Keep prompts short.
5. Lint: `recur bundles lint <dir>` and fix every error.
6. Install. With the `install_bundle` tool (Agent and Autopilot modes):
   write the pack in the worktree (for example `<worktree>/MyPack/`), lint
   it, then call `install_bundle` with that path. The human approves each
   install, so say what it adds. It lands in `~/.recur/bundles/MyPack/`
   and bundles reload, so the workflow is usable right away. Without the
   tool, write under `~/.recur/bundles/<Name>/` and tell the human to pick
   Navigate > Reload Bundles. Do not commit an install-only pack to the
   repo unless asked.

## Hard rules

- Do not edit `~/.recur/recur-skills`: it is overwritten on app update.
- Closed `uses` verbs only: `plan`, `implement`, `review`, `converge`,
  `verify`, `run`, `ensure_pr`, `artifact`. A pack cannot mint tools or skip the
  converge reviewer ≠ implementer split.
- A workflow opens a PR only if it has an `ensure_pr` phase. Converge / Docs /
  Security-style workflows idle without one; the human uses `open_pr` or the
  titlebar.
- `recur bundles create Name` scaffolds an **editor** pack (commands), not
  a workflow. For a workflow, start from [references/workflows.md](references/workflows.md)
  or copy `bundles/Workflows/` when this worktree is Recur.

Skills (`SKILL.md`) are a different thing: use `/create-skill` for those.
