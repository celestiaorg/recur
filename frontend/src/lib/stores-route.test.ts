import { beforeEach, describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import { parseChatRouteTarget, parseWorkspaceRouteTarget } from './chat-route'
import { observatoryJump, routeTarget, selectedWorkspaceId, view } from './stores'

describe('parseChatRouteTarget', () => {
  it('splits workspace id from tab code', () => {
    expect(parseChatRouteTarget('chat:demo.golden-burrito:PA2')).toEqual({
      workspaceId: 'demo.golden-burrito',
      code: 'PA2',
    })
    expect(parseChatRouteTarget('chat:LOOP-2218:MA')).toEqual({
      workspaceId: 'LOOP-2218',
      code: 'MA',
    })
  })

  it('returns null for non-chat or empty workspace targets', () => {
    expect(parseChatRouteTarget('workspace:LOOP-2218')).toBeNull()
    expect(parseChatRouteTarget('chat:')).toBeNull()
    expect(parseChatRouteTarget('pr:482')).toBeNull()
  })
})

describe('parseWorkspaceRouteTarget', () => {
  it('strips an optional plan or findings panel', () => {
    expect(parseWorkspaceRouteTarget('workspace:LOOP-2218')).toEqual({ workspaceId: 'LOOP-2218' })
    expect(parseWorkspaceRouteTarget('workspace:recur.zesty-quiche:plan')).toEqual({
      workspaceId: 'recur.zesty-quiche',
      panel: 'plan',
    })
    expect(parseWorkspaceRouteTarget('loop:W1:findings')).toEqual({ workspaceId: 'W1', panel: 'findings' })
    expect(parseWorkspaceRouteTarget('workspace:W1:review')).toEqual({ workspaceId: 'W1', panel: 'review' })
  })

  it('returns null for other kinds or empty ids', () => {
    expect(parseWorkspaceRouteTarget('chat:W1:PA')).toBeNull()
    expect(parseWorkspaceRouteTarget('workspace:')).toBeNull()
    expect(parseWorkspaceRouteTarget('pr:12')).toBeNull()
  })
})

describe('routeTarget', () => {
  beforeEach(() => {
    observatoryJump.set(null)
    selectedWorkspaceId.set('')
  })

  it('opens the plan panel for workspace:<id>:plan', () => {
    routeTarget('workspace:recur.zesty-quiche:plan')
    expect(get(selectedWorkspaceId)).toBe('recur.zesty-quiche')
    expect(get(view)).toBe('detail')
    expect(get(observatoryJump)).toEqual({ workspaceId: 'recur.zesty-quiche', panel: 'plan' })
  })

  it('opens the findings panel for workspace:<id>:findings', () => {
    routeTarget('workspace:W1:findings')
    expect(get(selectedWorkspaceId)).toBe('W1')
    expect(get(observatoryJump)).toEqual({ workspaceId: 'W1', panel: 'findings' })
  })

  it('opens the review surface for workspace:<id>:review', () => {
    routeTarget('workspace:LOOP-2218:review')
    expect(get(selectedWorkspaceId)).toBe('LOOP-2218')
    expect(get(view)).toBe('detail')
    expect(get(observatoryJump)).toEqual({ workspaceId: 'LOOP-2218', panel: 'review' })
  })

  it('opens a workspace without a panel jump', () => {
    routeTarget('workspace:LOOP-2218')
    expect(get(selectedWorkspaceId)).toBe('LOOP-2218')
    expect(get(view)).toBe('detail')
    expect(get(observatoryJump)).toBeNull()
  })
})
