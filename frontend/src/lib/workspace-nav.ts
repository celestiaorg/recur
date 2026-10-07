/** Sentinel cycle id for a project's Working copy row before OpenMainWorkspace. */
export const MAIN_SCAN_PREFIX = 'main:'

export function mainScanId(projectId: string): string {
  return MAIN_SCAN_PREFIX + projectId
}

export function projectIdFromMainScan(id: string): string | null {
  return id.startsWith(MAIN_SCAN_PREFIX) ? id.slice(MAIN_SCAN_PREFIX.length) : null
}

export type NavWorkspace = {
  id: string
  projectId: string
  archived?: boolean
  isMain?: boolean
  hostPrId?: string
  /** Fan-out child: nested under this workspace in the sidebar. */
  parentId?: string
  /** Occurrence workspace created by a scheduled prompt. */
  scheduleId?: string
}

/** Sidebar order for one project's task rows: each visible parent followed by
 *  its children (store order within each tier). A child whose parent is not
 *  in the list (archived, other project) renders as a root. */
export function nestWorkspaces<T extends { id: string; parentId?: string }>(items: readonly T[]): T[] {
  const ids = new Set(items.map((w) => w.id))
  const kids = new Map<string, T[]>()
  const roots: T[] = []
  for (const w of items) {
    const pid = (w.parentId ?? '').trim()
    if (pid && pid !== w.id && ids.has(pid)) {
      const list = kids.get(pid)
      if (list) list.push(w)
      else kids.set(pid, [w])
    } else {
      roots.push(w)
    }
  }
  const out: T[] = []
  for (const r of roots) {
    out.push(r)
    const list = kids.get(r.id)
    if (list) out.push(...list)
  }
  return out
}

/** True when this row should render nested under a parent in the same list. */
export function isNestedChild<T extends { id: string; parentId?: string }>(w: T, items: readonly T[]): boolean {
  const pid = (w.parentId ?? '').trim()
  return !!pid && pid !== w.id && items.some((o) => o.id === pid)
}

/** Split a project's task rows into human-created vs scheduled (and their fan-out children). */
export function splitScheduled<T extends { id: string; parentId?: string; scheduleId?: string }>(
  items: readonly T[],
): { regular: T[]; scheduled: T[] } {
  const scheduled = new Set<string>()
  for (const w of items) {
    if ((w.scheduleId ?? '').trim()) scheduled.add(w.id)
  }
  let grew = true
  while (grew) {
    grew = false
    for (const w of items) {
      if (scheduled.has(w.id)) continue
      const pid = (w.parentId ?? '').trim()
      if (pid && scheduled.has(pid)) {
        scheduled.add(w.id)
        grew = true
      }
    }
  }
  const regular: T[] = []
  const sched: T[] = []
  for (const w of items) {
    if (scheduled.has(w.id)) sched.push(w)
    else regular.push(w)
  }
  return { regular, scheduled: sched }
}

export type NavProject = {
  id: string
  groupId?: string
}

export type NavGroup = {
  id: string
}

/** First-wins unique by `id`. Sidebar keyed `{#each}` throws `each_key_duplicate`
 *  when a list contains the same id twice. Keys match Svelte: `undefined` and
 *  `''` are distinct. Pass `warn` only at store-write sites — `$derived` callers
 *  recompute on every `workspace.updated` and must not flood the console. */
export function uniqueById<T extends { id?: string }>(items: readonly T[], warn = false): T[] {
  if (items.length < 2) return items as T[]
  const seen = new Set<string | undefined>()
  const out: T[] = []
  const dropped: string[] = []
  for (const item of items) {
    const id = item.id
    if (seen.has(id)) {
      if (warn) dropped.push(String(id))
      continue
    }
    seen.add(id)
    out.push(item)
  }
  if (dropped.length) {
    console.warn('uniqueById: dropped duplicate ids', dropped)
  }
  return out.length === items.length ? (items as T[]) : out
}

/** Project ids in the same order the sidebar tree paints them. */
export function sidebarProjectIds(projects: NavProject[], groups: NavGroup[]): string[] {
  const groupIds = new Set(groups.map((g) => g.id))
  const byGroup = new Map<string, NavProject[]>()
  for (const p of uniqueById(projects)) {
    const gid = p.groupId && groupIds.has(p.groupId) ? p.groupId : ''
    const bucket = byGroup.get(gid)
    if (bucket) bucket.push(p)
    else byGroup.set(gid, [p])
  }
  const ordered: NavProject[] = []
  for (const g of uniqueById(groups)) {
    const members = byGroup.get(g.id)
    if (members) ordered.push(...members)
  }
  const ungrouped = byGroup.get('')
  if (ungrouped) ordered.push(...ungrouped)
  return ordered.map((p) => p.id)
}

/**
 * Navigable workspace ids in sidebar order: grouped projects, then ungrouped;
 * within a project the working copy (isMain), then scheduled workspaces, then
 * other task workspaces in store order (newest first).
 */
export function sidebarWorkspaceIds(
  workspaces: NavWorkspace[],
  projects: NavProject[],
  groups: NavGroup[],
): string[] {
  const projectOrder = sidebarProjectIds(projects, groups)
  const known = new Set(projectOrder)
  const mains = new Map<string, string>()
  const hasMain = new Set<string>()
  const loopsByProject = new Map<string, NavWorkspace[]>()
  for (const w of uniqueById(workspaces)) {
    if (!known.has(w.projectId)) continue
    if (w.isMain) {
      hasMain.add(w.projectId)
      if (!w.archived) mains.set(w.projectId, w.id)
      continue
    }
    if (w.archived || (w.hostPrId ?? '').trim()) continue
    const list = loopsByProject.get(w.projectId)
    if (list) list.push(w)
    else loopsByProject.set(w.projectId, [w])
  }
  const ids: string[] = []
  for (const pid of projectOrder) {
    const main = mains.get(pid)
    if (main) ids.push(main)
    else if (!hasMain.has(pid)) ids.push(mainScanId(pid))
    const loops = loopsByProject.get(pid)
    if (loops) {
      const { regular, scheduled } = splitScheduled(loops)
      ids.push(...nestWorkspaces(scheduled).map((w) => w.id))
      ids.push(...nestWorkspaces(regular).map((w) => w.id))
    }
  }
  return ids
}

/** Next/prev id with wrap. `delta` is +1 (down / next) or -1 (up / previous). */
export function neighborWorkspaceId(
  ids: string[],
  currentId: string,
  delta: number,
): string | null {
  if (ids.length === 0) return null
  const i = currentId ? ids.indexOf(currentId) : -1
  if (i < 0) return delta >= 0 ? ids[0]! : ids[ids.length - 1]!
  if (ids.length === 1) return null
  const step = delta >= 0 ? 1 : -1
  return ids[(i + step + ids.length) % ids.length]!
}

/**
 * How long the user must stay on one workspace before a peeked (auto-expanded)
 * project/group counts as a stop. Shorter than this is pass-through: leaving
 * re-collapses. Key-repeat and hesitant taps stay under this; sitting on a row
 * to read it does not.
 */
export const SCAN_COMMIT_MS = 1000

export type ScanPeek = {
  peekedProjects: string[]
  peekedGroups: string[]
  currentProjectId: string
  currentGroupId: string
}

export type ScanStep = {
  projectId: string
  groupId: string
  projectCollapsed: boolean
  groupCollapsed: boolean
}

export type ScanDiff = {
  expandProjects: string[]
  expandGroups: string[]
  collapseProjects: string[]
  collapseGroups: string[]
}

export function emptyScanPeek(): ScanPeek {
  return { peekedProjects: [], peekedGroups: [], currentProjectId: '', currentGroupId: '' }
}

/** Expand collapsed ancestors for this land; re-collapse uncommitted peeks we left. */
export function scanLand(prev: ScanPeek, step: ScanStep): { next: ScanPeek; diff: ScanDiff } {
  const peekedProjects = new Set(prev.peekedProjects)
  const peekedGroups = new Set(prev.peekedGroups)
  const expandProjects: string[] = []
  const expandGroups: string[] = []
  const collapseProjects: string[] = []
  const collapseGroups: string[] = []

  if (prev.currentProjectId && prev.currentProjectId !== step.projectId) {
    if (peekedProjects.delete(prev.currentProjectId)) {
      collapseProjects.push(prev.currentProjectId)
    }
  }
  if (prev.currentGroupId && prev.currentGroupId !== step.groupId) {
    if (peekedGroups.delete(prev.currentGroupId)) {
      collapseGroups.push(prev.currentGroupId)
    }
  }

  if (step.groupId && (step.groupCollapsed || peekedGroups.has(step.groupId))) {
    if (step.groupCollapsed && !peekedGroups.has(step.groupId)) {
      expandGroups.push(step.groupId)
    }
    peekedGroups.add(step.groupId)
  }
  if (step.projectId && (step.projectCollapsed || peekedProjects.has(step.projectId))) {
    if (step.projectCollapsed && !peekedProjects.has(step.projectId)) {
      expandProjects.push(step.projectId)
    }
    peekedProjects.add(step.projectId)
  }

  return {
    next: {
      peekedProjects: [...peekedProjects],
      peekedGroups: [...peekedGroups],
      currentProjectId: step.projectId,
      currentGroupId: step.groupId,
    },
    diff: { expandProjects, expandGroups, collapseProjects, collapseGroups },
  }
}

/** Dwell on the current row, or a click/palette landing. Leaving the peeked
 *  project/group (including an empty landing) re-collapses instead of persisting. */
export function scanCommit(
  prev: ScanPeek,
  landing?: { projectId: string; groupId: string } | null,
): {
  next: ScanPeek
  persistProjects: string[]
  persistGroups: string[]
  collapseProjects: string[]
  collapseGroups: string[]
} {
  const dest = landing === undefined
    ? { projectId: prev.currentProjectId, groupId: prev.currentGroupId }
    : { projectId: landing?.projectId ?? '', groupId: landing?.groupId ?? '' }
  const stayProject = !!dest.projectId && dest.projectId === prev.currentProjectId
  const stayGroup = !!dest.groupId && dest.groupId === prev.currentGroupId
  if (!stayProject && !stayGroup) {
    return {
      next: {
        peekedProjects: [],
        peekedGroups: [],
        currentProjectId: dest.projectId,
        currentGroupId: dest.groupId,
      },
      persistProjects: [],
      persistGroups: [],
      collapseProjects: prev.peekedProjects,
      collapseGroups: prev.peekedGroups,
    }
  }
  return {
    next: {
      peekedProjects: [],
      peekedGroups: [],
      currentProjectId: dest.projectId,
      currentGroupId: dest.groupId,
    },
    persistProjects: stayProject ? prev.peekedProjects : [],
    persistGroups: stayGroup ? prev.peekedGroups : [],
    collapseProjects: stayProject ? [] : prev.peekedProjects,
    collapseGroups: stayGroup ? [] : prev.peekedGroups,
  }
}

/** User toggled collapse themselves — stop treating it as a pass-through peek. */
export function scanForget(
  prev: ScanPeek,
  ids: { projectId?: string; groupId?: string },
): ScanPeek {
  return {
    ...prev,
    peekedProjects: ids.projectId
      ? prev.peekedProjects.filter((id) => id !== ids.projectId)
      : prev.peekedProjects,
    peekedGroups: ids.groupId
      ? prev.peekedGroups.filter((id) => id !== ids.groupId)
      : prev.peekedGroups,
  }
}

/**
 * Pixels to add to a scroller's scrollTop so a row sits in the visible band
 * (below sticky headers, above a bottom fade). Coordinates are viewport-space.
 */
export function scrollDeltaToReveal(
  rowTop: number,
  rowBottom: number,
  viewTop: number,
  viewBottom: number,
  topInset: number,
  bottomInset: number,
): number {
  const visibleTop = viewTop + topInset
  const visibleBottom = viewBottom - bottomInset
  if (visibleBottom <= visibleTop) return rowTop - visibleTop
  if (rowTop < visibleTop) return rowTop - visibleTop
  if (rowBottom > visibleBottom) return rowBottom - visibleBottom
  return 0
}
