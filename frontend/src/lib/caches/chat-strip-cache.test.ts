/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import {
  getChatStrip,
  pruneChatStrips,
  putChatStrip,
  resetChatStripsForTests,
  setChatStripMaxBytesForTests,
} from './chat-strip-cache'
import type { WorkspaceChatTab } from '../../../bindings/recur/internal/domain/models'

function tab(code: string, title = code): WorkspaceChatTab {
  return { code, title, updatedAt: 0 } as WorkspaceChatTab
}

afterEach(() => {
  resetChatStripsForTests()
})

describe('chat strip cache', () => {
  test('round-trips tabs and active code', () => {
    putChatStrip('ws', { tabs: [tab('PA', 'New chat'), tab('MA', 'Loop')], activeCode: 'MA' })
    expect(getChatStrip('ws')?.activeCode).toBe('MA')
    expect(getChatStrip('ws')?.tabs.map((t) => t.code)).toEqual(['PA', 'MA'])
  })

  test('round-trips tab-strip scroll and keeps it when omitted', () => {
    putChatStrip('ws', { tabs: [tab('PA')], activeCode: 'PA', tabScrollLeft: 64 })
    expect(getChatStrip('ws')?.tabScrollLeft).toBe(64)
    putChatStrip('ws', { tabs: [tab('PA'), tab('MA')], activeCode: 'MA' })
    expect(getChatStrip('ws')?.tabScrollLeft).toBe(64)
    putChatStrip('ws', { tabs: [tab('PA')], activeCode: 'PA', tabScrollLeft: 0 })
    expect(getChatStrip('ws')?.tabScrollLeft).toBe(0)
  })

  test('empty tabs do not clobber a previous snapshot', () => {
    putChatStrip('ws', { tabs: [tab('PA')], activeCode: 'PA' })
    putChatStrip('ws', { tabs: [], activeCode: 'MA' })
    expect(getChatStrip('ws')?.tabs).toHaveLength(1)
  })

  test('prunes workspaces that are no longer live', () => {
    putChatStrip('a', { tabs: [tab('PA')] })
    putChatStrip('b', { tabs: [tab('PA')] })
    pruneChatStrips(['a'])
    expect(getChatStrip('a')).toBeTruthy()
    expect(getChatStrip('b')).toBeNull()
  })

  test('evicts the oldest strip when over the byte budget', () => {
    setChatStripMaxBytesForTests(1)
    putChatStrip('a', { tabs: [tab('PA')] })
    putChatStrip('b', { tabs: [tab('PA')] })
    putChatStrip('c', { tabs: [tab('PA')] })
    expect(getChatStrip('a')).toBeNull()
    expect(getChatStrip('c')?.tabs).toHaveLength(1)
  })
})
