# `[[theme]]`

TextMate / VS Code colour theme. Shared ids: [editor.md](editor.md).
Conventional `themes/<name>.tmTheme.json` (plist `.tmTheme` accepted).
Copy `bundles/Themes/` or first-party Charcoal / Paper in `bundles/Recur/`
when this worktree is Recur.

Appears in Settings → Appearance by `id`, ordered by `order`. View →
Appearance mirrors the pick. If the active theme is disabled, Recur falls
back to Charcoal (or the next enabled theme).

## Fields

| Field | Req | Default | Notes |
|-------|-----|---------|-------|
| `id` | yes | | Settings preference value and override key |
| `name` | yes | | Display name |
| `path` | yes | | Pack-relative; no `..` or abs paths; file must exist |
| `uiTheme` | | `dark` | `light` \| `dark` |
| `order` | | 0 | Lower first; zero sorts after every positive order |
| `preview` | | | Settings card swatches (optional) |

### `preview`

| Field | Notes |
|-------|-------|
| `bg` | Editor background |
| `fg` | Default foreground |
| `fn` | Sample `fn` colour |
| `name` | Sample name / `main` colour |
| `string` | Sample `"hi"` colour |

Colours must parse as hex (lint/normalize). Omit the whole table if unused.

## Example

```toml
[[theme]]
id = "cool-glow"
name = "Cool Glow"
path = "themes/cool-glow.tmTheme.json"
uiTheme = "dark"
order = 30
preview = { bg = "#06071D", fg = "#E0E0E0", fn = "#B683CA", name = "#2BF1DC", string = "#8DFF8E" }
```

Theme-only packs are valid. Recur vendors the Shiki tm-themes catalog into
`bundles/Themes/` (`mise run fetch:tm-themes`). Terminal dock palettes are
`[[terminalTheme]]`: see [terminal-theme.md](terminal-theme.md).
