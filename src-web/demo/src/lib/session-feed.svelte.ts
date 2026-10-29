// Live workspace session + fact feed. One instance per WorkspaceView; Plan panel
// and the Loop cockpit both read it.

import { WorkspacesService, onSessionDelta, onSessionStep, onSessionThinking, onSessionToolResult, onSessionToolInput, onSessionTurnEnd, onSessionCompact, onSessionNotice, onSessionUser, onSessionFallback, onAgent, onFeed, isAgentRunningStatus } from './api'
import { mapChatItems, appendStream, applyStream, addTool, addUser, attachToolResult, patchToolInput, noteThinkingTokens, endTurn, applyCompact, applyNotice, applyFallback, mergePersistedWithLive } from './agentchat'
import type { AgentMsg } from '../components/AgentChat.svelte'
import type { FeedItem } from '../../bindings/recur/internal/domain/models'
import type { FeedSess } from './loop-cockpit'
import { isPairChatCode, isSpecialistSession, needsLiveTurnBreak, specialistName } from './loop-cockpit'
import { SessionLiveFlags } from './session-live'

function isLoopFact(f: FeedItem): boolean {
  return !!f.isDeviation || f.kind === 'outcome' || f.kind === 'deviation' || (!f.kind && !!f.text)
}

function codeFromAgentId(workspaceId: string, agentId: string): string {
  const prefix = workspaceId + '-'
  if (!agentId.startsWith(prefix)) return ''
  return agentId.slice(prefix.length)
}

export class SessionFeed {
  sessions = $state<FeedSess[]>([])
  facts = $state<FeedItem[]>([])
  liveCodes = $state<Set<string>>(new Set())

  #workspaceId = ''
  #chatCode = ''
  #generation = 0
  #ownerOf: Record<string, string> = {}
  #streamSeqByKey: Record<string, number> = {}
  #unsubs: (() => void)[] = []
  #reloadGenByCode: Record<string, number> = {}
  #live = new SessionLiveFlags()

  attach(workspaceId: string, chatCode = '') {
    if (workspaceId === this.#workspaceId && chatCode === this.#chatCode) return
    const sameWs = workspaceId === this.#workspaceId
    if (!sameWs) {
      this.detach()
      this.#workspaceId = workspaceId
    }
    this.#chatCode = chatCode
    this.#generation++
    this.sessions = []
    this.facts = []
    this.#ownerOf = {}
    this.#live.reset()
    this.liveCodes = new Set()
    this.#streamSeqByKey = {}
    this.#reloadGenByCode = {}
    if (!workspaceId) return
    void this.#load(workspaceId)
    if (!sameWs) this.#listen()
  }

  detach() {
    this.#generation++
    for (const u of this.#unsubs) u()
    this.#unsubs = []
    this.#workspaceId = ''
    this.#chatCode = ''
    this.#live.reset()
  }

  async #load(id: string) {
    const gen = this.#generation
    const owner = this.#chatCode
    const [sv, fx] = await Promise.all([
      WorkspacesService.WorkspaceSessions(id, owner, '').catch(() => null),
      WorkspacesService.WorkspaceFacts(id).catch(() => null),
    ])
    if (gen !== this.#generation) return
    const incoming: FeedSess[] = (sv ?? [])
      .filter((s) => !isPairChatCode(s.code) && this.#owns(s.ownerCode))
      .map((s) => ({
        id: s.id, code: s.code, name: s.name, kind: s.kind, ts: s.createdAt,
        messages: mapChatItems(s.items),
      }))
    this.sessions = this.#mergeLoaded(incoming)
    this.facts = this.#mergeFacts((fx ?? []).filter((f) => this.#owns(f.chatCode)))
  }

  #mergeFacts(incoming: FeedItem[]): FeedItem[] {
    const have = new Set(incoming.map((f) => f.id).filter(Boolean) as string[])
    const extras = this.facts.filter((f) => {
      if (f.id && have.has(f.id)) return false
      // Live stamps Date.now() when Ts is missing; persist uses event TS.
      // Same milestone, two clocks — don't keep both.
      return !incoming.some((x) =>
        x.text === f.text &&
        (x.agentCode || '') === (f.agentCode || '') &&
        Math.abs((x.ts || 0) - (f.ts || 0)) < 5000,
      )
    })
    return extras.length ? [...incoming, ...extras] : incoming
  }

  /** Light liveCodes so mergeLoopCockpit can inject a tail card. Don't stamp a
   *  session yet — upsertAgent races persist and Date.now() would sort the card
   *  before the prompt, then jump. */
  markRunning(codes: Iterable<string>) {
    for (const code of codes) {
      if (!this.#track(code)) continue
      this.#markRunning(code)
    }
  }

  #mergeLoaded(incoming: FeedSess[]): FeedSess[] {
    const liveByCode = new Map(this.sessions.map((s) => [s.code, s]))
    const seen = new Set<string>()
    const out: FeedSess[] = incoming.map((s) => {
      seen.add(s.code)
      const live = liveByCode.get(s.code)
      if (!live || !this.liveCodes.has(s.code)) return s
      return {
        ...s,
        id: live.id,
        ts: live.ts,
        liveTs: live.liveTs,
        name: s.name || live.name,
        messages: mergePersistedWithLive(s.messages, live.messages),
      }
    })
    for (const [code, live] of liveByCode) {
      if (!seen.has(code) && this.liveCodes.has(code)) out.push(live)
    }
    return out
  }

  #track(code: string): boolean {
    return !!code && isSpecialistSession({ code })
  }

  #owns(owner?: string): boolean {
    const tab = this.#chatCode
    if (!tab) return !owner
    if (!owner) return tab === 'PA' || tab === 'MA'
    return owner === tab
  }

  #forMe(id: string, code: string, owner?: string): boolean {
    if (id !== this.#workspaceId || !this.#track(code)) return false
    if (owner) this.#ownerOf[code] = owner
    const known = owner || this.#ownerOf[code]
    if (!known) return true
    return this.#owns(known)
  }

  #publishLive() {
    this.liveCodes = this.#live.snapshot()
  }

  #markRunning(code: string) {
    if (this.#live.markRunning(code)) {
      this.#publishLive()
      this.#touchLiveTs(code)
    }
  }

  #markSettled(code: string) {
    if (this.#live.markSettled(code)) this.#publishLive()
  }

  #clearLive(code: string) {
    if (this.#live.clearLive(code)) this.#publishLive()
  }

  #touchStream(code: string) {
    if (this.#live.touchFromStream(code)) {
      this.#publishLive()
      this.#touchLiveTs(code)
    }
  }

  #touchLiveTs(code: string) {
    const i = this.sessions.findIndex((s) => s.code === code)
    if (i < 0) return
    const next = this.sessions.slice()
    next[i] = { ...next[i], liveTs: Date.now() }
    this.sessions = next
  }

  async #reloadCode(code: string) {
    const id = this.#workspaceId
    if (!id) return
    const generation = this.#generation
    const owner = this.#chatCode
    const gen = (this.#reloadGenByCode[code] ?? 0) + 1
    this.#reloadGenByCode[code] = gen
    const sv = await WorkspacesService.WorkspaceSessions(id, owner, code).catch(() => null)
    if (generation !== this.#generation || this.#reloadGenByCode[code] !== gen) return
    const fresh = (sv ?? []).find((s) => s.code === code && this.#owns(s.ownerCode))
    if (!fresh) return
    const mapped: FeedSess = {
      id: fresh.id, code: fresh.code, name: fresh.name, kind: fresh.kind, ts: fresh.createdAt,
      messages: mapChatItems(fresh.items),
    }
    const i = this.sessions.findIndex((s) => s.code === code)
    const live = i >= 0 ? this.sessions[i] : undefined
    const nextSess = live && this.liveCodes.has(code)
      ? { ...mapped, id: live.id, ts: live.ts, liveTs: live.liveTs, name: mapped.name || live.name, messages: mergePersistedWithLive(mapped.messages, live.messages) }
      : mapped
    if (i < 0) this.sessions = [...this.sessions, nextSess]
    else {
      const next = this.sessions.slice()
      next[i] = nextSess
      this.sessions = next
    }
  }

  #patch(code: string, fn: (msgs: AgentMsg[]) => AgentMsg[]) {
    if (!this.#track(code)) return
    const now = Date.now()
    const i = this.sessions.findIndex((s) => s.code === code)
    if (i < 0) {
      this.sessions = [...this.sessions, {
        id: 'live-' + code, code, name: specialistName(code), kind: '', ts: now, liveTs: now, messages: fn([]),
      }]
    } else {
      const sess = this.sessions[i]
      const reopen = !this.liveCodes.has(code) && needsLiveTurnBreak(sess.messages)
      const msgs = reopen ? [...sess.messages, { turnEnd: true, ts: now }] : sess.messages
      const liveTs = sess.messages.length === 0 ? now
        : this.liveCodes.has(code) && sess.liveTs ? sess.liveTs : now
      const next = this.sessions.slice()
      next[i] = { ...sess, liveTs, messages: fn(msgs) }
      this.sessions = next
    }
    this.#touchStream(code)
  }

  #applyLive(code: string, kind: 'text' | 'thinking', text: string, opts: { seq?: number; replace?: boolean; done?: boolean; parentId?: string; uuid?: string }) {
    const key = `${this.#workspaceId}:${code}:${kind}${opts.parentId ? ':' + opts.parentId : ''}`
    const seq = opts.seq ?? 0
    if (seq > 0) {
      const prev = this.#streamSeqByKey[key] ?? 0
      if (seq <= prev) return
      this.#streamSeqByKey = { ...this.#streamSeqByKey, [key]: seq }
    }
    if (opts.replace || opts.done || opts.parentId) {
      this.#patch(code, (m) => applyStream(m, kind, text, { done: !!opts.done, parentId: opts.parentId, uuid: opts.uuid }))
    } else if (text) {
      this.#patch(code, (m) => appendStream(m, kind, text))
    }
  }

  #listen() {
    const idOf = () => this.#workspaceId
    this.#unsubs = [
      onSessionDelta((id, code, evt) => {
        if (!this.#forMe(id, code, evt.ownerCode)) return
        this.#applyLive(code, 'text', evt.text, evt)
      }),
      onSessionThinking((id, code, evt) => {
        if (!this.#forMe(id, code, evt.ownerCode)) return
        if (evt.text || evt.replace || evt.done) this.#applyLive(code, 'thinking', evt.text, evt)
        else this.#patch(code, (m) => noteThinkingTokens(m, evt.tokens, evt.uuid, evt.parentId))
      }),
      onSessionStep((id, code, step) => {
        if (this.#forMe(id, code, step.ownerCode)) this.#patch(code, (m) => addTool(m, step))
      }),
      onSessionToolResult((id, code, tid, result, isError, extras) => {
        if (this.#forMe(id, code)) this.#patch(code, (m) => attachToolResult(m, tid, result, isError, extras))
      }),
      onSessionToolInput((id, code, tid, extras) => {
        if (this.#forMe(id, code)) this.#patch(code, (m) => patchToolInput(m, tid, extras))
      }),
      onSessionTurnEnd((id, code, turn) => {
        if (!this.#forMe(id, code)) return
        this.#patch(code, (m) => endTurn(m, turn.durationMs))
        // Per provider run, not per working window (runCoder can emit several
        // turn_ends while still working). Drop live without latching settled so
        // the next nudge's deltas can re-light; idle is the latch.
        this.#clearLive(code)
      }),
      onSessionCompact((id, code, evt) => {
        if (this.#forMe(id, code)) this.#patch(code, (m) => applyCompact(m, evt))
      }),
      onSessionNotice((id, code, text) => {
        if (this.#forMe(id, code)) this.#patch(code, (m) => applyNotice(m, text))
      }),
      onSessionUser((id, code, text) => {
        if (this.#forMe(id, code)) this.#patch(code, (m) => addUser(m, text))
      }),
      onSessionFallback((id, code, evt) => {
        if (this.#forMe(id, code)) this.#patch(code, (m) => applyFallback(m, evt))
      }),
      onAgent((id, evt) => {
        if (id !== idOf() || evt.kind !== 'updated') return
        const code = codeFromAgentId(id, evt.agentId)
        if (!this.#track(code) || !this.#forMe(id, code)) return
        if (isAgentRunningStatus(evt.status)) {
          this.#markRunning(code)
          return
        }
        this.#markSettled(code)
        void this.#reloadCode(code)
      }),
      onFeed((id, event) => {
        if (id !== idOf() || !event) return
        const f = event as FeedItem
        if (!isLoopFact(f) || !this.#owns(f.chatCode)) return
        if (!f.ts) f.ts = Date.now()
        if (this.facts.some((x) => x.id && x.id === f.id)) {
          // Re-emitted card (progress): replace the payload, keep the slot.
          this.facts = this.facts.map((x) => (x.id === f.id ? { ...f, ts: x.ts || f.ts } : x))
        } else {
          this.facts = [...this.facts, f]
        }
      }),
    ]
  }
}
