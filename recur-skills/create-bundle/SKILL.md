---
name: create-bundle
description: >-
  Create or edit a Recur Bundle (bundle.toml). Use when the user wants a
  custom Loop, role, or policy; a To a PR / Converge / Converge-to-PR / Docs / Security
  variant; or a command, snippet, grammar, theme, terminal theme, symbol, language-data,
  or chat prompt action. Use when they ask to author, scaffold, or
  customize a Recur pack.
---
# Create a Recur Bundle

A bundle is a directory with one `bundle.toml`. Loops, roles, and policies
live in the same pack as editor items. There is no separate “loop file.”

## Where to write

| Kind | Path |
|------|------|
| User (default) | `~/.recur/bundles/<Name>/` |
| First-party (this repo only) | `bundles/<Name>/` when the worktree *is* Recur |
| Shipped runtime | `<DataDir>/Bundles`: seeded, overwritten. Do not hand-edit. |

User **id** wins over shipped. New capability → new ids. Customize a shipped
item in place → same id in a user pack.

If both a user pack and a Recur-repo change are plausible, ask with
`ask_user` before writing.

## Workflow

1. Classify the ask.
2. Read the matching reference **before** writing TOML. Load only what you need.
   - Loops / roles / policies → [references/loops.md](references/loops.md)
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
   shape, do not invent verbs or tool names.
4. Write `bundle.toml` plus prompts or scripts. Keep prompts short.
5. Lint: `recur bundles lint ~/.recur/bundles/<Name>` (or the repo path).

## Hard rules

- Do not edit `~/.recur/recur-skills`: it is overwritten on app update.
- Closed `uses` verbs only: `plan`, `implement`, `review`, `converge`,
  `verify`, `ensure_pr`, `artifact`. A pack cannot mint tools or skip the
  converge reviewer ≠ implementer split.
- A loop opens a PR only if it has an `ensure_pr` phase. Converge / Docs /
  Security-style loops idle without one; the human uses `open_pr` or the
  titlebar.
- `recur bundles create Name` scaffolds an **editor** pack (commands), not
  a loop. For a loop, start from [references/loops.md](references/loops.md)
  or copy `bundles/Loop/` when this worktree is Recur.

Skills (`SKILL.md`) are a different thing: use `/create-skill` for those.
