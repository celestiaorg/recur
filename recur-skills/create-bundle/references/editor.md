# Editor packs: shared rules

Read this first for any editor / chat item. Then open the kind file for
fields. Loops: [loops.md](loops.md).

| Kind | File |
|------|------|
| `[[command]]` | [command.md](command.md) |
| `[[snippet]]` | [snippet.md](snippet.md) |
| `[[grammar]]` | [grammar.md](grammar.md) |
| `[[theme]]` | [theme.md](theme.md) |
| `[[terminalTheme]]` | [terminal-theme.md](terminal-theme.md) |
| `[[languageData]]` | [language-data.md](language-data.md) |
| `[[symbol]]` | [symbol.md](symbol.md) |
| `[[prompt]]` | [prompt.md](prompt.md) |

One pack may mix kinds. Grammar-only / theme-only / terminal-theme-only / snippet-only / symbol-only
/ language-data-only packs are valid.

## `[bundle]`

| Field | Req | Notes |
|-------|-----|-------|
| `id` | yes | Filename-safe: ≤80 chars; letters/digits/`_`/`-`/`.`; no `/` `\` or `..` |
| `name` | | Display name; defaults to the folder name |
| `version` | | e.g. `"1.0.0"` |
| `author` | | Free text |

Every item `id` uses the same id rules. User **id** wins over shipped.
After grammar / language-data id-dedupe, colliding `scopeName`s also prefer user.

## Scaffold

```
recur bundles create MyPack
```

Writes `~/.recur/bundles/MyPack` (or `--dir`) with a TypeScript command, a
shebang command, and tests. Then edit. Do **not** use this as a loop start.

When this worktree is Recur, copy `bundles/Text`, `bundles/Go`, `bundles/Chat`.
Elsewhere, copy a pack under `~/.recur/bundles` or start from the scaffold.

## Keybindings

Optional `keyEquivalent` on a **command or snippet**. macOS chord tokens:
`cmd` `ctrl` `opt` `shift` `fn` plus `a`–`z`, `0`–`9`, `f1`–`f12`, `enter`,
`esc`, `tab`, `space`, arrows, `delete`, `backspace`. Examples:
`cmd+shift+u`, `opt+cmd+j`. Order-independent; case-insensitive.

App chords win (Save, Quick Open, …): the binding is dropped, the item stays
in ⌘⇧P. `recur bundles lint` errors on those collisions. Shared chords among
bundle items show a numbered picker.

## Validate

```
recur bundles lint ~/.recur/bundles/MyPack
recur bundles test MyPack
```

`lint`: schema, TS compile, shebang, keyEquivalent collisions. `test`:
in-bundle `tests/*.toml`. Malformed packs are skipped at discover; reasons
show in Settings → Bundles → Skipped packs.

Shebang = user-privilege OS process. TypeScript = sandboxed Worker. Only
install packs you trust.
