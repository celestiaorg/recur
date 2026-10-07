/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import {
  collectPreviewFindRanges, findInText, previewFindCountLabel, revealPreviewFindRange, PREVIEW_FIND_CAP,
} from './preview-find'

function root(html: string): HTMLElement {
  const el = document.createElement('div')
  el.innerHTML = html
  document.body.append(el)
  return el
}

describe('collectPreviewFindRanges', () => {
  it('finds case-insensitive matches in nested text', () => {
    const el = root('<p>Hello <b>hello</b> HELLO</p>')
    const { ranges, capped } = collectPreviewFindRanges(el, 'hello')
    expect(capped).toBe(false)
    expect(ranges).toHaveLength(3)
    expect(ranges[0].toString()).toBe('Hello')
    el.remove()
  })

  it('honours caseSensitive', () => {
    const el = root('<p>Hello hello</p>')
    expect(collectPreviewFindRanges(el, 'Hello', { caseSensitive: true }).ranges).toHaveLength(1)
    expect(collectPreviewFindRanges(el, 'Hello', { caseSensitive: false }).ranges).toHaveLength(2)
    el.remove()
  })

  it('skips script, style, and the find bar', () => {
    const el = root(
      '<p>needle</p><script>needle</script><style>needle</style>' +
      '<div class="file-preview-find">needle</div>',
    )
    expect(collectPreviewFindRanges(el, 'needle').ranges).toHaveLength(1)
    el.remove()
  })

  it('matches across inline text-node boundaries', () => {
    const el = root('<p>Hello <strong>world</strong></p>')
    const { ranges } = collectPreviewFindRanges(el, 'Hello world')
    expect(ranges).toHaveLength(1)
    expect(ranges[0].toString()).toBe('Hello world')
    el.remove()
  })

  it('applies whole-word against concatenated inline text', () => {
    const el = root('<p>hel<strong>lo</strong></p>')
    expect(collectPreviewFindRanges(el, 'lo', { wholeWord: true }).ranges).toHaveLength(0)
    expect(collectPreviewFindRanges(el, 'hello', { wholeWord: true }).ranges).toHaveLength(1)
    el.remove()
  })

  it('skips hidden and display:none text', () => {
    const el = root('<p hidden>needle</p><p style="display:none">needle</p><p>needle</p>')
    expect(collectPreviewFindRanges(el, 'needle').ranges).toHaveLength(1)
    el.remove()
  })

  it('skips text inside a hidden ancestor', () => {
    const el = root('<div hidden><p>needle</p></div><p>needle</p>')
    expect(collectPreviewFindRanges(el, 'needle').ranges).toHaveLength(1)
    el.remove()
  })

  it('does not match across block boundaries', () => {
    const el = root('<p>Hello</p><p>world</p>')
    expect(collectPreviewFindRanges(el, 'Hello world').ranges).toHaveLength(0)
    el.remove()
  })

  it('returns empty for an empty query', () => {
    const el = root('<p>hello</p>')
    expect(collectPreviewFindRanges(el, '').ranges).toHaveLength(0)
    expect(collectPreviewFindRanges(el, '(', { regexp: true }).valid).toBe(false)
    el.remove()
  })
})

describe('findInText', () => {
  it('matches regex', () => {
    expect(findInText('hello hallo', 'h.llo', { regexp: true })).toEqual([
      { start: 0, end: 5 },
      { start: 6, end: 11 },
    ])
  })

  it('returns null for an invalid regexp', () => {
    expect(findInText('hello', '(', { regexp: true })).toBeNull()
  })

  it('honours wholeWord', () => {
    expect(findInText('hello sayhello hello.', 'hello', { wholeWord: true })).toEqual([
      { start: 0, end: 5 },
      { start: 15, end: 20 },
    ])
  })

  it('applies wholeWord to regexp matches', () => {
    expect(findInText('cat cats cat', 'cat', { regexp: true, wholeWord: true })).toEqual([
      { start: 0, end: 3 },
      { start: 9, end: 12 },
    ])
  })
})

describe('previewFindCountLabel', () => {
  it('formats current / total and the cap', () => {
    expect(previewFindCountLabel(0, 0, false)).toBe('No results')
    expect(previewFindCountLabel(2, 5, false)).toBe('2 / 5')
    expect(previewFindCountLabel(1, PREVIEW_FIND_CAP, true)).toBe(`1 / ${PREVIEW_FIND_CAP}+`)
  })
})

describe('revealPreviewFindRange', () => {
  it('moves only the preview scroller, not overflow-hidden ancestors', () => {
    const outer = document.createElement('div')
    outer.style.overflow = 'hidden'
    Object.defineProperty(outer, 'scrollTop', { configurable: true, writable: true, value: 0 })

    const scroller = document.createElement('div')
    scroller.className = 'file-preview'
    scroller.style.overflow = 'auto'
    let top = 0
    Object.defineProperties(scroller, {
      scrollTop: {
        configurable: true,
        get: () => top,
        set: (v: number) => { top = v },
      },
      scrollHeight: { configurable: true, value: 2000 },
      clientHeight: { configurable: true, value: 200 },
      getBoundingClientRect: {
        value: () => ({ top: 80, bottom: 280, height: 200, left: 0, right: 400, width: 400, x: 0, y: 80, toJSON() {} }),
      },
    })

    const p = document.createElement('p')
    p.textContent = 'hello'
    scroller.append(p)
    outer.append(scroller)
    document.body.append(outer)

    const range = document.createRange()
    range.selectNodeContents(p)
    range.getBoundingClientRect = () => (
      { top: 900, bottom: 924, height: 24, left: 0, right: 80, width: 80, x: 0, y: 900, toJSON() {} }
    )

    revealPreviewFindRange(range)
    expect(outer.scrollTop).toBe(0)
    expect(scroller.scrollTop).toBeGreaterThan(0)
    outer.remove()
  })

  it('scrolls an inner overflow scroller on both axes', () => {
    const host = document.createElement('div')
    host.className = 'file-preview-host'

    const preview = document.createElement('div')
    preview.className = 'file-preview'
    preview.style.overflow = 'auto'
    let previewTop = 0
    Object.defineProperties(preview, {
      scrollTop: {
        configurable: true,
        get: () => previewTop,
        set: (v: number) => { previewTop = v },
      },
      scrollHeight: { configurable: true, value: 400 },
      clientHeight: { configurable: true, value: 400 },
      scrollWidth: { configurable: true, value: 400 },
      clientWidth: { configurable: true, value: 400 },
      getBoundingClientRect: {
        value: () => ({ top: 0, bottom: 400, height: 400, left: 0, right: 400, width: 400, x: 0, y: 0, toJSON() {} }),
      },
    })

    const pre = document.createElement('pre')
    pre.style.overflow = 'auto'
    let left = 0
    let top = 0
    Object.defineProperties(pre, {
      scrollTop: {
        configurable: true,
        get: () => top,
        set: (v: number) => { top = v },
      },
      scrollLeft: {
        configurable: true,
        get: () => left,
        set: (v: number) => { left = v },
      },
      scrollHeight: { configurable: true, value: 80 },
      clientHeight: { configurable: true, value: 80 },
      scrollWidth: { configurable: true, value: 2000 },
      clientWidth: { configurable: true, value: 200 },
      getBoundingClientRect: {
        value: () => ({ top: 40, bottom: 120, height: 80, left: 0, right: 200, width: 200, x: 0, y: 40, toJSON() {} }),
      },
    })

    const span = document.createElement('span')
    span.textContent = 'needle'
    pre.append(span)
    preview.append(pre)
    host.append(preview)
    document.body.append(host)

    const range = document.createRange()
    range.selectNodeContents(span)
    range.getBoundingClientRect = () => (
      { top: 50, bottom: 70, height: 20, left: 900, right: 980, width: 80, x: 900, y: 50, toJSON() {} }
    )

    revealPreviewFindRange(range)
    expect(left).toBeGreaterThan(0)
    expect(previewTop).toBe(0)
    host.remove()
  })

  it('pans the mermaid stage to an offscreen match without scrolling outer containers', () => {
    const host = document.createElement('div')
    host.className = 'file-preview-host'
    Object.defineProperty(host, 'scrollTop', { configurable: true, writable: true, value: 0 })

    const preview = document.createElement('div')
    preview.className = 'file-preview file-preview-mermaid'
    preview.style.overflow = 'hidden'

    const viewport = document.createElement('div')
    viewport.className = 'mmd-viewport'
    viewport.style.overflow = 'hidden'
    let viewTop = 0
    Object.defineProperties(viewport, {
      scrollTop: {
        configurable: true,
        get: () => viewTop,
        set: (v: number) => { viewTop = v },
      },
      getBoundingClientRect: {
        value: () => ({ top: 0, bottom: 400, height: 400, left: 0, right: 400, width: 400, x: 0, y: 0, toJSON() {} }),
      },
    })

    const stage = document.createElement('div')
    stage.className = 'mmd-stage'
    stage.style.transform = 'translate(-800px, -600px)'

    const label = document.createElement('span')
    label.textContent = 'needle'
    stage.append(label)
    viewport.append(stage)
    preview.append(viewport)
    host.append(preview)
    document.body.append(host)

    const range = document.createRange()
    range.selectNodeContents(label)
    range.getBoundingClientRect = () => (
      { top: -500, bottom: -480, height: 20, left: -700, right: -620, width: 80, x: -700, y: -500, toJSON() {} }
    )

    revealPreviewFindRange(range)
    expect(viewTop).toBe(0)
    expect(host.scrollTop).toBe(0)
    expect(stage.style.transform).toBe('translate(60px, 90px)')
    host.remove()
  })
})
