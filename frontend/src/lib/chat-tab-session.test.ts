import { beforeEach, describe, expect, test } from 'vitest'
import {
  clearActiveChatCode,
  loadActiveChatCode,
  pruneActiveChatCodes,
  resolveActiveChatCode,
  restoredChatCode,
  saveActiveChatCode,
} from './chat-tab-session'

class MemoryStorage {
  private data = new Map<string, string>()
  getItem(k: string) {
    return this.data.get(k) ?? null
  }
  setItem(k: string, v: string) {
    this.data.set(k, String(v))
  }
  removeItem(k: string) {
    this.data.delete(k)
  }
  clear() {
    this.data.clear()
  }
  key(i: number) {
    return [...this.data.keys()][i] ?? null
  }
  get length() {
    return this.data.size
  }
}

const tabs = [{ code: 'MA' }, { code: 'PA' }, { code: 'PA2' }]

describe('resolveActiveChatCode', () => {
  test('uses the saved code when it is still in the strip', () => {
    expect(resolveActiveChatCode(tabs, 'PA2')).toBe('PA2')
    expect(resolveActiveChatCode(tabs, 'PA')).toBe('PA')
    expect(resolveActiveChatCode(tabs, 'MA')).toBe('MA')
  })

  test('falls back to the first tab when saved is missing or stale', () => {
    expect(resolveActiveChatCode(tabs, null)).toBe('MA')
    expect(resolveActiveChatCode(tabs, '')).toBe('MA')
    expect(resolveActiveChatCode(tabs, 'PA9')).toBe('MA')
    expect(resolveActiveChatCode([], 'PA')).toBe('PA')
  })
})

describe('chat-tab-session storage', () => {
  beforeEach(() => {
    Object.defineProperty(globalThis, 'localStorage', {
      value: new MemoryStorage(),
      configurable: true,
    })
  })

  test('round-trips the active code per workspace', () => {
    saveActiveChatCode('ws-a', 'PA2')
    saveActiveChatCode('ws-b', 'PA')
    expect(loadActiveChatCode('ws-a')).toBe('PA2')
    expect(loadActiveChatCode('ws-b')).toBe('PA')
    expect(loadActiveChatCode('ws-missing')).toBeNull()
  })

  test('restoredChatCode prefers a still-open saved tab', () => {
    saveActiveChatCode('ws-a', 'PA2')
    expect(restoredChatCode('ws-a', tabs)).toBe('PA2')
  })

  test('restoredChatCode falls back when the saved tab was closed', () => {
    saveActiveChatCode('ws-a', 'PA9')
    expect(restoredChatCode('ws-a', tabs)).toBe('MA')
  })

  test('clearActiveChatCode removes one workspace and leaves others', () => {
    saveActiveChatCode('ws-a', 'PA2')
    saveActiveChatCode('ws-b', 'PA')
    clearActiveChatCode('ws-a')
    expect(loadActiveChatCode('ws-a')).toBeNull()
    expect(loadActiveChatCode('ws-b')).toBe('PA')
    clearActiveChatCode('ws-missing')
    expect(loadActiveChatCode('ws-b')).toBe('PA')
  })

  test('pruneActiveChatCodes drops deleted workspaces and keeps live ones', () => {
    saveActiveChatCode('ws-a', 'PA2')
    saveActiveChatCode('ws-b', 'PA')
    saveActiveChatCode('ws-gone', 'MA')
    pruneActiveChatCodes(['ws-a', 'ws-b'])
    expect(loadActiveChatCode('ws-a')).toBe('PA2')
    expect(loadActiveChatCode('ws-b')).toBe('PA')
    expect(loadActiveChatCode('ws-gone')).toBeNull()
  })
})
