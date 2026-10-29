<script lang="ts">
  // One wave of the board: the children started together (and what had just
  // landed to start them — waves are dispatch records, not barriers: their
  // children merge independently and later waves interleave), with live state
  // read from the workspace store (so the card updates as children run,
  // merge, or need you) and the engine's final outcome once a child is
  // archived. A live row opens the child; a settled row expands in
  // place into the child's FULL loop transcript — conductor turns, specialist
  // cards, and engine facts interleaved exactly as its own loop tab showed
  // them (same mergeLoopCockpit, same AgentChat, read-only). Sessions and
  // facts outlive the archive, so this works after the worktree is gone.
  import { tick } from 'svelte'
  import { get } from 'svelte/store'
  import { workspaces, workspacesRunning, openWorkspace, childTranscriptRequest } from '../lib/stores'
  import { workspaceIsRunning, chatModeMeta } from '../lib/modes'
  import { WorkspacesService, ObservatoryService } from '../lib/api'
  import { mapChatItems } from '../lib/agentchat'
  import { isPairChatCode, mergeLoopCockpit, type FeedSess } from '../lib/loop-cockpit'
  import AgentChat, { type AgentMsg, type ChatLogSession, type FindingRef } from './AgentChat.svelte'
  import SessionCard from './SessionCard.svelte'
  import Icon from './Icon.svelte'

  type FanOutChild = { id: string; title: string; steps: string[]; mode?: string; state?: string; note?: string }
  type FanOutWave = { wave: number; title?: string; after?: string[]; children: FanOutChild[]; done?: boolean }

  let { data, fallback = '' }: {
    data: string
    fallback?: string
  } = $props()

  const wave = $derived.by((): FanOutWave | null => {
    try {
      const w = JSON.parse(data) as FanOutWave
      return w && Array.isArray(w.children) ? w : null
    } catch {
      return null
    }
  })

  type RowState = { key: 'running' | 'reviewing' | 'needs' | 'merged' | 'failed' | 'conflict' | 'queued' | 'done'; label: string; live: boolean }

  function rowState(c: FanOutChild): RowState {
    const ws = $workspaces.find((w) => w.id === c.id)
    const final = (c.state ?? '').toLowerCase()
    if (final.startsWith('conflict')) return { key: 'conflict', label: c.state!, live: false }
    if (final.startsWith('failed') || final.startsWith('stopped')) return { key: 'failed', label: c.state!, live: false }
    if (final === 'closed') return { key: 'done', label: 'closed', live: false }
    if (final === 'merged' || (ws && (ws.state === 'merged' || ws.archived))) return { key: 'merged', label: 'merged', live: false }
    if (!ws) return { key: 'queued', label: c.state || 'starting', live: false }
    const running = workspaceIsRunning(ws, $workspacesRunning)
    // A chat child's row stays 'paused' mid-turn and its phase is the
    // creation placeholder ("editing"), so the live agent set decides
    // "running" and only loop children (step chips) show a phase.
    if (running && ws.state !== 'escalated') {
      const phase = c.steps.length && ws.phase && ws.phase !== 'idle' && ws.phase !== 'starting' ? ws.phase : 'working'
      return { key: 'running', label: phase, live: true }
    }
    if (!c.steps.length && !['failed', 'stopped', 'escalated'].includes(ws.state)) {
      return { key: 'done', label: c.state === 'settled' || ws.phase === 'idle' ? 'finished' : 'idle', live: false }
    }
    switch (ws.state) {
      case 'escalated':
        return { key: 'needs', label: 'needs you', live: false }
      case 'reviewing':
        return { key: 'reviewing', label: 'reviewing', live: running }
      case 'coding':
      case 'planning':
        return { key: 'running', label: ws.phase || ws.statusLabel || 'working', live: running }
      case 'queued':
        return { key: 'queued', label: 'queued', live: false }
      case 'failed':
      case 'stopped':
        return { key: 'failed', label: ws.state, live: false }
      case 'paused':
        return { key: 'done', label: ws.phase === 'artifact' || ws.phase === 'idle' ? 'finished' : 'paused', live: false }
      default:
        return { key: 'running', label: ws.statusLabel || ws.state, live: running }
    }
  }

  /** The child still exists as a workspace you can work in. */
  function isOpenable(c: FanOutChild): boolean {
    const ws = get(workspaces).find((w) => w.id === c.id)
    return !!ws && !ws.archived
  }

  // ---- inline transcripts for settled children ----
  type Transcript = { loading: boolean; messages: AgentMsg[]; findings: FindingRef[] }
  let openIds = $state<Record<string, boolean>>({})
  let transcripts = $state<Record<string, Transcript>>({})

  async function loadTranscript(c: FanOutChild) {
    const id = c.id
    if (transcripts[id]) return
    transcripts = { ...transcripts, [id]: { loading: true, messages: [], findings: [] } }
    // Everything is scoped to the tab that owned the child's run: the loop
    // conductor (MA) for a fan-out child, the default chat tab for a chat
    // child (no step chips), whose whole run is one PA conversation.
    const ws = get(workspaces).find((w) => w.id === id)
    const owner = c.steps.length ? ws?.loopChatCode || 'MA' : 'PA'
    const [items, sv, fx, findings] = await Promise.all([
      WorkspacesService.WorkspaceChatItems(id, owner).catch(() => null),
      WorkspacesService.WorkspaceSessions(id, owner, '').catch(() => null),
      WorkspacesService.WorkspaceFacts(id).catch(() => null),
      ObservatoryService.ListFindings(id).catch(() => [] as FindingRef[]),
    ])
    const conductor = mapChatItems(items ?? [])
    const sessions: FeedSess[] = (sv ?? [])
      .filter((s) => !isPairChatCode(s.code))
      .map((s) => ({
        id: s.id, code: s.code, name: s.name, kind: s.kind, ts: s.createdAt,
        messages: mapChatItems(s.items),
      }))
    const messages = mergeLoopCockpit(conductor, sessions, fx ?? [], new Set(), ws?.goal ?? '')
    transcripts = { ...transcripts, [id]: { loading: false, messages, findings: findings ?? [] } }
  }

  function toggleTranscript(c: FanOutChild) {
    const next = !openIds[c.id]
    openIds = { ...openIds, [c.id]: next }
    if (next) void loadTranscript(c)
  }

  function onRow(c: FanOutChild) {
    if (isOpenable(c)) openWorkspace(c.id)
    else toggleTranscript(c)
  }

  // A plan-panel link asked for one of our children: expand it, bring the row
  // into view, and flash it so the jump is legible.
  let rootEl = $state<HTMLElement | undefined>(undefined)
  let flashId = $state('')
  $effect(() => {
    const id = $childTranscriptRequest
    const child = id && wave ? wave.children.find((c) => c.id === id) : undefined
    if (!id || !child) return
    childTranscriptRequest.set(null)
    flashId = id
    setTimeout(() => { if (flashId === id) flashId = '' }, 1400)
    const row = () => rootEl?.querySelector<HTMLElement>(`[data-child="${id}"]`)
    if (!openIds[id]) openIds = { ...openIds, [id]: true }
    void loadTranscript(child).then(tick).then(() => {
      // The parent transcript has settled on the card by now; nudge the row in
      // case the expanded body pushed it, without dragging outer scrollers.
      row()?.scrollIntoView({ block: 'nearest' })
    })
  })

  const summary = $derived.by(() => {
    if (!wave) return ''
    const n = wave.children.length
    const merged = wave.children.filter((c) => rowState(c).key === 'merged').length
    if (merged === n) return `${n} merged`
    return `${merged} of ${n} merged`
  })
</script>

{#if wave}
  <div class="fanout-card" bind:this={rootEl}>
    <div class="fanout-head">
      <span class="ico" aria-hidden="true"><Icon name="git-fork" /></span>
      <span class="fanout-title">{wave.title ?? `Wave ${wave.wave}`}</span>
      {#if wave.after?.length}<span class="fanout-after" title="Started when these steps landed">after {wave.after.join(', ')}</span>{/if}
      <span>{wave.children.length} child workspace{wave.children.length === 1 ? '' : 's'}</span>
      <span class="fanout-sum">{summary}</span>
    </div>
    {#each wave.children as c (c.id)}
      {@const st = rowState(c)}
      {@const openable = isOpenable(c)}
      {@const expanded = !!openIds[c.id]}
      <button
        type="button"
        class="fanout-row clickable"
        class:expandable={!openable}
        class:expanded
        class:flash={flashId === c.id}
        data-child={c.id}
        aria-expanded={openable ? undefined : expanded}
        title={openable ? `Open ${c.id}` : (c.note ? `${c.note} — click for the transcript` : 'Show the child transcript')}
        onclick={() => onRow(c)}
      >
        <!-- Step chips; on hover an expandable row swaps them for a chevron,
             the same swap the sidebar's project pictures do. -->
        <span class="fanout-steps">
          <span class="fanout-chips">{#if c.mode}{@const mm = chatModeMeta(c.mode)}<span class="tstep" style="color:{mm.tint};background:color-mix(in srgb, {mm.tint} 14%, transparent)">{mm.label}</span>{/if}{#each c.steps as s (s)}<span class="tstep">{s}</span>{/each}</span>
          {#if !openable}<span class="fanout-car" aria-hidden="true"><Icon name="chevron-down" strokeWidth={2.4} /></span>{/if}
        </span>
        <span class="fanout-name">
          {c.title}
          {#if !c.steps.length && c.note}<span class="fanout-note">{c.note}</span>{/if}
        </span>
        <span class="fanout-state {st.key}">
          {#if st.live}
            <span class="sd" aria-label="running"></span>
          {:else if st.key === 'merged'}
            <Icon name="git-merge" />
          {:else if st.key === 'needs'}
            <Icon name="circle-alert" />
          {:else if st.key === 'failed' || st.key === 'conflict'}
            <Icon name="octagon-x" />
          {/if}
          {st.label}
        </span>
      </button>
      {#if expanded && !openable}
        {@const t = transcripts[c.id]}
        <div class="fanout-transcript">
          {#if !t || t.loading}
            <span class="fanout-transcript-empty">Loading transcript…</span>
          {:else if t.messages.length === 0}
            <span class="fanout-transcript-empty">No transcript was recorded for this child.</span>
          {:else}
            <div class="sess-body fanout-body">
              {#snippet childSessionCard(log: ChatLogSession)}
                <SessionCard
                  code={log.code}
                  name={log.name}
                  messages={log.messages}
                  live={false}
                  rootPath={$workspaces.find((w) => w.id === c.id)?.worktreePath ?? ''}
                  workspaceId={c.id}
                  findings={t.findings ?? []}
                />
              {/snippet}
              <AgentChat messages={t.messages} readonly workspaceId={c.id} sessionCard={childSessionCard} emptyText="No transcript was recorded for this child." />
            </div>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{:else}
  <div class="tc-head static"><span class="tc-meta">{@html fallback}</span></div>
{/if}
