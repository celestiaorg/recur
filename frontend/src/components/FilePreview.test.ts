/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushSync, mount, unmount, tick } from 'svelte'
import { fromStore, writable } from 'svelte/store'
import FilePreview from './FilePreview.svelte'
import { createPreviewScrollStore } from '../lib/preview-scroll'
import * as previewFind from '../lib/preview-find'

afterEach(() => {
  document.body.replaceChildren()
  vi.restoreAllMocks()
})

async function typeFind(target: HTMLElement, q: string) {
  const input = target.querySelector<HTMLInputElement>('.file-preview-find input')
  expect(input).toBeTruthy()
  input!.value = q
  input!.dispatchEvent(new Event('input', { bubbles: true }))
  flushSync()
  await tick()
  flushSync()
}

describe('FilePreview find', () => {
  it('opens a find bar and counts markdown matches', async () => {
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(FilePreview, {
      target,
      props: {
        kind: 'markdown',
        content: 'Hello world. Hello again.',
        path: 'README.md',
        scroll: createPreviewScrollStore(),
      },
    })
    flushSync()
    expect(target.querySelector('.file-preview-find')).toBeNull()
    component.toggleSearch()
    flushSync()
    await typeFind(target, 'hello')
    expect(target.querySelector('.cm-recur-search-count')?.textContent).toBe('1 / 2')
    const toggles = target.querySelectorAll('.file-preview-find .cm-recur-search-toggle')
    expect(toggles).toHaveLength(3)
    ;(toggles[1] as HTMLButtonElement).click()
    flushSync()
    await tick()
    flushSync()
    await typeFind(target, 'H.llo')
    expect(target.querySelector('.cm-recur-search-count')?.textContent).toBe('1 / 2')
    unmount(component)
  })

  it('opens on ⌘F and closes on Escape', async () => {
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(FilePreview, {
      target,
      props: {
        kind: 'markdown',
        content: 'needle in a haystack',
        path: 'notes.md',
        scroll: createPreviewScrollStore(),
      },
    })
    flushSync()
    window.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'f', code: 'KeyF', metaKey: true, ctrlKey: true, bubbles: true, cancelable: true,
    }))
    flushSync()
    await tick()
    expect(target.querySelector('.file-preview-find')).toBeTruthy()
    await typeFind(target, 'needle')
    expect(target.querySelector('.cm-recur-search-count')?.textContent).toBe('1 / 1')
    window.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'Escape', bubbles: true, cancelable: true,
    }))
    flushSync()
    expect(target.querySelector('.file-preview-find')).toBeNull()
    unmount(component)
  })

  it('does not intercept ⌘F when inactive or when a CodeMirror editor owns the event', async () => {
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(FilePreview, {
      target,
      props: {
        kind: 'markdown',
        content: 'needle',
        path: 'notes.md',
        scroll: createPreviewScrollStore(),
        active: false,
      },
    })
    flushSync()
    window.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'f', code: 'KeyF', metaKey: true, ctrlKey: true, bubbles: true, cancelable: true,
    }))
    flushSync()
    expect(target.querySelector('.file-preview-find')).toBeNull()
    unmount(component)

    const target2 = document.createElement('div')
    document.body.append(target2)
    const component2 = mount(FilePreview, {
      target: target2,
      props: {
        kind: 'markdown',
        content: 'needle',
        path: 'notes.md',
        scroll: createPreviewScrollStore(),
      },
    })
    flushSync()
    const cm = document.createElement('div')
    cm.className = 'cm-editor'
    const inner = document.createElement('textarea')
    cm.append(inner)
    document.body.append(cm)
    inner.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'f', code: 'KeyF', metaKey: true, ctrlKey: true, bubbles: true, cancelable: true,
    }))
    flushSync()
    expect(target2.querySelector('.file-preview-find')).toBeNull()
    cm.remove()
    unmount(component2)
  })

  it('does not open Find for SVG previews', async () => {
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(FilePreview, {
      target,
      props: {
        kind: 'svg',
        content: '<svg xmlns="http://www.w3.org/2000/svg"><text>needle</text></svg>',
        path: 'icon.svg',
        scroll: createPreviewScrollStore(),
      },
    })
    flushSync()
    expect(component.toggleSearch()).toBe(false)
    flushSync()
    expect(target.querySelector('.file-preview-find')).toBeNull()
    window.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'f', code: 'KeyF', metaKey: true, ctrlKey: true, bubbles: true, cancelable: true,
    }))
    flushSync()
    expect(target.querySelector('.file-preview-find')).toBeNull()
    unmount(component)
  })

  it('recollects matches after the preview DOM is replaced', async () => {
    const target = document.createElement('div')
    document.body.append(target)
    const component = mount(FilePreview, {
      target,
      props: {
        kind: 'markdown',
        content: 'nothing yet',
        path: 'notes.md',
        scroll: createPreviewScrollStore(),
      },
    })
    flushSync()
    component.toggleSearch()
    flushSync()
    await typeFind(target, 'hello')
    expect(target.querySelector('.cm-recur-search-count')?.textContent).toBe('No results')
    const md = target.querySelector('.file-preview-md')
    expect(md).toBeTruthy()
    const p = document.createElement('p')
    p.textContent = 'hello'
    md!.append(p)
    await Promise.resolve()
    flushSync()
    expect(target.querySelector('.cm-recur-search-count')?.textContent).toBe('1 / 1')
    unmount(component)
  })

  it('restores highlights for the active preview and ignores inactive paints', async () => {
    const applied: string[][] = []
    vi.spyOn(previewFind, 'applyPreviewFindHighlights').mockImplementation((ranges) => {
      applied.push(ranges.map((r) => r.toString()))
    })
    vi.spyOn(previewFind, 'clearPreviewFindHighlights').mockImplementation(() => {})
    const settle = async () => { flushSync(); await tick(); flushSync() }

    const aOn = fromStore(writable(true))
    const bOn = fromStore(writable(false))
    const a = document.createElement('div')
    const b = document.createElement('div')
    document.body.append(a, b)
    const componentA = mount(FilePreview, {
      target: a,
      props: {
        kind: 'markdown',
        content: 'alpha needle',
        path: 'a.md',
        scroll: createPreviewScrollStore(),
        get active() { return aOn.current },
      },
    })
    const componentB = mount(FilePreview, {
      target: b,
      props: {
        kind: 'markdown',
        content: 'beta needle',
        path: 'b.md',
        scroll: createPreviewScrollStore(),
        get active() { return bOn.current },
      },
    })
    try {
      flushSync()
      componentA.toggleSearch()
      flushSync()
      await typeFind(a, 'needle')
      expect(applied.at(-1)).toEqual(['needle'])

      aOn.current = false
      bOn.current = true
      await settle()
      componentB.toggleSearch()
      flushSync()
      await typeFind(b, 'beta')
      expect(applied.at(-1)).toEqual(['beta'])

      bOn.current = false
      aOn.current = true
      await settle()
      expect(applied.at(-1)).toEqual(['needle'])
    } finally {
      unmount(componentA)
      unmount(componentB)
    }
  })
})
