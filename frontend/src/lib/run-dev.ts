// Kick off a project's named run commands in Observatory terminals.
import { get } from 'svelte/store'
import { projects, setTerminalDockOpen, workspaces, pushToast, paletteOpen, commandsOpen, openInOpen, shortcutsOpen, syntaxModeOpen, runMenuOpen, stopMenuOpen } from './stores'
import { writable } from 'svelte/store'
import { Events } from '@wailsio/runtime'
import { InterruptTerminal } from '../../bindings/recur/internal/app/terminalservice'
import { ProjectsService, call, bridgeErrorMessage } from './api'
import { wsMainBlocked } from './modes'
import { defaultRun, projectRuns, type RunCommand } from './run-commands'
import type { Project } from '../../bindings/recur/internal/domain/models'

export type RunDevRequest = {
  workspaceId: string
  cmd: string
  name: string
  nonce: number
}

/** Latest pending "run" request; TerminalDock consumes matching ones. */
export const runDevRequest = writable<RunDevRequest | null>(null)

/**
 * Live session id per workspace per run name. In-memory only: the Go PTY
 * outlives a closed dock, so this survives unmount.
 */
export const runSessionBy = writable<Record<string, Record<string, string>>>({})

/**
 * Dedicated tab per run name. Survives stop so the next start of that
 * command types into the same live shell.
 */
export const runTabBy = writable<Record<string, Record<string, string>>>({})

/** In-flight InterruptTerminal per session. */
const inflightStop = new Map<string, Promise<void>>()
/** Ignore term.idle while Interrupt+Write is in flight so a leftover idle doesn't clear the new run. */
const startingRuns = new Set<string>()
/** Recur launched a named run on this session; term.busy may re-adopt it. */
const launchedRuns = new Set<string>()
const sawBusy = new Set<string>()
const busyWatch = new Map<string, number>()
let runEventsWired = false

export function markRunStarting(sessionId: string) {
  if (!sessionId) return
  startingRuns.add(sessionId)
  launchedRuns.add(sessionId)
  sawBusy.delete(sessionId)
  clearBusyWatch(sessionId)
}

export function markRunStarted(sessionId: string) {
  startingRuns.delete(sessionId)
}

function clearBusyWatch(sessionId: string) {
  const t = busyWatch.get(sessionId)
  if (t !== undefined) {
    clearTimeout(t)
    busyWatch.delete(sessionId)
  }
}

/** Drop the optimistic "running" state if watchIdle never saw a foreground command. */
export function armRunBusyWatch(sessionId: string) {
  if (!sessionId) return
  clearBusyWatch(sessionId)
  if (sawBusy.has(sessionId)) return
  busyWatch.set(sessionId, window.setTimeout(() => {
    busyWatch.delete(sessionId)
    if (!sawBusy.has(sessionId)) {
      launchedRuns.delete(sessionId)
      clearRunBySessionId(sessionId)
    }
    sawBusy.delete(sessionId)
  }, 2000))
}

function adoptLaunchedSession(sessionId: string) {
  if (!launchedRuns.has(sessionId)) return
  const tabs = get(runTabBy)
  for (const [ws, byName] of Object.entries(tabs)) {
    for (const [name, id] of Object.entries(byName)) {
      if (id === sessionId) {
        setRunSession(ws, name, sessionId)
        return
      }
    }
  }
}

export function wireRunEvents() {
  if (runEventsWired) return
  runEventsWired = true
  const onDone = (e: any) => {
    const id = e.data?.id
    if (typeof id !== 'string' || !id) return
    launchedRuns.delete(id)
    if (startingRuns.has(id)) return
    clearRunBySessionId(id)
  }
  Events.On('term.busy', (e: any) => {
    const id = e.data?.id
    if (typeof id !== 'string' || !id) return
    sawBusy.add(id)
    startingRuns.delete(id)
    clearBusyWatch(id)
    adoptLaunchedSession(id)
  })
  Events.On('term.idle', onDone)
  Events.On('term.exit', onDone)
}

export function projectForWorkspace(
  workspaceId: string,
  wsList = get(workspaces),
  projList = get(projects),
): Project | undefined {
  const ws = wsList.find((w) => w.id === workspaceId)
  if (!ws) return undefined
  return projList.find((p) => p.id === ws.projectId)
}

/** Pinned (or first) run command for a workspace, or null if unset. */
export function workspaceRun(
  workspaceId: string,
  wsList = get(workspaces),
  projList = get(projects),
): RunCommand | null {
  return defaultRun(projectForWorkspace(workspaceId, wsList, projList))
}

/** Resolve the default run command string, or "" if unset. */
export function workspaceDevCmd(
  workspaceId: string,
  wsList = get(workspaces),
  projList = get(projects),
): string {
  return workspaceRun(workspaceId, wsList, projList)?.cmd ?? ''
}

export function workspaceRuns(
  workspaceId: string,
  wsList = get(workspaces),
  projList = get(projects),
): RunCommand[] {
  return projectRuns(projectForWorkspace(workspaceId, wsList, projList))
}

export function runningRuns(
  workspaceId: string,
  sessions: Record<string, Record<string, string>> = get(runSessionBy),
): { name: string; sessionId: string }[] {
  const byName = sessions[workspaceId]
  if (!byName) return []
  return Object.entries(byName).map(([name, sessionId]) => ({ name, sessionId }))
}

export function isAnyRunRunning(
  workspaceId: string,
  sessions: Record<string, Record<string, string>> = get(runSessionBy),
): boolean {
  return runningRuns(workspaceId, sessions).length > 0
}

export function isRunRunning(
  workspaceId: string,
  name: string,
  sessions: Record<string, Record<string, string>> = get(runSessionBy),
): boolean {
  return !!name && !!sessions[workspaceId]?.[name]
}

export function runTabId(
  workspaceId: string,
  name: string,
  tabs: Record<string, Record<string, string>> = get(runTabBy),
): string {
  return tabs[workspaceId]?.[name] ?? ''
}

export function runNameForSession(
  workspaceId: string,
  sessionId: string,
  sessions: Record<string, Record<string, string>> = get(runSessionBy),
  tabs: Record<string, Record<string, string>> = get(runTabBy),
): string {
  if (!sessionId) return ''
  const running = sessions[workspaceId]
  if (running) {
    for (const [name, id] of Object.entries(running)) {
      if (id === sessionId) return name
    }
  }
  const dedicated = tabs[workspaceId]
  if (dedicated) {
    for (const [name, id] of Object.entries(dedicated)) {
      if (id === sessionId) return name
    }
  }
  return ''
}

export function setRunSession(workspaceId: string, name: string, sessionId: string) {
  if (!workspaceId || !name || !sessionId) return
  runSessionBy.update((cur) => {
    const prev = cur[workspaceId] ?? {}
    if (prev[name] === sessionId) return cur
    return { ...cur, [workspaceId]: { ...prev, [name]: sessionId } }
  })
  setRunTab(workspaceId, name, sessionId)
}

export function clearRunSession(workspaceId: string, name: string, sessionId?: string) {
  runSessionBy.update((cur) => {
    const prev = cur[workspaceId]
    if (!prev?.[name]) return cur
    if (sessionId && prev[name] !== sessionId) return cur
    const nextRuns = { ...prev }
    delete nextRuns[name]
    if (!Object.keys(nextRuns).length) {
      const next = { ...cur }
      delete next[workspaceId]
      return next
    }
    return { ...cur, [workspaceId]: nextRuns }
  })
}

export function clearRunBySessionId(sessionId: string) {
  if (!sessionId) return
  runSessionBy.update((cur) => {
    let changed = false
    const next: Record<string, Record<string, string>> = {}
    for (const [ws, runs] of Object.entries(cur)) {
      const kept: Record<string, string> = {}
      for (const [name, id] of Object.entries(runs)) {
        if (id === sessionId) {
          changed = true
          continue
        }
        kept[name] = id
      }
      if (Object.keys(kept).length) next[ws] = kept
    }
    return changed ? next : cur
  })
}

export function setRunTab(workspaceId: string, name: string, sessionId: string) {
  if (!workspaceId || !name || !sessionId) return
  runTabBy.update((cur) => {
    const prev = cur[workspaceId] ?? {}
    if (prev[name] === sessionId) return cur
    return { ...cur, [workspaceId]: { ...prev, [name]: sessionId } }
  })
}

export function clearRunTab(workspaceId: string, name: string, sessionId?: string) {
  runTabBy.update((cur) => {
    const prev = cur[workspaceId]
    if (!prev?.[name]) return cur
    if (sessionId && prev[name] !== sessionId) return cur
    const nextRuns = { ...prev }
    delete nextRuns[name]
    if (!Object.keys(nextRuns).length) {
      const next = { ...cur }
      delete next[workspaceId]
      return next
    }
    return { ...cur, [workspaceId]: nextRuns }
  })
}

export function clearRunTabBySessionId(workspaceId: string, sessionId: string) {
  if (!workspaceId || !sessionId) return
  runTabBy.update((cur) => {
    const prev = cur[workspaceId]
    if (!prev) return cur
    let changed = false
    const kept: Record<string, string> = {}
    for (const [name, id] of Object.entries(prev)) {
      if (id === sessionId) {
        changed = true
        continue
      }
      kept[name] = id
    }
    if (!changed) return cur
    if (!Object.keys(kept).length) {
      const next = { ...cur }
      delete next[workspaceId]
      return next
    }
    return { ...cur, [workspaceId]: kept }
  })
}

/**
 * Open the terminal dock and run a command (pinned default when run is omitted).
 * Does not stop other running commands.
 */
export function requestRunDev(workspaceId: string, run?: RunCommand | null): boolean {
  if (wsMainBlocked(get(workspaces).find((w) => w.id === workspaceId))) return false
  const chosen = run ?? workspaceRun(workspaceId)
  const cmd = chosen?.cmd?.trim() ?? ''
  if (!cmd) {
    pushToast('info', 'No run command — set one in project setup')
    return false
  }
  runMenuOpen.set(false)
  runDevRequest.set({
    workspaceId,
    cmd,
    name: chosen?.name?.trim() || 'dev',
    nonce: Date.now(),
  })
  setTerminalDockOpen(workspaceId, true)
  return true
}

/** Claim a pending request for this workspace (once). */
export function takeRunDevRequest(workspaceId: string): RunDevRequest | null {
  const req = get(runDevRequest)
  if (!req || req.workspaceId !== workspaceId) return null
  runDevRequest.set(null)
  return req
}

export function waitForSessionStop(sessionId: string): Promise<void> {
  return inflightStop.get(sessionId) ?? Promise.resolve()
}

function isMissingSession(err: unknown): boolean {
  return /terminal: session .+ not found/i.test(bridgeErrorMessage(err))
}

/**
 * Stop one running command by signalling its foreground process group.
 * The login shell stays; term.idle (or this promise) clears the running UI.
 */
export function requestStopRun(workspaceId: string, name: string): boolean {
  const sessionId = get(runSessionBy)[workspaceId]?.[name]
  if (!sessionId) return false
  return requestStopSession(workspaceId, name, sessionId)
}

export function requestStopSession(workspaceId: string, name: string, sessionId: string): boolean {
  if (!sessionId) return false
  if (inflightStop.has(sessionId)) return true
  stopMenuOpen.set(false)
  let p: Promise<void>
  p = InterruptTerminal(sessionId)
    .then(() => {
      clearRunSession(workspaceId, name, sessionId)
    })
    .catch((err: unknown) => {
      if (isMissingSession(err)) {
        clearRunSession(workspaceId, name, sessionId)
        return
      }
      pushToast('error', `Failed to stop ${name}: ${bridgeErrorMessage(err)}`)
    })
    .finally(() => {
      if (inflightStop.get(sessionId) === p) inflightStop.delete(sessionId)
    })
  inflightStop.set(sessionId, p)
  return true
}

export function requestStopAll(workspaceId: string): boolean {
  const runs = runningRuns(workspaceId)
  if (!runs.length) return false
  for (const r of runs) requestStopSession(workspaceId, r.name, r.sessionId)
  return true
}

export function openRunMenu(workspaceId: string): boolean {
  if (!workspaceId) return false
  if (wsMainBlocked(get(workspaces).find((w) => w.id === workspaceId))) return false
  if (!workspaceDevCmd(workspaceId)) return false
  stopMenuOpen.set(false)
  const next = !get(runMenuOpen)
  if (next) {
    paletteOpen.set(false)
    commandsOpen.set(false)
    openInOpen.set(false)
    shortcutsOpen.set(false)
    syntaxModeOpen.set(false)
  }
  runMenuOpen.set(next)
  return true
}

export function openStopMenu(workspaceId: string): boolean {
  if (!workspaceId) return false
  const runs = runningRuns(workspaceId)
  if (!runs.length) return false
  runMenuOpen.set(false)
  if (runs.length === 1) {
    stopMenuOpen.set(false)
    return requestStopRun(workspaceId, runs[0]!.name)
  }
  const next = !get(stopMenuOpen)
  if (next) {
    paletteOpen.set(false)
    commandsOpen.set(false)
    openInOpen.set(false)
    shortcutsOpen.set(false)
    syntaxModeOpen.set(false)
  }
  stopMenuOpen.set(next)
  return true
}

/** Persist the pinned Run-button default on the project (Recur DB, not toml). */
export async function pinProjectRun(project: Project, name: string): Promise<boolean> {
  const pinnedRun = name.trim()
  const saved = await call(
    ProjectsService.UpdateProjectRunConfig({ ...project, pinnedRun } as any),
    'Could not pin run command',
  )
  if (!saved) return false
  projects.update((list) => list.map((p) => (p.id === saved.id ? { ...p, ...saved } : p)))
  return true
}
