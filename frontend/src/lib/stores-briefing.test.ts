/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import { briefingFocus, go, inbox, inboxByWorkspace, navKey, view } from './stores'

afterEach(() => {
  go('briefing')
  briefingFocus.set('')
  inbox.set([])
})

describe('briefing navigation', () => {
  it('maps legacy inbox to Briefing and keeps the sidebar on Briefing', () => {
    go('inbox')
    expect(get(view)).toBe('briefing')
    expect(navKey('inbox')).toBe('briefing')
    expect(navKey('briefing')).toBe('briefing')
  })

  it('can focus Needs you', () => {
    go('briefing', { focus: 'needs' })
    expect(get(view)).toBe('briefing')
    expect(get(briefingFocus)).toBe('needs')
  })
})

describe('inboxByWorkspace', () => {
  it('groups only unread workspace-joined notifications', () => {
    inbox.set([
      { id: 'a', workspaceId: 'WS-1', unread: true },
      { id: 'b', workspaceId: 'WS-1', unread: true },
      { id: 'c', workspaceId: 'WS-1', unread: false },
      { id: 'd', workspaceId: 'WS-2', unread: true },
      { id: 'e', unread: true },
    ] as never)
    const m = get(inboxByWorkspace)
    expect(m['WS-1']?.map((i) => i.id)).toEqual(['a', 'b'])
    expect(m['WS-2']).toHaveLength(1)
    expect(Object.keys(m)).toEqual(['WS-1', 'WS-2'])
  })
})
