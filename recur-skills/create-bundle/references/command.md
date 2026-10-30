# `[[command]]`

Palette / key / willSave macros. Shared ids and keys: [editor.md](editor.md).

Copy `bundles/Text` (TypeScript) or `bundles/Diff` (shebang) when this
worktree is Recur. Else `recur bundles create MyPack`.

## Fields

| Field | Req | Default | Notes |
|-------|-----|---------|-------|
| `id` | yes | | Same id rules as `[bundle].id` |
| `name` | yes | | Palette label |
| `category` | | bundle name | Palette group; free-form, unowned |
| `order` | | 0 | Lower first within a category |
| `scope` | | | TextMate scope filter (comma-OR + dotted prefix) |
| `fileGlob` | | | Path filter, e.g. `*.go` |
| `input` | | `selection` | `none` \| `selection` \| `document` \| `line` \| `word` |
| `fallbackInput` | | | Same enum; used when `input` is empty |
| `output` | | `replaceSelection` | See routing below |
| `triggers` | | `["palette"]` | `palette` (⌘⇧P) and/or `willSave` |
| `keyEquivalent` | | | See [editor.md](editor.md) |
| `language` | | | Scratch highlighter when `output = "newDocument"` (`diff`, `json`, `md`, …) |
| `openInPreview` | | false | Only with `newDocument`; previewable langs: html/htm, md/markdown/mdx, svg, mmd/mermaid |
| `interpreter` | | `shebang` | `typescript` \| `shebang` (`js` / `native` rejected) |
| `script` | * | | Pack-relative path. XOR `body` |
| `body` | * | | Inline script. XOR `script` |
| `prompts` | | | Pre-run input fields (see below) |

`*`: `body` **or** `script` is required, not both.

### Output routing

`discard` · `replaceSelection` · `replaceDocument` · `replaceInput` ·
`insertText` · `insertSnippet` · `newDocument` · `showAsTooltip` ·
`copyToClipboard` · `showAsToast`

Exit `0` applies the routing. Non-zero toasts stderr and discards
buffer-mutating routes. `showAsToast` / `showAsTooltip` / `copyToClipboard`
may still fire if stdout is non-empty. Tools that use exit `1` as success
(`diff`) must normalize to `0`.

`willSave`: before `WriteFile`, ~15s budget (each command ≤12s). Failure,
empty stdout, or shrinking a >100 char doc by >~90% writes the original
and toasts.

### `[[command.prompts]]` (optional)

Collected before run; values land on `ctx.prompts[name]`.

| Field | Req | Notes |
|-------|-----|-------|
| `name` | yes | Key in `prompts` |
| `label` | | UI label |
| `type` | | `text` \| `choice` \| `password` |
| `default` | | |
| `choices` | | Required for `choice` |

## TypeScript

Import only `@recur/sdk`. Export `default function (ctx: MacroContext): MacroResult`.

```ts
import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input.toUpperCase())
}
```

`MacroResult` is a string or `{ text, output?, language?, openInPreview? }`.
Runtime `language` / `openInPreview` win over the manifest. Inline `body`
is auto-wrapped with the default export.

## Shebang

`#!` plus `sh` / `perl` / `awk` / `node` / `python`. stdin = input.
Env: `RECUR_SELECTED_TEXT`, `RECUR_CURRENT_LINE`, `RECUR_LINE_NUMBER`,
`RECUR_COLUMN_NUMBER`, `RECUR_CURRENT_WORD`, `RECUR_FILEPATH`,
`RECUR_FILENAME`, `RECUR_DIRECTORY`, `RECUR_PROJECT_DIRECTORY`,
`RECUR_SCOPE`, `RECUR_TAB_SIZE`, `RECUR_SOFT_TABS`, `RECUR_CLIPBOARD`,
`RECUR_BUNDLE_SUPPORT`, `RECUR_INPUT_<NAME>`. Default deadline 30s
(willSave ≤12s).

## Example

```toml
[[command]]
id = "acme.text.uppercase"
name = "Uppercase Selection"
category = "Text"
order = 10
input = "selection"
fallbackInput = "document"
output = "replaceInput"
triggers = ["palette"]
interpreter = "typescript"
script = "src/uppercase.ts"
```

## Tests

`tests/*.toml`: `command` = id or name. Inline `input` / `stdout` or
`inputFile` / `stdoutFile` (not both per field). Optional `skipStdout = true`.
See shipped `bundles/Text/tests/`.
