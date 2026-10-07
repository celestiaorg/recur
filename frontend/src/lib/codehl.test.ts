import { describe, expect, test } from 'vitest'
import { highlightHtmlCodeBlocks, looksLikeUnifiedDiff, parseUnifiedDiff } from './codehl'

describe('highlightHtmlCodeBlocks', () => {
  test('tints typescript fences from LSP hover markdown', () => {
    const html = '<pre><code class="language-typescript">(alias) function toErrorResponse(error: unknown): Record&lt;string, unknown&gt;</code></pre>'
    const out = highlightHtmlCodeBlocks(html)
    expect(out).toContain('class="sh-kw">function</span>')
    expect(out).toContain('class="sh-kw">unknown</span>')
    expect(out).toContain('Record')
  })

  test('leaves already-highlighted fences alone', () => {
    const html = '<pre><code class="language-ts"><span class="sh-kw">function</span> f()</code></pre>'
    expect(highlightHtmlCodeBlocks(html)).toBe(html)
  })
})

describe('looksLikeUnifiedDiff', () => {
  test('accepts full unified diffs', () => {
    const src = `--- a/hello.txt
+++ b/hello.txt
@@ -1,2 +1,2 @@
 hello
-world
+universe
`
    expect(looksLikeUnifiedDiff(src)).toBe(true)
  })

  test('accepts Codex hunk-only diffs', () => {
    const src = `@@ -1,2 +1,2 @@
 hello
-world
+universe
`
    expect(looksLikeUnifiedDiff(src)).toBe(true)
  })

  test('rejects plain file bodies', () => {
    expect(looksLikeUnifiedDiff('export const hi = 1;\n')).toBe(false)
    expect(looksLikeUnifiedDiff('{"file_path":"/tmp/a.ts"}')).toBe(false)
  })
})

describe('parseUnifiedDiff', () => {
  test('parses Codex hunk-only bodies', () => {
    const rows = parseUnifiedDiff(`@@ -1,2 +1,2 @@
 hello
-world
+universe
`)
    expect(rows.filter((r) => r.text !== '' || r.kind !== 'ctx')).toEqual([
      { kind: 'ctx', text: 'hello' },
      { kind: 'del', text: 'world' },
      { kind: 'add', text: 'universe' },
    ])
  })
})
