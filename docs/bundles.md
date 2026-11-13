# Recur Bundles

Recur Bundles are TextMate-inspired packs that extend the editor with **commands**
(macros), **grammars** (syntax highlighting), **themes** (scope colours),
**terminal themes** (xterm.js palettes),
**language data** (comment tokens + indent patterns), **snippets** (Tab
triggers + optional keyEquivalent), **symbols** (TextMate showInSymbolList
for the open-file outline), **language servers** (LSP processes for the
Observatory editor), and **settings** (persisted secrets/values for
shebang scripts).
Each bundle is a directory containing a single `bundle.toml` manifest.

**Shipped bundles are the reference**, licensed Apache 2.0 so you can copy them freely. Copy `bundles/Text/src/*.ts` or a shebang
`body` when building your own under `~/.recur/bundles`. For highlighting, see
`bundles/Go/syntaxes/` and `bundles/Themes/themes/`.

## Why TOML

Manifests use TOML so script bodies embed in `'''…'''` literals without escaping.
Parsing happens in `recur-bundles` (the `toml` crate); views see plain structs.

## Manifest schema

```toml
[bundle]
name = "Text"
id = "recur.text"          # filename-safe: ≤80 chars; letters/digits/_-.; no / \ or ..
version = "1.0.0"
author = "You"

[[grammar]]                # optional: TextMate language grammar
id = "recur.languages.go"  # override key (user pack with same id wins)
scopeName = "source.go"    # TextMate runtime identity (injections / embeds)
path = "syntaxes/go.tmLanguage.json"   # prefer *.tmLanguage.json (plist *.tmLanguage ok)
language = "go"            # optional prefs / status-bar label
fileTypes = ["go"]         # extensions without dot, or full basenames
# firstLineMatch = "…"
# injectTo = ["source.js"]
# embeddedLanguages = { "source.js" = "javascript" }

[[languageData]]           # optional: comment tokens + indent for a TM scope
id = "recur.langdata.go"   # override key (user pack with same id wins)
scopeName = "source.go"    # merged onto the grammar with this scope (dotted prefixes ok)
lineComment = "//"
blockCommentStart = "/*"   # both block ends required if either is set
blockCommentEnd = "*/"
# JavaScript regex sources (not Oniguruma (?x)):
# increaseIndentPattern = "[{(\\[]\\s*(?://.*)?$"
# decreaseIndentPattern = "^\\s*[})\\]]"
# indentNextLinePattern = "…"
# zeroIndentPattern = "^\\s*#"
# disableIndentCorrections = true   # still indent a new line; don't rewrite a non-empty one
# closeBrackets = ["<", "_", "*"]   # extra openers merged onto CM ()[]{} '" `

[[symbol]]                 # optional: TextMate showInSymbolList (open-file outline)
id = "recur.symbols.source.function"
scope = "entity.name.function"   # token-scope selector (comma / space / > / -)
# show = false                     # exclude a more-specific scope
# kind = "func"                    # outline label; inferred from scope if omitted
# transform = 's/^["'']|["'']$//g' # JS regex s/// (not Oniguruma)

[[theme]]                  # optional: TextMate / VS Code colour theme
id = "cool-glow"           # Settings preference value (and override key)
name = "Cool Glow"
path = "themes/cool-glow.tmTheme.json"  # conventional *.tmTheme.json
uiTheme = "dark"           # light | dark
order = 30                 # optional Settings display order (lower first)
# Optional Settings picker swatches (bg/fg + sample fn / main / "hi"):
preview = { bg = "#06071D", fg = "#E0E0E0", fn = "#B683CA", name = "#2BF1DC", string = "#8DFF8E" }

[[terminalTheme]]          # optional: xterm.js ITheme palette
id = "homebrew"
name = "Homebrew"
path = "themes/homebrew.json"
uiTheme = "dark"
order = 80

[[command]]
id = "recur.text.sort-lines-asc"  # BundleCache path stem; same id rules as bundle.id
name = "Sort Lines Ascending"
category = "Text"
order = 10
input = "selection"         # none|selection|document|line|word
fallbackInput = "document"
output = "replaceInput"
# language = "diff"         # optional; scratch-tab highlighter for newDocument
# openInPreview = true      # optional; open newDocument on Preview (html/md/svg/mermaid)
triggers = ["palette"]      # palette (⌘⇧P) | willSave
keyEquivalent = "cmd+shift+u"  # optional; see Keybindings
interpreter = "typescript"  # typescript | shebang
script = "src/sort-lines-asc.ts"
# Or inline: body = ''' … '''

[[snippet]]                # optional: Tab trigger and/or keyEquivalent
id = "recur.go.ife"        # override key (user pack with same id wins)
name = "If Err"
tabTrigger = "ife"         # word before the caret; letter then letters/digits/_
# keyEquivalent = "shift+cmd+w"
scope = "source.go"        # comma-OR + dotted prefix; and/or fileGlob
fileGlob = "*.go"
body = '''
if ${1:err} != nil {
	${2:log.Fatal(${1:err})}
}
'''
# VS Code placeholders: $0 / $1 / ${1:default} / ${1|a,b|}.
# Repeat the same tabstop at each mirror (`${1:err}` twice, or `$1` after `${1:err}`).
# v1 vars: $TM_SELECTED_TEXT (wrap) and $ISO_DATE (YYYY-MM-DD).

[[languageServer]]
id = "gopls"                 # stable id (Settings toggle / dedupe key)
languages = ["go", "go.mod", "go.work"]
fileGlob = ["*.go", "go.mod", "go.work"]
command = "gopls"            # PATH fallback name
args = ["serve"]
managed = "gopls"            # optional; pin registry key → download into DataDir

[[setting]]                # optional: Settings → Integrations + shebang helpers
id = "acme.jira.token"     # override key (user pack with same id wins)
name = "api_token"         # machine name → RECUR_SETTING_API_TOKEN
label = "API token"        # Settings row title (default = name)
description = "Jira API token from Atlassian account settings"
secret = true              # password field + eye toggle
placeholder = "ATATT…"
# url = "https://id.atlassian.com/manage-profile/security/api-tokens"
# urlLabel = "Open Atlassian"
```

`[[languageServer]]` declares a long-lived LSP process for the Observatory editor
(not a palette/willSave macro). When `managed` is set, Recur installs a pinned
binary under `<DataDir>/bin/lsp/<id>/` on first use:

| `managed` | How it installs |
|-----------|-----------------|
| `gopls` | `go install` (needs `go`) |
| `rust-analyzer` | GitHub release `.gz` + SHA-256 |
| `marksman` | GitHub release binary + SHA-256 |
| `ruff` | GitHub release `.tar.gz` + SHA-256 |
| `taplo` | GitHub release `.gz` + SHA-256 |
| `typescript-language-server` | `npm install --prefix` (needs `npm`; pulls `typescript` too) |
| `basedpyright` | `npm install --prefix` |
| `yaml-language-server` | `npm install --prefix` |
| `bash-language-server` | `npm install --prefix` |
| `svelte-language-server` | `npm install --prefix` (bin: `svelteserver`) |
| `vue-language-server` | `npm install --prefix` (`@vue/language-server`; pairs with `typescript-language-server`) |
| `vscode-json-language-server` / `vscode-html-language-server` / `vscode-css-language-server` | shared `vscode-langservers-extracted` npm install |

PATH-only (no pin; binary must already be on `PATH`): `clangd`, `jdtls`, `omnisharp` (`-lsp`), `haskell-language-server-wrapper` (`--lsp`).

Otherwise `command` must resolve on `PATH`. Overrides: `RECUR_LSP_GOPLS`,
`RECUR_LSP_RUST_ANALYZER`, `RECUR_LSP_MARKSMAN`,
`RECUR_LSP_TYPESCRIPT_LANGUAGE_SERVER`, `RECUR_LSP_BASEDPYRIGHT`,
`RECUR_LSP_RUFF`, `RECUR_LSP_TAPLO`, `RECUR_LSP_JSON`, `RECUR_LSP_HTML`,
`RECUR_LSP_CSS`, `RECUR_LSP_YAML`, `RECUR_LSP_BASH_LANGUAGE_SERVER`,
`RECUR_LSP_SVELTE_LANGUAGE_SERVER`, `RECUR_LSP_VUE_LANGUAGE_SERVER`. User packs under `~/.recur/bundles`
override shipped servers with the same `id`. Disable servers in Settings → Bundles → Language servers.
Several servers may attach to one language (Python: basedpyright + ruff).
rust-analyzer gets `cargo.targetDir = true`, so its checks build into `target/rust-analyzer` and never hold the build lock your own cargo commands need.

## Grammars & themes

- **JSON first:** ship `syntaxes/<name>.tmLanguage.json` and `themes/<name>.tmTheme.json`.
  Classic plist `.tmLanguage` / `.tmTheme` is accepted by the client parser.
- **Discovery:** same roots as commands (`<DataDir>/Bundles` + `~/.recur/bundles`).
  User wins on matching `id` for commands, prompts, grammars, themes, terminal themes, language data, snippets, symbols, language servers, and settings.
  After grammar / language-data id-dedupe, colliding `scopeName`s also prefer user.
- **Override a language:** ship a user grammar with the same `id` as the shipped
  entry (e.g. `recur.languages.solidity`) and your own `path` / `fileTypes`.
- **Runtime:** the editor tokenizes with `recur-textmate` (vscode-textmate
  semantics on Oniguruma) and paints from theme scope colours (no conversion to
  other token names). The bundle loader only discovers and serves file bytes.
- **Grammar-only / theme-only / terminal-theme-only / language-data-only / snippet-only / symbol-only / language-server-only / setting-only bundles** (no `[[command]]`) are valid.
- `[[languageData]]` is not a Settings preference. Recur merges it onto `ListGrammars`
  by `scopeName` (then dotted prefixes: `source.go` → `source`) so CodeMirror
  `toggleComment` (`Mod-/`) and `indentOnInput` get per-language tokens/patterns.
  Strict JSON omits comment tokens. Markdown uses HTML `<!-- -->`, not `#`.
  Entries may live in any pack; `bundles/LanguageData` is the shipped table for
  langs that have no command/snippet pack. `closeBrackets` extras add HTML/XML
  `<>` and Markdown `_` / `*` (plus `<>`) onto CodeMirror's default pairs.
  Enter between `<div>|</div>` or empty `{}`/`()`/`[]` splits the pair
  (TextMate Return-inside-empty-item), not just indent-the-next-line.
- `[[symbol]]` is also not a Settings preference. Recur matches token scopes from
  the open buffer's TextMate highlighter (the same stacks used for colouring)
  against `showInSymbolList`-style selectors and fills the Outline panel /
  Go to Symbol list. The editor status bar shows the last symbol at or before
  the caret (click opens Go to Symbol). Workspace Find Defs stay on Go
  line-regex until LSP.
  `transform` is JavaScript `s/pattern/repl/flags` (not Oniguruma `(?x)` /
  conditionals). Shipped: Source `entity.name.function` (+ hide calls/support),
  Python decorator exclude, Markdown headings (hash-count indent; skip fenced
  code), HTML ids, YAML keys, CSS selectors. Go methods are listed as
  `Type.Name` from the same-line receiver (params/returns wait on LSP).
- Bundle themes appear in Settings → Appearance by `[[theme]].id`, ordered by
  `[[theme]].order`.
- Bundle terminal themes appear in Settings → Appearance and View → Appearance
  by `[[terminalTheme]].id`. Empty preference keeps the app-chrome mapping. Shipped
  palettes live in **`bundles/Terminal/`**.

### Where shipped grammars / themes come from

There is no single “official TextMate.org CDN.” In practice editors use:

| Source | What it is |
|--------|------------|
| [microsoft/vscode `extensions/*/syntaxes`](https://github.com/microsoft/vscode/tree/main/extensions) | Maintained JSON grammars for built-in langs |
| Language upstreams (e.g. [TypeScript-TmLanguage](https://github.com/microsoft/TypeScript-TmLanguage), Vue/Svelte repos) | Authoritative for that language |
| Classic [textmate/* .tmbundle](https://github.com/textmate) | Original plist bundles (still valid) |
| [shikijs/textmate-grammars-themes](https://github.com/shikijs/textmate-grammars-themes) | Curated mirror of the above with provenance |

Recur vendors grammars into **`bundles/Languages/`** and the full Shiki
**tm-themes** catalog (plus Cool Glow, Made of Code, Aura, and a short list of
classic TextMate ports) into **`bundles/Themes/`** (re-fetch with
`mise run fetch:tm-grammars` / `fetch:tm-themes`). First-party Charcoal /
Paper live in **`bundles/Recur/`**.
Provenance JSON and a compact `NOTICE` (SPDX + upstream links; full license
texts stay at the Shiki NOTICE URL) sit next to each pack. Tokenization is
TextMate-only: there is no Lezer / `@codemirror/lang-*` fallback. Settings
uses a compact searchable list (Charcoal / Paper pinned); **View → Appearance**
mirrors app / editor / terminal theme choices in the native menu. The editor footer
language label is clickable (⌘K **Change language mode**): the pick is
remembered per file on the workspace editor session (not localStorage), with
an optional **Always open *.ext as…** association.

## Keybindings

Optional `keyEquivalent` on a command **or snippet** uses explicit macOS chord syntax:

| Token | Meaning |
|-------|---------|
| `cmd` | ⌘ Command |
| `ctrl` | ⌃ Control |
| `opt` | ⌥ Option |
| `shift` | ⇧ Shift |
| `fn` | Fn |
| key | `a`–`z`, `0`–`9`, `f1`–`f12`, `enter`, `esc`, `tab`, `space`, arrows, `delete`, `backspace`, … |

Examples: `cmd+shift+u`, `opt+cmd+j`. Modifiers are order-independent; matching is case-insensitive.

**App chords win.** If a bundle claims a shortcut the app already owns (Save, Quick Open, Run Command, …), that binding is dropped at runtime; the command remains available in ⌘⇧P. `recur bundles lint` reports the collision as an error for authors.

**Sharing is allowed.** When several palette commands or snippets share a chord (after scope / `fileGlob` filtering), the editor shows a TextMate-style menu grouped by category with numbered picks (`1`–`9`). A single match runs immediately. Bindings only fire while the editor has focus. Tab-trigger collisions use the same picker.

**Tab.** If the word before the caret equals a snippet `tabTrigger` for this file, Tab expands the body (CodeMirror placeholders). Typing a trigger prefix also lists matching snippets in the editor autocomplete menu (above buffer words; Tab/Enter inserts). Otherwise Tab indents. Placeholder fields take Tab first (`nextSnippetField`).
⌘⇧P / status-bar **Bundles** lists palette commands **and** snippets for the focused file, filtered by `scope` / `fileGlob`. Snippet rows show the tab trigger (`ife⇥`).

## Interpreters

| Interpreter | How it runs |
|-------------|-------------|
| `typescript` | swc in `recur-bundles` → cache `<DataDir>/BundleCache` → Worker on JavaScriptCore (`recur_bundles::js`, no node) + frozen `MacroContext` |
| `shebang` | OS subprocess, stdin = input, `RECUR_*` env |

`interpreter = "js"` and `"native"` are **rejected**. Use TypeScript (even for
plain JS-shaped code: the TypeScript compiler accepts it) or shebang.

### TypeScript contract

Import what you need from **`@recur/sdk`** (authoring copy: `bundles/sdk/`;
the compiler resolves the same module and bundles it into the Worker JS).

The Worker (`crates/bundles/src/js.rs`, JavaScriptCore) has the ECMAScript
built-ins with `Intl`, plus `console`, `setTimeout` / `setInterval` /
`queueMicrotask`, `TextEncoder` / `TextDecoder` (UTF-8), `atob` / `btoa`,
`URL` / `URLSearchParams`, `crypto.getRandomValues` / `randomUUID`,
`structuredClone`, `AbortController`, `performance.now` and `self`. It has
**no `fetch`** (nor `Headers` / `Request` / `Response` / `Blob`, or
`crypto.subtle`), no `require`, and no file access: use a shebang command for
network or disk work.

| Export | Kind |
|--------|------|
| `MacroContext` | input shape (`input`, `selectedText`, `filepath`, …) |
| `MacroResult` | `string` or `{ text; output?; language?; openInPreview? }` |
| `OutputRouting` | union of routing literals |
| `Output` | frozen map: use `Output.replaceSelection`, not raw strings |
| `replaceEdited(ctx, text)` | helper → selection replace, else document |

Only `@recur/sdk` imports are allowed. IDE path mapping: `bundles/tsconfig.json`.

```ts
import { Output, replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input.toUpperCase())
  // or: return { text: "…", output: Output.showAsToast }
}
```

Bare statement bodies (inline `body = '''…'''`) are auto-wrapped with a default
`@recur/sdk` import and `export default async function (ctx) { … }`.

### Shebang

Authors pick `sh` / `perl` / `awk` / `node` / `python` via `#!`. Prefer this for
tools that need the real filesystem, network, or a runtime on `PATH`.

## Shipped examples (`bundles/`)

| Bundle | Commands |
|--------|----------|
| `Text` | sort / unique / reverse / case / em dashes→hyphens|colons / slugify / strip WS / enumerate / randomize / join / split / filter lines; Word Count; Wrap at Column |
| `Source` | spaces↔tabs, to camel/snake/Pascal/kebab, toggle quotes / smart quotes, align assignments; default `[[symbol]]` (`entity.name.function`, hide calls/support) |
| `Encode` | URL / HTML entities / Base64 encode+decode |
| `JSON` | Format / Minify / Validate; Format on Save |
| `Diff` | Diff with Clipboard (`shebang` + `script=`) |
| `Go` / `Rust` / `Python` / `JavaScript` / `Shell` / `C` | format-on-save (`gofmt`, `rustfmt`, `ruff`/`black`, `deno fmt`/`prettier`, `shfmt`, `clang-format`): pass-through if tool missing |
| `LanguageData` | comment tokens + indent patterns + extra closeBrackets (`[[languageData]]`); CSS `[[symbol]]` selectors |
| `HTML` | Insert Open/Close Tag (⌃<); wrap (⌃W); `<br>` (⌃↩); TM HTML tab catalog (`div`, `form`, `table`, `ul`, `audio`, …; HTML5 + HTML 4.01 doctypes, no XHTML); id symbols: markup in HTML, Svelte, Vue, JSX/TSX, Astro, MDX, Handlebars (document-level doctypes/`html`/`head` stay on HTML/Svelte/Vue) |
| `Markdown` | wrap/heading/list; Hard Linebreak (⌃↩, two trailing spaces); heading symbols |
| `Terminal` | xterm.js `ITheme` palettes (`[[terminalTheme]]`) for Settings → Appearance |
| `Go` / `JavaScript` / `Python` / `Shell` / `XML` / `Source` / `Text` | snippet catalogs (`ife`, `log`, `def`, shebangs, quote wrap, lorem, `isoD`, …) |

## Categories

Free-form, unowned labels. Multiple bundles can contribute to `"Text"`. The
**Run Command** palette (⌘⇧P) groups by category (lowest `order`, then name).
A **Recent** section sits on top: last-used command first, then the rest by
frecency (run count × recency decay, 7-day half-life).
⌘K stays go-to search for workspaces / PRs / projects, plus any bundle
`[[search]]` groups.

## Settings

**Settings → Bundles** is a nav group with a page per contribution kind:

| Page | Source | Off means |
|------|--------|-----------|
| Commands | `[[command]]` | Leaves the Run Command palette, keybindings, and willSave |
| Prompt actions | `[[prompt]]` | Leaves the composer `/` menu and first-party invoke paths |
| Grammars | `[[grammar]]` | Not used for path highlighting |
| Themes | `[[theme]]` | Leaves Settings → Appearance and View → Appearance |
| Snippets | `[[snippet]]` | Do not expand on Tab or keyEquivalent, and leave the autocomplete menu |
| Language servers | `[[languageServer]]` | Soft-degrade in the Observatory editor (no completions / diagnostics) |

Commands are grouped by **category** (palette-aligned). Grammars, themes, and
snippets are searchable and grouped by pack. Disabled ids persist on
`Settings.disabledBundleCommands` / `disabledBundlePrompts` /
`disabledBundleGrammars` / `disabledBundleThemes` / `disabledBundleSnippets` /
`disabledLanguageServers`
in the app database. There is
no whole-bundle master switch. If the active editor theme is turned off, Recur
falls back to Charcoal (or the next enabled theme).

## Directories

| Root | Path |
|------|------|
| Default | `<DataDir>/Bundles` (seeded from repo `bundles/`, embedded in the app; each folder is content-hashed: when the shipped hash changes the directory is replaced and stamped with `.recur-seed`) |
| User | `~/.recur/bundles` (id wins over default; **prefer this for customizations**) |
| Cache | `<DataDir>/BundleCache` |

After installing, creating, or editing a bundle, choose **Navigate → Reload Bundles** to pick up grammars, themes, terminal themes, prompts, commands, and keybindings without restarting. Skipped bundles (bad manifest, collisions) are counted in the toast and listed in Settings → Bundles.

## Shebang environment

`RECUR_SELECTED_TEXT`, `RECUR_CURRENT_LINE`, `RECUR_LINE_NUMBER`,
`RECUR_COLUMN_NUMBER`, `RECUR_CURRENT_WORD`, `RECUR_FILEPATH`, `RECUR_FILENAME`,
`RECUR_DIRECTORY`, `RECUR_PROJECT_DIRECTORY`, `RECUR_SCOPE`, `RECUR_TAB_SIZE`,
`RECUR_SOFT_TABS`, `RECUR_CLIPBOARD`, `RECUR_BUNDLE_SUPPORT`, `RECUR_INPUT_<NAME>`.
Prompt shebang also gets workspace vars (`RECUR_WORKSPACE_ID`, …,
`RECUR_CONVENTIONAL_COMMITS` = `1` when project.toml sets
`conventional_commits = true`; TypeScript gets `ctx.conventionalCommits`) plus
`RECUR_SETTING_<NAME>` / `RECUR_BUNDLE_SETTINGS` for that pack’s `[[setting]]`
values (never TypeScript Workers). `[[search]]` also sets `RECUR_SEARCH_QUERY`
(and the complete-script args env so a pack can reuse its `complete` script).

Shebang commands run in a process group with a **30s** default deadline
(`RunContextDTO.timeoutMs` can lower it: willSave uses ≤12s per command).
There is no UI cancel control in v1; the deadline is what stops a runaway
process (the process group is killed when the context ends).

Malformed packs under a scan root are **skipped** (other bundles still load).
Reasons show in **Settings → Bundles → Commands → Skipped packs** and via
`recur bundles lint`.

## Output routing

`discard` · `replaceSelection` · `replaceDocument` · `replaceInput` ·
`insertText` · `insertSnippet` · `newDocument` · `showAsTooltip` ·
`copyToClipboard` · `showAsToast`

**Exit code:** exit `0` means apply the routed output. A non-zero exit toasts
stderr and **discards** buffer-mutating routings (`replace*` / `insert*` /
`newDocument`). `showAsToast` / `showAsTooltip` / `copyToClipboard` may still
fire when stdout is non-empty. Tools that use exit `1` as a normal outcome
(e.g. `diff`) must normalize to `0` before returning: see
`bundles/Diff/bin/diff-clipboard.sh`.

`showAsHTML` was removed: use `newDocument` with `language` + optional
`openInPreview` (HTML Preview is a sandboxed iframe: `allow-scripts allow-forms`,
no `allow-same-origin`; https CDN scripts/styles and `fetch` to https work,
`localStorage` is in-memory only).

### `newDocument` language & preview

Scratch tabs need a highlighter. Set `language` on the command (file extension /
lang id: `diff`, `json`, `ts`, `md`, `html`, `svg`, …) or return it from a
TypeScript macro:

```ts
return { text, output: Output.newDocument, language: 'diff' }
```

Runtime `language` wins over the manifest. Default is `txt` (plain).

Set `openInPreview = true` (manifest) or `openInPreview: true` (macro result) to
open the scratch on the **Preview** chip when the language supports it (`html`,
`htm`, `md`/`markdown`/`mdx`, `svg`, `mmd`/`mermaid`). Otherwise it opens on
**Code**. Macro result wins over the manifest.

`recur bundles lint` errors if the manifest sets `openInPreview` without
`output = "newDocument"` or with a non-previewable `language`.

## willSave

Runs before `WriteFile` under a ~15s overall budget (each command gets
`timeoutMs` ≤ 12s). On failure/timeout, empty stdout, or shrinking the document
by more than ~90% (when the original is >100 chars), Recur writes the **original**
document and toasts: the transform is discarded.

## Testing

Put cases next to the bundle under `tests/*.toml`. External authors use the same
layout under `~/.recur/bundles/MyPack/tests/`. Runtime seeding skips `tests/` so
DataDir stays lean; tests stay in git / your user pack.

Payloads can be **inline** or **files** (paths relative to the TOML file). Use
one or the other per field: not both.

```toml
# Inline (small cases)
[[test]]
name = "uppercases selection"
command = "Uppercase"
[test.context]
input = "Hello"
selectedText = "Hello"
[test.expect]
stdout = "HELLO"
output = "replaceSelection"

# Snippet bodies (no caret assertions; command XOR snippet)
[[test]]
name = "ife body"
snippet = "recur.go.ife"
[test.expect]
stdout = '''
if ${1:err} != nil {
	${2:log.Fatal(${1:err})}
}
'''

# File payloads (nicer diffs for multi-line text)
[[test]]
name = "sorts lines ascending"
command = "recur.text.sort-lines-asc"
[test.context]
inputFile = "sort-asc/input.txt"
selectedTextFile = "sort-asc/input.txt"
[test.expect]
stdoutFile = "sort-asc/expect.txt"
output = "replaceSelection"
```

Optional expect fields: `language`, `openInPreview` (effective values after
manifest defaults + MacroResult overrides: same resolution as the app).
Optional `*File` fields: `inputFile`, `selectedTextFile`, `documentFile`,
`currentLineFile`, `currentWordFile`, `clipboardFile`, `stdoutFile`, `stderrFile`.
Other context knobs stay inline (`scope`, `tabSize`, `softTabs`, `prompts`, …).
Non-deterministic commands may set `skipStdout = true` (still checks exit / output).

Shipped packs under `bundles/*/tests/` are the reference suite (also covered by
`cargo test -p recur-bundles`).

## CLI (`recur`)

The Recur CLI is separate from the desktop GUI binary. A
packaged Recur.app ships it at `Contents/MacOS/recur-cli` and, on
launch, symlinks that file to `/usr/local/bin/recur` (when writable) and
`~/.local/bin/recur` so `recur` is on PATH. Contributors run it with
`cargo run -p recur-cli -- …` (or `target/debug/recur-cli` after `mise run build`).

A bare path opens that file or folder in Recur (`recur .` is the current
directory):

```bash
recur .                              # open cwd in the desktop app
recur ./path/to/file-or-folder
recur bundles create MyPack          # → ~/.recur/bundles/MyPack (from embedded scaffold)
recur bundles test MyPack            # name under bundles-dir
recur bundles test Text              # also resolves ./bundles/Text
recur bundles test ./bundles         # every child with bundle.toml
recur bundles lint ./bundles         # schema, TS compile, shebang, keyEquivalent
recur completion zsh > ~/.zfunc/_recur  # bash, zsh, fish, powershell, elvish
```

Config / env:

| Key | Env | Default |
|-----|-----|---------|
| `bundles-dir` | `RECUR_BUNDLES_DIR` | `~/.recur/bundles` |

Optional file: `~/.recur/config.yaml` (`bundles-dir: …`).

**Create scaffold** lives as real files under `crates/cli/scaffold/` (embedded
into the CLI). Edit that tree to change what `recur bundles create` drops:
placeholders: `{{BUNDLE_NAME}}`, `{{BUNDLE_ID}}`, `{{TS_CMD_ID}}`,
`{{SH_CMD_ID}}`.

TypeScript cases compile with the same swc path as the app and run on the same
JavaScriptCore Worker (`recur_bundles::js`). Shebang cases use `bundles.Run`.

## Chat prompt actions (`[[prompt]]`)

Prompt actions are **user-invoked chat verbs** (not Agent Skills, not editor
commands). They appear in the composer `/` menu above skills (alphabetical,
distinct “Prompt” badge). Selecting one invokes immediately unless `takesArgs`
is set: then the host inserts Discord-style labeled arg slots instead of a
skill chip. The slash-name menu still only opens when `/` is at offset 0 with
trailing whitespace: once args start, that menu closes and the args picker
takes over (placeholder plus optional `complete` hits). Enter on a whole-composer
line `/slash rest` invokes with `ctx.args` / `RECUR_PROMPT_ARGS` set to `rest`.
Optional mode tab routing → run script → send.

```toml
[[prompt]]
id = "recur.chat.fix-findings"
name = "Fix Findings"
description = "Fix or dispute open reviewer findings"
slash = "fix-findings"          # default: kebab of name
mode = "agent"                  # optional: ask|plan|agent|review|loop → find idle tab / create
takesArgs = false              # optional: true → Discord-style labeled slots instead of invoking
timeoutMs = 5000                # optional; default 5s, ceiling 30s
# tab_group = "ops"           # optional: share a tab slot (nearest in group / create); requires mode
# fresh_session = true        # optional: new provider session on the landed tab; not with mode=loop
interpreter = "typescript"      # or shebang
script = "src/fix-findings.ts"
```

`complete` is how a prompt action owns search/autocomplete for its args: the
composer is generic. Optional `icon = "icon.svg"` (pack-relative, static SVG)
replaces the default hash glyph on those complete rows and on ⌘K hits for
that slash. Hardcoded `fill="#…"` is rewritten to `currentColor`. Interpreter is inferred from the extension (`.ts` / `.js` →
the TypeScript Worker; anything else → shebang). TypeScript complete must not touch
secrets (Worker sandbox); it also gets `ctx.diagnostics`, the workspace's LSP problems. Shebang complete gets the same env as the invoke
script (`RECUR_PROMPT_ARGS`, named `RECUR_PROMPT_ARG_*`, `RECUR_PROMPT_PARAMS`,
`RECUR_PROMPT_COMPLETE_ARG` for the focused slot, and that bundle’s
`[[setting]]` values as `RECUR_SETTING_<NAME>` / `RECUR_BUNDLE_SETTINGS`) and runs in the workspace worktree. Timeout 4s; at most
25 hits. Stdout / return:

```json
{"hits":[{"insert":"ENG-123","label":"ENG-123","detail":"Fix login · In Progress","badge":"Issue","action":""}],"message":""}
```

Named parameters (`[[prompt.arg]]`) render Discord-style labeled slots after
`/slash` instead of a skill chip. They imply `takesArgs`. Scripts see
`ctx.params` / `RECUR_PROMPT_ARG_ISSUE` (plus `ctx.args` / `RECUR_PROMPT_ARGS`
as the remainder). Complete scripts see the **focused slot**: `ctx.arg` /
`RECUR_PROMPT_COMPLETE_ARG` (the `[[prompt.arg]]` name), `ctx.args` /
`RECUR_PROMPT_ARGS` (that slot's value), and `ctx.params` / `RECUR_PROMPT_ARG_*`
(every slot). One complete script can branch on `ctx.arg` so each input has its
own suggestions. TakesArgs without `[[prompt.arg]]` synthesizes one slot
named `args`. `optional = true` on a slot lets Enter on an empty chip invoke
(scripts treat empty as unset); omit it to keep the slot required (Linear).

Put `[[prompt.arg]]` **after every scalar** on that `[[prompt]]` (`timeoutMs`,
`script`, `complete`, …). A nested table in the middle of the prompt steals
following keys onto the arg.

```toml
[[prompt]]
id = "recur.linear.work"
slash = "linear"
takesArgs = true
interpreter = "shebang"
script = "src/linear.sh"
complete = "src/complete.sh"
icon = "icon.svg"   # optional .svg for complete / ⌘K rows (else hash)

[[prompt.arg]]
name = "issue"
placeholder = "TEAM-123 or paste a Linear URL"
# optional = true   # empty Enter invokes; omit to keep the slot required
```

`insert` replaces the trailing args (and is what Enter runs). `action` is an
optional host URI (`settings:integrations` opens Settings). Action-only hits
need no `insert` (e.g. “Connect Linear…”).

`mode = "loop"` plus `loop = "recur.loops.converge"` (or another `[[loop]]` id)
routes like the other modes (idle tab already in Loop, else a new chat) and
sends a canned engage: `/review-and-fix` from Agent does not need a typed goal.

TypeScript / shebang export `(ctx: PromptContext) => PromptResult` with
`{ prompt, display? }`. `PromptContext` includes workspace vars (`workspaceId`,
`projectId`, `worktree`, `port`), `args` (trailing `/slash` remainder), and a
focused-editor snapshot. Shebang env: `RECUR_PROMPT_ARGS` (capped like other
blobs). Scripts that ignore args stay valid.

## Palette search (`[[search]]`) and New Task expand (`[[expand]]`)

Shebang only (TypeScript Workers never see `RECUR_SETTING_*`). Search-only or
expand-only packs are valid. The host debounce (⌘K 200ms) and goroutine fan-out
live in core: packs do not debounce.

```toml
[[search]]
id = "recur.linear.search"
name = "Linear"          # ⌘K section header
icon = "icon.svg"
interpreter = "shebang"
script = "src/complete.sh"
timeoutMs = 4000         # default 4s, cap 30s

[[expand]]
id = "recur.linear.expand"
name = "Linear issue"
match = '''(?i)^(https?://\S*linear\.app\S*|…)'''  # Rust regex; compiled at parse
interpreter = "shebang"
script = "src/linear.sh"
timeoutMs = 15000        # default 15s
```

`match` uses the Rust [`regex` crate syntax](https://docs.rs/regex/latest/regex/#syntax).
Classes like `\d \s \w \b` are Unicode-aware; wrap them in `(?-u:…)` (or start
the pattern with `(?-u)`) for ASCII-only. `\Q…\E` quoting is not supported.

Empty `match` means the host never auto-runs that expander. Bad regex is a
parse/lint error. `BundleService.SearchPalette` / `ExpandGoal` inject that
pack’s `[[setting]]` env. Failed/timeout search packs return an empty group;
others still show. Expand skip: empty `prompt` or `{"skip":true}` → try the
next matching expander, then the original text. A matched expander that errors
toasts (same as today). No Settings → Bundles toggle in v1.

**Contract:** `@path` and `/skill` tokens inside `prompt` are real composer
dialect: the host wraps them on send (do not pre-wrap). Wrap embedded blobs
that must not be scanned (git diffs, etc.) in nonce-paired
`⟦raw:ID⟧`…`⟦/raw:ID⟧` (bare `⟦raw⟧`…`⟦/raw⟧` is ok only when the body cannot
contain those strings). Markers are stripped before storage/provider. Session
stores the full wire as `text` and the short label as `display_text`. An empty
`prompt` is a no-send success: the host toasts `display` and does not start a
chat turn (used by `/update` when git finishes cleanly). An optional
`"issue": {"source": "linear", "id": "<stable tracker id>", "key": "ENG-123"}`
links the workspace the prompt runs in (or the one New Task creates) to that
issue, so the Issues view shows View workspace for it. If nobody is assigned
yet, linking also assigns the issue to the user on GitHub or Linear. Use the
tracker's stable id (Linear issue UUID, GitHub issue node id), not the human key.

Shipped flagships live in `bundles/Chat`: `/fix-findings` (`mode=agent`,
`tab_group = "ops"`, `fresh_session`; invoke from the review chat and it
jumps to the ops tab),
`/commit` + `/commit-push` (`mode=agent`, `tab_group = "ops"`, `fresh_session`; message from the
diff), `/fix-checks` (`mode=agent`, `tab_group = "ops"`, `fresh_session`; GitHub failing checks), `/address-review`
(`mode=agent`, `tab_group = "ops"`, `fresh_session`, optional `reviewer` arg; unresolved trusted review feedback on the GitHub PR; an empty prompt when nothing needs a response, which keeps automatic runs silent), `/fix-problem`
(`mode=agent`, `tab_group = "ops"`, `fresh_session`; one LSP diagnostic, sent by the Problems panel's AI button; `complete` lists the workspace's current diagnostics from `ctx.diagnostics`), `/fix-merge` (`mode=agent`, `tab_group = "ops"`, `fresh_session`; sent with the error when the titlebar Merge fails, so the agent clears the blocker and the human retries), `/open-pr` + `/open-draft-pr`
(`mode=agent`, `tab_group = "ops"`, `fresh_session`, shebang: same `open-pr.sh`; `/open-draft-pr` sets
`RECUR_PR_DRAFT=1` so the agent calls `mcp__recur__open_pr` with `draft: true`.
Embeds git status/diff so the agent writes the GitHub title/body from the
full workspace diff; Recur owns push + `gh pr create` / `--draft`. If a
GitHub PR is already open, the shebang emits an empty prompt so the host
toasts and does not start an LLM turn or open a tab: push from the titlebar
instead. A
Local fallback does not fast-fail so create can retry),
`/update` (`mode=agent`, `tab_group = "ops"`, `fresh_session` only if git needs the agent;
clean git merge/rebase with no LLM; conflicts and other git failures fall back
to an agent prompt on the ops tab), `/review`
(`mode=review`, `tab_group = "review"`, shebang embeds git status/diff so the agent can skip discovery
loops; `fresh_session` isolates a provider session on that tab), `/re-review` (`mode=review`, no `tab_group`, same git embed after
fixes; routes to the newest idle Review tab like other ungrouped mode prompts, then
resumes that tab's prior pass when it can: delta diff only),
`/simplify-review` (`mode=review`, `tab_group = "simplify-review"`, over-engineering only: files findings with
yagni/stdlib/native/delete/shrink tags; `/review` stays bugs/risks; `fresh_session`), and
`/simplify-audit` (`mode=review`, `tab_group = "simplify-audit"`, whole-repo over-engineering, same findings
contract; `fresh_session`).
`bundles/Linear` ships `/linear` (`takesArgs`, no `mode` so it stays on the current chat, shebang: GraphQL
`issue` using `RECUR_SETTING_API_KEY` from the Linear bundle’s `[[setting]]`).
Composer arg complete is the prompt’s `complete = "src/complete.sh"` (shebang
GraphQL `searchIssues`). The same script is the pack’s `[[search]]` for ⌘K;
`[[expand]]` (`src/linear.sh`) paste-expands New Task / ⌘K picks when `match`
hits. Host only runs matching expanders: prose goals are unchanged.
Agent Linear tools (backlog / triage) are Settings → MCP (`D-user-mcp`), not shebang.
`bundles/GitHub` ships `/github` (`takesArgs`, shebang over the `gh` CLI, no
API key): `#12` resolves against the workspace repo, plus `owner/repo#12` and
issue URLs; pull requests are rejected. Its `[[expand]]` paste-expands issue
URLs and `owner/repo#12` in New Task. Both `/linear` and `/github` return
`issue`, so the workspace is linked to the issue it works on.

User bundles may
shadow slash names (user > default after id dedupe). Typed
follow-ups resume the tab they are typed on.

## Security

Shebang = user-privilege OS process. TypeScript = Worker sandbox with a sealed
context: still only install bundles you trust. File writes stay behind
`CodeService` worktree gating.

## Roadmap

Phase C snippets (`[[snippet]]`, Tab expansion, Settings toggles) ship in
`bundles/Go`, `JavaScript`, `Python`, `Shell`, `HTML`, `XML`, `Source`, and `Text`.
Phase A command packs (Math, Markdown wrap, Shell execute, YAML, XML, C clang-format,
Text leftovers, Source braces) ship under `bundles/`.
Phase B comment tokens + indent ship in `bundles/LanguageData`.
