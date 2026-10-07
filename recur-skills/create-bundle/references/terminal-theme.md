# `[[terminalTheme]]`

xterm.js `ITheme` palette for the terminal dock. Shared ids: [editor.md](editor.md).
Conventional `themes/<name>.json`. Copy `bundles/Terminal/` when this worktree
is Recur.

Appears in Settings → Appearance and View → Appearance as **Terminal theme**,
ordered by `order`. Empty preference (Match app) keeps the CSS-variable mapping
from the app chrome. If the chosen id is missing, Recur falls back to Match app.

## Fields

| Field | Req | Default | Notes |
|-------|-----|---------|-------|
| `id` | yes | | Settings preference value and override key |
| `name` | yes | | Display name |
| `path` | yes | | Pack-relative `*.json`; no `..` or abs paths; file must exist |
| `uiTheme` | | `dark` | `light` \| `dark` |
| `order` | | 0 | Lower first; zero sorts after every positive order |

The JSON file is an [xterm.js ITheme](https://xtermjs.org/docs/api/terminal/interfaces/itheme/):
`background` and `foreground` are required; ANSI / cursor / selection colours
are optional. Colours are `#hex` or `rgb`/`hsl` CSS.

## Example

```toml
[[terminalTheme]]
id = "homebrew"
name = "Homebrew"
path = "themes/homebrew.json"
uiTheme = "dark"
order = 80
```

```json
{
  "background": "#000000",
  "foreground": "#00ff00",
  "cursor": "#23ff18",
  "green": "#00a600"
}
```

Terminal-theme-only packs are valid. User **id** wins over shipped.
