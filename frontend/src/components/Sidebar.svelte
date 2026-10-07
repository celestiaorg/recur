<script lang="ts">
  import { tick, untrack } from 'svelte'
  import { get } from 'svelte/store'
  import {
    projectGroups, projects, workspaces, prs, inbox, attention, view,
    selectedWorkspaceId, projectFormId, go, openWorkspace, openPR, openProject, openNewTask, navKey, workspacesRunning, scheduleEditor,
    workspaceScanEvent, workspaceScanPending, userQuestions, permissionRequests, railCollapsed,
    settings,
  } from '../lib/stores'
  import { ProjectsService, SettingsService, WorkspacesService, call, callOk, loadInitial, refreshProjects, setProjectHidden } from '../lib/api'
  import { ProjectGroup } from '../../bindings/recur/internal/domain/models'
  import type { Project, Workspace } from '../../bindings/recur/internal/domain/models'
  import { formatCompactCount } from '../lib/format'
  import { prForWorkspace, prIconName, prLifecycle } from '../lib/colors'
  import { workspaceCanRemove, workspaceIsRunning, parkedNeedsYouByWorkspace, sidebarUnreadTitle } from '../lib/modes'
  import { prChatHostRunningCount, prChatHostUnseenCount } from '../lib/pr-workspace'
  import { markClickEcho, takeClickEcho } from '../lib/armed-confirm'
  import { removeWorkspace } from '../lib/remove-workspace'
  import {
    RAIL_PEEK_COARSE_EDGE_PX, RAIL_PEEK_EDGE_PX, RAIL_PEEK_HIDE_MS, RAIL_PEEK_SHOW_MS,
    closeRailPeek, isRailPeekSurface, openRailPeek, pointerFarFromRailEdge,
    pointerInLeftEdge, pointerLeftDocument, railPeek, railPeekHeld, railPeekLocked,
    type RailPeekRect,
  } from '../lib/rail-peek'
  import {
    emptyScanPeek, scanCommit, scanForget, scanLand, scrollDeltaToReveal, projectIdFromMainScan,
    uniqueById, nestWorkspaces, isNestedChild, splitScheduled, type ScanDiff, type ScanPeek,
  } from '../lib/workspace-nav'
  import DecodeText from './DecodeText.svelte'
  import Icon from './Icon.svelte'
  import ProjectPic from './ProjectPic.svelte'
  import ProviderUsageMeter from './ProviderUsageMeter.svelte'
  import ResizeHandle from './ResizeHandle.svelte'

  // per-project collapse state, seeded from each project's persisted `open` flag
  // (default open/expanded). Toggles write back via UpdateProjectRunConfig.
  let collapsed = $state<Record<string, boolean>>({})
  $effect(() => {
    let next: Record<string, boolean> | null = null
    for (const p of $projects) {
      if (!(p.id in collapsed)) {
        if (!next) next = { ...collapsed }
        next[p.id] = !p.open
      }
    }
    if (next) collapsed = next
  })

  // per-group collapse, seeded from group.open
  let groupCollapsed = $state<Record<string, boolean>>({})
  $effect(() => {
    let next: Record<string, boolean> | null = null
    for (const g of $projectGroups) {
      if (!(g.id in groupCollapsed)) {
        if (!next) next = { ...groupCollapsed }
        next[g.id] = !g.open
      }
    }
    if (next) groupCollapsed = next
  })

  // Scheduled subgroup under a project. Default collapsed; opening a scheduled
  // workspace expands it so keyboard-cycle / dashboard open can land on the row.
  let scheduledCollapsed = $state<Record<string, boolean>>({})

  // Keyboard-cycle peek: expand collapsed ancestors while scanning; re-collapse
  // if the user leaves before dwelling (commit). Manual caret toggles forget.
  let scan = $state<ScanPeek>(emptyScanPeek())

  function applyScanDiff(diff: ScanDiff) {
    if (diff.expandProjects.length || diff.collapseProjects.length) {
      const next = { ...collapsed }
      for (const id of diff.expandProjects) next[id] = false
      for (const id of diff.collapseProjects) next[id] = true
      collapsed = next
    }
    if (diff.expandGroups.length || diff.collapseGroups.length) {
      const next = { ...groupCollapsed }
      for (const id of diff.expandGroups) next[id] = false
      for (const id of diff.collapseGroups) next[id] = true
      groupCollapsed = next
    }
  }

  async function persistScanOpens(projectIds: string[], groupIds: string[]) {
    for (const id of projectIds) {
      const p = get(projects).find((x) => x.id === id)
      if (!p || p.open) continue
      const saved = await call(
        ProjectsService.UpdateProjectRunConfig({ ...p, open: true } as any),
        'Could not save project view',
      )
      if (saved) {
        projects.update((arr) => arr.map((x) => (x.id === id ? { ...x, open: true } : x)))
      }
    }
    for (const id of groupIds) {
      const g = get(projectGroups).find((x) => x.id === id)
      if (!g || g.open) continue
      const saved = await call(
        ProjectsService.UpdateProjectGroup({ ...g, open: true }),
        'Could not save group view',
      )
      if (saved) {
        projectGroups.update((arr) => arr.map((x) => (x.id === id ? { ...x, open: true } : x)))
      }
    }
  }

  // untrack: scanCommit always allocates a new object; writing scan must not re-run this effect.
  $effect(() => {
    const ev = $workspaceScanEvent
    if (ev.seq === 0) return
    untrack(() => {
      if (ev.kind === 'commit') {
        const landing = landingFromScanId(ev.workspaceId)
        const { next, persistProjects, persistGroups, collapseProjects, collapseGroups } = scanCommit(scan, landing)
        scan = next
        applyScanDiff({ expandProjects: [], expandGroups: [], collapseProjects, collapseGroups })
        if (persistProjects.length || persistGroups.length) {
          void persistScanOpens(persistProjects, persistGroups)
        }
        const mainPid = projectIdFromMainScan(ev.workspaceId)
        if (mainPid) {
          const p = get(projects).find((x) => x.id === mainPid)
          const seq = ev.seq
          if (p) {
            void openMain(p, { unlessScanMoved: seq }).finally(() => {
              if (get(workspaceScanEvent).seq !== seq) return
              if (get(workspaceScanPending)) workspaceScanPending.set(false)
            })
          } else {
            workspaceScanPending.set(false)
          }
        }
        return
      }
      const mainPid = projectIdFromMainScan(ev.workspaceId)
      const ws = mainPid ? undefined : get(workspaces).find((w) => w.id === ev.workspaceId)
      const projectId = mainPid || ws?.projectId
      if (!projectId) return
      const p = get(projects).find((x) => x.id === projectId)
      const groups = get(projectGroups)
      const gid = p?.groupId && groups.some((g) => g.id === p.groupId) ? p.groupId : ''
      const { next, diff } = scanLand(scan, {
        projectId,
        groupId: gid,
        projectCollapsed: !!collapsed[projectId],
        groupCollapsed: !!gid && !!groupCollapsed[gid],
      })
      scan = next
      applyScanDiff(diff)
      if (ws) revealScheduledFor(ws.id)
      void tick().then(() => scrollWorkspaceRowIntoView(ev.workspaceId))
    })
  })

  function landingFromScanId(id: string): { projectId: string; groupId: string } {
    const mainPid = projectIdFromMainScan(id)
    const pid = mainPid || get(workspaces).find((w) => w.id === id)?.projectId || ''
    if (!pid) return { projectId: '', groupId: '' }
    const p = get(projects).find((x) => x.id === pid)
    const groups = get(projectGroups)
    const gid = p?.groupId && groups.some((g) => g.id === p.groupId) ? p.groupId : ''
    return { projectId: pid, groupId: gid }
  }

  function scanHighlightId(ev: { kind: string; workspaceId: string }): string | null {
    if (ev.kind === 'land') return ev.workspaceId
    if ($workspaceScanPending && projectIdFromMainScan(ev.workspaceId)) return ev.workspaceId
    return null
  }

  function revealScheduledFor(workspaceId: string) {
    const w = get(workspaces).find((x) => x.id === workspaceId)
    if (!w || w.isMain || w.archived || (w.hostPrId ?? '').trim()) return
    const siblings = get(workspaces).filter((x) => x.projectId === w.projectId && !x.archived && !x.isMain && !(x.hostPrId ?? '').trim())
    if (!splitScheduled(siblings).scheduled.some((x) => x.id === w.id)) return
    if (scheduledCollapsed[w.projectId] === false) return
    scheduledCollapsed = { ...scheduledCollapsed, [w.projectId]: false }
  }

  $effect(() => {
    const id = $selectedWorkspaceId
    if (($view !== 'detail' && $view !== 'escalation') || !id) return
    untrack(() => {
      revealScheduledFor(id)
      void tick().then(() => scrollWorkspaceRowIntoView(id))
    })
  })

  function projectRowOn(p: Project) {
    return $view === 'project' && !!p.id && p.id === $projectFormId
  }

  function mainRowOn(p: Project, main: Workspace | undefined) {
    const scanId = scanHighlightId($workspaceScanEvent)
    if (scanId) {
      return projectIdFromMainScan(scanId) === p.id
        || (!!main && scanId === main.id)
    }
    // selectedWorkspaceId is kept as restore memory; only the live workspace
    // view (and its escalation overlay) should light the tree row.
    if ($view !== 'detail' && $view !== 'escalation') return false
    return !!main && main.id === $selectedWorkspaceId
  }

  function taskRowOn(id: string) {
    const scanId = scanHighlightId($workspaceScanEvent)
    if (scanId) return scanId === id
    if ($view !== 'detail' && $view !== 'escalation') return false
    return id === $selectedWorkspaceId
  }

  function cssPx(el: Element, prop: string, fallback: number): number {
    const n = parseFloat(getComputedStyle(el).getPropertyValue(prop))
    return Number.isFinite(n) ? n : fallback
  }

  /** Scroll .rail-tree so the row clears sticky headers and the bottom fade.
   *  Native scrollIntoView misses this port: mask-image + sticky .tproj-h. */
  function scrollWorkspaceRowIntoView(workspaceId: string) {
    const tree = treeEl
    if (!tree || !workspaceId) return
    const row = projectIdFromMainScan(workspaceId)
      ? tree.querySelector<HTMLElement>(`.tproj[data-project-id="${CSS.escape(projectIdFromMainScan(workspaceId)!)}"] .tloop.tmain`)
      : tree.querySelector<HTMLElement>(`[data-workspace-id="${CSS.escape(workspaceId)}"]`)
    if (!row || !row.offsetHeight) return
    const host = tree.closest('.rail-scroll') ?? tree
    const fade = cssPx(host, '--rail-fade', 20)
    const projH = cssPx(host, '--rail-proj-h', 30)
    const groupH = cssPx(host, '--rail-group-h', 26)
    const topInset = projH + (row.closest('.tgroup') ? groupH : 0)
    const treeRect = tree.getBoundingClientRect()
    const rowRect = row.getBoundingClientRect()
    const delta = scrollDeltaToReveal(
      rowRect.top,
      rowRect.bottom,
      treeRect.top,
      treeRect.bottom,
      topInset,
      fade,
    )
    if (delta === 0) return
    const max = Math.max(0, tree.scrollHeight - tree.clientHeight)
    tree.scrollTop = Math.min(max, Math.max(0, tree.scrollTop + delta))
    updateTreeEdges()
  }

  async function toggleProject(p: Project) {
    scan = scanForget(scan, { projectId: p.id })
    const nextCollapsed = !collapsed[p.id]
    collapsed = { ...collapsed, [p.id]: nextCollapsed }
    const saved = await call(
      ProjectsService.UpdateProjectRunConfig({ ...p, open: !nextCollapsed } as any),
      'Could not save project view',
    )
    if (!saved) {
      collapsed = { ...collapsed, [p.id]: !nextCollapsed }
      return
    }
    projects.update((arr) => arr.map((x) => (x.id === p.id ? { ...x, open: saved.open } : x)))
  }

  async function toggleGroup(g: ProjectGroup) {
    scan = scanForget(scan, { groupId: g.id })
    const nextCollapsed = !groupCollapsed[g.id]
    groupCollapsed = { ...groupCollapsed, [g.id]: nextCollapsed }
    const saved = await call(
      ProjectsService.UpdateProjectGroup({ ...g, open: !nextCollapsed }),
      'Could not save group view',
    )
    if (!saved) {
      groupCollapsed = { ...groupCollapsed, [g.id]: !nextCollapsed }
      return
    }
    projectGroups.update((arr) => arr.map((x) => (x.id === g.id ? { ...x, open: saved.open } : x)))
  }

  function projectShown(p: Project): boolean {
    return !p.hidden || !!$settings?.showHiddenProjects
  }

  async function toggleShowHiddenProjects() {
    const cur = get(settings)
    if (!cur) return
    const saved = await call(
      SettingsService.SetSettings({ ...cur, showHiddenProjects: !cur.showHiddenProjects } as any),
      'Could not save settings',
    )
    if (saved) settings.set(saved)
  }

  function groupIdOf(p: Project): string {
    return p.groupId || ''
  }

  const projectsByGroup = $derived.by(() => {
    const m = new Map<string, Project[]>()
    for (const p of uniqueById($projects)) {
      if (!projectShown(p)) continue
      const gid = groupIdOf(p)
      if (!m.has(gid)) m.set(gid, [])
      m.get(gid)!.push(p)
    }
    return m
  })

  /** Rebuild global project order: groups first (by group sort), then ungrouped.
   *  `bucketOrder` replaces that group's membership (ids may move across groups). */
  function rebuildOrder(bucketGroupId: string, bucketOrder: Project[]): Project[] {
    const list = get(projects)
    const groups = get(projectGroups)
    const bucketIds = new Set(bucketOrder.map((p) => p.id))
    const normalized = uniqueById(bucketOrder).map((p) => ({ ...p, groupId: bucketGroupId || '' }))
    const byGroup = new Map<string, Project[]>()
    for (const p of list) {
      if (bucketIds.has(p.id)) continue
      const gid = groupIdOf(p)
      if (!byGroup.has(gid)) byGroup.set(gid, [])
      byGroup.get(gid)!.push(p)
    }
    byGroup.set(bucketGroupId, normalized)
    const next: Project[] = []
    const seen = new Set<string>()
    for (const g of uniqueById(groups)) {
      for (const p of byGroup.get(g.id) || []) {
        if (seen.has(p.id)) continue
        next.push(p)
        seen.add(p.id)
      }
    }
    for (const p of byGroup.get('') || []) {
      if (seen.has(p.id)) continue
      next.push(p)
      seen.add(p.id)
    }
    for (const p of list) {
      if (seen.has(p.id) || bucketIds.has(p.id)) continue
      next.push({ ...p, groupId: '' })
      seen.add(p.id)
    }
    return next
  }

  // Free pointer-drag: projects (within/across groups) or group headers (group order).
  const DRAG_THRESHOLD = 4
  const FLIP_MS = 180
  const SETTLE_MS = 200
  let treeEl = $state<HTMLDivElement | null>(null)
  let treeShellEl = $state<HTMLDivElement | null>(null)
  let dragKind = $state<'project' | 'group' | null>(null)
  let dragFromId = $state<string | null>(null)
  let dragActive = $state(false)
  let dragSettling = $state(false)
  let dragX = $state(0)
  let dragY = $state(0)
  let dragW = $state(0)
  let dragH = $state(0)
  let dragOffsetX = 0
  let dragOffsetY = 0
  let orderBeforeDrag: Project[] | null = null
  let groupOrderBeforeDrag: ProjectGroup[] | null = null
  let dropTargetGroupId = $state<string | null>(null)
  let openedDuringDrag = new Set<string>()
  let suppressProjClick = false
  let suppressGroupClick = false
  let activePointerId: number | null = null

  function prefersReducedMotion() {
    return typeof window !== 'undefined' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches
  }

  /** Capture vertical positions for FLIP (siblings + placeholder). */
  function measureFlipTops(kind: 'project' | 'group'): Map<string, number> {
    const map = new Map<string, number>()
    if (!treeEl) return map
    if (kind === 'group') {
      for (const el of treeEl.querySelectorAll<HTMLElement>('.tgroup[data-group-id]:not(.is-float)')) {
        const id = el.dataset.groupId
        if (id) map.set(id, el.getBoundingClientRect().top)
      }
      const ph = treeEl.querySelector('.tgroup-ph') as HTMLElement | null
      if (ph) map.set('__ph__', ph.getBoundingClientRect().top)
      return map
    }
    for (const el of treeEl.querySelectorAll<HTMLElement>('.tproj[data-project-id]:not(.is-float)')) {
      const id = el.dataset.projectId
      if (id) map.set(id, el.getBoundingClientRect().top)
    }
    const ph = treeEl.querySelector('.tproj-ph') as HTMLElement | null
    if (ph) map.set('__ph__', ph.getBoundingClientRect().top)
    return map
  }

  /** Animate list items from pre-reorder tops to their new layout positions. */
  async function flipToNewPositions(kind: 'project' | 'group', from: Map<string, number>) {
    await tick()
    if (!treeEl || prefersReducedMotion() || from.size === 0) return

    const movers: { el: HTMLElement; dy: number }[] = []
    const itemSel = kind === 'group'
      ? '.tgroup[data-group-id]:not(.is-float)'
      : '.tproj[data-project-id]:not(.is-float)'
    for (const el of treeEl.querySelectorAll<HTMLElement>(itemSel)) {
      const id = kind === 'group' ? el.dataset.groupId : el.dataset.projectId
      if (!id || !from.has(id)) continue
      const dy = from.get(id)! - el.getBoundingClientRect().top
      if (Math.abs(dy) < 0.5) continue
      movers.push({ el, dy })
    }
    const ph = treeEl.querySelector(kind === 'group' ? '.tgroup-ph' : '.tproj-ph') as HTMLElement | null
    if (ph && from.has('__ph__')) {
      const dy = from.get('__ph__')! - ph.getBoundingClientRect().top
      if (Math.abs(dy) >= 0.5) movers.push({ el: ph, dy })
    }
    if (!movers.length) return

    for (const { el, dy } of movers) {
      el.style.transition = 'none'
      el.style.transform = `translateY(${dy}px)`
    }
    void treeEl.offsetHeight // reflow so the invert sticks before Play
    for (const { el } of movers) {
      el.style.transition = `transform ${FLIP_MS}ms cubic-bezier(.2,.8,.2,1)`
      el.style.transform = ''
    }
    window.setTimeout(() => {
      for (const { el } of movers) {
        if (!el.isConnected) continue
        el.style.transition = ''
        el.style.transform = ''
      }
    }, FLIP_MS + 40)
  }

  /** Which group (or '') the pointer is over — used for cross-group drops. */
  function resolveDropGroup(clientY: number): string {
    if (!treeEl) return ''
    const groupEls = [...treeEl.querySelectorAll<HTMLElement>('.tgroup[data-group-id]:not(.is-float)')]
    for (const el of groupEls) {
      const rect = el.getBoundingClientRect()
      if (clientY >= rect.top && clientY <= rect.bottom) {
        return el.dataset.groupId || ''
      }
    }
    // Between / near a group: snap to the nearest group's vertical range.
    let bestId = ''
    let bestDist = Infinity
    for (const el of groupEls) {
      const rect = el.getBoundingClientRect()
      const dist =
        clientY < rect.top ? rect.top - clientY :
        clientY > rect.bottom ? clientY - rect.bottom : 0
      if (dist < bestDist && dist < 28) {
        bestDist = dist
        bestId = el.dataset.groupId || ''
      }
    }
    if (bestDist < 28) return bestId
    return ''
  }

  function ensureGroupExpandedForDrop(groupId: string) {
    if (!groupId || !groupCollapsed[groupId]) return
    groupCollapsed = { ...groupCollapsed, [groupId]: false }
    openedDuringDrag.add(groupId)
  }

  function reorderByPointerY(clientY: number) {
    if (!dragFromId || !treeEl || dragSettling || dragKind !== 'project') return
    const targetGroupId = resolveDropGroup(clientY)
    dropTargetGroupId = targetGroupId
    if (targetGroupId) ensureGroupExpandedForDrop(targetGroupId)

    const list = get(projects)
    const dragged = list.find((p) => p.id === dragFromId)
    if (!dragged) return

    const others = list.filter((p) => p.id !== dragFromId && groupIdOf(p) === targetGroupId)

    let insertAt = others.length
    for (let i = 0; i < others.length; i++) {
      const el = treeEl.querySelector(
        `.tproj[data-project-id="${CSS.escape(others[i].id)}"]:not(.is-float)`,
      ) as HTMLElement | null
      if (!el) continue
      const rect = el.getBoundingClientRect()
      if (clientY < rect.top + rect.height / 2) {
        insertAt = i
        break
      }
    }

    // Dropping on a group header (above its first project) inserts at top.
    if (targetGroupId && others.length) {
      const gEl = treeEl.querySelector(
        `.tgroup[data-group-id="${CSS.escape(targetGroupId)}"]:not(.is-float) .tgroup-h`,
      ) as HTMLElement | null
      if (gEl) {
        const hr = gEl.getBoundingClientRect()
        if (clientY < hr.bottom) insertAt = 0
      }
    }

    const moved = { ...dragged, groupId: targetGroupId || '' }
    const nextBucket = [...others]
    nextBucket.splice(insertAt, 0, moved)

    const prevBucket = list.filter((p) => groupIdOf(p) === targetGroupId || p.id === dragFromId)
    const same =
      groupIdOf(dragged) === targetGroupId &&
      prevBucket.length === nextBucket.length &&
      nextBucket.every((p, i) => p.id === prevBucket[i]?.id)
    if (same) return

    const from = measureFlipTops('project')
    projects.set(rebuildOrder(targetGroupId, nextBucket))
    void flipToNewPositions('project', from)
  }

  function reorderGroupsByPointerY(clientY: number) {
    if (!dragFromId || !treeEl || dragSettling || dragKind !== 'group') return
    const list = get(projectGroups)
    const dragged = list.find((g) => g.id === dragFromId)
    if (!dragged) return
    const others = list.filter((g) => g.id !== dragFromId)

    let insertAt = others.length
    for (let i = 0; i < others.length; i++) {
      const el = treeEl.querySelector(
        `.tgroup[data-group-id="${CSS.escape(others[i].id)}"]:not(.is-float)`,
      ) as HTMLElement | null
      if (!el) continue
      const rect = el.getBoundingClientRect()
      if (clientY < rect.top + rect.height / 2) {
        insertAt = i
        break
      }
    }

    const next = [...others]
    next.splice(insertAt, 0, dragged)
    if (next.every((g, i) => g.id === list[i].id)) return
    const from = measureFlipTops('group')
    projectGroups.set(next)
    void flipToNewPositions('group', from)
  }

  function autoScroll(clientY: number) {
    if (!treeEl) return
    const rect = treeEl.getBoundingClientRect()
    const edge = 36
    if (clientY < rect.top + edge) treeEl.scrollTop -= 10
    else if (clientY > rect.bottom - edge) treeEl.scrollTop += 10
  }

  function onHeaderPointerDown(e: PointerEvent, id: string) {
    if (e.button !== 0 || dragSettling || dragActive) return
    if ((e.target as HTMLElement | null)?.closest('.pacts, .pmenu')) return

    const header = e.currentTarget as HTMLElement
    const block = header.closest('.tproj') as HTMLElement | null
    if (!block) return

    const startX = e.clientX
    const startY = e.clientY
    const rect = block.getBoundingClientRect()
    dragOffsetX = e.clientX - rect.left
    dragOffsetY = e.clientY - rect.top
    dragW = rect.width
    dragH = rect.height
    activePointerId = e.pointerId
    suppressProjClick = false

    const onMove = (ev: PointerEvent) => {
      if (ev.pointerId !== activePointerId || dragSettling) return
      if (!dragActive) {
        if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < DRAG_THRESHOLD) return
        dragActive = true
        dragKind = 'project'
        dragFromId = id
        orderBeforeDrag = get(projects)
        suppressProjClick = true
        document.body.classList.add('dragging-projects')
        try { header.setPointerCapture(ev.pointerId) } catch { /* already captured */ }
      }
      ev.preventDefault()
      dragX = ev.clientX - dragOffsetX
      dragY = ev.clientY - dragOffsetY
      reorderByPointerY(ev.clientY)
      autoScroll(ev.clientY)
    }

    const onUp = (ev: PointerEvent) => {
      if (ev.pointerId !== activePointerId) return
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', onUp)
      try { header.releasePointerCapture(ev.pointerId) } catch { /* already released */ }
      void finishDrag()
    }

    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
  }

  function onGroupHeaderPointerDown(e: PointerEvent, id: string) {
    if (e.button !== 0 || dragSettling || dragActive || renamingGroupId === id) return
    if ((e.target as HTMLElement | null)?.closest('.gacts, .gname-input')) return

    const header = e.currentTarget as HTMLElement
    const block = header.closest('.tgroup') as HTMLElement | null
    if (!block) return

    const startX = e.clientX
    const startY = e.clientY
    const rect = block.getBoundingClientRect()
    dragOffsetX = e.clientX - rect.left
    dragOffsetY = e.clientY - rect.top
    dragW = rect.width
    dragH = rect.height
    activePointerId = e.pointerId
    suppressGroupClick = false

    const onMove = (ev: PointerEvent) => {
      if (ev.pointerId !== activePointerId || dragSettling) return
      if (!dragActive) {
        if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < DRAG_THRESHOLD) return
        dragActive = true
        dragKind = 'group'
        dragFromId = id
        groupOrderBeforeDrag = get(projectGroups)
        suppressGroupClick = true
        document.body.classList.add('dragging-groups')
        try { header.setPointerCapture(ev.pointerId) } catch { /* already captured */ }
      }
      ev.preventDefault()
      dragX = ev.clientX - dragOffsetX
      dragY = ev.clientY - dragOffsetY
      reorderGroupsByPointerY(ev.clientY)
      autoScroll(ev.clientY)
    }

    const onUp = (ev: PointerEvent) => {
      if (ev.pointerId !== activePointerId) return
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', onUp)
      try { header.releasePointerCapture(ev.pointerId) } catch { /* already released */ }
      void finishGroupDrag()
    }

    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
  }

  /** Ease the floating block into the placeholder, then unfloat. */
  async function settleFloatIntoPlaceholder() {
    await tick()
    const kind = dragKind
    const phSel = kind === 'group' ? '.tgroup-ph' : '.tproj-ph'
    const floatSel = kind === 'group' ? '.tgroup.is-float' : '.tproj.is-float'
    // Placeholder stays in the scroll list; the float lives outside .rail-tree so
    // mask/transform ancestors can't break position:fixed (viewport) coordinates.
    const ph = treeEl?.querySelector(phSel) as HTMLElement | null
    const floatEl = (treeShellEl ?? treeEl)?.querySelector(floatSel) as HTMLElement | null
    if (!ph || !floatEl) return

    const target = ph.getBoundingClientRect()
    if (prefersReducedMotion()) {
      dragX = target.left
      dragY = target.top
      return
    }

    dragSettling = true
    try {
      await floatEl.animate(
        [
          {
            left: `${dragX}px`,
            top: `${dragY}px`,
            boxShadow: '0 10px 32px rgba(0,0,0,.4)',
            borderColor: 'var(--line)',
          },
          {
            left: `${target.left}px`,
            top: `${target.top}px`,
            boxShadow: '0 0 0 rgba(0,0,0,0)',
            borderColor: 'transparent',
          },
        ],
        { duration: SETTLE_MS, easing: 'cubic-bezier(.2,.8,.2,1)', fill: 'forwards' },
      ).finished
    } catch {
      /* animation cancelled (element detached) */
    }
    dragX = target.left
    dragY = target.top
    for (const a of floatEl.getAnimations()) a.cancel()
  }

  async function finishDrag() {
    const wasActive = dragActive
    const snapshot = orderBeforeDrag
    const opened = [...openedDuringDrag]
    const draggedId = dragFromId
    activePointerId = null
    document.body.classList.remove('dragging-projects')
    dropTargetGroupId = null
    openedDuringDrag = new Set()

    if (!wasActive || dragKind !== 'project') {
      dragActive = false
      dragKind = null
      dragFromId = null
      orderBeforeDrag = null
      dragSettling = false
      return
    }

    await settleFloatIntoPlaceholder()

    dragActive = false
    dragKind = null
    dragFromId = null
    orderBeforeDrag = null
    dragSettling = false
    queueMicrotask(updateTreeEdges)
    if (!snapshot) return

    const current = get(projects)
    const orderSame =
      current.length === snapshot.length &&
      current.every((p, i) => p.id === snapshot[i].id)
    const moved = current.filter((p) => {
      const prev = snapshot.find((x) => x.id === p.id)
      return !!prev && groupIdOf(prev) !== groupIdOf(p)
    })
    if (orderSame && !moved.length) {
      // No persist — re-collapse groups only opened transiently for targeting.
      for (const gid of opened) {
        groupCollapsed = { ...groupCollapsed, [gid]: true }
      }
      return
    }

    for (const p of moved) {
      const saved = await call(
        ProjectsService.SetProjectGroup(p.id, groupIdOf(p)),
        'Could not move project',
      )
      if (!saved) {
        await loadInitial()
        for (const gid of opened) {
          groupCollapsed = { ...groupCollapsed, [gid]: true }
        }
        return
      }
      projects.update((arr) =>
        arr.map((x) => (x.id === p.id ? { ...x, groupId: saved.groupId || '' } : x)),
      )
    }

    const dropped = get(projects).find((p) => p.id === draggedId)
    const landedGid = dropped ? groupIdOf(dropped) : ''
    // Keep only the drop-target group open; re-collapse pass-through expands.
    for (const gid of opened) {
      if (gid !== landedGid) {
        groupCollapsed = { ...groupCollapsed, [gid]: true }
        continue
      }
      const g = get(projectGroups).find((x) => x.id === gid)
      if (!g || g.open) continue
      const saved = await call(
        ProjectsService.UpdateProjectGroup({ ...g, open: true }),
        'Could not save group view',
      )
      if (saved) {
        projectGroups.update((arr) => arr.map((x) => (x.id === gid ? { ...x, open: true } : x)))
      }
    }

    const ok = await callOk(
      ProjectsService.ReorderProjects(get(projects).map((p) => p.id)),
      'Could not reorder projects',
    )
    if (!ok) await loadInitial()
  }

  async function finishGroupDrag() {
    const wasActive = dragActive
    const snapshot = groupOrderBeforeDrag
    activePointerId = null
    document.body.classList.remove('dragging-groups')

    if (!wasActive || dragKind !== 'group') {
      dragActive = false
      dragKind = null
      dragFromId = null
      groupOrderBeforeDrag = null
      dragSettling = false
      return
    }

    await settleFloatIntoPlaceholder()

    dragActive = false
    dragKind = null
    dragFromId = null
    groupOrderBeforeDrag = null
    dragSettling = false
    queueMicrotask(updateTreeEdges)
    if (!snapshot) return

    const current = get(projectGroups)
    const unchanged =
      current.length === snapshot.length &&
      current.every((g, i) => g.id === snapshot[i].id)
    if (unchanged) return

    const ok = await callOk(
      ProjectsService.ReorderProjectGroups(current.map((g) => g.id)),
      'Could not reorder groups',
    )
    if (!ok) projectGroups.set(snapshot)
  }

  function onProjHeaderClick(p: Project) {
    if (suppressProjClick) {
      suppressProjClick = false
      return
    }
    toggleProject(p)
  }

  function onGroupHeaderClick(g: ProjectGroup) {
    if (suppressGroupClick) {
      suppressGroupClick = false
      return
    }
    if (renamingGroupId === g.id) return
    toggleGroup(g)
  }

  // ---- group CRUD / rename / move menu ----
  let renamingGroupId = $state<string | null>(null)
  let renameDraft = $state('')
  let renameInputEl = $state<HTMLInputElement | null>(null)

  let moveMenu = $state<{ projectId: string; x: number; y: number } | null>(null)

  $effect(() => {
    if (!renamingGroupId || !renameInputEl) return
    renameInputEl.focus()
    renameInputEl.select()
  })

  async function addGroup() {
    const saved = await call(
      ProjectsService.AddProjectGroup(new ProjectGroup({ id: '', name: 'New group', open: true })),
      'Could not create group',
    )
    if (!saved) return
    projectGroups.update((arr) => (arr.some((g) => g.id === saved.id) ? arr : [...arr, saved]))
    groupCollapsed = { ...groupCollapsed, [saved.id]: false }
    renamingGroupId = saved.id
    renameDraft = saved.name
  }

  function startRenameGroup(g: ProjectGroup, e?: Event) {
    e?.stopPropagation()
    renamingGroupId = g.id
    renameDraft = g.name
  }

  async function commitRenameGroup() {
    const id = renamingGroupId
    if (!id) return
    const name = renameDraft.trim() || 'New group'
    renamingGroupId = null
    const g = get(projectGroups).find((x) => x.id === id)
    if (!g || g.name === name) return
    const saved = await call(
      ProjectsService.UpdateProjectGroup({ ...g, name }),
      'Could not rename group',
    )
    if (!saved) return
    projectGroups.update((arr) => arr.map((x) => (x.id === id ? { ...x, name: saved.name } : x)))
  }

  function cancelRenameGroup() {
    renamingGroupId = null
  }

  async function deleteGroup(g: ProjectGroup, e?: Event) {
    e?.stopPropagation()
    const prevGroups = get(projectGroups)
    const prevProjects = get(projects)
    projectGroups.update((arr) => arr.filter((x) => x.id !== g.id))
    projects.update((arr) => arr.map((p) => (p.groupId === g.id ? { ...p, groupId: '' } : p)))
    const ok = await callOk(ProjectsService.DeleteProjectGroup(g.id), 'Could not delete group')
    if (!ok) {
      projectGroups.set(prevGroups)
      projects.set(prevProjects)
    }
  }

  function openMoveMenu(p: Project, e: MouseEvent) {
    e.preventDefault()
    e.stopPropagation()
    moveMenu = { projectId: p.id, x: e.clientX, y: e.clientY }
  }

  function closeMoveMenu() {
    moveMenu = null
  }

  async function moveProjectToGroup(projectId: string, groupId: string) {
    closeMoveMenu()
    const prev = get(projects)
    projects.update((arr) => arr.map((p) => (p.id === projectId ? { ...p, groupId } : p)))
    // Keep display order coherent after membership change.
    const moved = get(projects).find((p) => p.id === projectId)
    if (moved) {
      const bucket = get(projects).filter((p) => groupIdOf(p) === groupId)
      projects.set(rebuildOrder(groupId, bucket))
    }
    const saved = await call(
      ProjectsService.SetProjectGroup(projectId, groupId),
      'Could not move project',
    )
    if (!saved) {
      projects.set(prev)
      return
    }
    projects.update((arr) => arr.map((p) => (p.id === projectId ? { ...p, groupId: saved.groupId || '' } : p)))
    const ok = await callOk(
      ProjectsService.ReorderProjects(get(projects).map((p) => p.id)),
      'Could not reorder projects',
    )
    // Membership may already be persisted; refetch instead of restoring a stale snapshot.
    if (!ok) await loadInitial()
  }

  $effect(() => {
    if (!moveMenu) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') closeMoveMenu() }
    const onDown = (e: MouseEvent) => {
      if ((e.target as HTMLElement | null)?.closest?.('.rail-move-menu')) return
      closeMoveMenu()
    }
    window.addEventListener('keydown', onKey)
    window.addEventListener('mousedown', onDown)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('mousedown', onDown)
    }
  })

  const unread = $derived($inbox.filter((i) => i.unread).length)
  const active = $derived(navKey($view))
  const uniqueGroups = $derived(uniqueById($projectGroups))
  const uniqueProjects = $derived(uniqueById($projects))
  const ungrouped = $derived(uniqueProjects.filter((p) => projectShown(p) && (!p.groupId || !uniqueGroups.some((g) => g.id === p.groupId))))
  const hiddenCount = $derived(uniqueProjects.filter((p) => p.hidden).length)
  const projLabel = $derived.by(() => {
    if ($projects.length === 0) return 'no projects'
    if ($projects.length === 1) return '1 project'
    return `${$projects.length} projects`
  })
  const prHostRunning = $derived(prChatHostRunningCount($workspaces, $workspacesRunning))
  const prHostUnseen = $derived(prChatHostUnseenCount($workspaces, $attention.byWorkspace))
  const visibleRunning = $derived.by(() => {
    const seen = new Set<string>()
    let n = 0
    for (const w of $workspaces) {
      if (w.archived || (w.hostPrId ?? '').trim() || seen.has(w.id)) continue
      seen.add(w.id)
      if (workspaceIsRunning(w, $workspacesRunning)) n++
    }
    return n
  })
  const runningLabel = $derived.by(() => {
    const n = visibleRunning
    return `${n} ${n === 1 ? 'workspace' : 'workspaces'} running`
  })
  const prRunningLabel = $derived.by(() => {
    const n = prHostRunning
    return `${n} ${n === 1 ? 'PR agent' : 'PR agents'} running`
  })
  const parkedByWs = $derived(parkedNeedsYouByWorkspace($workspaces, $userQuestions, $permissionRequests))
  const needsYou = $derived.by(() => {
    const seen = new Set<string>()
    let n = 0
    for (const w of $workspaces) {
      if (w.archived || (w.hostPrId ?? '').trim() || seen.has(w.id) || !(parkedByWs.get(w.id) ?? 0)) continue
      seen.add(w.id)
      n++
    }
    return n
  })
  const statusLabel = $derived(needsYou > 0 ? `${needsYou} ${needsYou === 1 ? 'needs' : 'need'} you` : runningLabel)
  const footerIsPRHosts = $derived(needsYou === 0 && visibleRunning === 0 && prHostRunning > 0)
  const footerLabel = $derived(footerIsPRHosts ? prRunningLabel : statusLabel)

  // Archived workspaces (removed) are hidden from the sidebar tree by default.
  // Main working copies get their own pinned row per project, so they never
  // appear in the task-workspace list. Remaining rows follow the live store
  // (newest first: ListWorkspaces + upsertWorkspace).
  const visibleLoops = $derived(uniqueById($workspaces.filter((l) => !l.archived && !l.isMain && !(l.hostPrId ?? '').trim())))

  const mainByProject = $derived.by(() => {
    const m: Record<string, Workspace> = {}
    for (const w of $workspaces) if (w.isMain) m[w.projectId] = w
    return m
  })

  async function openMain(p: Project, opts?: { unlessScanMoved?: number }) {
    // Re-stamp isGit so a mid-session `git init` reveals the sidebar "+" without reload.
    void refreshProjects()
    const ws = await call(WorkspacesService.OpenMainWorkspace(p.id), 'Could not open the main working copy')
    if (!ws) return
    workspaces.update((arr) =>
      arr.some((w) => w.id === ws.id) ? arr.map((w) => (w.id === ws.id ? ws : w)) : [...arr, ws],
    )
    if (opts?.unlessScanMoved != null && get(workspaceScanEvent).seq !== opts.unlessScanMoved) return
    openWorkspace(ws.id)
  }

  // Hover icons on task rows: View PR (when linked) + Remove (two-click).
  const REMOVE_ARM_MS = 3000
  let armedRemoveId = $state<string | null>(null)
  let armSeq = 0
  let armReadySeq = $state(0)
  let removeRiskById = $state<Record<string, string>>({})
  let removingId = $state<string | null>(null)
  let armTimer: ReturnType<typeof setTimeout> | undefined
  const removeEcho = { eatClick: false }
  const riskFetches = new Map<string, Promise<boolean>>()
  const riskFetchGen = new Map<string, number>()

  function clearArmedRemove() {
    if (armTimer) {
      clearTimeout(armTimer)
      armTimer = undefined
    }
    armedRemoveId = null
    armReadySeq = 0
  }

  function restartArmTimer(id: string) {
    if (armTimer) clearTimeout(armTimer)
    armTimer = setTimeout(() => {
      armTimer = undefined
      if (armedRemoveId === id) {
        armedRemoveId = null
        armReadySeq = 0
      }
    }, REMOVE_ARM_MS)
  }

  function dropRemoveRisk(id: string) {
    if (!(id in removeRiskById)) return
    const next = { ...removeRiskById }
    delete next[id]
    removeRiskById = next
  }

  /** Hover may reuse a result; arm always issues a fresh inspect. True only when this generation succeeded. */
  function fetchRemoveRisk(id: string, force = false): Promise<boolean> {
    if (!force) {
      const inflight = riskFetches.get(id)
      if (inflight) return inflight
      if (Object.prototype.hasOwnProperty.call(removeRiskById, id)) {
        return Promise.resolve(true)
      }
    }
    const gen = (riskFetchGen.get(id) ?? 0) + 1
    riskFetchGen.set(id, gen)
    const p = WorkspacesService.WorkspaceDiscardRisk(id)
      .then((risk) => {
        if (riskFetchGen.get(id) !== gen) return false
        removeRiskById = { ...removeRiskById, [id]: risk ?? '' }
        return true
      })
      .catch(() => {
        if (riskFetchGen.get(id) !== gen) return false
        dropRemoveRisk(id)
        return false
      })
      .finally(() => {
        if (riskFetches.get(id) === p) riskFetches.delete(id)
      })
    riskFetches.set(id, p)
    return p
  }

  function prefetchRemoveRisk(id: string) {
    void fetchRemoveRisk(id, false)
  }

  function armRemove(id: string) {
    const token = ++armSeq
    armedRemoveId = id
    armReadySeq = 0
    void fetchRemoveRisk(id, true).then((ok) => {
      if (token !== armSeq || armedRemoveId !== id) return
      if (!ok) {
        clearArmedRemove()
        return
      }
      armReadySeq = token
      restartArmTimer(id)
    })
  }

  function onViewPR(prId: string) {
    clearArmedRemove()
    openPR(prId)
  }

  function commitRemove(id: string) {
    clearArmedRemove()
    removingId = id
    dropRemoveRisk(id)
    void removeWorkspace(id).finally(() => {
      if (removingId === id) removingId = null
    })
  }

  function onRemovePointerUp(e: PointerEvent, id: string, can: boolean) {
    if (e.button !== 0 || !can || removingId || armedRemoveId !== id) return
    if (armReadySeq !== armSeq) return
    markClickEcho(removeEcho)
    commitRemove(id)
  }

  function onRemoveWorkspace(id: string, can: boolean) {
    if (takeClickEcho(removeEcho)) return
    if (!can || removingId) return
    if (armedRemoveId !== id) {
      armRemove(id)
      return
    }
    if (armReadySeq !== armSeq) return
    commitRemove(id)
  }

  function removeConfirmTitle(id: string): string {
    const risk = removeRiskById[id]
    return risk ? `Confirm remove — ${risk}` : 'Confirm remove'
  }

  function suppressRepeatKey(e: KeyboardEvent) {
    if (e.repeat) e.preventDefault()
  }

  let fadeTop = $state(false)
  let secFade = $state(false)
  let fadeBot = $state(false)
  let treeScrolling = $state(false)
  let thumbH = $state(0)
  let thumbTop = $state(0)
  let scrollHideTimer: ReturnType<typeof setTimeout> | undefined

  const STICKY_SLACK = 1.5
  const GROUP_STICKY_H = 26

  function updateTreeEdges() {
    const el = treeEl
    if (!el) return
    const { scrollTop, scrollHeight, clientHeight } = el
    const maxScroll = scrollHeight - clientHeight
    fadeTop = scrollTop > 1
    fadeBot = maxScroll > 1 && scrollTop < maxScroll - 1
    const track = Math.max(clientHeight, 1)
    const ratio = Math.min(1, track / Math.max(scrollHeight, 1))
    thumbH = Math.max(24, track * ratio)
    thumbTop = maxScroll > 0 ? (scrollTop / maxScroll) * (track - thumbH) : 0

    // Hierarchical under-fades: only the innermost stuck header fades content
    // beneath it; Projects (.sec) fades when nothing is stuck at the tree top.
    const treeTop = el.getBoundingClientRect().top
    let topLevelStuck = false
    el.querySelectorAll<HTMLElement>('.tgroup-h').forEach((header) => {
      const stuck = fadeTop && Math.abs(header.getBoundingClientRect().top - treeTop) <= STICKY_SLACK
      header.classList.toggle('is-stuck', stuck)
      if (stuck) topLevelStuck = true
    })
    el.querySelectorAll<HTMLElement>('.tproj-h').forEach((header) => {
      const inGroup = !!header.closest('.tgroup')
      const expected = treeTop + (inGroup ? GROUP_STICKY_H : 0)
      const stuck = fadeTop && Math.abs(header.getBoundingClientRect().top - expected) <= STICKY_SLACK
      header.classList.toggle('is-stuck', stuck)
      if (stuck && !inGroup) topLevelStuck = true
    })
    el.querySelectorAll<HTMLElement>('.tgroup-h').forEach((header) => {
      const group = header.closest('.tgroup')
      const childStuck = !!group?.querySelector('.tproj-h.is-stuck')
      header.classList.toggle('fade-under', header.classList.contains('is-stuck') && !childStuck)
    })
    el.querySelectorAll<HTMLElement>('.tproj-h').forEach((header) => {
      header.classList.toggle('fade-under', header.classList.contains('is-stuck'))
    })
    secFade = fadeTop && !topLevelStuck
  }

  function onTreeScroll() {
    updateTreeEdges()
    treeScrolling = true
    clearTimeout(scrollHideTimer)
    scrollHideTimer = setTimeout(() => {
      treeScrolling = false
    }, 700)
  }

  $effect(() => {
    void $projects
    void $projectGroups
    void collapsed
    void groupCollapsed
    void scheduledCollapsed
    void visibleLoops
    queueMicrotask(updateTreeEdges)
  })

  $effect(() => {
    const el = treeEl
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      if (get(railCollapsed) && !get(railPeek)) return
      updateTreeEdges()
    })
    ro.observe(el)
    return () => ro.disconnect()
  })

  // Collapsed-rail flyout: hover or tap the left edge to overlay the sidebar.
  let showPeekTimer: ReturnType<typeof setTimeout> | undefined
  let hidePeekTimer: ReturnType<typeof setTimeout> | undefined
  let edgeHover = $state(false)
  let cachedBody: RailPeekRect | null = null

  const peekLocked = $derived(railPeekHeld({
    collapsed: $railCollapsed,
    menuOpen: !!moveMenu || !!armedRemoveId,
    dragging: dragActive,
  }))

  function clearPeekTimers() {
    if (showPeekTimer) {
      clearTimeout(showPeekTimer)
      showPeekTimer = undefined
    }
    if (hidePeekTimer) {
      clearTimeout(hidePeekTimer)
      hidePeekTimer = undefined
    }
  }

  function showPeekNow() {
    clearPeekTimers()
    if (!$railCollapsed) return
    edgeHover = false
    openRailPeek()
  }

  function hidePeekNow(force = false) {
    if (!force && peekLocked) return
    clearPeekTimers()
    edgeHover = false
    closeRailPeek(force)
  }

  function scheduleShowPeek() {
    if (!$railCollapsed || $railPeek) return
    if (hidePeekTimer) {
      clearTimeout(hidePeekTimer)
      hidePeekTimer = undefined
    }
    if (showPeekTimer) return
    showPeekTimer = setTimeout(() => {
      showPeekTimer = undefined
      showPeekNow()
    }, RAIL_PEEK_SHOW_MS)
  }

  function scheduleHidePeek() {
    if (!$railCollapsed || peekLocked) return
    edgeHover = false
    if (showPeekTimer) {
      clearTimeout(showPeekTimer)
      showPeekTimer = undefined
    }
    if (!$railPeek || hidePeekTimer) return
    hidePeekTimer = setTimeout(() => {
      hidePeekTimer = undefined
      hidePeekNow()
    }, RAIL_PEEK_HIDE_MS)
  }

  function refreshBodyBox(): RailPeekRect | null {
    cachedBody = document.querySelector('.body')?.getBoundingClientRect() ?? null
    return cachedBody
  }

  function onPeekPointerMove(e: PointerEvent) {
    if (!$railCollapsed) return
    if (e.pointerType === 'touch') return
    if (cachedBody && pointerFarFromRailEdge(e.clientX, {
      peeking: $railPeek,
      showPending: !!showPeekTimer,
      locked: peekLocked,
    }, undefined, cachedBody.left)) return
    const box = cachedBody ?? refreshBodyBox()
    if (!box) return
    const inEdge = pointerInLeftEdge(e.clientX, e.clientY, box)
    edgeHover = inEdge && !$railPeek
    if (inEdge || isRailPeekSurface(e.target) || peekLocked) {
      scheduleShowPeek()
      if (hidePeekTimer) {
        clearTimeout(hidePeekTimer)
        hidePeekTimer = undefined
      }
      return
    }
    scheduleHidePeek()
  }

  function onPeekPointerDown(e: PointerEvent) {
    if (!$railCollapsed) return
    if (isRailPeekSurface(e.target)) {
      showPeekNow()
      return
    }
    if (e.target instanceof Element && e.target.closest('.tb-rail')) return
    if ($railPeek && !peekLocked) hidePeekNow(true)
  }

  function onHotzonePointerDown(e: PointerEvent) {
    e.preventDefault()
    showPeekNow()
  }

  function onPeekDocumentOut(e: PointerEvent) {
    if (!pointerLeftDocument(e)) return
    onPeekPointerOut()
  }

  function onPeekPointerOut() {
    if (!$railCollapsed || peekLocked) return
    edgeHover = false
    if (showPeekTimer) {
      clearTimeout(showPeekTimer)
      showPeekTimer = undefined
    }
    if ($railPeek) hidePeekNow()
  }

  $effect(() => {
    if (!$railCollapsed) {
      cachedBody = null
      edgeHover = false
      closeRailPeek(true)
      clearPeekTimers()
      return
    }
    refreshBodyBox()
    const el = document.querySelector('.body')
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => refreshBodyBox())
    ro.observe(el)
    return () => ro.disconnect()
  })

  $effect(() => {
    railPeekLocked.set(peekLocked)
  })

  $effect(() => {
    if (peekLocked && hidePeekTimer) {
      clearTimeout(hidePeekTimer)
      hidePeekTimer = undefined
    }
  })

  $effect(() => () => {
    clearTimeout(scrollHideTimer)
    clearPeekTimers()
    clearArmedRemove()
  })
</script>

{#snippet unreadCt(unseen: number, parked: number)}
  {@const n = unseen + parked}
  {#if n > 0}
    <span class="ct" title={sidebarUnreadTitle(unseen, parked)}>{n > 9 ? '9+' : n}</span>
  {/if}
{/snippet}

{#snippet loopRows(rows: Workspace[], nest = false)}
  {#each rows as l, li (l.id)}
    {@const wsPR = prForWorkspace($prs, l.id)}
    {@const wsPRLife = prLifecycle(wsPR) ?? (l.state === 'merged' ? 'merged' : null)}
    {@const canRm = workspaceCanRemove(l, $workspacesRunning.has(l.id))}
    {@const rmArmed = armedRemoveId === l.id}
    {@const nested = nest || isNestedChild(l, rows)}
    {@const nestedLast = nest
      ? li === rows.length - 1
      : nested && !(rows[li + 1] && rows[li + 1].parentId === l.parentId && isNestedChild(rows[li + 1], rows))}
    <div
      class="tloop"
      class:tchild={nested}
      class:tchild-last={nestedLast}
      class:on={taskRowOn(l.id)}
      class:merged={l.state === 'merged' || wsPRLife === 'merged'}
      class:is-armed={rmArmed}
      data-workspace-id={l.id}
      onpointerenter={() => { if (canRm) prefetchRemoveRisk(l.id) }}
      onpointerleave={() => { if (armedRemoveId === l.id) clearArmedRemove() }}
    >
      <button type="button" class="tloop-hit" onclick={() => openWorkspace(l.id)}>
        {#if workspaceIsRunning(l, $workspacesRunning)}
          <span class="sd pulse" title="Agent running" aria-label="Agent running"></span>
        {:else if wsPRLife}
          <Icon class="br pr-{wsPRLife}" name={wsPR ? prIconName(wsPR) : 'git-merge'} />
        {:else}
          <Icon class="br" name="git-branch" />
        {/if}
        <span class="tn"><DecodeText text={l.short} sourceKey={`${l.id}:short`} /></span>
        {#if nested && l.parentStepIds?.length}
          <span class="tstep" title="Plan step{l.parentStepIds.length === 1 ? '' : 's'} {l.parentStepIds.join(', ')} of the parent workspace">{l.parentStepIds.join(' ')}</span>
        {/if}
        <span class="tloop-tail">
          {#if l.diff.add || l.diff.del}
            <span class="df" title="+{l.diff.add} −{l.diff.del}"><span class="a">+{formatCompactCount(l.diff.add)}</span><span class="r">−{formatCompactCount(l.diff.del)}</span></span>
          {/if}
          {@render unreadCt($attention.byWorkspace[l.id] ?? 0, parkedByWs.get(l.id) ?? 0)}
        </span>
      </button>
      <span class="tloop-acts">
        {#if wsPR}
          <button
            type="button"
            class="lact lact-pr"
            title={wsPR.number ? `View PR #${wsPR.number}` : 'View PR'}
            aria-label={wsPR.number ? `View PR #${wsPR.number}` : 'View PR'}
            onclick={() => onViewPR(wsPR.id)}
            onkeydown={suppressRepeatKey}
          >
            <Icon name={prIconName(wsPR)} />
          </button>
        {/if}
        <button
          type="button"
          class="lact lact-rm"
          class:armed={rmArmed}
          class:off={!canRm}
          title={canRm ? (rmArmed ? removeConfirmTitle(l.id) : 'Remove') : 'Stop the workspace first'}
          aria-label={canRm ? (rmArmed ? removeConfirmTitle(l.id) : 'Remove') : 'Stop the workspace first'}
          disabled={!canRm}
          onpointerup={(e) => onRemovePointerUp(e, l.id, canRm)}
          onclick={() => onRemoveWorkspace(l.id, canRm)}
          onkeydown={suppressRepeatKey}
        >
          <Icon name="trash" />
        </button>
      </span>
    </div>
  {/each}
{/snippet}

{#snippet projectCard(p: Project, floating = false)}
  {@const split = splitScheduled(visibleLoops.filter((l) => l.projectId === p.id))}
  {@const items = nestWorkspaces(split.regular)}
  {@const scheduledItems = nestWorkspaces(split.scheduled)}
  {@const main = mainByProject[p.id]}
  {@const projUnseen = $attention.byProject[p.id] ?? 0}
  {@const mainUnseen = main ? ($attention.byWorkspace[main.id] ?? 0) : 0}
  {@const mainParked = main ? (parkedByWs.get(main.id) ?? 0) : 0}
  {@const projParked = mainParked + items.reduce((n, l) => n + (parkedByWs.get(l.id) ?? 0), 0) + scheduledItems.reduce((n, l) => n + (parkedByWs.get(l.id) ?? 0), 0)}
  {@const schedUnseen = scheduledItems.reduce((n, l) => n + ($attention.byWorkspace[l.id] ?? 0), 0)}
  {@const schedParked = scheduledItems.reduce((n, l) => n + (parkedByWs.get(l.id) ?? 0), 0)}
  {@const schedClosed = scheduledCollapsed[p.id] ?? true}
  {@const branchLabel = main?.branch || ''}
  <!-- Checkout branch only. Project.baseBranch is the worktree default (often origin/…) — not HEAD. -->
  {@const settingsOn = projectRowOn(p)}
  <div
    class="tproj"
    role="listitem"
    data-project-id={p.id}
    class:collapsed={collapsed[p.id]}
    class:is-hidden={p.hidden}
    class:is-float={floating}
    class:is-settling={floating && dragSettling}
    style={floating ? `left:${dragX}px;top:${dragY}px;width:${dragW}px` : undefined}
  >
    <button
      class="tproj-h"
      class:on={settingsOn}
      aria-current={settingsOn ? 'page' : undefined}
      onpointerdown={(e) => onHeaderPointerDown(e, p.id)}
      onclick={() => onProjHeaderClick(p)}
      oncontextmenu={(e) => openMoveMenu(p, e)}
    >
      <span class="tico">
        <ProjectPic class="pic" pic={p.pic} color={p.color} name={p.name} />
        <Icon class="car" name="chevron-down" strokeWidth={2.4} />
      </span>
      <span class="pn">{p.name}</span>
      {@render unreadCt(projUnseen, projParked)}
      <span class="pacts">
        {#if p.hidden}
          <span class="padd" title="Unhide {p.name}" aria-label="Unhide {p.name}" role="button" tabindex="0"
            onclick={(e) => { e.stopPropagation(); void setProjectHidden(p.id, false) }}
            onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); void setProjectHidden(p.id, false) } }}>
            <Icon name="eye" />
          </span>
        {/if}
        <span class="pset" title="Project settings" aria-label="Project settings for {p.name}" role="button" tabindex="0"
          onclick={(e) => { e.stopPropagation(); openProject(p.id) }}
          onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); openProject(p.id) } }}>
          <Icon name="settings" />
        </span>
        {#if p.isGit}
          <span class="padd" title="New schedule in {p.name}" aria-label="New schedule in {p.name}" role="button" tabindex="0"
            onclick={(e) => { e.stopPropagation(); scheduleEditor.set({ projectId: p.id }) }}
            onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); scheduleEditor.set({ projectId: p.id }) } }}>
            <Icon name="clock-plus" />
          </span>
          <span class="padd" title="New workspace in {p.name}" aria-label="New workspace in {p.name}" role="button" tabindex="0"
            onclick={(e) => { e.stopPropagation(); openNewTask(p.id) }}
            onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); openNewTask(p.id) } }}>
            <Icon name="plus" />
          </span>
        {/if}
      </span>
    </button>
    <div class="tloops">
      <button
        class="tloop tmain"
        class:on={mainRowOn(p, main)}
        data-workspace-id={main?.id ?? ''}
        title="Working copy — {p.name}'s real checkout"
        onclick={() => openMain(p)}
      >
        {#if main && workspaceIsRunning(main, $workspacesRunning)}
          <span class="sd pulse" title="Agent running" aria-label="Agent running"></span>
        {:else}
          <Icon class="br" name="house" />
        {/if}
        <span class="tn">Working copy</span>
        {#if branchLabel}<span class="tbranch">{branchLabel}</span>{/if}
        {#if main && (main.diff.add || main.diff.del)}
          <span class="df" title="+{main.diff.add} −{main.diff.del}"><span class="a">+{formatCompactCount(main.diff.add)}</span><span class="r">−{formatCompactCount(main.diff.del)}</span></span>
        {/if}
        {@render unreadCt(mainUnseen, mainParked)}
      </button>
      {#if scheduledItems.length}
        <div class="tsched" class:collapsed={schedClosed}>
          <button
            type="button"
            class="tsched-h"
            aria-expanded={!schedClosed}
            onclick={() => (scheduledCollapsed = { ...scheduledCollapsed, [p.id]: !schedClosed })}
          >
            <Icon class="gcar" name="chevron-down" strokeWidth={2.4} />
            <span class="tsched-lbl">Scheduled</span>
            {@render unreadCt(schedUnseen, schedParked)}
          </button>
          <div class="tsched-body">
            {@render loopRows(scheduledItems, true)}
          </div>
        </div>
      {/if}
      {#if items.length}
        {@render loopRows(items)}
      {/if}
      {#if !items.length && !scheduledItems.length && p.isGit}
        <div class="tproj-empty">no active workspaces</div>
      {/if}
    </div>
  </div>
{/snippet}

{#snippet projectBlock(p: Project)}
  {@const floating = dragActive && dragKind === 'project' && dragFromId === p.id}
  {#if floating}
    <div class="tproj-ph" style="height:{dragH}px" aria-hidden="true"></div>
  {:else}
    {@render projectCard(p)}
  {/if}
{/snippet}

{#snippet groupCard(g: ProjectGroup, floating = false)}
  {@const members = projectsByGroup.get(g.id) ?? []}
  {@const gUnseen = members.reduce((n, p) => n + ($attention.byProject[p.id] ?? 0), 0)}
  {@const gParked = $workspaces.reduce((n, w) => n + (!w.archived && !(w.hostPrId ?? '').trim() && members.some((p) => p.id === w.projectId) ? (parkedByWs.get(w.id) ?? 0) : 0), 0)}
  <div
    class="tgroup"
    class:collapsed={groupCollapsed[g.id]}
    class:drop-target={!floating && dragActive && dragKind === 'project' && dropTargetGroupId === g.id}
    class:is-float={floating}
    class:is-settling={floating && dragSettling}
    data-group-id={g.id}
    style={floating ? `left:${dragX}px;top:${dragY}px;width:${dragW}px` : undefined}
  >
    <button
      class="tgroup-h"
      onpointerdown={(e) => onGroupHeaderPointerDown(e, g.id)}
      onclick={() => onGroupHeaderClick(g)}
    >
      <Icon class="gcar" name="chevron-down" strokeWidth={2.4} />
      {#if renamingGroupId === g.id}
        <input
          class="gname-input"
          bind:this={renameInputEl}
          bind:value={renameDraft}
          aria-label="Group name"
          onclick={(e) => e.stopPropagation()}
          onkeydown={(e) => {
            if (e.key === 'Enter') { e.preventDefault(); void commitRenameGroup() }
            if (e.key === 'Escape') { e.preventDefault(); cancelRenameGroup() }
          }}
          onblur={() => void commitRenameGroup()}
        />
      {:else}
        <span class="gn">{g.name}</span>
      {/if}
      {#if groupCollapsed[g.id]}
        {@render unreadCt(gUnseen, gParked)}
      {/if}
      <span class="gacts">
        <span
          class="gact"
          title="Rename group"
          aria-label="Rename {g.name}"
          role="button"
          tabindex="0"
          onclick={(e) => startRenameGroup(g, e)}
          onkeydown={(e) => { if (e.key === 'Enter') startRenameGroup(g, e) }}
        >
          <Icon name="pencil" />
        </span>
        <span
          class="gact"
          title="Delete group"
          aria-label="Delete {g.name}"
          role="button"
          tabindex="0"
          onclick={(e) => void deleteGroup(g, e)}
          onkeydown={(e) => { if (e.key === 'Enter') void deleteGroup(g, e) }}
        >
          <Icon name="trash" />
        </span>
      </span>
    </button>
    {#if !groupCollapsed[g.id]}
      <div class="tgroup-body">
        {#each members as p (p.id)}
          {@render projectBlock(p)}
        {:else}
          <div class="tproj-empty">no projects</div>
        {/each}
      </div>
    {/if}
  </div>
{/snippet}

<svelte:window
  onpointermove={$railCollapsed ? onPeekPointerMove : undefined}
  onpointerdown={$railCollapsed ? onPeekPointerDown : undefined}
  onpointerout={$railCollapsed ? onPeekDocumentOut : undefined}
  onblur={$railCollapsed ? onPeekPointerOut : undefined}
/>

{#if $railCollapsed}
  <div
    class="rail-hotzone"
    class:on={$railPeek}
    class:near={edgeHover}
    style:--rail-hotzone-edge="{RAIL_PEEK_EDGE_PX}px"
    style:--rail-hotzone-coarse="{RAIL_PEEK_COARSE_EDGE_PX}px"
    role="presentation"
    aria-hidden="true"
    onpointerdown={onHotzonePointerDown}
  ></div>
{/if}

<aside
  class="rail"
  class:is-peek={$railPeek}
  aria-label="Sidebar"
  inert={$railCollapsed && !$railPeek}
  onpointerleave={scheduleHidePeek}
>
  <div class="rail-top">
    <button class="nav inbox-nav" class:on={active === 'briefing'} onclick={() => go('briefing')}>
      <Icon name="inbox" />Inbox<span class="ct" class:zero={unread === 0}>{unread}</span>
    </button>
    <button class="nav" class:on={active === 'pr'} onclick={() => go('pr')}>
      {#if prHostRunning > 0}
        <span class="sd pulse" title="Agent running" aria-label="Agent running"></span>
      {:else}
        <Icon name="git-pull-request" />
      {/if}
      Pull requests
      <span class="ct">{$prs.length}</span>
      {#if prHostUnseen > 0}
        <span
          class="ct unseen"
          title="{prHostUnseen} unseen agent response{prHostUnseen === 1 ? '' : 's'}"
          aria-label="{prHostUnseen} unseen agent response{prHostUnseen === 1 ? '' : 's'}"
        >{prHostUnseen > 9 ? '9+' : prHostUnseen}</span>
      {/if}
    </button>
    <button class="nav" class:on={active === 'schedules'} onclick={() => go('schedules')}>
      <Icon name="clock" />Schedules
      <span class="pacts">
        <span class="padd" title="New schedule" aria-label="New schedule" role="button" tabindex="0"
          onclick={(e) => { e.stopPropagation(); scheduleEditor.set({}) }}
          onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); scheduleEditor.set({}) } }}>
          <Icon name="plus" />
        </span>
      </span>
    </button>
  </div>

  <div class="rail-scroll" class:fade-top={secFade}>
    <div class="sec">
      <span class="st">Projects</span>
      <div class="sa">
        {#if hiddenCount > 0 || $settings?.showHiddenProjects}
          {@const label = $settings?.showHiddenProjects ? 'Hide hidden projects' : `Show ${hiddenCount} hidden project${hiddenCount === 1 ? '' : 's'}`}
          <button title={label} aria-label={label} aria-pressed={!!$settings?.showHiddenProjects} onclick={() => void toggleShowHiddenProjects()}>
            <Icon name={$settings?.showHiddenProjects ? 'eye' : 'eye-off'} />
          </button>
        {/if}
        <button title="Add group" aria-label="Add group" onclick={() => void addGroup()}><Icon name="folder-plus" /></button>
        <button title="Add project" aria-label="Add project" onclick={() => openProject('')}><Icon name="house-plus" /></button>
      </div>
    </div>
    <div class="rail-tree-shell" bind:this={treeShellEl}>
      <div
        id="tree"
        class="rail-tree"
        role="list"
        class:fade-bot={fadeBot}
        bind:this={treeEl}
        onscroll={onTreeScroll}
      >
        {#each uniqueGroups as g (g.id)}
          {@const groupFloating = dragActive && dragKind === 'group' && dragFromId === g.id}
          {#if groupFloating}
            <div class="tgroup-ph" style="height:{dragH}px" aria-hidden="true"></div>
          {:else}
            {@render groupCard(g)}
          {/if}
        {/each}

        {#each ungrouped as p (p.id)}
          {@render projectBlock(p)}
        {/each}
      </div>
      <!-- Drag floats sit outside .rail-tree so mask-image / transforms can't skew position:fixed. -->
      {#if dragActive && dragKind === 'project' && dragFromId}
        {@const fp = $projects.find((p) => p.id === dragFromId)}
        {#if fp}
          {@render projectCard(fp, true)}
        {/if}
      {/if}
      {#if dragActive && dragKind === 'group' && dragFromId}
        {@const fg = $projectGroups.find((g) => g.id === dragFromId)}
        {#if fg}
          {@render groupCard(fg, true)}
        {/if}
      {/if}
      <div
        class="rail-sb"
        class:on={treeScrolling && (fadeTop || fadeBot)}
        aria-hidden="true"
      >
        <div class="rail-sb-thumb" style="height:{thumbH}px;transform:translateY({thumbTop}px)"></div>
      </div>
    </div>
  </div>

  <div class="rail-foot">
    <div class="rail-foot-bar">
      <button onclick={() => go('settings')} title="Settings" aria-label="Settings"><Icon name="settings" /></button>
      <button title="Get started" aria-label="Get started" onclick={() => go('welcome')}><Icon name="help" /></button>
      <div class="sp"></div>
      <ProviderUsageMeter />
    </div>
    <div class="rail-foot-stats">
      <div class="rail-stat"><span>{projLabel}</span></div>
      {#if needsYou > 0}
        <button type="button" class="rail-stat needs" title="Open Inbox" aria-label="{footerLabel}. Open Inbox" onclick={() => go('briefing', { focus: 'needs' })}>
          <span>{footerLabel}</span>
        </button>
      {:else if footerIsPRHosts}
        <button type="button" class="rail-stat run" title="Open pull requests" aria-label="{footerLabel}. Open pull requests" onclick={() => go('pr')}>
          <span>{footerLabel}</span>
        </button>
      {:else}
        <div class="rail-stat" class:run={visibleRunning > 0}><span>{footerLabel}</span></div>
      {/if}
    </div>
  </div>
  <ResizeHandle panel="rail" edge="end" />
</aside>

{#if moveMenu}
  {@const cur = $projects.find((p) => p.id === moveMenu?.projectId)}
  <div
    class="rail-move-menu"
    style="left:{moveMenu.x}px;top:{moveMenu.y}px"
    role="menu"
  >
    <button type="button" role="menuitem" onclick={() => { void setProjectHidden(moveMenu!.projectId, !cur?.hidden); closeMoveMenu() }}>
      {cur?.hidden ? 'Unhide project' : 'Hide project'}
    </button>
    <div class="rail-move-label">Move to group</div>
    {#if cur?.groupId}
      <button type="button" role="menuitem" onclick={() => void moveProjectToGroup(moveMenu!.projectId, '')}>
        Ungrouped
      </button>
    {/if}
    {#each uniqueGroups.filter((g) => g.id !== cur?.groupId) as g (g.id)}
      <button type="button" role="menuitem" onclick={() => void moveProjectToGroup(moveMenu!.projectId, g.id)}>
        {g.name}
      </button>
    {:else}
      {#if !cur?.groupId}
        <div class="rail-move-empty">No groups yet</div>
      {/if}
    {/each}
  </div>
{/if}
