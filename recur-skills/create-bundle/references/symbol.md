# `[[symbol]]`

Open-file outline / Go to Symbol. Not a Settings preference. Matches
TextMate token scopes from the open buffer (same stacks as colouring).
The status bar shows the last symbol at or before the caret (click opens
Go to Symbol). Workspace Find Defs stay on Go line-regex until LSP.

Shared ids: [editor.md](editor.md). Copy `bundles/Source` (functions) or
`bundles/Markdown` (headings) when this worktree is Recur.

## Fields

| Field | Req | Default | Notes |
|-------|-----|---------|-------|
| `id` | yes | | Override key |
| `scope` | yes | | Token-scope selector (`comma` / space / `>` / `-`), not a file grammar |
| `show` | | `true` | `false` excludes a more-specific scope from a broader include |
| `kind` | | inferred from scope | Outline label (`func`, …) |
| `transform` | | | JavaScript `s/pattern/repl/flags` — **not** Oniguruma `(?x)` |

Symbol-only packs are valid.

## Example

```toml
[[symbol]]
id = "acme.symbols.source.function"
scope = "entity.name.function"
kind = "func"
# show = false
# transform = 's/^["'']|["'']$//g'
```

Shipped patterns: Source `entity.name.function` (hide calls/support),
Python decorator exclude, Markdown headings (hash-count indent; skip
fenced code), HTML ids, YAML keys, CSS selectors. Go methods list as
`Type.Name` from the same-line receiver.
