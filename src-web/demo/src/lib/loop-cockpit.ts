// Loop-chat cockpit: merge conductor turns with specialist sessions and engine
// facts into one transcript. A PA* tab that is not the engaged Loop stays
// unmerged (its own Agent/Ask/… thread). The engaged Loop tab (MA or the
// loop-started PA* conductor) is the cockpit.

import type { AgentMsg, ChatLogSession } from '../components/AgentChat.svelte'
import type { FeedItem } from './types'
import type { ChatMode } from './modes'

export type FeedSess = {
  id: string
  code: string
  name: string
  kind: string
  ts: number
  /** Wall time this live dispatch started: last slice stamps here, not createdAt. */
  liveTs?: number
  messages: AgentMsg[]
}

export function isPairChatCode(code: string): boolean {
  return code === 'PA' || /^PA[0-9]+$/.test(code)
}

export function isSpecialistSession(s: { code: string }): boolean {
  return s.code !== 'MA' && !isPairChatCode(s.code)
}

/** The Loop cockpit (specialist cards + facts) belongs on the conductor tab:
 *  legacy MA, or a PA* tab that has engaged a run (loop pickers pinned). */
export function isLoopCockpitTab(code: string, loopPinned: boolean): boolean {
  return code === 'MA' || loopPinned
}

export function specialistName(code: string): string {
  switch (code) {
    case 'CO': return 'Coder'
    case 'RV': return 'Reviewer'
    case 'TE': return 'Tester'
    case 'QA': return 'QA'
    case 'PL': return 'Planner'
    case 'SR': return 'Security reviewer'
    case 'WR': return 'Writer'
    case 'ED': return 'Editor'
    default: return code
  }
}

/** Chat-mode slot this specialist runs in (Loop bundle [[role]].mode). */
export function specialistMode(code: string): ChatMode {
  switch (code) {
    case 'PL': return 'plan'
    case 'RV':
    case 'SR':
    case 'ED': return 'review'
    case 'QA': return 'ask'
    case 'MA': return 'loop'
    default: return 'agent'
  }
}

/** Wall-clock label for a finished loop. Keep in sync with Go formatLoopElapsed. */
export function formatLoopElapsed(ms: number): string {
  if (ms <= 0) return ''
  const sec = Math.round(ms / 1000)
  if (sec < 60) return `${sec}s`
  const m = Math.floor(sec / 60)
  const s = sec % 60
  if (m >= 60) {
    const h = Math.floor(m / 60)
    const rm = m % 60
    return rm === 0 ? `${h}h` : `${h}h ${rm}m`
  }
  return s === 0 ? `${m}m` : `${m}m ${String(s).padStart(2, '0')}s`
}

function factPlain(text: string): string {
  return text.replace(/<[^>]*>/g, '').trim()
}

/** Phase-start chatter the specialist card already covers. */
export function isPhaseKickoffFact(text: string): boolean {
  return /^(Reviewing the diff \(iteration \d+\)\.|Driving the app in a browser to verify the change\.|Checking the plan against the implementer's handoff\.|Planning the change and definition-of-done\.|Dispatched from chat:)/.test(factPlain(text))
}

/** Commit / handoff lines that belong on the specialist card, not as their own row. */
export function isSpecialistWrapFact(text: string): boolean {
  return /^(Committed |Handoff:|No file changes to commit this iteration\.)/.test(factPlain(text))
}

function wrapFactMessage(text: string, ts: number): AgentMsg {
  const plain = factPlain(text)
  let noteLabel = 'Notice'
  if (/^Handoff:/.test(plain)) noteLabel = 'Handoff'
  else if (/^Committed |^No file changes/.test(plain)) noteLabel = 'Commit'
  return { note: true, noteLabel, noteHtml: true, body: text, ts }
}

function insertByTs(messages: AgentMsg[], msg: AgentMsg): AgentMsg[] {
  const ts = msg.ts || 0
  const next = messages.slice()
  let i = next.length
  while (i > 0 && (next[i - 1].ts || 0) > ts) i--
  next.splice(i, 0, msg)
  return next
}

function msgTs(m: AgentMsg): number {
  if (m.ts && m.ts > 0) return m.ts
  if (m.optimistic || m.streaming || m.queued) return Number.MAX_SAFE_INTEGER
  return 0
}

function firstTs(messages: AgentMsg[], fallback: number): number {
  for (const m of messages) {
    if (m.ts && m.ts > 0) return m.ts
  }
  return fallback
}

const LIVE_TS = Number.MAX_SAFE_INTEGER

/**
 * One durable session per agent code accretes every loop iteration. Split on a
 * new user prompt or a turnEnd so the cockpit paints one card per dispatch,
 * stamped at that turn's time: not one card at the first createdAt.
 */
export function splitSessionTurns(s: FeedSess): FeedSess[] {
  const msgs = s.messages
  const turns: AgentMsg[][] = []
  let cur: AgentMsg[] = []
  for (const m of msgs) {
    if (m.me && cur.length) {
      turns.push(cur)
      cur = []
    }
    cur.push(m)
    if (m.turnEnd) {
      turns.push(cur)
      cur = []
    }
  }
  if (cur.length) turns.push(cur)
  const parts = turns.length ? turns : [[]]
  // Always suffix the turn index so the first card's key stays stable when a
  // second dispatch lands (s.id → s.id#0 would remount the whole transcript).
  return parts.map((messages, i) => {
    const last = i === parts.length - 1
    const fallback = last && s.liveTs ? s.liveTs : s.ts
    // upsertAgent (working) races persist. An empty kickoff card sorted at
    // Date.now() jumps once the prompt stamps. Keep empties at the live tail.
    const ts = last && messages.length === 0 ? LIVE_TS : firstTs(messages, fallback)
    return {
      ...s,
      id: `${s.id}#${i}`,
      ts,
      messages,
    }
  })
}

function mergeTs(m: AgentMsg): number {
  const t = msgTs(m)
  return t > 0 ? t : LIVE_TS
}

/** Interleave specialist cards and engine facts into the Maestro transcript.
 *  Maestro rows keep their own order (same as pair chat) so a live turn cannot
 *  lose earlier steps, and a "completed in" footer cannot jump mid-turn.
 *  openingPrompt is the loop goal, pinned as the first user bubble while the
 *  conductor chat only holds client-side optimistic rows (escalation answers).
 *  A slot planner (PL) never writes its prompt into the MA transcript. */
export function mergeLoopCockpit(
  ma: AgentMsg[],
  sessions: FeedSess[],
  facts: FeedItem[],
  liveCodes: Set<string>,
  openingPrompt = '',
): AgentMsg[] {
  const queued: AgentMsg[] = []
  const maRows: AgentMsg[] = []
  for (const m of ma) {
    if (m.queued) queued.push(m)
    else maRows.push(m)
  }
  const prompt = openingPrompt.trim()
  const firstUser = maRows.find((m) => m.me)
  if (prompt && (!firstUser || (firstUser.optimistic && firstUser.body !== prompt))) {
    maRows.unshift({ me: true, body: prompt, ts: 1 })
  }

  type Extra = { ts: number; seq: number; msg: AgentMsg }
  const extras: Extra[] = []
  let seq = 0
  const slices: FeedSess[] = []
  for (const s of sessions) {
    if (!isSpecialistSession(s)) continue
    slices.push(...splitSessionTurns(s))
  }
  const have = new Set(slices.map((s) => s.code))
  for (const code of liveCodes) {
    if (!code || have.has(code) || !isSpecialistSession({ code })) continue
    have.add(code)
    slices.push(...splitSessionTurns({
      id: 'live-' + code,
      code,
      name: specialistName(code),
      kind: '',
      ts: LIVE_TS,
      messages: [],
    }))
  }
  const liveSlice = new Map<string, string>()
  for (const s of slices) {
    if (liveCodes.has(s.code)) liveSlice.set(s.code, s.id)
  }
  for (const s of slices) {
    const ts = s.ts || 0
    extras.push({
      ts,
      seq: seq++,
      msg: {
        ts,
        log: {
          kind: 'session',
          id: s.id,
          code: s.code,
          name: s.name,
          messages: s.messages,
          live: liveSlice.get(s.code) === s.id,
        },
      },
    })
  }
  for (const f of facts) {
    if (isPhaseKickoffFact(f.text ?? '')) continue
    const ts = f.ts || 0
    const code = f.agentCode || 'MA'
    if (isSpecialistWrapFact(f.text ?? '') && isSpecialistSession({ code })) {
      let host: (typeof extras)[number] | undefined
      let last: (typeof extras)[number] | undefined
      for (const e of extras) {
        if (e.msg.log?.kind !== 'session' || e.msg.log.code !== code) continue
        last = e
        if (e.ts <= ts) host = e
      }
      const log = (host ?? last)?.msg.log
      if (log?.kind === 'session') {
        log.messages = insertByTs(log.messages, wrapFactMessage(f.text ?? '', ts))
        continue
      }
    }
    extras.push({
      ts,
      seq: seq++,
      msg: {
        ts,
        log: {
          kind: 'fact',
          id: f.id,
          text: f.text,
          isDeviation: !!f.isDeviation,
          code: f.agentCode || 'MA',
          name: f.name || 'Maestro',
          ...(f.card ? { card: f.card, data: f.data ?? '' } : {}),
        },
      },
    })
  }
  extras.sort((a, b) => a.ts - b.ts || a.seq - b.seq)

  const firstUserIdx = maRows.findIndex((m) => m.me)
  const out: AgentMsg[] = []
  let ei = 0
  const takeBefore = (ts: number) => {
    if (ts <= 0) return
    while (ei < extras.length && extras[ei].ts > 0 && extras[ei].ts < ts) {
      out.push(extras[ei++].msg)
    }
  }
  for (let i = 0; i < maRows.length; i++) {
    // Opening prompt first: engine facts like "Starting loop for…" are
    // stamped before the planner records the goal as a user message.
    // Unstamped / streaming Maestro rows count as "now" so those facts insert
    // before the live tail instead of after it.
    if (i !== firstUserIdx) takeBefore(mergeTs(maRows[i]))
    out.push(maRows[i])
  }
  while (ei < extras.length) out.push(extras[ei++].msg)
  return [...clusterSiblingSessions(out), ...queued]
}

/** Pack adjacent specialist cards into one fanout group (Coder + Reviewer
 *  between Maestro facts), same chrome as a wave of child workspaces. */
function clusterSiblingSessions(msgs: AgentMsg[]): AgentMsg[] {
  const out: AgentMsg[] = []
  let group: ChatLogSession[] = []
  let groupTs = 0
  const flush = () => {
    if (!group.length) return
    if (group.length === 1) {
      out.push({ ts: groupTs, log: group[0] })
    } else {
      out.push({
        ts: groupTs,
        log: {
          kind: 'sessions',
          id: group[0].id,
          sessions: group,
        },
      })
    }
    group = []
  }
  for (const m of msgs) {
    if (m.log?.kind === 'session') {
      if (!group.length) groupTs = m.ts || 0
      group.push(m.log)
    } else {
      flush()
      out.push(m)
    }
  }
  flush()
  return out
}
