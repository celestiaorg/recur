/** Chat modes: how a chat tab's agent may act.
 *  The mode is a property of the chat session, not the workspace — it sits in
 *  the chat dock next to the model picker and is switchable mid-chat. Values
 *  mirror internal/domain NormalizeChatMode; unknown/empty degrade to agent. */

import type { IconName } from './icons'
import {
  coerceFlags, flagsEqual, flagsForModel, normalizeFlags,
  type CatalogModel, type FlagMap,
} from './models'
import { modelProvider, providerEnabled } from './providers'

export type ChatMode = 'ask' | 'plan' | 'agent' | 'review' | 'loop'

export type ChatModeOption = {
  value: ChatMode
  label: string
  /** One-line description for the selector row / tooltip. */
  hint: string
  /** Settings → Models title when richer than the dock label. */
  settingsTitle?: string
  /** Settings → Models description when richer than the dock hint. */
  settingsDesc?: string
  /** Icon + accent color: at-a-glance mode identity in the dock and menus.
   *  Tints stay coherent with the agent avatar palette (lib/colors.ts):
   *  Loop=amber (Maestro), Agent=cyan (coding agent), Review=indigo (Reviewer). */
  icon: IconName
  tint: string
}

/** Selector rows, in display order. Hints are
 *  deliberately terse — they render inline in the dock menu, where a sentence
 *  gets cut off. Plan's yellow is a literal (no palette var is a true yellow)
 *  and deliberately distinct from both Review's indigo and Loop's amber. */
export const CHAT_MODES: ChatModeOption[] = [
  { value: 'ask', label: 'Ask', hint: 'Q&A and reports', icon: 'message-question', tint: 'var(--emerald)' },
  { value: 'plan', label: 'Plan', hint: 'Investigate and plan', icon: 'notebook', tint: '#facc15' },
  {
    value: 'agent',
    label: 'Agent',
    hint: 'Read and edit files',
    settingsTitle: 'Agent / Coder',
    settingsDesc: 'Writes and edits code in Agent chats and loop coding turns',
    icon: 'message-code',
    tint: 'var(--cyan)',
  },
  { value: 'review', label: 'Review', hint: 'Review the diff', icon: 'git-compare', tint: 'var(--indigo)' },
  { value: 'loop', label: 'Loop', hint: 'Autopilot to a PR', icon: 'refresh-cw', tint: 'var(--amber)' },
]

/** Metadata for one mode (safe default: agent). */
export function chatModeMeta(m?: string | null): ChatModeOption {
  const mode = normalizeChatMode(m)
  return CHAT_MODES.find((o) => o.value === mode) ?? CHAT_MODES[2]
}

/** Whether the autopilot has (ever) been engaged on this workspace — mirrors
 *  domain.Workspace.HasRun (legacy loop records carry type ""/"loop" and no
 *  runStarted flag). */
export function wsHasRun(w?: { runStarted?: boolean; type?: string } | null): boolean {
  if (!w) return false
  return !!w.runStarted || (w.type ?? 'loop') === 'loop'
}

const RUN_ACTIVE = new Set(['queued', 'planning', 'coding', 'reviewing', 'verifying'])
const RUN_SETTLED_PAUSE = new Set(['idle', 'artifact', 'phase missing', 'loop refs missing'])

/** Autopilot currently owns the worktree (mirrors domain.RunActive), plus a live
 *  paused/escalated goroutine that will resume editing. Walker-idle pauses are
 *  settled — the human can commit/merge. */
export function wsRunActive(w?: { state?: string; phase?: string } | null): boolean {
  if (!w?.state) return false
  if (RUN_ACTIVE.has(w.state)) return true
  if (w.state === 'escalated') return true
  if (w.state === 'paused') return !RUN_SETTLED_PAUSE.has(w.phase ?? '')
  return false
}

/** Amber pulse: any live agent (Maestro included) or an in-flight loop.
 *  Escalated is "needs you", not a running pulse. Human pauses and board-cap
 *  exits own the worktree but are parked until Resume. */
export function workspaceIsRunning(
  w?: { id?: string; state?: string; phase?: string } | null,
  running?: ReadonlySet<string> | null,
): boolean {
  if (w?.id && running?.has(w.id)) return true
  if (!w?.state || w.state === 'escalated') return false
  return RUN_ACTIVE.has(w.state)
}

/** Parked items that need the human, per workspace. Stays until answered —
 *  unlike unseen-chat counts, which clear when the row is viewed. GroupNeeds
 *  (retryable fail, interrupted recover) counts once even without a prompt;
 *  hidden PR-chat hosts stay off the sidebar — their attention is the PR surface. */
export function parkedNeedsYouByWorkspace(
  workspaces: readonly { id?: string; state?: string; group?: string; archived?: boolean; hostPrId?: string }[] = [],
  questions: readonly { workspaceId?: string }[] = [],
  permissions: readonly { workspaceId?: string }[] = [],
): Map<string, number> {
  const n = new Map<string, number>()
  const hidden = new Set<string>()
  const bump = (id: string) => n.set(id, (n.get(id) ?? 0) + 1)
  for (const w of workspaces) {
    if (!w.id) continue
    if (w.archived || (w.hostPrId ?? '').trim()) {
      hidden.add(w.id)
      continue
    }
    if (w.state === 'escalated' || w.group === 'needs') bump(w.id)
  }
  for (const q of questions) {
    if (q.workspaceId && !hidden.has(q.workspaceId)) bump(q.workspaceId)
  }
  for (const p of permissions) {
    if (p.workspaceId && !hidden.has(p.workspaceId)) bump(p.workspaceId)
  }
  return n
}

export function sidebarUnreadTitle(unseen: number, parked: number): string {
  const n = unseen + parked
  if (n <= 0) return ''
  if (parked > 0) return n === 1 ? 'Needs you' : `${n} waiting`
  return `${n} unseen agent response${n === 1 ? '' : 's'}`
}

/** Autopilot finished — engine RemoveWorkspace allows these (checkLoopSettled). */
export const WORKSPACE_SETTLED_STATES = ['merged', 'stopped', 'failed', 'pr_open'] as const

export function workspaceIsSettled(state?: string | null): boolean {
  return !!state && (WORKSPACE_SETTLED_STATES as readonly string[]).includes(state)
}

/** Fields Remove visibility and eligibility read. Extra workspace fields are allowed. */
type WorkspaceRemoveRow = {
  isMain?: boolean
  archived?: boolean
  hostPrId?: string
  state?: string
  phase?: string
  runStarted?: boolean
  type?: string
}

/** Whether Remove is offered at all. Main checkouts, PR chat hosts, and
 *  archived rows never get the control; running loops still show it disabled. */
export function workspaceShowsRemove(
  w?: WorkspaceRemoveRow | null,
): w is WorkspaceRemoveRow {
  return !!w && !w.isMain && !w.archived && !(w.hostPrId ?? '').trim()
}

/** Whether Remove is allowed: a visible Remove control that is either a hand
 *  workspace or a settled loop. Running loops and in-flight agent turns must
 *  be stopped first. */
export function workspaceCanRemove(
  w?: WorkspaceRemoveRow | null,
  agentRunning = false,
): boolean {
  if (!workspaceShowsRemove(w) || agentRunning) return false
  if (!wsHasRun(w)) return true
  if (workspaceIsSettled(w.state)) return true
  return w.state === 'paused' && RUN_SETTLED_PAUSE.has(w.phase ?? '')
}

/** Git worktree and/or setup_cmd still in flight. Composer send queues on this
 *  and flushes when it clears; the editor overlay uses wsMainBlocked so a
 *  landed worktree can paint while setup_cmd finishes. */
export function wsCreating(w?: {
  lastError?: string
  worktreePath?: string
  phase?: string
  statusLabel?: string
} | null): boolean {
  return !!w && !w.lastError &&
    (w.phase === 'creating' || (w.phase === 'waiting for a slot' && !w.worktreePath))
}

/** Chat bouncing-status copy. Backend publishes phase "creating" once a slot
 *  is acquired; "waiting for a slot" is a real queue wait (with or without a
 *  worktree). */
export function wsSetupStatus(w?: {
  lastError?: string
  worktreePath?: string
  phase?: string
  statusLabel?: string
} | null): string {
  if (!w || w.lastError) return ''
  if (w.phase === 'creating') return 'Setting up the worktree…'
  if (w.phase === 'waiting for a slot') return 'Waiting for a loop slot…'
  return ''
}

/** Composer send queues until git/setup finish. Main and failed creates do not hold. */
export function wsChatHeld(w?: {
  isMain?: boolean
  lastError?: string
  worktreePath?: string
  phase?: string
  statusLabel?: string
  runStarted?: boolean
  type?: string
} | null): boolean {
  if (!w || w.isMain || wsCreateFailed(w)) return false
  return wsCreating(w) || !(w.worktreePath ?? '').trim()
}

export function wsCreateFailed(w?: {
  lastError?: string
  worktreePath?: string
  phase?: string
  statusLabel?: string
  runStarted?: boolean
  type?: string
} | null): boolean {
  // A loop whose dispatch failed also gets StatusLabel "Couldn't create"
  // (failCreating). That's a running loop's problem, not a failed create.
  if (!w || !w.lastError || wsHasRun(w)) return false
  if (w.statusLabel === "Couldn't create") return true
  return w.phase === 'failed'
}

/** Editor + files overlay. Unblocked once the worktree is on disk — same as a
 *  loop waiting for a slot after allocate — even while phase is still creating.
 *  No tree plus a create/allocate error overlays even for a loop (HasRun),
 *  because there is nothing to show; a failed setup that kept the tree does
 *  not (the status strip surfaces lastError so files stay reachable). */
export function wsMainBlocked(w?: {
  lastError?: string
  worktreePath?: string
  phase?: string
  statusLabel?: string
  runStarted?: boolean
  type?: string
} | null): boolean {
  if (
    !!w && !w.worktreePath && !!w.lastError &&
    (w.phase === 'failed' || w.statusLabel === "Couldn't create")
  ) {
    return true
  }
  if (w?.worktreePath) return false
  return wsCreating(w)
}

export function normalizeChatMode(m?: string | null): ChatMode {
  switch (m) {
    case 'ask':
    case 'plan':
    case 'agent':
    case 'review':
    case 'loop':
      return m
    default:
      return 'agent'
  }
}

/** Next mode in selector order (Ask → Plan → Agent → Review → Loop → Ask).
 *  excludeLoop skips the Loop row — used by the main working copy, which can
 *  never engage the autopilot. excludeAgent also drops Agent (PR chat hosts
 *  may only Ask/Plan/Review against the project checkout). */
export function cycleChatMode(m?: string | null, excludeLoop = false, excludeAgent = false): ChatMode {
  const modes = CHAT_MODES.filter((o) =>
    !(excludeLoop && o.value === 'loop') && !(excludeAgent && o.value === 'agent'),
  )
  const cur = normalizeChatMode(m)
  const i = modes.findIndex((o) => o.value === cur)
  return modes[(i < 0 ? 0 : i + 1) % modes.length].value
}

/** Options offered by the dock selector — icon + tint + hint travel to the
 *  Select rows so every mode reads at a glance. excludeLoop drops the Loop row
 *  for surfaces where the autopilot can't run (the main working copy).
 *  excludeAgent drops Agent for PR chat hosts. */
export function chatModeOptions(excludeLoop = false, excludeAgent = false): {
  value: string; label: string; icon: IconName; tint: string; hint: string
}[] {
  return CHAT_MODES
    .filter((m) => !(excludeLoop && m.value === 'loop') && !(excludeAgent && m.value === 'agent'))
    .map((m) => ({
      value: m.value, label: m.label, icon: m.icon, tint: m.tint, hint: m.hint,
    }))
}

/** Settings fields that seed each mode's default model (Settings → Models). */
export type ModeModelSettings = {
  modelAsk?: string
  modelPlan?: string
  modelAgent?: string
  modelReview?: string
  modelLoop?: string
  flagsAsk?: FlagMap
  flagsPlan?: FlagMap
  flagsAgent?: FlagMap
  flagsReview?: FlagMap
  flagsLoop?: FlagMap
  disabledProviders?: string[] | null
}

/** Raw Settings default for a chat mode — one Settings slot per mode. */
export function defaultModelForMode(
  mode: string | null | undefined,
  set: ModeModelSettings | null | undefined,
): { model: string; flags: Record<string, string> } {
  const fallback = 'claude-code/claude-sonnet-5-5'
  switch (normalizeChatMode(mode)) {
    case 'ask':
      return {
        model: set?.modelAsk || fallback,
        flags: coerceFlags(set?.flagsAsk) ?? {},
      }
    case 'plan':
      return {
        model: set?.modelPlan || fallback,
        flags: coerceFlags(set?.flagsPlan) ?? {},
      }
    case 'review':
      return {
        model: set?.modelReview || set?.modelLoop || fallback,
        flags: coerceFlags(set?.flagsReview) ?? {},
      }
    case 'loop':
      return {
        model: set?.modelLoop || fallback,
        flags: coerceFlags(set?.flagsLoop) ?? {},
      }
    default: // agent
      return {
        model: set?.modelAgent || fallback,
        flags: coerceFlags(set?.flagsAgent) ?? {},
      }
  }
}

/** Catalogue-resolved default (missing flag dims filled from the model).
 *  If the Settings slot names a turned-off provider (or a ref the catalogue
 *  no longer lists), pick the first catalogue model whose provider is on. */
export function resolvedDefaultForMode(
  mode: string | null | undefined,
  set: ModeModelSettings | null | undefined,
  catalog: CatalogModel[],
): { model: string; flags: Record<string, string> } {
  const d = defaultModelForMode(mode, set)
  let model = d.model
  let flags = d.flags
  const enabled = providerEnabled(set?.disabledProviders, modelProvider(model))
  const listed = catalog.some((m) => m.ref === model)
  if (catalog.length > 0 && (!enabled || !listed)) {
    const first = catalog.find((m) =>
      providerEnabled(set?.disabledProviders, m.provider || modelProvider(m.ref)),
    )
    if (first) {
      model = first.ref
      flags = {}
    }
  }
  return {
    model,
    flags: normalizeFlags(flagsForModel(catalog, model), flags),
  }
}

/** True when the dock selection still matches this mode's Settings default —
 *  i.e. the user has not overridden the model/flags. Unused chats then follow
 *  the new mode's default; an explicit pick is kept across modes. */
export function isModeDefaultSelection(
  mode: string | null | undefined,
  model: string,
  flags: FlagMap,
  set: ModeModelSettings | null | undefined,
  catalog: CatalogModel[],
): boolean {
  const d = resolvedDefaultForMode(mode, set, catalog)
  return model === d.model && flagsEqual(flags, d.flags)
}

/** Placeholder tab label until the first user turn (matches Go chatTabTitle). */
export const NEW_CHAT_TITLE = 'New chat'

/** True once this chat has had a turn: transcript rows, or a tab title other
 *  than the empty placeholder (title survives reload before history hydrates). */
export function chatHasStarted(
  title?: string | null,
  messages?: ReadonlyArray<{ queued?: boolean }> | null,
): boolean {
  if (messages?.some((m) => !m.queued)) return true
  const t = (title ?? '').trim()
  return t !== '' && t !== NEW_CHAT_TITLE
}

/** Model/flags to keep after a chat mode switch.
 *
 *  When pickers are not hydrated yet, the UI falls back to the coder model with
 *  empty flags — that must NOT count as an explicit override (prompt actions that
 *  create/select a tab then call onChatModeChange used to keep the coder model
 *  on Review/Plan/Ask). Unhydrated → always take the new mode's Settings default.
 *  Unused + hydrated + still on the old mode's default → follow the new default.
 *  A session that has already started keeps its model even on the old default —
 *  mode is tool policy, not a mid-chat model swap.
 *  Hydrated with an explicit pick → keep it. */
export function pickerAfterModeChange(
  fromMode: string | null | undefined,
  toMode: string | null | undefined,
  model: string,
  flags: FlagMap,
  set: ModeModelSettings | null | undefined,
  catalog: CatalogModel[],
  pickerHydrated: boolean,
  sessionStarted: boolean,
): { model: string; flags: Record<string, string> } {
  if (sessionStarted && pickerHydrated) {
    return { model, flags: coerceFlags(flags) ?? {} }
  }
  if (!pickerHydrated || isModeDefaultSelection(fromMode, model, flags, set, catalog)) {
    return resolvedDefaultForMode(toMode, set, catalog)
  }
  return { model, flags: coerceFlags(flags) ?? {} }
}
