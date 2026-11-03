# `[[snippet]]`

Tab trigger and/or keyEquivalent expansion. Shared ids and keys:
[editor.md](editor.md). Copy `bundles/Go` (`ife`) or `bundles/Text`.

## Fields

| Field | Req | Notes |
|-------|-----|-------|
| `id` | yes | Same id rules as `[bundle].id` |
| `name` | yes | Palette / picker label |
| `tabTrigger` | * | Word before caret: letter or `_`, then letters/digits/`_`; ≤40 |
| `keyEquivalent` | * | See [editor.md](editor.md) |
| `scope` | | TextMate scope filter (comma-OR + dotted prefix) |
| `fileGlob` | | Path filter, e.g. `*.go` |
| `body` | yes | VS Code placeholders |
| `order` | | Palette order among snippets |

`*`: set `tabTrigger` and/or `keyEquivalent` (at least one).

If the word before the caret equals a matching `tabTrigger`, Tab expands.
Typing a trigger prefix also lists snippets in autocomplete (above buffer
words). Otherwise Tab indents. Placeholder fields take Tab first.

⌘⇧P / status-bar Bundles lists snippets for the focused file (filtered by
`scope` / `fileGlob`). Rows show the trigger (`ife⇥`).

## Body syntax

VS Code: `$0` / `$1` / `${1:default}` / `${1|a,b|}`. Repeat the same
tabstop to mirror (`${1:err}` twice, or `$1` after `${1:err}`).

v1 vars: `$TM_SELECTED_TEXT` (wrap) and `$ISO_DATE` (`YYYY-MM-DD`).

## Example

```toml
[[snippet]]
id = "acme.go.ife"
name = "If Err"
tabTrigger = "ife"
scope = "source.go"
fileGlob = "*.go"
body = '''
if ${1:err} != nil {
	${2:log.Fatal(${1:err})}
}
'''
```

## Tests

```toml
[[test]]
name = "ife body"
snippet = "acme.go.ife"
[test.expect]
stdout = '''
if ${1:err} != nil {
	${2:log.Fatal(${1:err})}
}
'''
```

Command XOR snippet per test. No caret assertions.
