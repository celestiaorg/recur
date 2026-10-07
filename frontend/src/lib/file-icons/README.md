# File icons (from file-icons/atom)

Observatory file-explorer icons are vendored from
[file-icons/atom](https://github.com/file-icons/atom) (MIT).

We do **not** depend on the stale `atom-file-icons` npm port. Instead we keep a
thin matcher here and regenerate assets from upstream.

| Path | Source |
|------|--------|
| `generated/icondb.js` | `file-icons/atom` → `lib/icons/.icondb.js` |
| `generated/{colors,icons,fonts}.css` | compiled from `file-icons/atom` → `styles/*.less` |
| `generated/fonts/*.woff2` | `file-icons/atom` → `fonts/` |
| `generated/fonts/octicons.woff2` | `file-icons/vscode` → `icons/octicons.woff2` (not shipped in atom) |
| `generated/UPSTREAM` | pinned commit SHA from the last sync |

Matching code (`icon.ts`, `icon-tables.ts`) is a small TypeScript port of
upstream `lib/icons/{icon,icon-tables}.js`.

## Updating

```bash
cd frontend
pnpm run update-file-icons
```

That script:

1. Downloads LESS + `.icondb.js` from `file-icons/atom@master`
2. Compiles LESS → CSS (`lessc`)
3. Rewrites `atom://…` font URLs to `./fonts/…`
4. Strips regex lookbehinds in the DB (WKWebView / Safari)
5. Downloads font files (+ Octicons from `file-icons/vscode`)
6. Writes `generated/UPSTREAM` with the atom commit SHA

Commit the resulting `generated/` changes.

## Notes

- Only **files** get icons in the explorer (folders stay chevron + name).
- Icon size / positioning overrides live in `src/styles/_observatory.scss` (`.obs-node .fico`).
- If LESS compilation fails, ensure `less` is installed (`pnpm add -D less`).
- Upstream `file-icons.woff2` is missing a few codepoints still referenced by CSS
  (notably `dotenv` / U+EBB8). `dotenv-fix.css` remaps that icon to Octicons key
  so `.env` files are not blank in the tree.
