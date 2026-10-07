# `[[grammar]]`

TextMate highlighting. Shared ids: [editor.md](editor.md). Prefer
`syntaxes/<name>.tmLanguage.json` (plist `.tmLanguage` accepted). Copy
`bundles/Go/syntaxes/` or `bundles/Languages/` when this worktree is Recur.

Go only discovers and serves bytes. The webview tokenizes with
`vscode-textmate` + Oniguruma WASM. No Lezer fallback.

## Fields

| Field | Req | Notes |
|-------|-----|-------|
| `id` | yes | Override key (user pack with same id wins) |
| `scopeName` | yes | TextMate runtime identity; unique in the pack |
| `path` | yes | Pack-relative; no `..` or abs paths; file must exist |
| `language` | | Prefs / status-bar label (`go`, `solidity`, …) |
| `fileTypes` | | Extensions without dot, or full basenames |
| `firstLineMatch` | | Shebang / first-line detect |
| `injectTo` | | Host scopes to inject into |
| `embeddedLanguages` | | map: embedded scope → language id |

After id-dedupe, colliding `scopeName`s also prefer user. Override a shipped
language with the same `id` and your own `path` / `fileTypes`.

Grammar-only packs are valid.

## Example

```toml
[[grammar]]
id = "acme.languages.foo"
scopeName = "source.foo"
path = "syntaxes/foo.tmLanguage.json"
language = "foo"
fileTypes = ["foo"]
# firstLineMatch = "^#!.*foo"
# injectTo = ["source.js"]
# embeddedLanguages = { "source.js" = "javascript" }
```

Upstream sources when vendoring: VS Code `extensions/*/syntaxes`, language
repos, classic TextMate bundles, Shiki mirrors. First-party Recur vendors
into `bundles/Languages/` (`wails3 task fetch:tm-grammars`).
