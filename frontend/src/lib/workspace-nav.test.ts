import { describe, expect, test, vi } from 'vitest'
import {
  neighborWorkspaceId,
  sidebarWorkspaceIds,
  nestWorkspaces,
  isNestedChild,
  splitScheduled,
  emptyScanPeek,
  scanLand,
  scanCommit,
  scanForget,
  uniqueById,
  type NavGroup,
  type NavProject,
  type NavWorkspace,
  type ScanStep,
  scrollDeltaToReveal,
} from './workspace-nav'

function ws(
  id: string,
  projectId: string,
  extra: Partial<NavWorkspace> = {},
): NavWorkspace {
  return { id, projectId, ...extra }
}

describe('nestWorkspaces', () => {
  test('children follow their parent; orphans render as roots', () => {
    const items = [
      ws('child-b', 'p', { parentId: 'parent' }),
      ws('other', 'p'),
      ws('parent', 'p'),
      ws('child-a', 'p', { parentId: 'parent' }),
      ws('orphan', 'p', { parentId: 'gone' }),
    ]
    expect(nestWorkspaces(items).map((w) => w.id)).toEqual(['other', 'parent', 'child-b', 'child-a', 'orphan'])
    expect(isNestedChild(items[0], items)).toBe(true)
    expect(isNestedChild(items[4], items)).toBe(false)
  })
})

describe('splitScheduled', () => {
  test('groups occurrence workspaces and their fan-out children', () => {
    const items = [
      ws('sched', 'p', { scheduleId: 'daily' }),
      ws('task', 'p'),
      ws('child', 'p', { parentId: 'sched' }),
      ws('orphan', 'p', { parentId: 'gone' }),
    ]
    const { regular, scheduled } = splitScheduled(items)
    expect(regular.map((w) => w.id)).toEqual(['task', 'orphan'])
    expect(scheduled.map((w) => w.id)).toEqual(['sched', 'child'])
  })
})

describe('sidebarWorkspaceIds', () => {
  const projects: NavProject[] = [
    { id: 'storefront' },
    { id: 'payments', groupId: 'g-api' },
    { id: 'marketing' },
    { id: 'billing', groupId: 'g-api' },
  ]
  const groups: NavGroup[] = [{ id: 'g-api' }]

  test('groups first (group membership order), then ungrouped; main then tasks', () => {
    const workspaces: NavWorkspace[] = [
      ws('task-sf-new', 'storefront'),
      ws('task-pay', 'payments'),
      ws('main-sf', 'storefront', { isMain: true }),
      ws('task-sf-old', 'storefront'),
      ws('main-pay', 'payments', { isMain: true }),
      ws('task-mkt', 'marketing'),
      ws('task-bill', 'billing'),
      ws('archived-sf', 'storefront', { archived: true }),
      ws('orphan', 'gone'),
    ]
    expect(sidebarWorkspaceIds(workspaces, projects, groups)).toEqual([
      'main-pay',
      'task-pay',
      'main:billing',
      'task-bill',
      'main-sf',
      'task-sf-new',
      'task-sf-old',
      'main:marketing',
      'task-mkt',
    ])
  })

  test('skips archived and unknown projects; inserts a Working copy sentinel when missing', () => {
    expect(
      sidebarWorkspaceIds(
        [ws('a', 'storefront', { archived: true }), ws('b', 'storefront')],
        [{ id: 'storefront' }],
        [],
      ),
    ).toEqual(['main:storefront', 'b'])
  })

  test('skips PR chat hosts', () => {
    expect(
      sidebarWorkspaceIds(
        [ws('host', 'storefront', { hostPrId: 'pr-1' }), ws('task', 'storefront')],
        [{ id: 'storefront' }],
        [],
      ),
    ).toEqual(['main:storefront', 'task'])
  })

  test('scheduled workspaces sit under Working copy, with children nested', () => {
    expect(
      sidebarWorkspaceIds(
        [
          ws('sched', 'storefront', { scheduleId: 'daily' }),
          ws('task', 'storefront'),
          ws('child', 'storefront', { parentId: 'sched' }),
        ],
        [{ id: 'storefront' }],
        [],
      ),
    ).toEqual(['main:storefront', 'sched', 'child', 'task'])
  })

  test('drops duplicate workspace and project ids so keyed sidebar rows stay unique', () => {
    expect(
      sidebarWorkspaceIds(
        [ws('task-a', 'storefront'), ws('task-a', 'storefront'), ws('task-b', 'storefront')],
        [{ id: 'storefront' }, { id: 'storefront' }],
        [],
      ),
    ).toEqual(['main:storefront', 'task-a', 'task-b'])
  })
})

describe('uniqueById', () => {
  test('keeps first occurrence and returns the same array when already unique', () => {
    const unique = [{ id: 'a' }, { id: 'b' }]
    expect(uniqueById(unique)).toBe(unique)
    expect(uniqueById([{ id: 'a', n: 1 }, { id: 'a', n: 2 }, { id: 'b', n: 3 }])).toEqual([
      { id: 'a', n: 1 },
      { id: 'b', n: 3 },
    ])
  })

  test('empty string and missing ids are distinct keys (same as Svelte each)', () => {
    expect(uniqueById([{ id: '' }, { id: undefined }, { id: 'x' }])).toEqual([
      { id: '' },
      { id: undefined },
      { id: 'x' },
    ])
  })

  test('warns only when asked, with the colliding id values', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    uniqueById([{ id: 'a' }, { id: 'a' }, { id: '' }, { id: '' }])
    expect(warn).not.toHaveBeenCalled()
    uniqueById([{ id: 'a' }, { id: 'a' }, { id: '' }, { id: '' }], true)
    expect(warn).toHaveBeenCalledWith('uniqueById: dropped duplicate ids', ['a', ''])
    warn.mockRestore()
  })
})

describe('neighborWorkspaceId', () => {
  const ids = ['a', 'b', 'c']

  test('next/prev wrap', () => {
    expect(neighborWorkspaceId(ids, 'a', 1)).toBe('b')
    expect(neighborWorkspaceId(ids, 'c', 1)).toBe('a')
    expect(neighborWorkspaceId(ids, 'a', -1)).toBe('c')
    expect(neighborWorkspaceId(ids, 'b', -1)).toBe('a')
  })

  test('no current id: next is first, prev is last', () => {
    expect(neighborWorkspaceId(ids, '', 1)).toBe('a')
    expect(neighborWorkspaceId(ids, '', -1)).toBe('c')
    expect(neighborWorkspaceId(ids, 'missing', 1)).toBe('a')
    expect(neighborWorkspaceId(ids, 'missing', -1)).toBe('c')
  })

  test('empty list and already-selected singleton are no-ops', () => {
    expect(neighborWorkspaceId([], 'a', 1)).toBeNull()
    expect(neighborWorkspaceId(['only'], 'only', 1)).toBeNull()
    expect(neighborWorkspaceId(['only'], 'only', -1)).toBeNull()
    expect(neighborWorkspaceId(['only'], '', 1)).toBe('only')
    expect(neighborWorkspaceId(['only'], 'other', -1)).toBe('only')
  })
})

describe('scanLand / scanCommit', () => {
  const collapsedA: ScanStep = {
    projectId: 'a',
    groupId: '',
    projectCollapsed: true,
    groupCollapsed: false,
  }
  const collapsedB: ScanStep = {
    projectId: 'b',
    groupId: '',
    projectCollapsed: true,
    groupCollapsed: false,
  }
  const openA: ScanStep = {
    projectId: 'a',
    groupId: '',
    projectCollapsed: false,
    groupCollapsed: false,
  }

  test('expands a collapsed project and re-collapses it when leaving uncommitted', () => {
    const first = scanLand(emptyScanPeek(), collapsedA)
    expect(first.diff.expandProjects).toEqual(['a'])
    expect(first.next.peekedProjects).toEqual(['a'])

    const still = scanLand(first.next, { ...openA, projectId: 'a' })
    expect(still.diff.expandProjects).toEqual([])
    expect(still.diff.collapseProjects).toEqual([])
    expect(still.next.peekedProjects).toEqual(['a'])

    const left = scanLand(still.next, collapsedB)
    expect(left.diff.collapseProjects).toEqual(['a'])
    expect(left.diff.expandProjects).toEqual(['b'])
    expect(left.next.peekedProjects).toEqual(['b'])
  })

  test('commit keeps the current peek open so a later leave does not re-collapse', () => {
    const first = scanLand(emptyScanPeek(), collapsedA)
    const { next, persistProjects } = scanCommit(first.next)
    expect(persistProjects).toEqual(['a'])
    expect(next.peekedProjects).toEqual([])

    const left = scanLand(next, collapsedB)
    expect(left.diff.collapseProjects).toEqual([])
    expect(left.diff.expandProjects).toEqual(['b'])
  })

  test('commit to a different project re-collapses the peek instead of persisting it', () => {
    const first = scanLand(emptyScanPeek(), collapsedA)
    const { next, persistProjects, collapseProjects } = scanCommit(first.next, {
      projectId: 'b',
      groupId: '',
    })
    expect(persistProjects).toEqual([])
    expect(collapseProjects).toEqual(['a'])
    expect(next.peekedProjects).toEqual([])
    const left = scanLand(next, collapsedB)
    expect(left.diff.collapseProjects).toEqual([])
  })

  test('scanCommit always returns a new peek object', () => {
    // Sidebar $effect writes this; a new object every time would retrigger the
    // effect if the write were tracked (effect_update_depth_exceeded).
    const first = scanCommit(emptyScanPeek(), { projectId: 'a', groupId: '' })
    const again = scanCommit(first.next, { projectId: 'a', groupId: '' })
    expect(again.next).not.toBe(first.next)
    expect(again.next).toEqual(first.next)
  })

  test('already-open projects are not peeked', () => {
    const first = scanLand(emptyScanPeek(), openA)
    expect(first.diff.expandProjects).toEqual([])
    expect(first.next.peekedProjects).toEqual([])
    const left = scanLand(first.next, collapsedB)
    expect(left.diff.collapseProjects).toEqual([])
  })

  test('peeks collapsed groups and re-collapses them when leaving the group', () => {
    const pay: ScanStep = {
      projectId: 'payments',
      groupId: 'g-api',
      projectCollapsed: true,
      groupCollapsed: true,
    }
    const bill: ScanStep = {
      projectId: 'billing',
      groupId: 'g-api',
      projectCollapsed: true,
      groupCollapsed: false,
    }
    const sf: ScanStep = {
      projectId: 'storefront',
      groupId: '',
      projectCollapsed: false,
      groupCollapsed: false,
    }

    const intoPay = scanLand(emptyScanPeek(), pay)
    expect(intoPay.diff.expandGroups).toEqual(['g-api'])
    expect(intoPay.diff.expandProjects).toEqual(['payments'])

    const intoBill = scanLand(intoPay.next, bill)
    expect(intoBill.diff.collapseProjects).toEqual(['payments'])
    expect(intoBill.diff.collapseGroups).toEqual([])
    expect(intoBill.diff.expandProjects).toEqual(['billing'])
    expect(intoBill.next.peekedGroups).toEqual(['g-api'])

    const out = scanLand(intoBill.next, sf)
    expect(out.diff.collapseProjects).toEqual(['billing'])
    expect(out.diff.collapseGroups).toEqual(['g-api'])
  })

  test('scanForget drops a manual toggle from the peek set', () => {
    const first = scanLand(emptyScanPeek(), collapsedA)
    const forgotten = scanForget(first.next, { projectId: 'a' })
    const left = scanLand(forgotten, collapsedB)
    expect(left.diff.collapseProjects).toEqual([])
  })
})

describe('scrollDeltaToReveal', () => {
  test('returns 0 when the row is already in the visible band', () => {
    expect(scrollDeltaToReveal(80, 108, 0, 200, 30, 20)).toBe(0)
  })

  test('scrolls up when the row sits under the top inset (sticky header)', () => {
    expect(scrollDeltaToReveal(10, 38, 0, 200, 30, 20)).toBe(-20)
  })

  test('scrolls down when the row sits in the bottom fade', () => {
    expect(scrollDeltaToReveal(170, 198, 0, 200, 30, 20)).toBe(18)
  })

  test('aligns the top when the row is taller than the band', () => {
    expect(scrollDeltaToReveal(0, 400, 0, 200, 30, 20)).toBe(-30)
  })
})
