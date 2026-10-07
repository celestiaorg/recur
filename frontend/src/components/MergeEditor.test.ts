/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushSync, mount, unmount } from 'svelte'
import { fromStore, writable } from 'svelte/store'
import { EditorView } from '@codemirror/view'
import { openSearchPanel, getSearchQuery, searchPanelOpen } from '@codemirror/search'
import MergeEditor from './MergeEditor.svelte'

vi.mock('../lib/snippets', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../lib/snippets')>()
  return { ...orig, prefetchSnippets: vi.fn() }
})
vi.mock('../lib/cm-lang', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../lib/cm-lang')>()
  return { ...orig, loadLanguageForPath: () => null }
})

function docWithHunk(mid: string): string {
  const preamble = Array.from({ length: 20 }, (_, i) => `const a${i} = ${i}`)
  const tail = Array.from({ length: 20 }, (_, i) => `const b${i} = ${i}`)
  return ['package main', '', ...preamble, `func main() { println("${mid}") }`, ...tail, ''].join('\n')
}

const original = docWithHunk('old')

function cmView(target: HTMLElement): EditorView {
  const view = EditorView.findFromDOM(target.querySelector('.cm-editor')!)
  if (!view) throw new Error('no EditorView')
  return view
}

function frames(n = 2): Promise<void> {
  return new Promise((resolve) => {
    const step = (left: number) => {
      if (left <= 0) resolve()
      else requestAnimationFrame(() => step(left - 1))
    }
    step(n)
  })
}

describe('MergeEditor collapseUnchanged', () => {
  afterEach(() => {
    document.body.replaceChildren()
  })

  it('recollapses unchanged lines after an external doc swap', () => {
    const doc = fromStore(writable(docWithHunk('new')))
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(MergeEditor, {
      target,
      props: {
        get doc() { return doc.current },
        original,
        path: 'src/main.go',
      },
    })
    flushSync()
    expect(target.querySelector('.cm-collapsedLines')).toBeTruthy()
    expect(target.textContent).toContain('println("new")')
    expect([...target.querySelectorAll('.cm-line')].some((el) => el.textContent === 'const a10 = 10')).toBe(false)

    doc.current = docWithHunk('newer')
    flushSync()
    expect(target.textContent).toContain('println("newer")')
    expect(target.querySelector('.cm-collapsedLines')).toBeTruthy()
    expect([...target.querySelectorAll('.cm-line')].some((el) => el.textContent === 'const a10 = 10')).toBe(false)

    unmount(component)
  })

  it('keeps find/replace query and panel across a live remount', () => {
    const doc = fromStore(writable(docWithHunk('new')))
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(MergeEditor, {
      target,
      props: {
        get doc() { return doc.current },
        original,
        path: 'src/main.go',
      },
    })
    flushSync()
    const view = cmView(target)
    openSearchPanel(view)
    const input = target.querySelector('[name=search]') as HTMLInputElement
    const replace = target.querySelector('[name=replace]') as HTMLInputElement
    const caseBtn = target.querySelector('.cm-recur-search-toggle') as HTMLButtonElement
    input.value = 'println'
    input.dispatchEvent(new Event('input'))
    replace.value = 'print'
    replace.dispatchEvent(new Event('input'))
    caseBtn.click()
    expect(searchPanelOpen(view.state)).toBe(true)

    doc.current = docWithHunk('newer')
    flushSync()
    const next = cmView(target)
    expect(next).not.toBe(view)
    expect(searchPanelOpen(next.state)).toBe(true)
    const q = getSearchQuery(next.state)
    expect(q.search).toBe('println')
    expect(q.replace).toBe('print')
    expect(q.caseSensitive).toBe(true)
    expect((target.querySelector('[name=search]') as HTMLInputElement).value).toBe('println')

    unmount(component)
  })

  it('restores focused replace input after a live remount', () => {
    const doc = fromStore(writable(docWithHunk('new')))
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(MergeEditor, {
      target,
      props: {
        get doc() { return doc.current },
        original,
        path: 'src/main.go',
      },
    })
    flushSync()
    openSearchPanel(cmView(target))
    const replace = target.querySelector('[name=replace]') as HTMLInputElement
    replace.value = 'print'
    replace.dispatchEvent(new Event('input'))
    replace.focus()
    replace.setSelectionRange(1, 3)
    expect(document.activeElement).toBe(replace)

    doc.current = docWithHunk('newer')
    flushSync()
    const next = target.querySelector('[name=replace]') as HTMLInputElement
    expect(next).not.toBe(replace)
    expect(document.activeElement).toBe(next)
    expect(next.selectionStart).toBe(1)
    expect(next.selectionEnd).toBe(3)

    unmount(component)
  })

  it('restores editor focus after a live remount and does not steal it', () => {
    const doc = fromStore(writable(docWithHunk('new')))
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(MergeEditor, {
      target,
      props: {
        get doc() { return doc.current },
        original,
        path: 'src/main.go',
      },
    })
    flushSync()
    vi.spyOn(document, 'hasFocus').mockReturnValue(true)
    cmView(target).focus()
    expect(cmView(target).hasFocus).toBe(true)

    doc.current = docWithHunk('newer')
    flushSync()
    expect(cmView(target).hasFocus).toBe(true)

    const outside = document.createElement('textarea')
    document.body.append(outside)
    outside.focus()
    expect(cmView(target).hasFocus).toBe(false)

    doc.current = docWithHunk('newest')
    flushSync()
    expect(document.activeElement).toBe(outside)
    expect(cmView(target).hasFocus).toBe(false)

    unmount(component)
  })

  it('restores horizontal scroll after a live remount', async () => {
    const doc = fromStore(writable(docWithHunk('new')))
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(MergeEditor, {
      target,
      props: {
        get doc() { return doc.current },
        original,
        path: 'src/main.go',
      },
    })
    flushSync()
    const scroller = cmView(target).scrollDOM
    Object.defineProperty(scroller, 'clientHeight', { configurable: true, value: 400 })
    Object.defineProperty(scroller, 'clientWidth', { configurable: true, value: 400 })
    Object.defineProperty(scroller, 'scrollWidth', { configurable: true, value: 2400 })
    scroller.scrollLeft = 180

    doc.current = docWithHunk('newer')
    flushSync()
    await frames()
    expect(cmView(target).scrollDOM.scrollLeft).toBe(180)

    unmount(component)
  })
})
