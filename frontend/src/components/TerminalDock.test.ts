/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushSync, mount, unmount, tick } from 'svelte'

const HAY = 'Hello world hello'

const { runtimeMock, terminals } = vi.hoisted(() => ({
  runtimeMock: {
    System: { IsMac: vi.fn(() => true) },
    Events: { On: vi.fn(() => () => {}), Emit: vi.fn(async () => {}) },
    Clipboard: { SetText: vi.fn(async () => {}), Text: vi.fn(async () => 'pasted') },
  },
  terminals: [] as { paste: ReturnType<typeof vi.fn> }[],
}))

vi.mock('@wailsio/runtime', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@wailsio/runtime')>()),
  ...runtimeMock,
}))

vi.mock('@xterm/xterm', () => {
  class Terminal {
    cols = 80
    rows = 24
    element = document.createElement('div')
    options: Record<string, unknown> = {}
    dimensions = { css: { cell: { width: 8, height: 17 } } }
    parser = { registerOscHandler() {} }
    paste = vi.fn()
    constructor() { this.element.className = 'xterm'; terminals.push(this) }
    getSelection() { return 'selected text' }
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
import { OpenTerminal } from '../../bindings/recur/internal/app/terminalservice'

afterEach(() => {
  document.body.replaceChildren()
  runtimeMock.System.IsMac.mockReturnValue(true)
  vi.clearAllMocks()
})

function press(el: HTMLElement, init: KeyboardEventInit): KeyboardEvent {
  const e = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init })
  el.dispatchEvent(e)
  return e
}

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

describe('TerminalDock chords off macOS', () => {
  async function mountDock() {
    const target = document.createElement('div')
    target.style.width = '800px'
    target.style.height = '320px'
    document.body.append(target)
    const component = mount(TerminalDock, {
      target,
      props: { workspaceId: 'ws-1', gpu: false },
    })
    await waitFor(() => !!target.querySelector('.term-tab'))
    await waitFor(() => !!target.querySelector('.xterm'))
    const dock = target.querySelector<HTMLElement>('.term-dock')!
    return { target, component, dock }
  }

  it('leaves bare Ctrl chords to the shell and takes Ctrl+Shift for tabs and find', async () => {
    runtimeMock.System.IsMac.mockReturnValue(false)
    const { target, component, dock } = await mountDock()

    // Ctrl+T / Ctrl+W / Ctrl+F are readline keys: untouched, so xterm sends them.
    expect(press(dock, { key: 't', code: 'KeyT', ctrlKey: true }).defaultPrevented).toBe(false)
    expect(press(dock, { key: 'w', code: 'KeyW', ctrlKey: true }).defaultPrevented).toBe(false)
    expect(press(dock, { key: 'f', code: 'KeyF', ctrlKey: true }).defaultPrevented).toBe(false)
    await settle()
    expect(vi.mocked(OpenTerminal)).not.toHaveBeenCalled()
    expect(target.querySelectorAll('.term-tab').length).toBe(1)
    expect(target.querySelector('.cm-recur-search')).toBeNull()

    vi.mocked(OpenTerminal).mockResolvedValueOnce({ id: 'sess-2', title: 'zsh' } as never)
    expect(press(dock, { key: 'T', code: 'KeyT', ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true)
    await waitFor(() => target.querySelectorAll('.term-tab').length === 2)
    expect(target.querySelectorAll('.term-tab').length).toBe(2)

    expect(press(dock, { key: 'W', code: 'KeyW', ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true)
    await waitFor(() => target.querySelectorAll('.term-tab').length === 1)
    expect(target.querySelectorAll('.term-tab').length).toBe(1)

    expect(press(dock, { key: 'F', code: 'KeyF', ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true)
    await settle()
    expect(target.querySelector('.cm-recur-search')).toBeTruthy()
    unmount(component)
  })

  it('copies and pastes with Ctrl+Shift+C / Ctrl+Shift+V', async () => {
    runtimeMock.System.IsMac.mockReturnValue(false)
    const { component, dock } = await mountDock()

    expect(press(dock, { key: 'C', code: 'KeyC', ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true)
    await settle()
    expect(runtimeMock.Clipboard.SetText).toHaveBeenCalledWith('selected text')

    expect(press(dock, { key: 'V', code: 'KeyV', ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true)
    // Panes park and reuse xterm instances across mounts, so match any of them.
    await waitFor(() => terminals.some((t) => t.paste.mock.calls.length > 0))
    expect(terminals.flatMap((t) => t.paste.mock.calls)).toEqual([['pasted']])

    // Bare Ctrl+C is the shell's interrupt, not copy.
    expect(press(dock, { key: 'c', code: 'KeyC', ctrlKey: true }).defaultPrevented).toBe(false)
    unmount(component)
  })
})
