# `[[prompt]]`

Composer `/` verbs. **Not** Agent Skills and **not** editor commands.
They appear above skills (alphabetical, “Prompt” badge). Selecting one
invokes immediately: optional mode tab → run script → send.

Shared ids: [editor.md](editor.md). Copy `bundles/Chat` when this worktree
is Recur (`/review`, `/fix-findings`, `/commit`, `/open-pr`, …). User packs
may shadow a slash after id dedupe.

## Fields

| Field | Req | Default | Notes |
|-------|-----|---------|-------|
| `id` | yes | | Same id rules as `[bundle].id` |
| `name` | yes | | Menu label |
| `description` | | | Menu subtitle |
| `slash` | | kebab of `name` | `^[a-z][a-z0-9-]*$` (no leading `/` in the field) |
| `mode` | | (stay on current chat) | `ask` \| `plan` \| `agent` \| `review` \| `loop`: find idle tab / create |
| `loop` | | | `[[loop]]` id. Implies `mode = "loop"`. Pins that loop on the routed tab before send |
| `timeoutMs` | | 5000 | `0` or `1..30000` |
| `tab_group` | | | Shared chat-tab slot (`^[a-z][a-z0-9-]*$`). Requires `mode`. Nearest idle tab in the group, else create. Omit: nearest idle tab in `mode`, else create |
| `fresh_session` | | false | New provider session on the landed tab, independent of `tab_group`. Not valid with `mode = "loop"` |
| `interpreter` | | `shebang` | `typescript` \| `shebang` |
| `script` | * | | Pack-relative path. XOR `body` |
| `body` | * | | Inline. XOR `script` |

`*`: `body` **or** `script` is required, not both.

## Script contract

Export `(ctx: PromptContext) => PromptResult` with `{ prompt, display? }`.

- `prompt` is the user-turn text. Empty = no-send success (toast `display`
  only; used by `/update` when git is clean).
- `display` is a one-line UI label ≤120 chars (default = prompt name).
- `@path` and `/skill` tokens inside `prompt` are real composer dialect:
  the host wraps them. Do **not** pre-wrap.
- Embed blobs that must not be scanned (git diffs) in nonce-paired
  `⟦raw:ID⟧`…`⟦/raw:ID⟧` (bare `⟦raw⟧`…`⟦/raw⟧` only when the body cannot
  contain those strings). Markers are stripped before storage/provider.

TypeScript: import `{ type PromptContext, type PromptResult }` from
`@recur/sdk`. Shebang: print JSON `{"prompt":"...","display":"..."}`.

`PromptContext` has workspace vars (`workspaceId`, `projectId`, `worktree`,
`port`) plus a focused-editor snapshot (selection, document, filepath, …).

## Example

```toml
[[prompt]]
id = "acme.chat.fix-findings"
name = "Fix Findings"
description = "Fix or dispute open reviewer findings"
slash = "fix-findings"
mode = "agent"
tab_group = "ops"
fresh_session = true
timeoutMs = 5000
interpreter = "typescript"
script = "src/fix-findings.ts"
```

A loop kickoff from any chat (idle Loop tab or a new one; no typed goal):

```toml
[[prompt]]
id = "acme.chat.review-and-fix"
name = "Review & fix"
slash = "review-and-fix"
mode = "loop"
loop = "recur.loops.converge"
interpreter = "shebang"
body = '''
#!/bin/sh
printf '%s\n' '{"prompt":"Review the current worktree and fix until the bar passes.","display":"Review & fix"}'
'''
```

The slash `prompt` is the first user-turn wire. `[[loop]] prompt` is the
conductor’s standing charter on later Maestro turns: see
[loops.md](loops.md). Do not confuse with `[[command.prompts]]` (pre-run
fields on a command) or with `SKILL.md` (Agent Skills: `/create-skill`).
