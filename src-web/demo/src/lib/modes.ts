/** Chat modes: how a chat tab's agent may act.
 *  The mode is a property of the chat session, not the workspace — it sits in
 *  the chat dock next to the model picker and is switchable mid-chat. Values
 *  mirror internal/domain NormalizeChatMode; unknown/empty degrade to agent. */

import type { IconName } from './icons'
import {
  type FlagMap,
} from './models'

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
