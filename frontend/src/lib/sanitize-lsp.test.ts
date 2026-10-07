/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
// Installs the chat DOMPurify hook (strips classes except sh-*). Same load order as the app.
import './markdown'
import { sanitizeLspHTML, stripDangerousHtml } from './sanitize-lsp'

describe('stripDangerousHtml', () => {
  test('removes img onerror', () => {
    const out = stripDangerousHtml('<img src=x onerror="alert(1)">')
    expect(out).not.toContain('onerror')
    expect(out).not.toContain('alert')
  })
})

describe('sanitizeLspHTML', () => {
  test('strips event handlers then highlights fences', () => {
    const html = '<p>hi</p><img src=x onerror="alert(1)"><pre><code class="language-js">const x = 1</code></pre>'
    const out = sanitizeLspHTML(html)
    expect(out).not.toContain('onerror')
    expect(out).not.toContain('alert')
    expect(out).toContain('sh-kw')
  })

  test('tints hover fences after the chat sanitizer strips language-* classes', () => {
    const go = sanitizeLspHTML('<pre><code class="language-go">func (e *Engine) send(ok bool) string</code></pre>')
    expect(go).toContain('class="sh-kw">func</span>')
    expect(go).toContain('class="sh-kw">bool</span>')

    const ts = sanitizeLspHTML('<pre><code class="language-typescript">(alias) function toErrorResponse(error: unknown): void</code></pre>')
    expect(ts).toContain('class="sh-kw">function</span>')
    expect(ts).toContain('class="sh-kw">unknown</span>')
  })
})
