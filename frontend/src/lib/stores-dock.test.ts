import { beforeEach, describe, expect, it, vi } from 'vitest'
import { get } from 'svelte/store'

const setChrome = vi.fn((..._args: unknown[]) => Promise.resolve())
vi.mock('../../bindings/recur/internal/app/terminalservice', () => ({
  SetTerminalDockChrome: (...args: unknown[]) => setChrome(...args),
  ListTerminalDockChrome: () => Promise.resolve([]),
}))

const {
  applyTerminalDockChrome,
  setTerminalDockMaximized,
  setTerminalDockOpen,
  setTerminalBeside,
  terminalDockChromeTouched,
  terminalDockMaximizedBy,
  terminalDockOpenBy,
  editorHiddenBy,
  terminalBesideBy,
  terminalBesidePctBy,
} = await import('./stores')

beforeEach(() => {
  setChrome.mockClear()
  terminalDockChromeTouched.clear()
  terminalDockOpenBy.set({})
  terminalDockMaximizedBy.set({})
  editorHiddenBy.set({})
  terminalBesideBy.set({})
  terminalBesidePctBy.set({})
})

describe('terminal dock chrome', () => {
  it('restores maximize from Go chrome', () => {
    applyTerminalDockChrome([{ workspaceId: 'ws-1', open: true, maximized: true }])
    expect(get(terminalDockOpenBy)['ws-1']).toBe(true)
    expect(get(terminalDockMaximizedBy)['ws-1']).toBe(true)
  })

  it('writes chrome to the data-dir RPC when maximizing', () => {
    setTerminalDockMaximized('ws-1', true)
    expect(get(terminalDockMaximizedBy)['ws-1']).toBe(true)
    const last = setChrome.mock.calls.at(-1)
    expect(last?.[0]).toBe('ws-1')
    expect(last?.[1]).toMatchObject({ open: true, maximized: true })
  })

  it('does not restore maximize when the dock was hidden', () => {
    setTerminalDockMaximized('ws-1', true)
    setTerminalDockOpen('ws-1', false)
    expect(get(terminalDockMaximizedBy)['ws-1']).toBeFalsy()
    const last = setChrome.mock.calls.at(-1)
    expect(last?.[1]).toMatchObject({ open: false, maximized: false })
  })

  it('does not clobber a dock toggle that landed during boot', () => {
    setTerminalDockOpen('ws-1', false)
    applyTerminalDockChrome([{ workspaceId: 'ws-1', open: true, maximized: true }])
    expect(get(terminalDockOpenBy)['ws-1']).toBeFalsy()
    expect(get(terminalDockMaximizedBy)['ws-1']).toBeFalsy()
  })

  it('restores beside from Go chrome even when the dock is closed', () => {
    applyTerminalDockChrome([{ workspaceId: 'ws-1', beside: true, besidePct: 40 }])
    expect(get(terminalBesideBy)['ws-1']).toBe(true)
    expect(get(terminalBesidePctBy)['ws-1']).toBe(40)
    expect(get(terminalDockOpenBy)['ws-1']).toBeFalsy()
  })

  it('restores a saved split width when beside is off', () => {
    applyTerminalDockChrome([{ workspaceId: 'ws-1', besidePct: 40 }])
    expect(get(terminalBesidePctBy)['ws-1']).toBe(40)
    expect(get(terminalBesideBy)['ws-1']).toBeFalsy()
  })

  it('writes beside to the data-dir RPC', () => {
    setTerminalBeside('ws-1', true)
    expect(get(terminalBesideBy)['ws-1']).toBe(true)
    expect(get(terminalDockOpenBy)['ws-1']).toBe(true)
    const last = setChrome.mock.calls.at(-1)
    expect(last?.[1]).toMatchObject({ open: true, beside: true })
  })
})
