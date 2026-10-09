// Bridge between the Svelte stores and the bound Go services + runtime events.
// The frontend calls bound methods and subscribes to events — never reaches into
// the engine directly (PRD §4 frontend note).
import { Events } from '@wailsio/runtime'
import {
  ProjectsService, WorkspacesService, PRsService, PolicyService,
  SettingsService, InboxService, CodeService, ObservatoryService,
  AgentsService, PermissionsService, QuestionsService, ArtifactsService, EscalationService,
  TerminalService, BriefingService,
} from '../../bindings/recur/internal/app'
import {
  projectGroups, projects, workspaces, prs, inbox, briefing, attention, policy, settings, resource, providerUsage, pushToast, booted,
  workspacesRunning, agentsRunning, permissionRequests, userQuestions, applyTerminalDockChrome, instanceLabel,
  pendingChatFocus, issuesStale,
} from './stores'
import { get, writable } from 'svelte/store'
import { wireTerminalEvents } from './terminal'
import { wireLspEvents } from './lsp'
import { wireRunEvents } from './run-dev'
import { prefetchModelCatalog } from './models'
import { clearActiveChatCode, pruneActiveChatCodes } from './chat-tab-session'
import { dropEditorSession } from './editor-session'
import { pruneObservatorySnapshots } from './caches/observatory-cache'
import { pruneChatStrips } from './caches/chat-strip-cache'
import { pruneChatHeaps } from './caches/chat-heap-cache'
import { hoistCacheBustCodes, migrateEvictedHoistDrafts, noteHoistDestBust } from './hoist-chat-keys'
import { pruneSessionTabParks } from './caches/cm-session-tabs'
import { pruneParkedTerminals, releaseParkedTerminal } from './caches/term-park'
import { DetachTerminal } from '../../bindings/recur/internal/app/terminalservice'
import { Group, WorkspaceState } from '../../bindings/recur/internal/domain/models'
import type { Agent, PR, PendingLineComment, Project, Workspace, PermissionRequest, UserQuestion } from '../../bindings/recur/internal/domain/models'
import { uniqueById } from './workspace-nav'
import { keepCommitFiles } from './pr-commits'
import { keepMergedPR } from './colors'
import { pruneWorkspaceLand, dropWorkspaceLand } from './caches/workspace-land-cache'
import { dropFindingIdsRemovedFromPending } from './findings-ui'

let gitWarned = false
function noteGitWarning(res: { gitWarning?: string } | null | undefined) {
  const w = (res?.gitWarning ?? '').trim()
  if (!w || gitWarned) return
  gitWarned = true
  pushToast('error', w)
}

/** Pull a human message out of a Wails/bridge rejection (sometimes JSON-wrapped). */
export function bridgeErrorMessage(err: unknown): string {
  const raw =
    err && typeof err === 'object' && 'message' in err
      ? String((err as { message: unknown }).message)
      : String(err ?? '')
  if (raw.startsWith('{')) {
    try {
      const parsed = JSON.parse(raw) as { message?: unknown }
      if (typeof parsed.message === 'string' && parsed.message) return parsed.message
    } catch {
      /* not JSON */
    }
  }
  return raw
}

/** True when a ReadFile/Outline-style rejection is "path is a directory". */
export function isDirectoryError(err: unknown): boolean {
  return /\bis a directory\b/i.test(bridgeErrorMessage(err))
}

/** True when a recorded worktree path is gone (`code.ErrWorktreeMissing`).
 *  Matches the stable "worktree is not a directory" prefix — do not reword
 *  that sentinel without updating this and the Go Error() string together. */
export function isWorktreeMissing(err: unknown): boolean {
  return /worktree is not a directory/i.test(bridgeErrorMessage(err))
}

// Live DiffStat from workspace.updated, keyed by workspace id. ListWorkspaces
// returns a pre-refresh snapshot and schedules git async; an event can land
// before (or after) workspaces.set(list). Stashing lets setWorkspacesFromList
// keep the fresher +N/−M instead of clobbering it with the stale snapshot —
// and recovers events that arrived while the store was still empty on boot.
const liveDiffByWorkspace = new Map<string, { add: number; del: number }>()
/** Modal-cancel / hard-delete ids. Stale list/events must not resurrect them;
 *  CreateWorkspace reusing a kitchen slug clears the tombstone on upsert. */
const droppedWorkspaceIds = new Set<string>()

/** Apply a ListWorkspaces snapshot without wiping fresher live diff stats. */
export function setWorkspacesFromList(list: Workspace[] | null | undefined) {
  const incoming = uniqueById(list ?? [], true).filter((w) => !droppedWorkspaceIds.has(w.id))
  workspaces.set(
    incoming.map((w) => {
      const live = liveDiffByWorkspace.get(w.id)
      if (!live) return w
      return { ...w, diff: live }
    }),
  )
  pruneActiveChatCodes(incoming.map((w) => w.id))
}

/** Persist a project's sidebar hidden flag. */
export async function setProjectHidden(projectId: string, hidden: boolean) {
  const p = get(projects).find((x) => x.id === projectId)
  if (!p) return
  const saved = await call(ProjectsService.UpdateProjectRunConfig({ ...p, hidden } as any), 'Could not save project view')
  if (saved) projects.update((arr) => arr.map((x) => (x.id === projectId ? { ...x, hidden: saved.hidden } : x)))
}

/** Link a workspace to a tracker issue. Best effort: an abandoned pending
 *  workspace is already gone by the time this lands. The link shows at once;
 *  the Issues list refreshes once the backend has assigned the issue. */
export async function linkWorkspaceIssue(workspaceId: string, issue: { source: string; id: string; key?: string }) {
  if (!workspaceId || !issue?.source || !issue.id) return
  const ref = { key: '', ...issue }
  workspaces.update((arr) => arr.map((w) => (w.id === workspaceId ? { ...w, issue: ref } : w)))
  const ws = await WorkspacesService.SetWorkspaceIssue(workspaceId, ref).catch(() => null)
  if (ws) issuesStale.update((n) => n + 1)
}

/** Append a Linear project name / team key to a project's LinearProjects. */
export async function linkLinearProject(projectId: string, key: string) {
  const p = get(projects).find((x) => x.id === projectId)
  if (!p || !key) return
  const cur = (p.linearProjects ?? '').split(',').map((s) => s.trim()).filter(Boolean)
  if (cur.some((s) => s.toLowerCase() === key.toLowerCase())) return
  const linearProjects = [...cur, key].join(', ')
  const saved = await call(ProjectsService.UpdateProjectRunConfig({ ...p, linearProjects } as any), 'Could not link Linear project')
  if (saved) projects.update((arr) => arr.map((x) => (x.id === projectId ? { ...x, linearProjects: saved.linearProjects } : x)))
}

/** Fold one project into the live list (autosave / OpenPath). */
export function upsertProject(p: Project) {
  if (!p?.id) return
  projects.update((arr) => {
    const i = arr.findIndex((x) => x.id === p.id)
    if (i < 0) return [...arr, p]
    const next = [...arr]
    next[i] = { ...next[i], ...p }
    return next
  })
}

/** Matches engine githubPRSyncMinInterval so a dashboard revisit skips the RPC. */
const githubPRSyncMinMs = 30_000
let lastGitHubPRSync = 0

export function githubPRSyncDue(): boolean {
  return Date.now() - lastGitHubPRSync >= githubPRSyncMinMs
}

/** Merge a GitHub-sync snapshot. The payload is the persisted store, so server
 *  files/comments win; pendingComments is the only client-only field. */
export function setPRsFromSync(list: PR[]) {
  lastGitHubPRSync = Date.now()
  prs.update((arr) => {
    const prevById = new Map(arr.map((p) => [p.id, p]))
    return (list ?? []).map((p) => {
      const prev = prevById.get(p.id)
      if (!prev) return p
      return {
        ...p,
        workspaceId: p.workspaceId || prev.workspaceId,
        projectId: p.projectId || prev.projectId,
        comments: p.comments ?? prev.comments,
        files: p.files ?? prev.files,
        commits: keepCommitFiles(prev.commits, p.commits ?? []),
        pendingComments: prev.pendingComments,
      }
    })
  })
}

/** Insert a workspace the UI just created. If the row already exists, keep it —
 *  workspace.updated may have already patched worktreePath/phase, and a stale
 *  CreateWorkspace snapshot must not clobber that. */
const fetchingWorkspace = new Set<string>()
const refetchWorkspace = new Set<string>()

function fetchUnknownWorkspace(id: string) {
  if (!id || droppedWorkspaceIds.has(id)) return
  fetchingWorkspace.add(id)
  const pull = (): Promise<unknown> =>
    WorkspacesService.GetWorkspace(id).then((ws) => {
      if (refetchWorkspace.delete(id)) return pull()
      if (droppedWorkspaceIds.has(id)) return
      if (ws && !ws.archived) upsertWorkspace(ws)
    })
  void pull()
    .catch(() => {})
    .finally(() => fetchingWorkspace.delete(id))
}

export function upsertWorkspace(ws: Workspace) {
  if (!ws?.id || ws.archived) return
  droppedWorkspaceIds.delete(ws.id)
  const live = liveDiffByWorkspace.get(ws.id)
  const next = live ? { ...ws, diff: live } : ws
  workspaces.update((arr) => {
    if (arr.some((w) => w.id === next.id)) return arr
    return [next, ...arr]
  })
}

/** Drop a workspace from the live list immediately (delete/discard or modal cancel). */
export function dropWorkspace(id: string) {
  if (id) droppedWorkspaceIds.add(id)
  liveDiffByWorkspace.delete(id)
  workspaces.update((arr) => arr.filter((w) => w.id !== id))
  clearActiveChatCode(id)
  dropEditorSession(id)
  dropWorkspaceLand(id)
  const live = get(workspaces).map((w) => w.id)
  pruneObservatorySnapshots(live)
  pruneChatStrips(live)
  pruneChatHeaps(live)
  pruneSessionTabParks(live)
  pruneWorkspaceLand(live)
  dropParkedTerms(live)
}

/** Release per-workspace UI caches after Remove (worktree gone; archived row may remain). */
export function pruneWorkspaceClientState(id: string) {
  liveDiffByWorkspace.delete(id)
  clearActiveChatCode(id)
  dropEditorSession(id)
  dropWorkspaceLand(id)
  const live = get(workspaces)
    .filter((w) => w.id !== id && !w.archived)
    .map((w) => w.id)
  pruneObservatorySnapshots(live)
  pruneChatStrips(live)
  pruneChatHeaps(live)
  pruneSessionTabParks(live)
  pruneWorkspaceLand(live)
  dropParkedTerms(live)
}

function dropParkedTerms(keepWorkspaceIds: string[]) {
  for (const { id, payload } of pruneParkedTerminals(keepWorkspaceIds)) {
    releaseParkedTerminal(payload)
    DetachTerminal(id).catch(() => {})
  }
}

// call() wraps a bound-method promise so every fallible mutation gets a uniform
// error toast on rejection (Wails marshals a trailing Go `error` into a promise
// rejection) instead of failing silently. Returns null on failure so callers can
// bail with `if (result === null) return`.
export async function call<T>(p: Promise<T>, failMsg: string): Promise<T | null> {
  try { return await p }
  catch (err: unknown) { pushToast('error', `${failMsg}: ${bridgeErrorMessage(err)}`); return null }
}

// callOk is for bound methods with no meaningful success value (Go `error`-only
// returns, e.g. PauseLoop/StopLoop/DeleteProject). Wails resolves those with `null`
// on success, which is indistinguishable from call()'s null-on-failure sentinel —
// callOk instead reports success as a boolean so callers can't misread a
// null-but-successful response as a failure.
export async function callOk(p: Promise<unknown>, failMsg: string): Promise<boolean> {
  try { await p; return true }
  catch (err: unknown) { pushToast('error', `${failMsg}: ${bridgeErrorMessage(err)}`); return false }
}

// An agent counts as "running" for the sidebar indicator when it's mid-turn.
export function isAgentRunningStatus(status: string | undefined): boolean {
  return status === 'working' || status === 'directing' || status === 'planning' || status === 'replanning'
}

/** Synthetic agentsRunning id for an Ask Recur tab (`app-APP`, `app-AP2`, …). */
export function appChatAgentId(code: string): string {
  return 'app-' + (code || 'APP')
}

// Ask Recur has no workspace row. Bucket running ids here so seed/prune of
// real workspaces cannot wipe them, and so they never join workspacesRunning.
const APP_AGENTS_BUCKET = 'app'

function isAppChatAgentEvent(d: { workspaceId?: string; surface?: string; agentId?: string }): boolean {
  if (d.surface === 'app') return true
  return !d.workspaceId && typeof d.agentId === 'string' && d.agentId.startsWith('app-')
}

// Per-workspace set of agent IDs currently running. Derived into workspacesRunning.
const runningAgentIds = new Map<string, Set<string>>()
// Monotonic seq of agent.updated handling — seed snapshots must not clobber
// workspaces that received live events while ListLoopAgents was in flight.
let agentEventSeq = 0
const workspaceAgentEventSeq = new Map<string, number>()
let agentsRunningSeedGen = 0

function publishAgentsRunning() {
  const workspaceIds = new Set<string>()
  const agentIds = new Set<string>()
  for (const [workspaceId, agents] of runningAgentIds) {
    if (agents.size > 0 && workspaceId !== APP_AGENTS_BUCKET) workspaceIds.add(workspaceId)
    for (const agentId of agents) agentIds.add(agentId)
  }
  workspacesRunning.set(workspaceIds)
  agentsRunning.set(agentIds)
}

function setAgentRunning(workspaceId: string, agentId: string, running: boolean) {
  if (!workspaceId || !agentId) return
  agentEventSeq++
  workspaceAgentEventSeq.set(workspaceId, agentEventSeq)
  let set = runningAgentIds.get(workspaceId)
  if (!set) {
    if (!running) return
    set = new Set()
    runningAgentIds.set(workspaceId, set)
  }
  if (running) set.add(agentId)
  else set.delete(agentId)
  if (set.size === 0) runningAgentIds.delete(workspaceId)
  publishAgentsRunning()
}

function clearWorkspaceAgentsRunning(workspaceId: string) {
  if (!runningAgentIds.delete(workspaceId)) return
  workspaceAgentEventSeq.delete(workspaceId)
  publishAgentsRunning()
}

function applyAgentsRunningSnapshot(workspaceId: string, agents: Agent[]) {
  const set = new Set<string>()
  for (const a of agents) {
    if (isAgentRunningStatus(a.status) && a.id) set.add(a.id)
  }
  if (set.size) runningAgentIds.set(workspaceId, set)
  else runningAgentIds.delete(workspaceId)
}

// seedAgentsRunning hydrates the sidebar indicator from persisted agent status so a
// reload mid-turn shows the amber dot without waiting for the next event.
async function seedAgentsRunning(list: Workspace[]) {
  const gen = ++agentsRunningSeedGen
  const startedAt = agentEventSeq
  const visible = (list ?? []).filter((w) => !w.archived)
  const snaps = await Promise.all(visible.map(async (w) => {
    try {
      const agents = await AgentsService.ListLoopAgents(w.id)
      return { id: w.id, agents: agents ?? [] }
    } catch {
      return { id: w.id, agents: [] as Agent[] }
    }
  }))
  if (gen !== agentsRunningSeedGen) return // superseded by a newer loadInitial

  for (const { id, agents } of snaps) {
    // Live agent.updated during the fetch is newer than this snapshot — keep it.
    if ((workspaceAgentEventSeq.get(id) ?? 0) > startedAt) continue
    applyAgentsRunningSnapshot(id, agents)
  }
  // Drop indicators for workspaces that no longer exist (e.g. after ClearAll).
  const current = new Set((get(workspaces) ?? []).filter((w) => !w.archived).map((w) => w.id))
  for (const id of [...runningAgentIds.keys()]) {
    if (id === APP_AGENTS_BUCKET) continue
    if (!current.has(id)) {
      runningAgentIds.delete(id)
      workspaceAgentEventSeq.delete(id)
    }
  }
  publishAgentsRunning()
}

function applyAttention(raw: {
  unseen?: number
  inboxUnread?: number
  dock?: number
  appUnseen?: number
  byWorkspace?: Record<string, number | undefined> | null
  byProject?: Record<string, number | undefined> | null
} | null | undefined) {
  if (!raw) return
  const clean = (m: Record<string, number | undefined> | null | undefined) => {
    const out: Record<string, number> = {}
    if (!m) return out
    for (const [k, v] of Object.entries(m)) {
      if (typeof v === 'number' && v > 0) out[k] = v
    }
    return out
  }
  attention.set({
    unseen: raw.unseen ?? 0,
    inboxUnread: raw.inboxUnread ?? 0,
    dock: raw.dock ?? 0,
    appUnseen: raw.appUnseen ?? 0,
    byWorkspace: clean(raw.byWorkspace),
    byProject: clean(raw.byProject),
  })
}

let briefingRefreshTimer = 0
function scheduleBriefingRefresh() {
  if (briefingRefreshTimer) return
  briefingRefreshTimer = window.setTimeout(() => {
    briefingRefreshTimer = 0
    BriefingService.GetBriefing()
      .then((s) => { if (s) briefing.set(s) })
      .catch(() => {})
  }, 120)
}

function sessionWorkspaceId(d: { surface?: string; workspaceId?: string }): string | null {
  if (d.surface === 'app') return d.workspaceId ?? ''
  if (!d.workspaceId) return null
  return d.workspaceId
}

// refreshProjects re-stamps isGit (and the rest of the project list) without a
// full loadInitial — used when opening New workspace / Project so a mid-session
// `git init` (or a moved path) updates the sidebar + / picker without a reload.
// prefetchProviderUsage fills the sidebar meter after first paint. Collect can
// take seconds (and may prompt the keychain), so this stays off the boot await.
export function prefetchProviderUsage() {
  return SettingsService.GetProviderUsage()
    .then((s) => { if (s) providerUsage.set(s) })
    .catch(() => {})
}

/** Force a collect so a just-enabled provider is fetched, not served from cache. */
export function refreshProviderUsage() {
  return SettingsService.RefreshProviderUsage()
    .then((s) => { if (s) providerUsage.set(s) })
    .catch(() => {})
}

export async function refreshProjects() {
  try {
    const pj = await ProjectsService.ListProjects()
    projects.set(uniqueById(pj ?? [], true))
  } catch {
    /* best-effort; keep the last known list */
  }
}

// loadInitial pulls the seeded dataset from Go into the stores on boot.
export async function loadInitial() {
  try {
    const [pj, pg, lp, pr, ib, pol, st, res, att, dockChrome, label] = await Promise.all([
      ProjectsService.ListProjects(),
      ProjectsService.ListProjectGroups(),
      WorkspacesService.ListWorkspaces(),
      PRsService.ListPRs(),
      InboxService.GetInbox(),
      PolicyService.GetPolicy(),
      SettingsService.GetSettings(),
      SettingsService.GetResource(),
      WorkspacesService.AttentionCounts().catch(() => null),
      TerminalService.ListTerminalDockChrome().catch(() => []),
      SettingsService.GetInstanceLabel().catch(() => ''),
    ])
    projects.set(uniqueById(pj ?? [], true))
    projectGroups.set(uniqueById(pg ?? [], true))
    setWorkspacesFromList(lp)
    prs.set(pr ?? [])
    inbox.set(ib ?? [])
    applyAttention(att)
    policy.set(pol ?? null)
    settings.set(st ?? null)
    resource.set(res ?? null)
    instanceLabel.set(typeof label === 'string' ? label.trim() : '')
    noteGitWarning(res)
    applyTerminalDockChrome(dockChrome)
    // Warm the model catalogue so the solo-chat dock paints labels, not raw refs.
    prefetchModelCatalog()
    // Sidebar usage chips — Collect can take seconds; do not block first paint.
    void prefetchProviderUsage()
    // GitHub PRs from every Recur project — don't block first paint on gh.
    if (githubPRSyncDue()) {
      void PRsService.SyncGitHubPRs()
        .then((list) => { if (list) setPRsFromSync(list) })
        .catch(() => {})
    }
    // Sidebar running dots — best-effort; don't block first paint on agent lists.
    void seedAgentsRunning(lp ?? [])
    void BriefingService.GetBriefing()
      .then((s) => { if (s) briefing.set(s) })
      .catch(() => {})
    // Permission prompts only ever arrive as events, so a reload mid-turn would
    // otherwise lose them while the agent stays blocked. Rebuild the queue.
    PermissionsService.ListPendingPermissions()
      .then((q) => permissionRequests.set(q ?? []))
      .catch(() => {})
    QuestionsService.ListPendingQuestions()
      .then((q) => userQuestions.set(q ?? []))
      .catch(() => {})
    booted.set(true)
  } catch (err: any) {
    pushToast('error', `Failed to load Recur: ${err?.message ?? String(err)}`)
    booted.set(true) // unblock the UI so the user isn't stuck on skeletons
  }
}

// upsertPR merges a full (or partial) PR into the store — inserts when unknown so
// engine-created PRs appear in-session without a reload.
function upsertPR(partial: Partial<PR> & { id: string }) {
  prs.update((arr) => {
    const i = arr.findIndex((p) => p.id === partial.id)
    if (i < 0) return [...arr, partial as PR]
    const next = [...arr]
    const prev = next[i]
    next[i] = keepMergedPR(prev, {
      ...prev,
      ...partial,
      commits: keepCommitFiles(prev.commits, partial.commits ?? prev.commits),
    })
    return next
  })
}

/** Patch pending line comments locally so the diff/conversation update without GetPR. */
export const postedFindingIds = writable(new Set<string>())

export function markPostedFinding(id: string) {
  const k = id.trim()
  if (!k) return
  postedFindingIds.update((s) => (s.has(k) ? s : new Set([...s, k])))
}

export function forgetPostedFinding(id: string) {
  const k = id.trim()
  if (!k) return
  postedFindingIds.update((s) => {
    if (!s.has(k)) return s
    const n = new Set(s)
    n.delete(k)
    return n
  })
}

export function resetPostedFindings() {
  postedFindingIds.set(new Set())
}

export function patchPRPendingComments(
  prId: string,
  next: PendingLineComment[] | ((prev: PendingLineComment[]) => PendingLineComment[]),
) {
  prs.update((arr) =>
    arr.map((p) => {
      if (p.id !== prId) return p
      const prev = (p.pendingComments ?? []) as PendingLineComment[]
      const updated = typeof next === 'function' ? next(prev) : next
      postedFindingIds.update((s) => dropFindingIdsRemovedFromPending(s, prev, updated))
      return { ...p, pendingComments: updated }
    }),
  )
}

// fetchAndUpsertPR pulls the full PR from Go and inserts/updates the store. Used
// when pr.updated arrives for an unknown id (engine just opened a PR).
function fetchAndUpsertPR(prId: string) {
  PRsService.GetPR(prId)
    .then((full) => { if (full?.id) upsertPR(full) })
    .catch(() => {})
}

// wireEvents subscribes to every §15.2 event and keeps the stores live.
export function wireEvents() {
  // term.data / term.exit fan out to whichever pane owns each PTY session. Kept in
  // its own module because the payloads are base64 VT bytes on a hot path, not
  // store updates like everything below.
  wireTerminalEvents()
  wireLspEvents()
  wireRunEvents()

  // workspace.event — prepend an activity item to the relevant loop's feed (WorkspaceDetail
  // owns its own feed store; here we just bump the loop's "last" line for the lists).
  Events.On('workspace.event', (e: any) => {
    const { workspaceId, event } = e.data ?? {}
    if (!workspaceId || !event) return
    workspaces.update((arr) =>
      arr.map((l) => (l.id === workspaceId ? { ...l, last: stripTags(event.text ?? l.last) } : l)),
    )
    feedListeners.forEach((fn) => fn(workspaceId, event))
  })

  Events.On('workspace.updated', (e: any) => {
    const d = e.data ?? {}
    if (d.workspaceId && d.diff && typeof d.diff.add === 'number' && typeof d.diff.del === 'number') {
      liveDiffByWorkspace.set(d.workspaceId, { add: d.diff.add, del: d.diff.del })
    }
    // An engine-created row (fan-out child, scheduled prompt) has no client-side
    // create path: fetch it once so it appears in the sidebar without a reload.
    if (d.workspaceId && !get(workspaces).some((l) => l.id === d.workspaceId)) {
      if (fetchingWorkspace.has(d.workspaceId)) refetchWorkspace.add(d.workspaceId)
      else fetchUnknownWorkspace(d.workspaceId)
    }
    workspaces.update((arr) =>
      arr.map((l) =>
        l.id === d.workspaceId
          ? {
              ...l,
              state: d.state ?? l.state,
              phase: d.phase ?? l.phase,
              iteration: d.iteration ?? l.iteration,
              // Carry convergence[] through — persisted workspace state from the
              // engine (workspace.updated includes it on every tick).
              convergence: d.convergence ?? l.convergence,
              archived: d.archived ?? l.archived,
              // group/statusLabel/progress are persisted by the engine; without them
              // sidebar grouping/status can stay stale until a full reload.
              group: d.group ?? l.group,
              statusLabel: d.statusLabel ?? l.statusLabel,
              progress: d.progress ?? l.progress,
              // rename_workspace updates goal/short on workspace.updated.
              goal: d.goal ?? l.goal,
              short: d.short ?? l.short,
              // A filed/edited plan (submit_plan, approval, step updates) paints
              // the Plan panel live.
              plan: d.plan ?? l.plan,
              definitionOfDone: d.definitionOfDone ?? l.definitionOfDone,
              // Mid-session branch rename/checkout (human or agent).
              branch: d.branch ?? l.branch,
              baseBranch: d.baseBranch ?? l.baseBranch,
              // Live sidebar +N/−M (git numstat vs base / HEAD).
              diff: d.diff ?? l.diff,
              behindBase: d.behindBase ?? l.behindBase,
              // Eager create: worktree/port land after git+setup; lastError on failure.
              worktreePath: d.worktreePath !== undefined ? d.worktreePath : l.worktreePath,
              port: d.port !== undefined ? d.port : l.port,
              lastError: d.lastError !== undefined ? d.lastError : l.lastError,
              hostPrId: d.hostPrId !== undefined ? d.hostPrId : l.hostPrId,
              loopChatCode: d.loopChatCode !== undefined ? d.loopChatCode : l.loopChatCode,
              scheduleId: d.scheduleId !== undefined ? d.scheduleId : l.scheduleId,
            }
          : l,
      ),
    )
    if (d.workspaceId && typeof d.focusChatCode === 'string' && d.focusChatCode) {
      pendingChatFocus.set({ workspaceId: d.workspaceId, code: d.focusChatCode })
    }
    if (d.workspaceId && d.chatsChanged) {
      let detail: ChatsChangedDetail | undefined
      if (typeof d.fromWorkspaceId === 'string' && d.fromWorkspaceId) {
        const raw = d.codeRemap
        const codeRemap =
          raw && typeof raw === 'object' && !Array.isArray(raw)
            ? (raw as Record<string, string>)
            : {}
        const movedCodes = Array.isArray(d.movedCodes)
          ? (d.movedCodes as unknown[]).filter((x): x is string => typeof x === 'string' && !!x)
          : []
        detail = { fromWorkspaceId: d.fromWorkspaceId, codeRemap, movedCodes }
        noteHoistDestBust(d.workspaceId, hoistCacheBustCodes([], codeRemap, movedCodes))
        migrateEvictedHoistDrafts(d.fromWorkspaceId, d.workspaceId, codeRemap)
      }
      chatsChangedListeners.forEach((fn) => fn(d.workspaceId, detail))
    }
    scheduleBriefingRefresh()
  })

  // loop.deleted — a loop was permanently removed (Phase 3.4): drop it from the
  // live list so it disappears from every view without a full refetch.
  Events.On('workspace.deleted', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    dropWorkspace(d.workspaceId)
    clearWorkspaceAgentsRunning(d.workspaceId)
    scheduleBriefingRefresh()
  })

  // workspace.removed — worktree torn down, store row kept archived: prune
  // client caches bound to the deleted directory (same forget surface as delete).
  Events.On('workspace.removed', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    pruneWorkspaceClientState(d.workspaceId)
    clearWorkspaceAgentsRunning(d.workspaceId)
    scheduleBriefingRefresh()
  })

  // agent.step / agent.updated — the engine streams these as an agent works. Route
  // them to the per-loop agent listeners so WorkspaceDetail's inspector roster stays live.
  Events.On('agent.step', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    agentListeners.forEach((fn) => fn(d.workspaceId, { kind: 'step', agentId: d.agentId, step: d.step }))
  })

  Events.On('agent.updated', (e: any) => {
    const d = e.data ?? {}
    const app = isAppChatAgentEvent(d)
    if (!d.workspaceId && !app) return
    // Keep chat-tab busy + the sidebar amber indicator in sync with live status.
    // Ask Recur uses an empty workspaceId; bucket it so it cannot light a row.
    setAgentRunning(app ? APP_AGENTS_BUCKET : d.workspaceId, d.agentId, isAgentRunningStatus(d.status))
    if (app) return
    agentListeners.forEach((fn) => fn(d.workspaceId, { kind: 'updated', agentId: d.agentId, status: d.status }))
    scheduleBriefingRefresh()
  })

  Events.On('resource.updated', (e: any) => {
    resource.set(e.data ?? null)
    noteGitWarning(e.data)
  })

  Events.On('inbox.updated', () => {
    InboxService.GetInbox().then((ib) => inbox.set(ib ?? []))
    // Dock = unseen + non-chat inbox unread; refresh rollup so sidebar counts stay honest.
    WorkspacesService.AttentionCounts().then(applyAttention).catch(() => {})
    scheduleBriefingRefresh()
  })

  Events.On('attention.updated', (e: any) => {
    const d = e.data ?? {}
    applyAttention({
      unseen: d.unseen,
      inboxUnread: d.inboxUnread,
      dock: d.dock,
      appUnseen: d.appUnseen,
      byWorkspace: d.byWorkspace ?? {},
      byProject: d.byProject ?? {},
    })
    attentionListeners.forEach((fn) => fn({
      workspaceId: d.workspaceId ?? '',
      code: d.code ?? '',
      tabUnseen: !!d.tabUnseen,
      clearAllUnseen: !!d.clearAllUnseen,
    }))
    scheduleBriefingRefresh()
  })

  Events.On('pr.updated', (e: any) => {
    const d = e.data ?? {}
    if (!d.prId) return
    let known = false
    prs.update((arr) => {
      const i = arr.findIndex((p) => p.id === d.prId)
      if (i < 0) return arr
      known = true
      const next = [...arr]
      const prev = next[i]
      const patched = {
        ...prev,
        state: d.state ?? prev.state,
        group: d.group ?? prev.group,
        checksOk: d.checksOk ?? prev.checksOk,
        hasChecks: d.hasChecks ?? prev.hasChecks,
        reviewRequested: d.reviewRequested ?? prev.reviewRequested,
        attentionReasons: d.attentionReasons ?? prev.attentionReasons,
        participants: d.participants ?? prev.participants,
        reviewers: d.reviewers ?? prev.reviewers,
        fileCount: d.fileCount ?? prev.fileCount,
        title: d.title ?? prev.title,
        number: d.number ?? prev.number,
        when: d.when ?? prev.when,
        diff: d.diff ?? prev.diff,
        behindBase: d.behindBase ?? prev.behindBase,
        updatingBase: d.updatingBase !== undefined ? d.updatingBase : prev.updatingBase,
        branch: d.branch ?? prev.branch,
        base: d.base ?? prev.base,
        workspaceId: d.workspaceId ?? prev.workspaceId,
        projectId: d.projectId ?? prev.projectId,
        pendingComments: 'pendingComments' in d ? (d.pendingComments ?? []) : prev.pendingComments,
        draft: 'draft' in d ? !!d.draft : prev.draft,
        mergeable: d.mergeable ?? prev.mergeable,
        mergeStateStatus: d.mergeStateStatus ?? prev.mergeStateStatus,
        reviewDecision: d.reviewDecision ?? prev.reviewDecision,
        allowedMergeMethods: d.allowedMergeMethods ?? prev.allowedMergeMethods,
      }
      next[i] = keepMergedPR(prev, patched)
      return next
    })
    // Engine-created PRs aren't in the store yet — fetch the full record so the
    // list, sidebar count, inbox Review, and loop inspector all see it live.
    if (!known) fetchAndUpsertPR(d.prId)
    prListeners.forEach((fn) => fn(d.prId, d))
    // Pending-comment writes are local; skip GetPR (gh checks / conversation).
    if (!d.pendingOnly) prRefreshListeners.forEach((fn) => fn(d.prId))
    scheduleBriefingRefresh()
  })

  Events.On('pr.removed', (e: any) => {
    const id = e.data?.prId
    if (!id) return
    prs.update((arr) => arr.filter((p) => p.id !== id))
  })

  // pr.comment — a new thread entry (human / Maestro / Coder) during review
  // (PRD §12.3, §15.2). Append it to the PR's thread live so the conversation
  // updates without a refetch.
  Events.On('pr.comment', (e: any) => {
    const d = e.data ?? {}
    if (!d.prId || !d.comment) return
    prCommentListeners.forEach((fn) => fn(d.prId, d.comment))
  })

  // permission.requested — an agent is blocked mid-tool-call waiting for a yes/no
  // (Settings.bypassPermissions off). The agent subprocess is holding an open
  // request the whole time it sits in this queue, so surface it immediately.
  // A loop request also lands in the inbox (nobody is watching autopilot); a
  // chat request renders inline in AgentChat when that tab is focused, else the
  // floating PermissionPrompts card.
  Events.On('permission.requested', (e: any) => {
    const d = e.data as PermissionRequest | undefined
    if (!d?.id) return
    permissionRequests.update((q) => (q.some((r) => r.id === d.id) ? q : [...q, d]))
    if (d.surface === 'loop') {
      InboxService.GetInbox().then((ib) => inbox.set(ib ?? []))
    }
    scheduleBriefingRefresh()
  })

  // permission.resolved — the request left the queue, whether the human answered
  // it here, answered it in another window, or its turn ended underneath it.
  Events.On('permission.resolved', (e: any) => {
    const d = e.data ?? {}
    if (!d.id) return
    permissionRequests.update((q) => q.filter((r) => r.id !== d.id))
    InboxService.GetInbox().then((ib) => inbox.set(ib ?? []))
    scheduleBriefingRefresh()
  })

  // question.asked — interactive ask_user (park + idle chat + inbox; workspace stays usable).
  Events.On('question.asked', (e: any) => {
    const d = e.data ?? {}
    const q = d.question as UserQuestion | undefined
    if (!q?.id) return
    userQuestions.update((arr) => (arr.some((x) => x.id === q.id) ? arr : [...arr, q]))
    InboxService.GetInbox().then((ib) => inbox.set(ib ?? []))
    scheduleBriefingRefresh()
  })

  Events.On('question.answered', (e: any) => {
    const d = e.data ?? {}
    if (!d.id) return
    userQuestions.update((q) => q.filter((x) => x.id !== d.id))
    InboxService.GetInbox().then((ib) => inbox.set(ib ?? []))
    scheduleBriefingRefresh()
  })

  Events.On('chat.mode_changed', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId || !d.agentCode || !d.mode) return
    // WorkspaceView / AgentChat listen via onChatModeChanged if needed; also refresh inbox unused.
    chatModeListeners.forEach((fn) => fn(d.workspaceId, d.agentCode, d.mode))
  })

  Events.On('artifact.presented', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId || !d.artifact) return
    artifactPresentListeners.forEach((fn) => fn(d.workspaceId, d.artifact))
  })

  Events.On('artifact.created', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId || !d.artifact) return
    artifactCreatedListeners.forEach((fn) => fn(d.workspaceId, d.artifact))
  })

  // escalation.raised — a loop paused to ask (PRD §13). Route it to "needs you"
  // (sidebar status), surface the live escalation, and refresh the inbox so the
  // actionable row appears. The question itself renders inline in Loop chat —
  // stay on this workspace's detail if that's already on screen.
  Events.On('escalation.raised', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    workspaces.update((arr) =>
      arr.map((l) =>
        l.id === d.workspaceId
          ? { ...l, group: Group.GroupNeeds, state: WorkspaceState.StateEscalated, statusLabel: 'Needs you', phase: 'asking a question' }
          : l,
      ),
    )
    InboxService.GetInbox().then((ib) => inbox.set(ib ?? []))
    escRaisedListeners.forEach((fn) => fn(d.workspaceId, d.escalation))
    scheduleBriefingRefresh()
  })

  // escalation.resolved — the human answered; the loop leaves "needs you" and
  // resumes (PRD §13). Clear escalated state live (don't wait for the next
  // workspace.updated) so Pause/Resume aren't stuck behind a stale "Needs you".
  Events.On('escalation.resolved', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    if (!d.open) {
      workspaces.update((arr) =>
        arr.map((l) =>
          l.id === d.workspaceId
            ? {
                ...l,
                group: Group.GroupActive,
                state: l.state === WorkspaceState.StateEscalated ? WorkspaceState.StateCoding : l.state,
                statusLabel: l.state === WorkspaceState.StateEscalated ? 'Working' : l.statusLabel,
                phase: l.state === WorkspaceState.StateEscalated ? 'resuming' : l.phase,
              }
            : l,
        ),
      )
    }
    InboxService.GetInbox().then((ib) => inbox.set(ib ?? []))
    escListeners.forEach((fn) => fn(d.workspaceId, d.answer))
    scheduleBriefingRefresh()
  })

  // session.delta — assistant text for ANY agent's live session. Payloads are
  // cumulative snapshots + monotonic seq (not raw chunks). Wails v3 (beta.9+)
  // delivers frontend events in emit order; seq still lets the UI drop a
  // stale last-seen snapshot and refuse a later turn that restarted at 1.
  // replace:true = snapshot (set body); done:true = block finished (close row).
  Events.On('session.delta', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.text !== 'string' || !d.text) return
    sessionDeltaListeners.forEach((fn) => fn(workspaceId, d.code ?? '', {
      text: d.text,
      seq: typeof d.seq === 'number' ? d.seq : 0,
      replace: !!d.replace,
      done: !!d.done,
      parentId: typeof d.parentId === 'string' && d.parentId ? d.parentId : undefined,
      uuid: typeof d.uuid === 'string' ? d.uuid : undefined,
      ownerCode: typeof d.ownerCode === 'string' ? d.ownerCode : undefined,
    }))
  })

  // session.step — a tool call an agent just made, streamed live so a tool-heavy
  // turn shows activity instead of a blank bubble. Carries the agent's own
  // description + full input for the detail card.
  Events.On('session.step', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.tool !== 'string') return
    sessionStepListeners.forEach((fn) => fn(workspaceId, d.code ?? '', {
      id: d.id ?? '', tool: d.tool, toolInput: d.toolInput ?? '',
      description: d.description ?? '', input: d.input ?? '',
      parentId: typeof d.parentId === 'string' ? d.parentId : '',
      uuid: typeof d.uuid === 'string' ? d.uuid : undefined,
      ownerCode: typeof d.ownerCode === 'string' ? d.ownerCode : undefined,
    }))
  })

  // workspace.fs_changed — the watched worktree changed on disk (agent turn,
  // external editor, terminal, git — anything). The Observatory refreshes off
  // this single trigger. Optional paths/rescan feed the file index.
  Events.On('workspace.fs_changed', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    const paths = Array.isArray(d.paths) ? d.paths.map((p: unknown) => String(p)) : undefined
    workspaceFsListeners.forEach((fn) => fn(d.workspaceId, paths, !!d.rescan))
  })

  // workspace.file_index — readiness only (paths stay in Go; SearchFiles ranks).
  Events.On('workspace.file_index', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    const payload: FileIndexPayload = {
      workspaceId: d.workspaceId,
      gen: typeof d.gen === 'number' ? d.gen : 0,
      ready: !!d.ready,
      building: !!d.building,
    }
    fileIndexListeners.forEach((fn) => fn(payload))
  })

  // app.focused — OS window regained focus (file-index backstop runs in Go).
  Events.On('app.focused', (e: any) => {
    const d = e.data ?? {}
    appFocusedListeners.forEach((fn) => fn(typeof d.workspaceId === 'string' ? d.workspaceId : ''))
  })

  // session.tool_result — a tool call's result, matched to the call by id so the
  // expanded card fills in live. May also carry late input/toolInput (Codex MCP).
  Events.On('session.tool_result', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.id !== 'string') return
    const extras = (typeof d.input === 'string' && d.input) || (typeof d.toolInput === 'string' && d.toolInput)
      ? {
          input: typeof d.input === 'string' ? d.input : undefined,
          toolInput: typeof d.toolInput === 'string' ? d.toolInput : undefined,
        }
      : undefined
    sessionToolResultListeners.forEach((fn) => fn(workspaceId, d.code ?? '', d.id, d.result ?? '', !!d.isError, extras))
  })

  // session.tool_input — args-only refresh (ask_user park backfill when Codex
  // stored {}). Does not settle the tool result.
  Events.On('session.tool_input', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.id !== 'string') return
    const extras = {
      input: typeof d.input === 'string' ? d.input : undefined,
      toolInput: typeof d.toolInput === 'string' ? d.toolInput : undefined,
    }
    if (!extras.input && !extras.toolInput) return
    sessionToolInputListeners.forEach((fn) => fn(workspaceId, d.code ?? '', d.id, extras))
  })

  // session.compact — provider is summarizing (or finished summarizing) context.
  Events.On('session.compact', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.status !== 'string') return
    const evt: SessionCompact = {
      status: d.status,
      trigger: typeof d.trigger === 'string' ? d.trigger : undefined,
      summary: typeof d.summary === 'string' ? d.summary : undefined,
      preTokens: typeof d.preTokens === 'number' ? d.preTokens : undefined,
    }
    sessionCompactListeners.forEach((fn) => fn(workspaceId, d.code ?? '', evt))
  })

  // session.notice — Recur status (e.g. draining a leftover Cursor run). Not
  // assistant text; must not be folded into session.delta.
  Events.On('session.notice', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.text !== 'string' || !d.text.trim()) return
    sessionNoticeListeners.forEach((fn) => fn(workspaceId, d.code ?? '', d.text))
  })

  // session.user — mid-turn steer (Cursor D49 follow-up) so the live transcript
  // shows the Maestro message instead of only after a reload.
  Events.On('session.user', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.text !== 'string' || !d.text) return
    sessionUserListeners.forEach((fn) => fn(workspaceId, d.code ?? '', d.text))
  })

  // session.fallback — Claude Fable (or similar) declined; another model continued.
  Events.On('session.fallback', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null) return
    const retracted = Array.isArray(d.retracted)
      ? d.retracted.filter((u: unknown) => typeof u === 'string' && u)
      : undefined
    if ((typeof d.to !== 'string' || !d.to) && !retracted?.length) return
    const evt: SessionFallback = {
      from: typeof d.from === 'string' ? d.from : undefined,
      to: typeof d.to === 'string' ? d.to : undefined,
      fromLabel: typeof d.fromLabel === 'string' ? d.fromLabel : undefined,
      toLabel: typeof d.toLabel === 'string' ? d.toLabel : undefined,
      retracted,
    }
    sessionFallbackListeners.forEach((fn) => fn(workspaceId, d.code ?? '', evt))
  })

  // provider.usage — subscription-quota snapshot for the sidebar footer.
  // Connect may mint a user API key into Settings after browser login.
  Events.On('provider.usage', (e: any) => {
    const d = e.data ?? e
    if (d && typeof d === 'object') providerUsage.set(d)
    const cursor = Array.isArray(d?.providers)
      ? d.providers.find((p: { id?: string; sourceId?: string }) => p?.id === 'cursor')
      : null
    if (cursor?.sourceId === 'cursor_connect' && !get(settings)?.cursorApiKey) {
      void SettingsService.GetSettings().then((s) => { if (s) settings.set(s) })
      void prefetchModelCatalog()
    }
  })

  // session.context_usage — mid-turn (or final) context fill for the composer meter.
  Events.On('session.context_usage', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null) return
    const usage = contextUsagePayload(d)
    if (!usage) return
    sessionContextUsageListeners.forEach((fn) => fn(workspaceId, d.code ?? '', usage))
  })

  // session.turn_end — an agent turn finished, with the provider's own wall time
  // so the live footer matches the one after a reload. May also carry final
  // context usage so the composer meter reconciles.
  Events.On('session.turn_end', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null || typeof d.durationMs !== 'number') return
    const turn: SessionTurnEnd = { durationMs: d.durationMs }
    const usage = contextUsagePayload(d)
    if (usage) {
      turn.inputTokens = usage.inputTokens
      turn.cacheCreationTokens = usage.cacheCreationTokens
      turn.cacheReadTokens = usage.cacheReadTokens
      turn.contextWindow = usage.contextWindow
    }
    sessionTurnEndListeners.forEach((fn) => fn(workspaceId, d.code ?? '', turn))
  })

  // workspace.chat_tab — solo/pair Agent tab title (provisional or LLM upgrade).
  Events.On('workspace.chat_tab', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId || typeof d.code !== 'string' || typeof d.title !== 'string') return
    workspaceChatTabListeners.forEach((fn) => fn(d.workspaceId, d.code, d.title))
  })

  // findings.updated — a reviewer (loop iteration or Review-mode chat) filed a
  // finding; the Observatory findings panel refetches.
  Events.On('findings.updated', (e: any) => {
    const d = e.data ?? {}
    if (!d.workspaceId) return
    findingsListeners.forEach((fn) => fn(d.workspaceId))
    scheduleBriefingRefresh()
  })

  // github.branch_rules — background re-probe of whether a remote requires a
  // PR. The titlebar Merge button patches requirePR without waiting on git.
  Events.On('github.branch_rules', (e: any) => {
    const d = e.data ?? {}
    if (typeof d.projectId !== 'string' || typeof d.branch !== 'string' || typeof d.requirePR !== 'boolean') return
    const remote = typeof d.remote === 'string' ? d.remote : undefined
    githubBranchRulesListeners.forEach((fn) => fn(d.projectId, d.branch, d.requirePR, remote))
  })

  // session.thinking — same cumulative+seq snapshot model as session.delta.
  // Token-only events cover redacted Claude thinking.
  Events.On('session.thinking', (e: any) => {
    const d = e.data ?? {}
    const workspaceId = sessionWorkspaceId(d)
    if (workspaceId === null) return
    const text = typeof d.text === 'string' ? d.text : ''
    const tokens = typeof d.tokens === 'number' ? d.tokens : 0
    if (!text && !tokens) return
    sessionThinkingListeners.forEach((fn) => fn(workspaceId, d.code ?? '', {
      text,
      tokens,
      seq: typeof d.seq === 'number' ? d.seq : 0,
      replace: !!d.replace,
      done: !!d.done,
      parentId: typeof d.parentId === 'string' && d.parentId ? d.parentId : undefined,
      uuid: typeof d.uuid === 'string' ? d.uuid : undefined,
      ownerCode: typeof d.ownerCode === 'string' ? d.ownerCode : undefined,
    }))
  })

  Events.On('briefing.updated', () => {
    scheduleBriefingRefresh()
  })
}

function stripTags(html: string): string {
  return html.replace(/<[^>]+>/g, '')
}

// ---- lightweight pub/sub so view components can react to live events ----
type FeedFn = (workspaceId: string, event: any) => void
type PRFn = (prId: string, data: any) => void
type PRCommentFn = (prId: string, comment: any) => void
type PRRefreshFn = (prId: string) => void
type EscFn = (workspaceId: string, answer: string) => void
type EscRaisedFn = (workspaceId: string, escalation: any) => void
// AgentEvt carries either a streamed step or a status change for a loop's agent.
export type AgentEvt =
  | { kind: 'step'; agentId: string; step: any }
  | { kind: 'updated'; agentId: string; status: string }
type AgentFn = (workspaceId: string, evt: AgentEvt) => void
// Session stream listeners: every agent turn broadcasts (workspaceId, code)-tagged
// events from the engine's mirror, so any surface can follow any agent live.
// Live text snapshot: cumulative body + seq. replace = set body (not append);
// done = close the streaming row. Stale seqs must be ignored by the UI.
export type WorkspaceDelta = { text: string; seq?: number; replace?: boolean; done?: boolean; parentId?: string; uuid?: string; ownerCode?: string }
type SessionDeltaFn = (workspaceId: string, code: string, evt: WorkspaceDelta) => void
export type WorkspaceStep = {
  id: string
  tool: string
  toolInput: string
  description: string
  input: string
  /** Provider tool_use id of a parent Task when this call is a nested subagent tool. */
  parentId?: string
  uuid?: string
  ownerCode?: string
}
type SessionStepFn = (workspaceId: string, code: string, step: WorkspaceStep) => void
// Thinking snapshot — same shape as WorkspaceDelta, plus optional token estimate
// when the CLI redacts reasoning text.
export type WorkspaceThinking = {
  text: string
  tokens: number
  seq?: number
  replace?: boolean
  done?: boolean
  parentId?: string
  uuid?: string
  ownerCode?: string
}
type SessionThinkingFn = (workspaceId: string, code: string, evt: WorkspaceThinking) => void
type SessionToolResultFn = (
  workspaceId: string,
  code: string,
  id: string,
  result: string,
  isError: boolean,
  extras?: { input?: string; toolInput?: string },
) => void
type SessionToolInputFn = (
  workspaceId: string,
  code: string,
  id: string,
  extras: { input?: string; toolInput?: string },
) => void
/** Context fill for the composer meter (mid-turn or final). */
export type SessionContextUsage = {
  inputTokens: number
  cacheCreationTokens: number
  cacheReadTokens: number
  contextWindow: number
}

/** Turn-end payload: wall time plus optional context usage for the meter. */
export type SessionTurnEnd = {
  durationMs: number
  inputTokens?: number
  cacheCreationTokens?: number
  cacheReadTokens?: number
  contextWindow?: number
}
export type SessionCompact = {
  status: string
  trigger?: string
  summary?: string
  preTokens?: number
}
export type SessionFallback = {
  from?: string
  to?: string
  fromLabel?: string
  toLabel?: string
  retracted?: string[]
}
type SessionContextUsageFn = (workspaceId: string, code: string, usage: SessionContextUsage) => void
type SessionTurnEndFn = (workspaceId: string, code: string, turn: SessionTurnEnd) => void
type SessionCompactFn = (workspaceId: string, code: string, evt: SessionCompact) => void
type SessionNoticeFn = (workspaceId: string, code: string, text: string) => void
type SessionFallbackFn = (workspaceId: string, code: string, evt: SessionFallback) => void
type SessionUserFn = (workspaceId: string, code: string, text: string) => void

function contextUsagePayload(d: Record<string, unknown>): SessionContextUsage | null {
  const inputTokens = typeof d.inputTokens === 'number' ? d.inputTokens : 0
  const cacheCreationTokens = typeof d.cacheCreationTokens === 'number' ? d.cacheCreationTokens : 0
  const cacheReadTokens = typeof d.cacheReadTokens === 'number' ? d.cacheReadTokens : 0
  const contextWindow = typeof d.contextWindow === 'number' ? d.contextWindow : 0
  const used = inputTokens + cacheCreationTokens + cacheReadTokens
  if (used <= 0 || contextWindow <= 0) return null
  return { inputTokens, cacheCreationTokens, cacheReadTokens, contextWindow }
}
type WorkspaceFsFn = (workspaceId: string, paths?: string[], rescan?: boolean) => void
export type FileIndexPayload = {
  workspaceId: string
  gen: number
  ready: boolean
  building: boolean
}
type FileIndexFn = (payload: FileIndexPayload) => void
type AppFocusedFn = (workspaceId: string) => void
type WorkspaceChatTabFn = (workspaceId: string, code: string, title: string) => void
export type AttentionTabEvt = { workspaceId: string; code: string; tabUnseen: boolean; clearAllUnseen?: boolean }
type AttentionFn = (evt: AttentionTabEvt) => void
type FindingsUpdatedFn = (workspaceId: string) => void
type GithubBranchRulesFn = (projectId: string, branch: string, requirePR: boolean, remote?: string) => void
export type ChatsChangedDetail = {
  fromWorkspaceId?: string
  codeRemap?: Record<string, string>
  movedCodes?: string[]
}
type ChatsChangedFn = (workspaceId: string, detail?: ChatsChangedDetail) => void
const feedListeners = new Set<FeedFn>()
const prListeners = new Set<PRFn>()
const prCommentListeners = new Set<PRCommentFn>()
const prRefreshListeners = new Set<PRRefreshFn>()
const escListeners = new Set<EscFn>()
const escRaisedListeners = new Set<EscRaisedFn>()
const agentListeners = new Set<AgentFn>()
const sessionDeltaListeners = new Set<SessionDeltaFn>()
const sessionStepListeners = new Set<SessionStepFn>()
const sessionThinkingListeners = new Set<SessionThinkingFn>()
const sessionToolResultListeners = new Set<SessionToolResultFn>()
const sessionToolInputListeners = new Set<SessionToolInputFn>()
const sessionContextUsageListeners = new Set<SessionContextUsageFn>()
const sessionTurnEndListeners = new Set<SessionTurnEndFn>()
const sessionCompactListeners = new Set<SessionCompactFn>()
const sessionNoticeListeners = new Set<SessionNoticeFn>()
const sessionUserListeners = new Set<SessionUserFn>()
const sessionFallbackListeners = new Set<SessionFallbackFn>()
const workspaceFsListeners = new Set<WorkspaceFsFn>()
const fileIndexListeners = new Set<FileIndexFn>()
const appFocusedListeners = new Set<AppFocusedFn>()
const workspaceChatTabListeners = new Set<WorkspaceChatTabFn>()
const attentionListeners = new Set<AttentionFn>()
const findingsListeners = new Set<FindingsUpdatedFn>()
const githubBranchRulesListeners = new Set<GithubBranchRulesFn>()
const chatsChangedListeners = new Set<ChatsChangedFn>()

export function onFeed(fn: FeedFn) { feedListeners.add(fn); return () => feedListeners.delete(fn) }
export function onPRUpdate(fn: PRFn) { prListeners.add(fn); return () => prListeners.delete(fn) }
export function onPRComment(fn: PRCommentFn) { prCommentListeners.add(fn); return () => prCommentListeners.delete(fn) }
export function onPRRefresh(fn: PRRefreshFn) { prRefreshListeners.add(fn); return () => prRefreshListeners.delete(fn) }
export function onEscalationResolved(fn: EscFn) { escListeners.add(fn); return () => escListeners.delete(fn) }
export function onEscalationRaised(fn: EscRaisedFn) { escRaisedListeners.add(fn); return () => escRaisedListeners.delete(fn) }
export function onAgent(fn: AgentFn) { agentListeners.add(fn); return () => agentListeners.delete(fn) }
export function onSessionDelta(fn: SessionDeltaFn) { sessionDeltaListeners.add(fn); return () => sessionDeltaListeners.delete(fn) }
export function onSessionStep(fn: SessionStepFn) { sessionStepListeners.add(fn); return () => sessionStepListeners.delete(fn) }
export function onSessionThinking(fn: SessionThinkingFn) { sessionThinkingListeners.add(fn); return () => sessionThinkingListeners.delete(fn) }
export function onSessionToolResult(fn: SessionToolResultFn) { sessionToolResultListeners.add(fn); return () => sessionToolResultListeners.delete(fn) }
export function onSessionToolInput(fn: SessionToolInputFn) { sessionToolInputListeners.add(fn); return () => sessionToolInputListeners.delete(fn) }
export function onSessionContextUsage(fn: SessionContextUsageFn) { sessionContextUsageListeners.add(fn); return () => sessionContextUsageListeners.delete(fn) }
export function onSessionTurnEnd(fn: SessionTurnEndFn) { sessionTurnEndListeners.add(fn); return () => sessionTurnEndListeners.delete(fn) }
export function onSessionCompact(fn: SessionCompactFn) { sessionCompactListeners.add(fn); return () => sessionCompactListeners.delete(fn) }
export function onSessionNotice(fn: SessionNoticeFn) { sessionNoticeListeners.add(fn); return () => sessionNoticeListeners.delete(fn) }
export function onSessionUser(fn: SessionUserFn) { sessionUserListeners.add(fn); return () => sessionUserListeners.delete(fn) }
export function onSessionFallback(fn: SessionFallbackFn) { sessionFallbackListeners.add(fn); return () => sessionFallbackListeners.delete(fn) }
export function onWorkspaceFs(fn: WorkspaceFsFn) { workspaceFsListeners.add(fn); return () => workspaceFsListeners.delete(fn) }
export function onWorkspaceFileIndex(fn: FileIndexFn) { fileIndexListeners.add(fn); return () => fileIndexListeners.delete(fn) }
export function onAppFocused(fn: AppFocusedFn) { appFocusedListeners.add(fn); return () => appFocusedListeners.delete(fn) }
export function onWorkspaceChatTab(fn: WorkspaceChatTabFn) { workspaceChatTabListeners.add(fn); return () => workspaceChatTabListeners.delete(fn) }
export function onAttentionTab(fn: AttentionFn) { attentionListeners.add(fn); return () => attentionListeners.delete(fn) }

/**
 * answerPermission delivers a yes/no to the agent blocked on this request.
 * `remember` skips re-prompting for the same tool for the rest of that turn.
 *
 * The request is dropped locally before the round trip: an agent is stalled
 * until the answer lands, so the button must not feel like it did nothing. The
 * backend also emits permission.resolved, and both paths filter by id, so the
 * duplicate removal is harmless. A rejection means the request had already
 * expired or its turn ended — the prompt is stale either way, so it stays gone.
 */
export async function answerPermission(
  id: string,
  allow: boolean,
  opts: { reason?: string; remember?: boolean } = {},
) {
  permissionRequests.update((q) => q.filter((r) => r.id !== id))
  try {
    await PermissionsService.AnswerPermission(id, allow, opts.reason ?? '', opts.remember ?? false)
  } catch {
    /* already resolved elsewhere, timed out, or the turn ended */
  }
}

export type AskUserAnswerWire = {
  choice: string
  freeText: string
  attachments: { id: string; name: string; mime: string; data: string }[]
}

type QuestionAnsweredFn = (question: UserQuestion, answer: AskUserAnswerWire) => void
const questionAnswerListeners = new Set<QuestionAnsweredFn>()
const questionAnswerFailedListeners = new Set<QuestionAnsweredFn>()

/** Fired as soon as the UI accepts an answer — paint the user bubble before the resume streams. */
export function onQuestionAnswered(fn: QuestionAnsweredFn) {
  questionAnswerListeners.add(fn)
  return () => questionAnswerListeners.delete(fn)
}

/** Fired when AnswerQuestion RPC fails — roll back the optimistic bubble. */
export function onQuestionAnswerFailed(fn: QuestionAnsweredFn) {
  questionAnswerFailedListeners.add(fn)
  return () => questionAnswerFailedListeners.delete(fn)
}

/** Returns true when the answer was accepted (resume turn will start). */
export async function answerQuestion(
  id: string,
  choice = '',
  freeText = '',
  attachments: { id: string; name: string; mime: string; data: string }[] = [],
): Promise<boolean> {
  // Drop the card immediately so a double-click can't re-submit. A rejection
  // means the question is gone server-side (answered elsewhere / expired) —
  // do not resurrect a zombie card with no broker counterpart.
  const question = get(userQuestions).find((q) => q.id === id)
  userQuestions.update((q) => q.filter((r) => r.id !== id))
  const answer: AskUserAnswerWire = { choice, freeText, attachments }
  try {
    // Paint before the RPC returns — the engine can start the resume turn (and
    // the UI can receive session.delta) while this promise is still pending.
    if (question) {
      questionAnswerListeners.forEach((fn) => fn(question, answer))
    }
    await QuestionsService.AnswerQuestion(id, choice, freeText, attachments)
    return true
  } catch (err) {
    if (question) {
      questionAnswerFailedListeners.forEach((fn) => fn(question, answer))
    }
    pushToast('error', bridgeErrorMessage(err) || 'That question is no longer waiting')
    return false
  }
}

/** Returns true when the loop accepted the escalation answer and will resume. */
export async function answerEscalation(
  workspaceId: string,
  choice = '',
  freeText = '',
  agentCode = '',
): Promise<boolean> {
  try {
    await EscalationService.AnswerEscalation(workspaceId, choice, freeText, agentCode)
    return true
  } catch (err) {
    pushToast('error', bridgeErrorMessage(err) || 'Could not send the answer')
    return false
  }
}

type ChatModeFn = (workspaceId: string, agentCode: string, mode: string) => void
type ArtifactFn = (workspaceId: string, artifact: unknown) => void
const chatModeListeners = new Set<ChatModeFn>()
const artifactPresentListeners = new Set<ArtifactFn>()
const artifactCreatedListeners = new Set<ArtifactFn>()
export function onChatModeChanged(fn: ChatModeFn) { chatModeListeners.add(fn); return () => chatModeListeners.delete(fn) }
export function onArtifactPresented(fn: ArtifactFn) { artifactPresentListeners.add(fn); return () => artifactPresentListeners.delete(fn) }
export function onArtifactCreated(fn: ArtifactFn) { artifactCreatedListeners.add(fn); return () => artifactCreatedListeners.delete(fn) }

export function onFindingsUpdated(fn: FindingsUpdatedFn) { findingsListeners.add(fn); return () => findingsListeners.delete(fn) }
export function onGithubBranchRules(fn: GithubBranchRulesFn) { githubBranchRulesListeners.add(fn); return () => githubBranchRulesListeners.delete(fn) }
export function onChatsChanged(fn: ChatsChangedFn) { chatsChangedListeners.add(fn); return () => chatsChangedListeners.delete(fn) }

// re-export the services so components call them through one module.
export {
  ProjectsService, WorkspacesService, PRsService, PolicyService, SettingsService, InboxService,
  CodeService, ObservatoryService, QuestionsService, ArtifactsService, BriefingService,
}
export { AgentsService, EscalationService, MaestroService } from '../../bindings/recur/internal/app'
