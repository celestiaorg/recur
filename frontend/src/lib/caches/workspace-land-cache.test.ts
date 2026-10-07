import { describe, expect, it, beforeEach } from 'vitest'
import {
  dropWorkspaceLand,
  getWorkspaceLand,
  pruneWorkspaceLand,
  putWorkspaceLand,
  setWorkspaceLandMaxBytesForTests,
  WORKSPACE_LAND_MAX_BYTES,
  workspaceLandFrom,
} from './workspace-land-cache'

describe('workspace land cache', () => {
  beforeEach(() => {
    pruneWorkspaceLand([])
    setWorkspaceLandMaxBytesForTests(WORKSPACE_LAND_MAX_BYTES)
  })

  it('round-trips a snapshot per workspace', () => {
    putWorkspaceLand('ws-a', workspaceLandFrom({ dirty: true, remote: 'origin' }))
    putWorkspaceLand('ws-b', workspaceLandFrom({ aheadBase: 2, base: 'main' }))
    expect(getWorkspaceLand('ws-a')).toEqual(workspaceLandFrom({ dirty: true, remote: 'origin' }))
    expect(getWorkspaceLand('ws-b')?.aheadBase).toBe(2)
    expect(getWorkspaceLand('ws-c')).toBeNull()
  })

  it('copies out so later puts cannot mutate a recalled snap', () => {
    putWorkspaceLand('ws-a', workspaceLandFrom({ dirty: true }))
    const a = getWorkspaceLand('ws-a')!
    a.dirty = false
    expect(getWorkspaceLand('ws-a')?.dirty).toBe(true)
  })

  it('holds more than 24 snaps under the default byte budget', () => {
    for (let i = 0; i < 40; i++) {
      putWorkspaceLand(`ws-${i}`, workspaceLandFrom({ aheadBase: i }))
    }
    expect(getWorkspaceLand('ws-0')?.aheadBase).toBe(0)
    expect(getWorkspaceLand('ws-39')?.aheadBase).toBe(39)
  })

  it('evicts the oldest when over the byte budget', () => {
    setWorkspaceLandMaxBytesForTests(1)
    putWorkspaceLand('ws-0', workspaceLandFrom({ aheadBase: 0 }))
    putWorkspaceLand('ws-1', workspaceLandFrom({ aheadBase: 1 }))
    putWorkspaceLand('ws-2', workspaceLandFrom({ aheadBase: 2 }))
    expect(getWorkspaceLand('ws-0')).toBeNull()
    expect(getWorkspaceLand('ws-2')?.aheadBase).toBe(2)
  })

  it('drops and prunes gone workspaces', () => {
    putWorkspaceLand('keep', workspaceLandFrom({ dirty: true }))
    putWorkspaceLand('gone', workspaceLandFrom({ aheadBase: 1 }))
    dropWorkspaceLand('gone')
    expect(getWorkspaceLand('gone')).toBeNull()
    putWorkspaceLand('stale', workspaceLandFrom({ dirty: true }))
    pruneWorkspaceLand(['keep'])
    expect(getWorkspaceLand('stale')).toBeNull()
    expect(getWorkspaceLand('keep')?.dirty).toBe(true)
  })
})
