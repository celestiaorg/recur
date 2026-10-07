/** @vitest-environment happy-dom */
import { describe, expect, it, vi } from 'vitest'
import { EditorState, Text } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import {
  applyFileViewState,
  captureFileViewState,
  focusPreservingScroll,
  parseFileViewState,
  parseFileViews,
  peekHotShellEditorScrollTop,
  posAtLineCol,
  pruneFileViews,
  relocatePreservingOverflow,
  replaceDocKeepingScroll,
  resolveRestoredViewMode,
  restoreHotShellGeometry,
  scheduleApplyFileViewState,
  selectionFromViewState,
  stashHotShellGeometry,
} from './editor-view-state'

function mount(doc = 'alpha\nbeta\ngamma\ndelta') {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({ doc }),
  })
  Object.defineProperty(view.scrollDOM, 'clientHeight', { configurable: true, value: 400 })
  return { view, parent }
}

describe('posAtLineCol', () => {
  const doc = Text.of(['alpha', 'beta', 'gamma'])

  it('maps 1-based line/col onto a document offset', () => {
    expect(posAtLineCol(doc, 1, 1)).toBe(0)
    expect(posAtLineCol(doc, 2, 1)).toBe(6)
    expect(posAtLineCol(doc, 2, 3)).toBe(8)
  })

  it('clamps past the last line and past the line length', () => {
    expect(posAtLineCol(doc, 99, 1)).toBe(doc.line(3).from)
    expect(posAtLineCol(doc, 1, 99)).toBe(doc.line(1).to)
    expect(posAtLineCol(doc, 0, 0)).toBe(0)
  })
})

describe('selectionFromViewState', () => {
  const doc = Text.of(['alpha', 'beta', 'gamma'])

  it('builds a collapsed caret from line/col', () => {
    const sel = selectionFromViewState(doc, {
      viewMode: 'code', line: 2, col: 3, scrollTop: 0, scrollLeft: 0,
    })
    expect(sel.main.empty).toBe(true)
    expect(sel.main.head).toBe(8)
  })

  it('builds a range from optional anchor fields', () => {
    const sel = selectionFromViewState(doc, {
      viewMode: 'code',
      line: 3,
      col: 2,
      anchorLine: 1,
      anchorCol: 1,
      scrollTop: 0,
      scrollLeft: 0,
    })
    expect(sel.main.empty).toBe(false)
    expect(sel.main.from).toBe(0)
    expect(sel.main.to).toBe(doc.line(3).from + 1)
  })
})

describe('capture/apply FileViewState', () => {
  it('round-trips a mid-document caret', () => {
    const { view, parent } = mount()
    applyFileViewState(view, {
      viewMode: 'code',
      line: 3,
      col: 2,
      scrollTop: 0,
      scrollLeft: 0,
    })
    const cap = captureFileViewState(view, 'preview')
    expect(cap).not.toBeNull()
    expect(cap!.viewMode).toBe('preview')
    expect(cap!.line).toBe(3)
    expect(cap!.col).toBe(2)
    expect(cap!.anchorLine).toBeUndefined()
    expect(cap!.topLine).toBeGreaterThanOrEqual(1)
    view.destroy()
    parent.remove()
  })

  it('round-trips a non-empty selection', () => {
    const { view, parent } = mount()
    applyFileViewState(view, {
      viewMode: 'code',
      line: 3,
      col: 3,
      anchorLine: 2,
      anchorCol: 1,
      scrollTop: 0,
      scrollLeft: 0,
    })
    const cap = captureFileViewState(view, 'code')
    expect(cap).not.toBeNull()
    expect(cap!.line).toBe(3)
    expect(cap!.col).toBe(3)
    expect(cap!.anchorLine).toBe(2)
    expect(cap!.anchorCol).toBe(1)
    view.destroy()
    parent.remove()
  })

  it('applies captured horizontal scroll', () => {
    const { view, parent } = mount()
    applyFileViewState(view, {
      viewMode: 'code',
      line: 1,
      col: 1,
      scrollTop: 0,
      scrollLeft: 160,
    })
    expect(view.scrollDOM.scrollLeft).toBe(160)
    view.destroy()
    parent.remove()
  })

  it('keeps horizontal scroll after CodeMirror processes the queued target', async () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    let handled = 0
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'x'.repeat(400),
        extensions: [
          EditorView.scrollHandler.of((v) => {
            handled++
            v.scrollDOM.scrollLeft = 1
            return true
          }),
        ],
      }),
    })
    Object.defineProperty(view.scrollDOM, 'clientHeight', { configurable: true, value: 400 })
    Object.defineProperty(view.scrollDOM, 'clientWidth', { configurable: true, value: 400 })
    applyFileViewState(view, {
      viewMode: 'code',
      line: 1,
      col: 1,
      scrollTop: 0,
      scrollLeft: 180,
    })
    expect(view.scrollDOM.scrollLeft).toBe(180)
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(handled).toBeGreaterThan(0)
    expect(view.scrollDOM.scrollLeft).toBe(180)
    view.destroy()
    parent.remove()
  })

  it('keeps horizontal scroll after the deferred scheduleApplyFileViewState pass', async () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    let handled = 0
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'x'.repeat(400),
        extensions: [
          EditorView.scrollHandler.of((v) => {
            handled++
            v.scrollDOM.scrollLeft = 1
            return true
          }),
        ],
      }),
    })
    Object.defineProperty(view.scrollDOM, 'clientHeight', { configurable: true, value: 400 })
    Object.defineProperty(view.scrollDOM, 'clientWidth', { configurable: true, value: 400 })
    scheduleApplyFileViewState(view, {
      viewMode: 'code',
      line: 1,
      col: 1,
      scrollTop: 0,
      scrollLeft: 180,
    }, () => true)
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          requestAnimationFrame(() => resolve())
        })
      })
    })
    expect(handled).toBeGreaterThan(0)
    expect(view.scrollDOM.scrollLeft).toBe(180)
    view.destroy()
    parent.remove()
  })

  it('skips capture when the scroller is not laid out', () => {
    const { view, parent } = mount()
    Object.defineProperty(view.scrollDOM, 'clientHeight', { configurable: true, value: 0 })
    expect(captureFileViewState(view, 'code')).toBeNull()
    view.destroy()
    parent.remove()
  })
})

describe('replaceDocKeepingScroll', () => {
  it('replaces a shorter document without throwing on a stale scroll snapshot', () => {
    const { view, parent } = mount('x'.repeat(1405))
    replaceDocKeepingScroll(view, 'y'.repeat(1282), EditorView.scrollIntoView(1405))
    expect(view.state.doc.toString()).toBe('y'.repeat(1282))
    view.destroy()
    parent.remove()
  })

  it('keeps a mid-file scroll anchor instead of mapping it through a full-doc delete', () => {
    const { view, parent } = mount('x'.repeat(1405))
    const heads: number[] = []
    const inner = view.dispatch.bind(view)
    vi.spyOn(view, 'dispatch').mockImplementation((spec) => {
      if (spec && typeof spec === 'object' && !Array.isArray(spec) && 'effects' in spec) {
        const raw = spec.effects
        for (const e of Array.isArray(raw) ? raw : raw ? [raw] : []) {
          const head = (e as { value?: { range?: { head: number } } })?.value?.range?.head
          if (typeof head === 'number') heads.push(head)
        }
      }
      return inner(spec)
    })
    replaceDocKeepingScroll(view, 'y'.repeat(1282), EditorView.scrollIntoView(700))
    expect(view.state.doc.toString()).toBe('y'.repeat(1282))
    expect(heads).toContain(700)
    expect(heads).not.toContain(0)
    view.destroy()
    parent.remove()
  })

  it('recovers when CodeMirror applied the shorter doc then threw', () => {
    const { view, parent } = mount('x'.repeat(1405))
    expect(() => {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: 'y'.repeat(1282) },
        effects: EditorView.scrollIntoView(1405),
      })
    }).toThrow(/Invalid position 1405 in document of length 1282/)
    expect(view.state.doc.length).toBe(1282)
    replaceDocKeepingScroll(view, 'y'.repeat(1282), EditorView.scrollIntoView(1405))
    expect(view.state.doc.toString()).toBe('y'.repeat(1282))
    expect(view.contentDOM.textContent?.replace(/\n/g, '')).toBe('y'.repeat(1282))
    expect(() => view.dispatch({ changes: { from: 0, insert: 'z' } })).not.toThrow()
    expect(view.state.doc.sliceString(0, 1)).toBe('z')
    view.destroy()
    parent.remove()
  })
})

describe('focusPreservingScroll', () => {
  it('focuses without throwing on a mounted view', () => {
    const { view, parent } = mount()
    focusPreservingScroll(view)
    view.destroy()
    parent.remove()
  })

  it('re-dispatches a parked scroll even when not focusing', async () => {
    const { view, parent } = mount()
    const focus = vi.spyOn(view, 'focus')
    const dispatch = vi.spyOn(view, 'dispatch')
    const snap = view.scrollSnapshot()
    focusPreservingScroll(view, snap, undefined, { focus: false })
    expect(focus).not.toHaveBeenCalled()
    expect(dispatch).toHaveBeenCalledTimes(1)
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(dispatch).toHaveBeenCalledTimes(2)
    view.destroy()
    parent.remove()
  })

  it('still suppresses the deferred dispatch when still() is false without focusing', async () => {
    const { view, parent } = mount()
    const dispatch = vi.spyOn(view, 'dispatch')
    const snap = view.scrollSnapshot()
    let still = true
    focusPreservingScroll(view, snap, () => still, { focus: false })
    expect(dispatch).toHaveBeenCalledTimes(1)
    still = false
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(dispatch).toHaveBeenCalledTimes(1)
    view.destroy()
    parent.remove()
  })

  it('does not re-dispatch a scroll snapshot onto a swapped document', async () => {
    const { view, parent } = mount()
    let still = true
    focusPreservingScroll(view, undefined, () => still)
    still = false
    view.setState(EditorState.create({ doc: 'other\nfile' }))
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(view.state.doc.toString()).toBe('other\nfile')
    view.destroy()
    parent.remove()
  })
})

describe('parseFileViewState', () => {
  it('keeps optional previewScrollTop', () => {
    expect(parseFileViewState({
      line: 12,
      col: 4,
      scrollTop: 80,
      previewScrollTop: 240,
      previewScrollLeft: 16,
      previewZoom: 1.25,
    })).toEqual({
      viewMode: 'code',
      line: 12,
      col: 4,
      scrollTop: 80,
      scrollLeft: 0,
      previewScrollTop: 240,
      previewScrollLeft: 16,
      previewZoom: 1.25,
    })
  })

  it('keeps signed mermaid camera offsets', () => {
    expect(parseFileViewState({
      line: 1,
      col: 1,
      scrollTop: 0,
      previewScrollTop: -120,
      previewScrollLeft: -80,
      previewZoom: 2,
    })).toEqual({
      viewMode: 'code',
      line: 1,
      col: 1,
      scrollTop: 0,
      scrollLeft: 0,
      previewScrollTop: -120,
      previewScrollLeft: -80,
      previewZoom: 2,
    })
  })

  it('accepts a record without scrollTop and keeps optional topLine', () => {
    expect(parseFileViewState({
      line: 40,
      col: 1,
      topLine: 38,
    })).toEqual({
      viewMode: 'code',
      line: 40,
      col: 1,
      topLine: 38,
      scrollTop: 0,
      scrollLeft: 0,
    })
  })

  it('rejects garbage', () => {
    expect(parseFileViewState(null)).toBeNull()
    expect(parseFileViewState('nope')).toBeNull()
    expect(parseFileViewState({ line: 0, col: 1, scrollTop: 0 })).toBeNull()
    expect(parseFileViewState({ line: 1, col: 1, scrollTop: -4 })).toBeNull()
  })
})

describe('parseFileViews / pruneFileViews', () => {
  it('drops unknown paths and malformed entries', () => {
    const parsed = parseFileViews({
      'a.ts': { line: 2, col: 1, scrollTop: 10, viewMode: 'preview' },
      'bad.ts': { line: 'x' },
      '': { line: 1, col: 1, scrollTop: 0 },
    })
    expect(Object.keys(parsed)).toEqual(['a.ts'])
    expect(parsed['a.ts']?.viewMode).toBe('preview')
    expect(pruneFileViews(parsed, ['missing.ts', 'a.ts'])).toEqual({
      'a.ts': parsed['a.ts'],
    })
  })
})

describe('resolveRestoredViewMode', () => {
  it('honours an explicit diff-first open when the file has a status', () => {
    expect(resolveRestoredViewMode('preview', { wantDiff: true, canDiff: true })).toBe('diff')
    expect(resolveRestoredViewMode('preview', { wantDiff: true, canDiff: false })).toBe('code')
  })

  it('restores saved preview/diff only when that surface still exists', () => {
    expect(resolveRestoredViewMode('preview', { canPreview: true })).toBe('preview')
    expect(resolveRestoredViewMode('preview', { canPreview: false })).toBe('code')
    expect(resolveRestoredViewMode('diff', { canDiff: true })).toBe('diff')
    expect(resolveRestoredViewMode('diff', { canDiff: false })).toBe('code')
  })

  it('forces Diff for a deleted-on-disk file', () => {
    expect(resolveRestoredViewMode('code', { deleted: true })).toBe('diff')
  })

  it('binary images restore Preview (on-disk) or Diff (compare), never Code', () => {
    expect(resolveRestoredViewMode('code', { isImage: true })).toBe('preview')
    expect(resolveRestoredViewMode('preview', { isImage: true, wantDiff: true, canDiff: true })).toBe('diff')
    expect(resolveRestoredViewMode('diff', { isImage: true, canDiff: true })).toBe('diff')
    expect(resolveRestoredViewMode('diff', { isImage: true, canDiff: false })).toBe('preview')
    expect(resolveRestoredViewMode('preview', { isImage: true, deleted: true, canDiff: true })).toBe('diff')
  })
})

describe('stashHotShellGeometry', () => {
  it('keeps tab-strip overflow and a CodeMirror scroll snapshot', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `
      <div class="obs-tb-center"><div class="tab-strip-scroll"></div></div>
      <div class="obs-editor-pane"><div class="obs-surface"></div></div>
    `
    document.body.appendChild(shell)
    const strip = shell.querySelector<HTMLElement>('.tab-strip-scroll')!
    Object.defineProperty(strip, 'scrollLeft', { configurable: true, writable: true, value: 120 })
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 200 })
    const surface = shell.querySelector<HTMLElement>('.obs-surface')!
    const view = new EditorView({
      parent: surface,
      state: EditorState.create({ doc: 'alpha\nbeta\ngamma\n'.repeat(40) }),
    })
    Object.defineProperty(view.scrollDOM, 'clientHeight', { configurable: true, value: 400 })
    Object.defineProperty(view.scrollDOM, 'scrollTop', { configurable: true, writable: true, value: 80 })
    stashHotShellGeometry(shell)
    strip.scrollLeft = 0
    Object.defineProperty(view.scrollDOM, 'scrollTop', { configurable: true, writable: true, value: 0 })
    restoreHotShellGeometry(shell)
    expect(strip.scrollLeft).toBe(120)
    expect(peekHotShellEditorScrollTop(shell)).toBe(80)
    expect(view.scrollDOM.scrollTop).toBe(80)
    view.destroy()
    shell.remove()
  })

  it('skips editor scroll when a jump is activating the shell', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `
      <div class="obs-tb-center"><div class="tab-strip-scroll"></div></div>
      <div class="obs-editor-pane"><div class="obs-surface"></div></div>
    `
    document.body.appendChild(shell)
    const strip = shell.querySelector<HTMLElement>('.tab-strip-scroll')!
    Object.defineProperty(strip, 'scrollLeft', { configurable: true, writable: true, value: 120 })
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 200 })
    const surface = shell.querySelector<HTMLElement>('.obs-surface')!
    const view = new EditorView({
      parent: surface,
      state: EditorState.create({ doc: 'alpha\nbeta\ngamma\n'.repeat(40) }),
    })
    Object.defineProperty(view.scrollDOM, 'clientHeight', { configurable: true, value: 400 })
    Object.defineProperty(view.scrollDOM, 'scrollTop', { configurable: true, writable: true, value: 80 })
    stashHotShellGeometry(shell)
    strip.scrollLeft = 0
    Object.defineProperty(view.scrollDOM, 'scrollTop', { configurable: true, writable: true, value: 0 })
    restoreHotShellGeometry(shell, { skipEditor: true })
    expect(strip.scrollLeft).toBe(120)
    expect(view.scrollDOM.scrollTop).toBe(0)
    view.destroy()
    shell.remove()
  })

  it('keeps file-preview overflow', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `
      <div class="obs-editor-pane"><div class="obs-surface"><div class="file-preview"></div></div></div>
    `
    document.body.appendChild(shell)
    const preview = shell.querySelector<HTMLElement>('.file-preview')!
    Object.defineProperties(preview, {
      scrollTop: { configurable: true, writable: true, value: 420 },
      scrollLeft: { configurable: true, writable: true, value: 16 },
      clientHeight: { configurable: true, value: 400 },
    })
    stashHotShellGeometry(shell)
    preview.scrollTop = 0
    preview.scrollLeft = 0
    restoreHotShellGeometry(shell)
    expect(preview.scrollTop).toBe(420)
    expect(preview.scrollLeft).toBe(16)
    shell.remove()
  })

  it('keeps file-preview overflow when Code or Diff hides the surface', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `
      <div class="obs-editor-pane"><div class="obs-surface off"><div class="file-preview"></div></div></div>
    `
    document.body.appendChild(shell)
    const preview = shell.querySelector<HTMLElement>('.file-preview')!
    Object.defineProperties(preview, {
      scrollTop: { configurable: true, writable: true, value: 420 },
      scrollLeft: { configurable: true, writable: true, value: 16 },
      clientHeight: { configurable: true, value: 400 },
    })
    stashHotShellGeometry(shell)
    preview.scrollTop = 0
    preview.scrollLeft = 0
    restoreHotShellGeometry(shell)
    expect(preview.scrollTop).toBe(420)
    expect(preview.scrollLeft).toBe(16)
    shell.remove()
  })

  it('restores html preview iframe offset from data attributes', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `
      <div class="obs-editor-pane"><div class="obs-surface">
        <div class="file-preview" data-preview-top="90" data-preview-left="4"><iframe></iframe></div>
      </div></div>
    `
    document.body.appendChild(shell)
    const preview = shell.querySelector<HTMLElement>('.file-preview')!
    const iframe = preview.querySelector('iframe')!
    Object.defineProperty(preview, 'clientHeight', { configurable: true, value: 400 })
    const posts: unknown[] = []
    const cw = iframe.contentWindow
    if (cw) cw.postMessage = ((data: unknown) => { posts.push(data) }) as typeof cw.postMessage
    stashHotShellGeometry(shell)
    preview.dataset.previewTop = '0'
    preview.dataset.previewLeft = '0'
    restoreHotShellGeometry(shell, { defer: false })
    expect(posts).toContainEqual({ type: 'recur-preview-restore', top: 90, left: 4 })
    shell.remove()
  })

  it('keeps agent-chat overflow', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `<div class="chat-scroll"></div>`
    document.body.appendChild(shell)
    const chat = shell.querySelector<HTMLElement>('.chat-scroll')!
    Object.defineProperties(chat, {
      scrollTop: { configurable: true, writable: true, value: 240 },
      scrollHeight: { configurable: true, value: 800 },
      clientHeight: { configurable: true, value: 200 },
    })
    stashHotShellGeometry(shell)
    chat.scrollTop = 0
    restoreHotShellGeometry(shell)
    expect(chat.scrollTop).toBe(240)
    shell.remove()
  })

  it('keeps agent-chat tab-strip overflow', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `<div class="wsv-chat-tabs"><div class="tab-strip-scroll"></div></div>`
    document.body.appendChild(shell)
    const strip = shell.querySelector<HTMLElement>('.tab-strip-scroll')!
    Object.defineProperty(strip, 'scrollLeft', { configurable: true, writable: true, value: 88 })
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 200 })
    stashHotShellGeometry(shell)
    strip.scrollLeft = 0
    restoreHotShellGeometry(shell)
    expect(strip.scrollLeft).toBe(88)
    shell.remove()
  })

  it('restores overflow after the dock is reparented', () => {
    const home = document.createElement('div')
    const host = document.createElement('div')
    document.body.append(home, host)
    const shell = document.createElement('div')
    shell.innerHTML = `
      <div class="wsv-chat-tabs"><div class="tab-strip-scroll"></div></div>
      <div class="chat-scroll"></div>
    `
    home.appendChild(shell)
    const strip = shell.querySelector<HTMLElement>('.tab-strip-scroll')!
    const chat = shell.querySelector<HTMLElement>('.chat-scroll')!
    Object.defineProperty(strip, 'scrollLeft', { configurable: true, writable: true, value: 88 })
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 200 })
    Object.defineProperties(chat, {
      scrollTop: { configurable: true, writable: true, value: 240 },
      scrollHeight: { configurable: true, value: 800 },
      clientHeight: { configurable: true, value: 200 },
    })
    relocatePreservingOverflow(shell, (n) => {
      host.appendChild(n)
      strip.scrollLeft = 0
      chat.scrollTop = 0
    })
    expect(shell.parentElement).toBe(host)
    expect(strip.scrollLeft).toBe(88)
    expect(strip.style.overflow).toBe('')
    expect(chat.scrollTop).toBe(240)
    home.remove()
    host.remove()
  })

  it('does not stash a fabricated 0 from a display:none strip', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `<div class="wsv-chat-tabs"><div class="tab-strip-scroll"></div></div>`
    document.body.appendChild(shell)
    const strip = shell.querySelector<HTMLElement>('.tab-strip-scroll')!
    Object.defineProperty(strip, 'scrollLeft', { configurable: true, writable: true, value: 88 })
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 200 })
    stashHotShellGeometry(shell)
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 0 })
    strip.scrollLeft = 0
    stashHotShellGeometry(shell)
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 200 })
    restoreHotShellGeometry(shell, { defer: false })
    expect(strip.scrollLeft).toBe(88)
    shell.remove()
  })

  it('pins a bottom-stuck chat to the grown tail, not the old pixel', () => {
    const shell = document.createElement('div')
    shell.innerHTML = `<div class="chat-scroll"></div>`
    document.body.appendChild(shell)
    const chat = shell.querySelector<HTMLElement>('.chat-scroll')!
    const box = {
      scrollTop: 777,
      scrollHeight: 1000,
      clientHeight: 200,
    }
    Object.defineProperties(chat, {
      scrollTop: { configurable: true, get: () => box.scrollTop, set: (v: number) => { box.scrollTop = v } },
      scrollHeight: { configurable: true, get: () => box.scrollHeight },
      clientHeight: { configurable: true, get: () => box.clientHeight },
    })
    stashHotShellGeometry(shell)
    box.scrollHeight = 1400
    box.scrollTop = 0
    restoreHotShellGeometry(shell)
    expect(chat.scrollTop).toBe(1400)
    shell.remove()
  })
})
