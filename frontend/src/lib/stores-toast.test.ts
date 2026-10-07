import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { get } from 'svelte/store'
import { dismissToast, pushToast, toastEditorTheme, toasts } from './stores'

beforeEach(() => {
  vi.useFakeTimers()
  toasts.set([])
})

afterEach(() => {
  for (const t of get(toasts)) dismissToast(t.id)
  vi.useRealTimers()
})

describe('pushToast replace', () => {
  it('replaces a visible toast with the same key and restarts TTL', () => {
    pushToast('success', 'Editor theme: A', { replace: 'editor-theme' })
    pushToast('success', 'Editor theme: B', { replace: 'editor-theme' })
    expect(get(toasts)).toHaveLength(1)
    expect(get(toasts)[0].text).toBe('Editor theme: B')
    vi.advanceTimersByTime(3499)
    expect(get(toasts)).toHaveLength(1)
    vi.advanceTimersByTime(2)
    expect(get(toasts)).toHaveLength(0)
  })

  it('does not replace unrelated toasts', () => {
    pushToast('success', 'Saved')
    toastEditorTheme('Monokai')
    expect(get(toasts).map((t) => t.text)).toEqual(['Saved', 'Editor theme: Monokai'])
  })
})

describe('pushToast sticky and onDismiss', () => {
  it('does not auto-dismiss when ttlMs is 0', () => {
    pushToast('error', 'held', { ttlMs: 0 })
    vi.advanceTimersByTime(60_000)
    expect(get(toasts)).toHaveLength(1)
  })

  it('runs onDismiss when the toast is closed, not when skip is set', () => {
    const dismissed: string[] = []
    pushToast('error', 'held', { ttlMs: 0, onDismiss: () => dismissed.push('yes') })
    const id = get(toasts)[0].id
    dismissToast(id, { invokeOnDismiss: false })
    expect(dismissed).toEqual([])
    pushToast('error', 'held', { ttlMs: 0, onDismiss: () => dismissed.push('yes') })
    dismissToast(get(toasts)[0].id)
    expect(dismissed).toEqual(['yes'])
  })
})
