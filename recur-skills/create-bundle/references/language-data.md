# `[[languageData]]`

Comment tokens + indent + extra close-brackets for a TextMate scope.
Not a Settings preference. Merged onto `ListGrammars` by `scopeName`, then
dotted prefixes (`source.go` → `source`) so `Mod-/` and indent-on-input
work. Shared ids: [editor.md](editor.md).

Copy `bundles/LanguageData` when this worktree is Recur. Entries may live
in any pack.

## Fields

| Field | Req | Notes |
|-------|-----|-------|
| `id` | yes | Override key |
| `scopeName` | yes | Unique in the pack; most-specific match wins at runtime |
| `lineComment` | * | e.g. `"//"` |
| `blockCommentStart` | * | Both block ends required if either is set |
| `blockCommentEnd` | * | |
| `increaseIndentPattern` | * | **JavaScript** regex source, not Oniguruma `(?x)` |
| `decreaseIndentPattern` | * | |
| `indentNextLinePattern` | * | |
| `zeroIndentPattern` | * | |
| `disableIndentCorrections` | * | Still indent a new line; don't rewrite a non-empty one |
| `closeBrackets` | * | Extra **single-character** openers merged onto `()[]{} '" \`` |

`*`: set at least one payload (comment token, indent pattern,
`disableIndentCorrections`, or `closeBrackets`). Empty entries are rejected.

`<` pairs with `>`; `_` / `*` pair with themselves. Enter between
`<div>|</div>` or empty `{}`/`()`/`[]` splits the pair.

Strict JSON has no comment tokens. Markdown uses HTML `<!-- -->`, not `#`.

## Example

```toml
[[languageData]]
id = "acme.langdata.go"
scopeName = "source.go"
lineComment = "//"
blockCommentStart = "/*"
blockCommentEnd = "*/"
increaseIndentPattern = "[{(\\[]\\s*(?://.*)?$"
decreaseIndentPattern = "^\\s*[})\\]]"
# closeBrackets = ["<"]
```
