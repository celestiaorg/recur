/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { HTML_PREVIEW_CSP, HTML_PREVIEW_SCROLL_SCRIPT, PREVIEW_IFRAME_FIND, PREVIEW_IFRAME_FIND_KEY, PREVIEW_IFRAME_FIND_RESULT, PREVIEW_IFRAME_SCROLL, filePreviewFindActive, htmlPreviewSrcdoc, previewKindForPath } from './preview'

describe('previewKindForPath', () => {
  it('maps html and htm to html', () => {
    expect(previewKindForPath('demo.html')).toBe('html')
    expect(previewKindForPath('/x/Y.HTM')).toBe('html')
  })
})

describe('filePreviewFindActive', () => {
  const on = { showPreview: true, shellActive: true, mainSurface: 'editor', termOverlaying: false, editorHidden: false }

  it('is on only for a visible editor preview', () => {
    expect(filePreviewFindActive(on)).toBe(true)
    expect(filePreviewFindActive({ ...on, showPreview: false })).toBe(false)
    expect(filePreviewFindActive({ ...on, shellActive: false })).toBe(false)
    expect(filePreviewFindActive({ ...on, mainSurface: 'review' })).toBe(false)
    expect(filePreviewFindActive({ ...on, termOverlaying: true })).toBe(false)
  })

  it('is off after Hide editor even when preview state is otherwise live', () => {
    expect(filePreviewFindActive({ ...on, editorHidden: true })).toBe(false)
  })
})

describe('HTML_PREVIEW_CSP', () => {
  it('allows inline script/style and blocks subresource/fetch network hosts', () => {
    expect(HTML_PREVIEW_CSP).toContain("script-src 'unsafe-inline'")
    expect(HTML_PREVIEW_CSP).toContain("style-src 'unsafe-inline'")
    expect(HTML_PREVIEW_CSP).toContain("connect-src 'none'")
    expect(HTML_PREVIEW_CSP).toContain("default-src 'none'")
    expect(HTML_PREVIEW_CSP).toContain("frame-src 'none'")
    expect(HTML_PREVIEW_CSP).toContain("worker-src 'none'")
    expect(HTML_PREVIEW_CSP).not.toContain('https:')
    expect(HTML_PREVIEW_CSP).not.toContain('http:')
  })
})

describe('htmlPreviewSrcdoc', () => {
  const findStyle =
    '<style>::highlight(preview-find){background-color:#e6cc7788}' +
    '::highlight(preview-find-current){background-color:#e6a817;color:#111}</style>'
  const shellPrefix = `<!DOCTYPE html><html><head><meta http-equiv="Content-Security-Policy" content="${HTML_PREVIEW_CSP}">${findStyle}<script>${HTML_PREVIEW_SCROLL_SCRIPT}</script></head><body>`
  const shellSuffix = `</body></html>`

  it('always wraps content in a CSP-first document shell', () => {
    const frag = '<button id="b">Go</button><script>b.onclick=()=>{}</script>'
    expect(htmlPreviewSrcdoc(frag)).toBe(shellPrefix + frag + shellSuffix)
    expect(htmlPreviewSrcdoc(frag)).toContain(PREVIEW_IFRAME_SCROLL)
    expect(htmlPreviewSrcdoc(frag)).toContain(PREVIEW_IFRAME_FIND)
    expect(htmlPreviewSrcdoc(frag)).toContain(PREVIEW_IFRAME_FIND_KEY)
    expect(htmlPreviewSrcdoc(frag)).toContain('e.data.regexp')
    expect(htmlPreviewSrcdoc(frag)).toContain('e.data.wholeWord')
    expect(htmlPreviewSrcdoc(frag)).toContain('e.hidden')
    expect(htmlPreviewSrcdoc(frag)).toContain('overflowX')
    expect(htmlPreviewSrcdoc(frag)).toContain('SHOW_ELEMENT')
    const esc = HTML_PREVIEW_SCROLL_SCRIPT.match(/q\.replace\((\/(?:\\.|[^/])+\/g),('(?:\\.|[^'])*')\)/)
    expect(esc).toBeTruthy()
    const escape = new Function('q', `return q.replace(${esc![1]}, ${esc![2]})`) as (q: string) => string
    expect(escape('a+b')).toBe('a\\+b')
    expect(escape('hello')).toBe('hello')
  })

  it('wraps full documents without splicing into their head', () => {
    const src =
      '<!DOCTYPE html><html><head><title>T</title></head><body><script>1</script></body></html>'
    const out = htmlPreviewSrcdoc(src)
    expect(out.startsWith(shellPrefix)).toBe(true)
    expect(out.endsWith(shellSuffix)).toBe(true)
    expect(out).toContain('<title>T</title>')
    expect(out).toContain('<script>1</script>')
    // Outer CSP only once; author has no CSP meta here.
    expect(out.match(/Content-Security-Policy/g)?.length).toBe(1)
  })

  it('wraps html without a head the same way', () => {
    const src = '<HTML lang="en"><body>hi</body></HTML>'
    expect(htmlPreviewSrcdoc(src)).toBe(shellPrefix + src + shellSuffix)
  })

  it('keeps an author CSP meta in the payload (outer CSP still first)', () => {
    const src =
      '<html><head><meta http-equiv="Content-Security-Policy" content="default-src \'self\'">' +
      '</head><body>x</body></html>'
    const out = htmlPreviewSrcdoc(src)
    expect(out.startsWith(shellPrefix)).toBe(true)
    expect(out.match(/Content-Security-Policy/g)?.length).toBe(2)
    expect(out).toContain("default-src 'self'")
  })

  it('wraps empty content into a CSP document shell', () => {
    expect(htmlPreviewSrcdoc('')).toBe(shellPrefix + shellSuffix)
  })

  it('does not splice CSP into comment or header decoys', () => {
    const decoys = [
      `<!--<head>--><script>fetch('https://evil')</script>`,
      `<header></header><head></head><script>1</script>`,
      `<head data-x=">"></head><script>1</script>`,
      `<script>fetch('https://evil')</script><head></head>`,
    ]
    for (const src of decoys) {
      const out = htmlPreviewSrcdoc(src)
      expect(out.startsWith(shellPrefix)).toBe(true)
      expect(out).toBe(shellPrefix + src + shellSuffix)
    }
  })
})

describe('HTML_PREVIEW_SCROLL_SCRIPT find reveal', () => {
  it('scrolls the iframe document to a match below the fold', async () => {
    const iframe = document.createElement('iframe')
    document.body.append(iframe)
    const idoc = iframe.contentDocument
    const iw = iframe.contentWindow
    expect(idoc && iw).toBeTruthy()
    idoc!.body.innerHTML = '<p>needle</p>'

    Object.defineProperty(iw!, 'innerHeight', { configurable: true, value: 500 })
    Object.defineProperty(iw!, 'innerWidth', { configurable: true, value: 400 })
    const se = idoc!.scrollingElement || idoc!.documentElement
    let top = 0
    let left = 0
    Object.defineProperty(se, 'scrollTop', {
      configurable: true,
      get: () => top,
      set: (v: number) => { top = Number(v) },
    })
    Object.defineProperty(se, 'scrollLeft', {
      configurable: true,
      get: () => left,
      set: (v: number) => { left = Number(v) },
    })

    const orig = iw!.Range.prototype.getBoundingClientRect
    iw!.Range.prototype.getBoundingClientRect = function () {
      if (this.toString() === 'needle') {
        return {
          top: 2000, bottom: 2024, height: 24, left: 1800, right: 1880, width: 80,
          x: 1800, y: 2000, toJSON() {},
        } as DOMRect
      }
      return orig.apply(this)
    }

    iw!.eval(HTML_PREVIEW_SCROLL_SCRIPT)

    const result = await new Promise<{ total: number; index: number }>((resolve, reject) => {
      const t = setTimeout(() => reject(new Error('no find result')), 1000)
      window.addEventListener('message', function onMsg(e: MessageEvent) {
        if (!e.data || e.data.type !== PREVIEW_IFRAME_FIND_RESULT) return
        window.removeEventListener('message', onMsg)
        clearTimeout(t)
        resolve(e.data)
      })
      iw!.dispatchEvent(new iw!.MessageEvent('message', {
        data: { type: PREVIEW_IFRAME_FIND, op: 'query', search: 'needle' },
      }))
    })

    expect(result.total).toBe(1)
    expect(result.index).toBe(1)
    expect(top).toBeGreaterThan(0)
    expect(left).toBeGreaterThan(0)
    iframe.remove()
  })
})
