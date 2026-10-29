// Regenerates the differential fixtures with the frontend's vscode-textmate
// and vscode-oniguruma. Run from frontend/:
//   cp ../crates/textmate/tests/fixtures/differential/generate.cjs . && node generate.cjs && rm generate.cjs
const fs = require('fs'), path = require('path')
const vt = require('./node_modules/.pnpm/vscode-textmate@9.3.2/node_modules/vscode-textmate/release/main.js')
const onig = require('./node_modules/.pnpm/vscode-oniguruma@2.0.1/node_modules/vscode-oniguruma/release/main.js')
const root = path.resolve('..')
const grammars = {}
for (const b of fs.readdirSync(path.join(root, 'bundles'))) {
  const dir = path.join(root, 'bundles', b, 'syntaxes')
  if (!fs.existsSync(dir)) continue
  for (const f of fs.readdirSync(dir)) {
    const p = path.join(dir, f); const c = fs.readFileSync(p, 'utf8')
    grammars[JSON.parse(c).scopeName] = { c, p }
  }
}
const cases = [
  ['source.ts', 'frontend/src/lib/tm-engine.ts', 260],
  ['source.go', 'internal/bundles/scope.go', 200],
  ['source.svelte', 'frontend/src/components/Ring.svelte', 200],
  ['text.html.markdown', 'AGENTS.md', 200],
  ['source.rust', 'crates/textmate/src/rule.rs', 200],
  ['source.python', null, 0],
  ['source.css.scss', 'frontend/src/styles/_codemirror.scss', 200],
  ['source.json', 'frontend/package.json', 200],
  ['source.yaml', '.github/workflows/ci.yml', 200],
  ['source.shell', null, 0],
  ['text.html.basic', 'frontend/index.html', 200],
]
;(async () => {
  await onig.loadWASM(fs.readFileSync('./node_modules/.pnpm/vscode-oniguruma@2.0.1/node_modules/vscode-oniguruma/release/onig.wasm'))
  const registry = new vt.Registry({
    onigLib: Promise.resolve({ createOnigScanner: (p) => new onig.OnigScanner(p), createOnigString: (s) => new onig.OnigString(s) }),
    loadGrammar: async (s) => grammars[s] ? vt.parseRawGrammar(grammars[s].c, grammars[s].p) : null,
  })
  const charcoal = JSON.parse(fs.readFileSync(path.join(root, 'bundles/Recur/themes/charcoal.tmTheme.json'), 'utf8'))
  const rawTheme = { name: 'charcoal', settings: [{ settings: { foreground: charcoal.colors['editor.foreground'], background: charcoal.colors['editor.background'] } }, ...charcoal.tokenColors.filter((t) => t.scope)] }
  const themed = new vt.Registry({
    onigLib: Promise.resolve({ createOnigScanner: (p) => new onig.OnigScanner(p), createOnigString: (s) => new onig.OnigString(s) }),
    loadGrammar: async (s) => grammars[s] ? vt.parseRawGrammar(grammars[s].c, grammars[s].p) : null,
    theme: rawTheme,
  })
  themed.setTheme(rawTheme)
  for (const [scope, file, max] of cases) {
    if (!grammars[scope]) { console.error('missing', scope); continue }
    let text
    if (file) text = fs.readFileSync(path.join(root, file), 'utf8').split('\n').slice(0, max).join('\n')
    else if (scope === 'source.python') text = 'import os\n\nclass Foo(Base):\n    """Doc string\n    more"""\n    def bar(self, x: int = 3) -> str:\n        return f"{x!r} and {self.y}"  # comment\n\n@decorator\nasync def baz():\n    await go(lambda q: q ** 2)\n'
    else text = '#!/bin/bash\nset -euo pipefail\nfor f in "$@"; do\n  echo "file: ${f%.txt}" | grep -E \'^a\' >&2\ndone\ncat <<EOF\nhello $USER\nEOF\n'
    // Unicode stress line
    text += '\n// é ünïcödé 𝔘 日本語 "str" אב rtl'
    const g = await registry.loadGrammar(scope)
    let stack = vt.INITIAL
    const lines = []
    for (const line of text.split('\n')) {
      const r = g.tokenizeLine(line, stack)
      lines.push({ line, tokens: r.tokens.map((t) => [line.substring(t.startIndex, t.endIndex), t.scopes]) })
      stack = r.ruleStack
    }
    const tg = await themed.loadGrammar(scope)
    let tstack = vt.INITIAL
    const binary = []
    for (const line of text.split('\n')) {
      const r = tg.tokenizeLine2(line, tstack)
      const toks = []
      for (let i = 0; i < r.tokens.length; i += 2) {
        const end = i + 2 < r.tokens.length ? r.tokens[i + 2] : line.length
        toks.push([line.substring(r.tokens[i], end), r.tokens[i + 1]])
      }
      binary.push(toks)
      tstack = r.ruleStack
    }
    const out = path.join(root, 'crates/textmate/tests/fixtures/differential', scope + '.json')
    fs.writeFileSync(out, JSON.stringify({ scope, lines, binary, colorMap: themed.getColorMap().map((c) => c ?? '') }))
    console.log(scope, lines.length)
  }
})()
