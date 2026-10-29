# {{BUNDLE_NAME}}

Recur Bundle: editor command pack (TOML manifest + typescript and/or shebang).

## Layout

| Path | Purpose |
|------|---------|
| `bundle.toml` | Manifest: bundle meta + `[[command]]` / optional `[[snippet]]` / optional `[[symbol]]` |
| `src/*.ts` | TypeScript macros (`interpreter = "typescript"`) |
| `bin/*` | Shebang scripts (`interpreter = "shebang"`) |
| `tests/*.toml` | Engine tests (inline or `*File` fixtures) |
| `README.md` | This file |

This scaffold includes:

- **Uppercase Selection**: typescript sample (`src/uppercase.ts`)
- **Prefix Lines**: shebang sample (`bin/prefix-lines.sh`)

## Develop

Edit commands in `bundle.toml`, then the script they point at. TypeScript
macros import only from `@recur/sdk` (`MacroContext`, `Output`,
`replaceEdited`). Shebang scripts receive stdin + `RECUR_*` env vars.

Recur loads user packs from `~/.recur/bundles` (uuid wins over shipped defaults).

## Test

Run the real engine against `tests/`:

```bash
recur bundles test {{BUNDLE_NAME}}
recur bundles lint {{BUNDLE_NAME}}
```

Payloads may be inline in the TOML or files:

```toml
[test.context]
inputFile = "fixtures/input.txt"
[test.expect]
stdoutFile = "fixtures/expect.txt"
```

## Learn more

Full schema, interpreters, output routing, and examples: Recur `docs/bundles.md`.
