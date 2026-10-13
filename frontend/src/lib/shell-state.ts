/**
 * Last-open shell (workspace / inbox / PR / settings) across app restarts.
 *
 * The snapshot lives Go-side as <DataDir>/shell.json — same lifecycle as
 * window.json — so it survives webview storage wipes and a dev↔packaged
 * origin change. This module is the RPC client plus the restore resolver
 * (pure, so tests don't need the bridge).
 */

import { ShellService } from '../../bindings/recur/internal/app'
import { ShellState as BoundShellState } from '../../bindings/recur/internal/app/models'

export type ShellView =
  | 'detail' | 'settings' | 'project'
  | 'pr' | 'schedules' | 'issues' | 'prdetail' | 'inbox' | 'briefing' | 'welcome' | 'escalation'

export type ShellState = {
  view?: string
  workspaceId?: string
  prId?: string
  projectId?: string
}

export type ShellLive = {
  workspaceIds: ReadonlySet<string>
  prIds: ReadonlySet<string>
  projectIds: ReadonlySet<string>
}

export type ShellRestore =
  | { action: 'none' }
  | { action: 'workspace'; id: string }
  | { action: 'pr'; id: string }
  | { action: 'project'; id: string }
  | { action: 'view'; view: ShellView }

const TOP_VIEWS = new Set<ShellView>(['inbox', 'briefing', 'settings', 'pr', 'schedules', 'issues'])

/** Build the snapshot written on each navigation. */
export function snapshotShell(
  view: ShellView,
  workspaceId: string,
  prId: string,
  projectId: string,
): ShellState {
  switch (view) {
    case 'detail':
    case 'escalation':
      return { view, workspaceId }
    case 'prdetail':
      return { view, prId }
    case 'project':
      return { view, projectId }
    default:
      return { view }
  }
}

/**
 * Decide where to land after boot. Missing/archived targets fall through to
 * `none` (Briefing stays the in-memory default). Welcome is first-run only and
 * is never a restore target. A missing PR falls back to the
 * PR list rather than Briefing — the user was in that area.
 */
export function resolveShellRestore(
  saved: ShellState | null | undefined,
  live: ShellLive,
): ShellRestore {
  const view = (saved?.view ?? '').trim()
  if (!view) return { action: 'none' }
  if (view === 'detail' || view === 'escalation') {
    const id = (saved?.workspaceId ?? '').trim()
    if (id && live.workspaceIds.has(id)) return { action: 'workspace', id }
    return { action: 'none' }
  }
  if (view === 'prdetail') {
    const id = (saved?.prId ?? '').trim()
    if (id && live.prIds.has(id)) return { action: 'pr', id }
    return { action: 'view', view: 'pr' }
  }
  if (view === 'project') {
    const id = (saved?.projectId ?? '').trim()
    if (id && live.projectIds.has(id)) return { action: 'project', id }
    return { action: 'none' }
  }
  if (TOP_VIEWS.has(view as ShellView)) return { action: 'view', view: view as ShellView }
  return { action: 'none' }
}

export function applyShellRestore(
  result: ShellRestore,
  nav: {
    openWorkspace: (id: string) => void
    openPR: (id: string) => void
    openProject: (id: string) => void
    go: (v: ShellView) => void
  },
): void {
  switch (result.action) {
    case 'workspace':
      nav.openWorkspace(result.id)
      return
    case 'pr':
      nav.openPR(result.id)
      return
    case 'project':
      nav.openProject(result.id)
      return
    case 'view':
      nav.go(result.view)
      return
    default:
      return
  }
}

function fromBound(st: ShellState | null | undefined): ShellState | null {
  if (!st) return null
  return {
    view: st.view || undefined,
    workspaceId: st.workspaceId || undefined,
    prId: st.prId || undefined,
    projectId: st.projectId || undefined,
  }
}

export async function loadShellState(): Promise<ShellState | null> {
  try {
    return fromBound(await ShellService.GetShellState())
  } catch {
    return null
  }
}

let lastWritten = ''
let inFlight = false
let queued: ShellState | null = null

function persistable(st: ShellState): boolean {
  const view = (st.view ?? '').trim()
  // Welcome is first-run routing, not a restorable shell. Skip so a peek at
  // Get started does not clobber the last real view (and does not restore).
  return !!view && view !== 'welcome'
}

function writeShellState(st: ShellState): void {
  if (!persistable(st)) return
  const raw = JSON.stringify(st)
  if (raw === lastWritten) return
  if (inFlight) {
    queued = st
    return
  }
  inFlight = true
  void ShellService.SetShellState(new BoundShellState({
    view: st.view ?? '',
    workspaceId: st.workspaceId,
    prId: st.prId,
    projectId: st.projectId,
  })).then(() => {
    lastWritten = raw
  }).catch(() => {
    // Leave lastWritten unchanged so the next persist retries this state.
  }).finally(() => {
    inFlight = false
    if (queued) {
      const next = queued
      queued = null
      writeShellState(next)
    }
  })
}

/** Push the current shell to Go immediately. Disk coalescing + quit flush live there. */
export function persistShellState(st: ShellState): void {
  writeShellState(st)
}
