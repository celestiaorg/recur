/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushSync, mount, unmount, tick } from 'svelte'

const HAY = 'Hello world hello'

vi.mock('@xterm/xterm', () => {
  class Terminal {
    cols = 80
    rows = 24
    element = document.createElement('div')
    options: Record<string, unknown> = {}
    dimensions = { css: { cell: { width: 8, height: 17 } } }
    parser = { registerOscHandler() {} }
    constructor() { this.element.className = 'xterm' }
    loadAddon() {}
    open(host: HTMLElement) { host.append(this.element) }
    write() {}
    focus() {}
    clear() {}
    refresh() {}
    clearTextureAtlas() {}
    resize(cols: number, rows: number) { this.cols = cols; this.rows = rows }
    dispose() {}
    attachCustomKeyEventHandler() {}
    onData() { return { dispose() {} } }
    onTitleChange() { return { dispose() {} } }
    onResize() { return { dispose() {} } }
  }
  return { Terminal }
})

vi.mock('@xterm/addon-search', () => {
  class SearchAddon {
    #cb?: (e: { resultIndex: number; resultCount: number }) => void
    activate() {}
    dispose() {}
    onDidChangeResults(cb: (e: { resultIndex: number; resultCount: number }) => void) { this.#cb = cb }
    #cached?: string
    #last = { resultIndex: -1, resultCount: 0 }
    findNext(term: string, o?: { caseSensitive?: boolean }) { this.#emit(term, o); return !!term }
    findPrevious(term: string, o?: { caseSensitive?: boolean }) { this.#emit(term, o); return !!term }
    clearDecorations() { this.#cached = undefined }
    // Like the real addon, an unchanged term reuses the cached results.
    #emit(term: string, o?: { caseSensitive?: boolean }) {
      if (term && term === this.#cached) { this.#cb?.(this.#last); return }
      this.#cached = term || undefined
      const hay = o?.caseSensitive ? HAY : HAY.toLowerCase()
      const n = term ? hay.split(o?.caseSensitive ? term : term.toLowerCase()).length - 1 : 0
      this.#last = { resultIndex: n ? 0 : -1, resultCount: n }
      this.#cb?.(this.#last)
    }
  }
  return { SearchAddon }
})

vi.mock('../lib/api', () => ({
  call: async <T>(p: Promise<T>) => p,
  callOk: async (p: Promise<unknown>) => { await p; return true },
  bridgeErrorMessage: (e: unknown) => String(e),
}))

vi.mock('../../bindings/recur/internal/app/terminalservice', () => ({
  AckTerminal: vi.fn(async () => {}),
  AttachTerminal: vi.fn(async () => btoa('hello world hello\r\n')),
  ClearTerminal: vi.fn(async () => {}),
  CloseTerminal: vi.fn(async () => {}),
  DetachTerminal: vi.fn(async () => {}),
  InterruptTerminal: vi.fn(async () => {}),
  ListTerminalDockChrome: vi.fn(async () => []),
  ListTerminals: vi.fn(async () => [{ id: 'sess-1', title: 'zsh', exited: false, active: true }]),
  OpenTerminal: vi.fn(async () => ({ id: 'sess-1', title: 'zsh' })),
  ReorderTerminals: vi.fn(async () => {}),
  ResizeTerminal: vi.fn(async () => {}),
  RestoreWorkspaceTerminals: vi.fn(async () => []),
  SetActiveTerminal: vi.fn(async () => {}),
  SetTerminalDockChrome: vi.fn(async () => {}),
  SetTerminalTitle: vi.fn(async () => {}),
  WriteTerminal: vi.fn(async () => {}),
}))

import TerminalDock from './TerminalDock.svelte'

afterEach(() => {
  document.body.replaceChildren()
})

async function settle() {
  flushSync()
  await tick()
  flushSync()
}

async function waitFor(fn: () => boolean, tries = 40) {
  for (let i = 0; i < tries; i++) {
    await settle()
    if (fn()) return
    await new Promise((r) => setTimeout(r, 10))
  }
}

function cmdF(el: HTMLElement) {
  el.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'f', code: 'KeyF', metaKey: true, ctrlKey: true, bubbles: true, cancelable: true,
  }))
}

async function typeFind(root: HTMLElement, q: string) {
  const input = root.querySelector<HTMLInputElement>('.cm-recur-search-input')
  expect(input).toBeTruthy()
  input!.value = q
  input!.dispatchEvent(new Event('input', { bubbles: true }))
  await settle()
}

describe('TerminalDock find', () => {
  it('opens on ⌘F, counts matches, and closes on Escape', async () => {
    const target = document.createElement('div')
    target.style.width = '800px'
    target.style.height = '320px'
    document.body.append(target)
    const component = mount(TerminalDock, {
      target,
      props: { workspaceId: 'ws-1', gpu: false },
    })
    await waitFor(() => !!target.querySelector('.term-tab'))
    const dock = target.querySelector<HTMLElement>('.term-dock')
    expect(dock).toBeTruthy()
    await waitFor(() => !!target.querySelector('.xterm'))

    expect(target.querySelector('.cm-recur-search')).toBeNull()
    cmdF(dock!)
    await settle()
    expect(target.querySelector('.cm-recur-search')).toBeTruthy()

    await typeFind(target, 'hello')
    await waitFor(() => target.querySelector('.cm-recur-search-count')?.textContent === '1 / 2')
    expect(target.querySelector('.cm-recur-search-count')?.textContent).toBe('1 / 2')

    target.querySelector<HTMLButtonElement>('.cm-recur-search-toggle')!.click()
    await settle()
    expect(target.querySelector('.cm-recur-search-count')?.textContent).toBe('1 / 1')

    dock!.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'Escape', bubbles: true, cancelable: true,
    }))
    await settle()
    expect(target.querySelector('.cm-recur-search')).toBeNull()
    unmount(component)
  })
})
