// Display helpers for Ask Recur briefing tools (list/get/present workspace).
// Keep these pure so the "no JSON dump / friendly card" contracts stay testable
// without mounting AgentChat.

import type { WorkspaceBrief } from '../../bindings/recur/internal/domain/models'

export function isListWorkspaceBriefsTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__list_workspace_briefs') || tool === 'list_workspace_briefs'
}

export function isGetWorkspaceContextTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__get_workspace_context') || tool === 'get_workspace_context'
}

export function isPresentWorkspaceTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__present_workspace') || tool === 'present_workspace'
}

export function isBriefingTool(tool: string | undefined | null): boolean {
  return isListWorkspaceBriefsTool(tool) || isGetWorkspaceContextTool(tool) || isPresentWorkspaceTool(tool)
}

export type BriefingCountsView = { needs: number; working: number; pickup: number }

export type ListedWorkspaceBriefs = {
  counts: BriefingCountsView
  workspaces: WorkspaceBrief[]
}

export type WorkspaceContextView = {
  brief: WorkspaceBrief
  planSteps: { id: string; title: string; status: string; note?: string }[]
  agents: { code: string; name: string; status: string; task?: string }[]
  openAsks: { kind: string; id: string; summary: string }[]
  recentChats: { code: string; title: string; mode?: string }[]
  pr: { id: string; number: number; title: string; state?: string } | null
  conversation: { role: string; text: string }[]
}

export type PresentWorkspaceView = {
  ok: boolean
  target: string
  kind: string
  brief: WorkspaceBrief | null
  error: string
}

function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0
}

function parseJSONObject(raw: string): Record<string, unknown> | null {
  const t = raw.trim()
  const i = t.indexOf('{')
  if (i < 0) return null
  try {
    const o = JSON.parse(t.slice(i))
    if (!o || typeof o !== 'object' || Array.isArray(o)) return null
    return o as Record<string, unknown>
  } catch {
    return null
  }
}

function parseAction(raw: unknown): WorkspaceBrief['primary'] {
  if (!raw || typeof raw !== 'object') return { label: '', target: '' }
  const o = raw as Record<string, unknown>
  return {
    label: str(o.label),
    target: str(o.target),
    ...(str(o.kind) ? { kind: str(o.kind) } : {}),
  }
}

export function parseWorkspaceBrief(raw: unknown): WorkspaceBrief | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const o = raw as Record<string, unknown>
  const workspaceId = str(o.workspaceId) || str(o.workspace_id)
  const title = str(o.title)
  if (!workspaceId && !title) return null
  const reasons = Array.isArray(o.reasons)
    ? o.reasons.filter((r): r is string => typeof r === 'string' && !!r.trim())
    : []
  const secondary = Array.isArray(o.secondary)
    ? o.secondary.map(parseAction).filter((a) => a.target && a.label)
    : []
  return {
    workspaceId,
    projectId: str(o.projectId) || str(o.project_id) || undefined,
    projectName: str(o.projectName) || str(o.project_name) || undefined,
    title: title || workspaceId,
    branch: str(o.branch) || undefined,
    bucket: str(o.bucket),
    reasons,
    now: str(o.now),
    next: str(o.next),
    quietAge: str(o.quietAge) || str(o.quiet_age) || undefined,
    planSummary: str(o.planSummary) || str(o.plan_summary) || undefined,
    planDone: num(o.planDone) || num(o.plan_done) || undefined,
    planTotal: num(o.planTotal) || num(o.plan_total) || undefined,
    activeAgents: num(o.activeAgents) || num(o.active_agents) || undefined,
    latestChatTitle: str(o.latestChatTitle) || str(o.latest_chat_title) || undefined,
    diffAdd: num(o.diffAdd) || num(o.diff_add) || undefined,
    diffDel: num(o.diffDel) || num(o.diff_del) || undefined,
    prNumber: num(o.prNumber) || num(o.pr_number) || undefined,
    changedSinceViewed: o.changedSinceViewed === true || o.changed_since_viewed === true || undefined,
    primary: parseAction(o.primary),
    ...(secondary.length ? { secondary } : {}),
  }
}

export function parseListWorkspaceBriefs(result: string): ListedWorkspaceBriefs | null {
  const o = parseJSONObject(result)
  if (!o || !Array.isArray(o.workspaces)) return null
  const c = o.counts && typeof o.counts === 'object' && !Array.isArray(o.counts)
    ? o.counts as Record<string, unknown>
    : {}
  const workspaces: WorkspaceBrief[] = []
  for (const row of o.workspaces) {
    const b = parseWorkspaceBrief(row)
    if (b) workspaces.push(b)
  }
  return {
    counts: {
      needs: num(c.needs),
      working: num(c.working),
      pickup: num(c.pickup),
    },
    workspaces,
  }
}

export function parseWorkspaceContext(result: string): WorkspaceContextView | null {
  const o = parseJSONObject(result)
  if (!o) return null
  const brief = parseWorkspaceBrief(o.brief) || parseWorkspaceBrief(o)
  if (!brief) return null
  const planSteps = Array.isArray(o.planSteps) ? o.planSteps : Array.isArray(o.plan_steps) ? o.plan_steps : []
  const agents = Array.isArray(o.agents) ? o.agents : []
  const openAsks = Array.isArray(o.openAsks) ? o.openAsks : Array.isArray(o.open_asks) ? o.open_asks : []
  const recentChats = Array.isArray(o.recentChats) ? o.recentChats : Array.isArray(o.recent_chats) ? o.recent_chats : []
  const conversation = Array.isArray(o.conversation) ? o.conversation : []
  const prRaw = o.pr && typeof o.pr === 'object' && !Array.isArray(o.pr) ? o.pr as Record<string, unknown> : null
  return {
    brief,
    planSteps: planSteps.flatMap((s) => {
      if (!s || typeof s !== 'object') return []
      const r = s as Record<string, unknown>
      const title = str(r.title)
      if (!title) return []
      return [{ id: str(r.id), title, status: str(r.status), note: str(r.note) || undefined }]
    }),
    agents: agents.flatMap((a) => {
      if (!a || typeof a !== 'object') return []
      const r = a as Record<string, unknown>
      const code = str(r.code)
      if (!code) return []
      return [{ code, name: str(r.name) || code, status: str(r.status), task: str(r.task) || undefined }]
    }),
    openAsks: openAsks.flatMap((a) => {
      if (!a || typeof a !== 'object') return []
      const r = a as Record<string, unknown>
      const summary = str(r.summary)
      if (!summary) return []
      return [{ kind: str(r.kind), id: str(r.id), summary }]
    }),
    recentChats: recentChats.flatMap((c) => {
      if (!c || typeof c !== 'object') return []
      const r = c as Record<string, unknown>
      const code = str(r.code)
      if (!code) return []
      return [{ code, title: str(r.title) || code, mode: str(r.mode) || undefined }]
    }),
    pr: prRaw && (str(prRaw.id) || num(prRaw.number))
      ? { id: str(prRaw.id), number: num(prRaw.number), title: str(prRaw.title), state: str(prRaw.state) || undefined }
      : null,
    conversation: conversation.flatMap((l) => {
      if (!l || typeof l !== 'object') return []
      const r = l as Record<string, unknown>
      const text = str(r.text)
      if (!text) return []
      return [{ role: str(r.role) || 'assistant', text }]
    }),
  }
}

export function parsePresentWorkspaceTarget(raw: unknown): string {
  if (!raw || typeof raw !== 'object') return ''
  return str((raw as Record<string, unknown>).target)
}

export function parsePresentWorkspace(result: string, inputTarget = ''): PresentWorkspaceView {
  const r = (result ?? '').trim()
  const o = parseJSONObject(r)
  if (o && (typeof o.ok === 'boolean' || o.target || o.brief || o.error)) {
    const target = str(o.target) || inputTarget
    const brief = parseWorkspaceBrief(o.brief)
    const error = str(o.error)
    const ok = o.ok === true || (!error && (!!target || !!brief))
    return {
      ok: ok && !error,
      target,
      kind: str(o.kind) || targetKind(target),
      brief,
      error: error || (ok ? '' : r),
    }
  }
  const opening = r.match(/^Opening\s+(\S+)\.?$/i)
  if (opening?.[1]) {
    const target = opening[1].replace(/\.$/, '')
    return { ok: true, target, kind: targetKind(target), brief: null, error: '' }
  }
  const presented = r.match(/^Presented\s+(\S+)\.?$/i)
  if (presented?.[1]) {
    const target = presented[1].replace(/\.$/, '')
    return { ok: true, target, kind: targetKind(target), brief: null, error: '' }
  }
  if (/^Refused:/i.test(r)) {
    return { ok: false, target: inputTarget, kind: targetKind(inputTarget), brief: null, error: r }
  }
  if (!r) {
    return { ok: false, target: inputTarget, kind: targetKind(inputTarget), brief: null, error: '' }
  }
  return { ok: false, target: inputTarget, kind: targetKind(inputTarget), brief: null, error: r }
}

export function targetKind(target: string): string {
  const k = target.split(':')[0]?.trim() || ''
  return k === 'loop' ? 'workspace' : k
}

/** Workspace id encoded in a present_workspace target, or empty for pr/question. */
export function workspaceIdFromPresentTarget(target: string): string {
  const t = target.trim()
  const i = t.indexOf(':')
  if (i < 0) return t
  const kind = t.slice(0, i)
  const rest = t.slice(i + 1).trim()
  if (kind === 'workspace' || kind === 'loop') return rest.split(':')[0]?.trim() || ''
  if (kind === 'chat') return rest.split(':')[0]?.trim() || ''
  return ''
}

type HydrateWorkspace = { id: string; short?: string; goal?: string; branch?: string; projectId?: string }
type HydrateProject = { id: string; name?: string }

/** Fill a missing brief from live overview / workspace list so the pin is a real card. */
export function hydratePresentBrief(
  card: PresentWorkspaceView,
  overview: WorkspaceBrief[],
  workspaces: HydrateWorkspace[],
  projects: HydrateProject[],
): PresentWorkspaceView {
  if (card.brief) return card
  const id = workspaceIdFromPresentTarget(card.target)
  if (id) {
    const fromOverview = overview.find((w) => w.workspaceId === id)
    if (fromOverview) return { ...card, brief: fromOverview }
    const ws = workspaces.find((w) => w.id === id)
    if (ws) {
      const title = (ws.short || ws.goal || ws.id).trim() || id
      const proj = ws.projectId ? projects.find((p) => p.id === ws.projectId) : undefined
      return {
        ...card,
        brief: {
          workspaceId: ws.id,
          projectId: ws.projectId || undefined,
          projectName: proj?.name || ws.projectId || undefined,
          title,
          branch: ws.branch || undefined,
          bucket: '',
          reasons: [],
          now: '',
          next: '',
          primary: { label: 'Open', target: card.target, kind: card.kind || 'open' },
        },
      }
    }
  }
  if (!card.target) return card
  const title = id || card.target
  return {
    ...card,
    brief: {
      workspaceId: id,
      title,
      bucket: '',
      reasons: [],
      now: '',
      next: '',
      primary: { label: 'Open', target: card.target, kind: card.kind || 'open' },
    },
  }
}

export function briefingCountsMeta(counts: BriefingCountsView): string {
  const parts: string[] = []
  if (counts.needs) parts.push(`${counts.needs} need you`)
  if (counts.working) parts.push(`${counts.working} working`)
  if (counts.pickup) parts.push(`${counts.pickup} pick up`)
  return parts.join(' · ')
}

export function listWorkspaceBriefsLabel(pending: boolean, listed: ListedWorkspaceBriefs | null): string {
  if (pending) return 'Listing workspaces'
  const n = listed?.workspaces.length ?? 0
  if (!listed) return 'Listed workspaces'
  if (n === 0) return 'Listed workspaces · none'
  return n === 1 ? 'Listed 1 workspace' : `Listed ${n} workspaces`
}

export function getWorkspaceContextLabel(pending: boolean, title: string, failed: boolean): string {
  if (failed) return 'Workspace lookup failed'
  if (pending) return title ? `Looking up ${title}` : 'Looking up workspace'
  return title ? `Looked up ${title}` : 'Looked up workspace'
}

export function listWorkspaceBriefsSummary(listed: ListedWorkspaceBriefs): string {
  if (!listed.workspaces.length) return 'Nothing in flight.'
  if (listed.workspaces.length === 1) {
    const b = listed.workspaces[0]
    return [b.title, b.now || b.next].filter(Boolean).join(' — ')
  }
  return briefingCountsMeta(listed.counts) || `${listed.workspaces.length} workspaces`
}

export function workspaceContextSummary(ctx: WorkspaceContextView): string {
  return ctx.brief.now || ctx.brief.next || ctx.brief.title
}

export function presentWorkspaceLabel(pending: boolean, card: PresentWorkspaceView | null): string {
  if (pending) {
    const t = card?.brief?.title || card?.target
    return t ? `Presenting ${t}` : 'Presenting workspace'
  }
  if (card?.error || card?.ok === false) return 'Present workspace failed'
  const t = card?.brief?.title
  if (t) return `Presented ${t}`
  if (card?.target) return `Presented ${card.target}`
  return 'Presented workspace'
}
