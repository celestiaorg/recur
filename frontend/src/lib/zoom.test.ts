import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { get } from 'svelte/store'

const shell = vi.hoisted(() => ({
  GetUIZoom: vi.fn(),
  SetUIZoom: vi.fn(() => Promise.resolve()),
}))
vi.mock('../../bindings/recur/internal/app', () => ({ ShellService: shell }))

import { toasts } from './stores'
import { initZoom, uiZoom, zoomIn, zoomOut, zoomReset } from './zoom'

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal('document', { documentElement: { style: {} as Record<string, string> } })
  vi.stubGlobal('window', { dispatchEvent: vi.fn() })
  uiZoom.set(1)
  toasts.set([])
  shell.SetUIZoom.mockClear()
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('zoom stepping', () => {
  it('steps in and out one level at a time', () => {
    zoomIn()
    expect(get(uiZoom)).toBe(1.1)
    zoomIn()
    expect(get(uiZoom)).toBe(1.25)
    zoomOut()
    zoomOut()
    zoomOut()
    expect(get(uiZoom)).toBe(0.9)
  })

  it('clamps at both ends', () => {
    for (let i = 0; i < 20; i++) zoomIn()
    expect(get(uiZoom)).toBe(2)
    for (let i = 0; i < 20; i++) zoomOut()
    expect(get(uiZoom)).toBe(0.5)
  })

  it('resets to 100%', () => {
    zoomIn()
    zoomIn()
    zoomReset()
    expect(get(uiZoom)).toBe(1)
    expect(document.documentElement.style.zoom).toBe('1')
  })

  it('moves to the next step from an off-step value', () => {
    uiZoom.set(1.05)
    zoomIn()
    expect(get(uiZoom)).toBe(1.1)
    uiZoom.set(1.05)
    zoomOut()
    expect(get(uiZoom)).toBe(1)
  })
})

describe('zoom side effects', () => {
  it('sets CSS zoom, dispatches resize, toasts with a replace key and debounces the save', () => {
    zoomIn()
    zoomIn()
    expect(document.documentElement.style.zoom).toBe('1.25')
    expect(window.dispatchEvent).toHaveBeenCalledTimes(2)
    expect(get(toasts)).toHaveLength(1)
    expect(get(toasts)[0].text).toBe('Zoom 125%')
    expect(shell.SetUIZoom).not.toHaveBeenCalled()
    vi.advanceTimersByTime(300)
    expect(shell.SetUIZoom).toHaveBeenCalledTimes(1)
    expect(shell.SetUIZoom).toHaveBeenCalledWith(1.25)
  })

  it('does nothing at the limit', () => {
    uiZoom.set(2)
    zoomIn()
    expect(get(toasts)).toHaveLength(0)
    vi.advanceTimersByTime(1000)
    expect(shell.SetUIZoom).not.toHaveBeenCalled()
  })
})

describe('initZoom', () => {
  it('applies the saved level without a toast or save', async () => {
    shell.GetUIZoom.mockResolvedValue(1.5)
    await initZoom()
    expect(get(uiZoom)).toBe(1.5)
    expect(document.documentElement.style.zoom).toBe('1.5')
    expect(get(toasts)).toHaveLength(0)
    vi.advanceTimersByTime(1000)
    expect(shell.SetUIZoom).not.toHaveBeenCalled()
  })

  it('clamps an out-of-range value and ignores bridge errors', async () => {
    shell.GetUIZoom.mockResolvedValue(9)
    await initZoom()
    expect(get(uiZoom)).toBe(2)
    shell.GetUIZoom.mockRejectedValue(new Error('no bridge'))
    await initZoom()
    expect(get(uiZoom)).toBe(2)
  })
})
