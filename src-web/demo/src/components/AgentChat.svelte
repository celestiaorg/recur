<script module lang="ts">
  // Hoisted so svelte2tsx can export AgentMsg / ChatLog* without a cycle
  // (AgentMsg → ChatLog → ChatLogSession.messages: AgentMsg[]).
  import type { DigestFindingRef } from '../lib/agentchat'

  export type ChatAttachmentDraft = {
    id: string
    name: string
    mime: string
    dataUrl: string
  }

  export type ChatAttachmentView = {
    id: string
    name?: string
    mime?: string
    url?: string
    dataUrl?: string
  }

  export type FindingRef = DigestFindingRef & {
    statusReason?: string
  }

  export type ChatLogFact = {
    kind: 'fact'
    id?: string
    text: string
    isDeviation?: boolean
    code?: string
    name?: string
    /** Typed card ("fanout") rendered from data instead of text. */
    card?: string
    data?: string
  }
  export type ChatLogSession = {
    kind: 'session'
    id?: string
    code: string
    name: string
    messages: AgentMsg[]
    live: boolean
  }
  /** Consecutive specialist rows packed into one fanout card. */
  export type ChatLogSessions = {
    kind: 'sessions'
    id?: string
    sessions: ChatLogSession[]
  }
  export type ChatLog = ChatLogSession | ChatLogSessions | ChatLogFact

  export type AgentMsg = {
    body?: string
    me?: boolean
    note?: boolean
    /** Notice row label; defaults to "Notice". */
    noteLabel?: string
    /** Engine-fact markup (commit/handoff). Ordinary notices stay escaped text. */
    noteHtml?: boolean
    streaming?: boolean
    /**
     * Trailing store-mapped assistant row after a remount lost `streaming`.
     * `applyStream` may widen this row; a non-prefix snapshot opens a new one.
     */
    widenStream?: boolean
    tool?: string
    toolInput?: string
    toolId?: string
    description?: string
    input?: string
    result?: string
    resultError?: boolean
    resultTruncated?: boolean
    resultPending?: boolean
    turnEnd?: boolean
    durationMs?: number
    messageId?: string
    /** Turn footer: this turn changed files (turn diff). */
    checkpoint?: boolean
    stopped?: boolean
    error?: boolean
    thinking?: boolean
    tokens?: number
    compact?: boolean
    compacting?: boolean
    fallback?: boolean
    providerUUID?: string
    attachments?: ChatAttachmentView[]
    queued?: boolean
    pendingId?: string
    optimistic?: boolean
    ts?: number
    log?: ChatLog
    /** Nested subagent output rendered inside the parent Task card. */
    children?: AgentMsg[]
  }
</script>

<script lang="ts">
  // Agent chat surface: user/assistant text, tool steps (contiguous runs fold
  // into an "N tool calls" header), turn footers, loop cockpit cards, and the
  // composer.
  import type { Snippet } from 'svelte'
  import { highlightBash } from '../lib/shhl'
  import { lastAssistantTextInTurn, stepRowKey } from '../lib/agentchat'
  import { fadeScrollEdges } from '../lib/scroll-fade'
  import FanOutCard from './FanOutCard.svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import ContextUsageMeter from './ContextUsageMeter.svelte'
  import Icon from './Icon.svelte'

  let {
    messages = [],
    statusText = '',
    onSend,
    canStop = false,
    compact = false,
    resetKey = '',
    emptyText = '',
    placeholder = '',
    toolbar,
    readonly = false,
    onTurnDiff,
    sessionCard,
  }: {
    messages?: AgentMsg[]
    statusText?: string
    onSend?: (text: string) => void
    /** Loop walk is live on this tab. */
    canStop?: boolean
    compact?: boolean
    resetKey?: string
    emptyText?: string
    placeholder?: string
    toolbar?: Snippet
    /** Transcript-only mode: no composer dock (SessionCard in the Loop cockpit). */
    readonly?: boolean
    onTurnDiff?: (messageId: string) => void
    sessionCard?: Snippet<[ChatLogSession]>
  } = $props()

  type Row = { m: AgentMsg; i: number }
  type Block = { kind: 'msg'; m: AgentMsg; i: number } | { kind: 'steps'; start: number; rows: Row[] }
  const blocks = $derived.by<Block[]>(() => {
    const out: Block[] = []
    let steps: Extract<Block, { kind: 'steps' }> | null = null
    messages.forEach((m, i) => {
      if (m.tool && !m.turnEnd) {
        if (steps) steps.rows.push({ m, i })
        else out.push((steps = { kind: 'steps', start: i, rows: [{ m, i }] }))
      } else {
        steps = null
        out.push({ kind: 'msg', m, i })
      }
    })
    return out
  })

  let openGroups = $state<Record<number, boolean>>({})
  let openRows = $state<Record<string, boolean>>({})
  $effect(() => {
    void resetKey
    openGroups = {}
    openRows = {}
    nearBottom = true
  })

  function baseName(p: string): string {
    return p.slice(p.lastIndexOf('/') + 1)
  }
  function parsedInput(m: AgentMsg): Record<string, any> | null {
    try {
      const v = JSON.parse(m.input ?? '')
      return v && typeof v === 'object' ? v : null
    } catch {
      return null
    }
  }
  function fileOf(m: AgentMsg): string {
    const p = parsedInput(m)
    const fp = p?.file_path ?? p?.file
    return typeof fp === 'string' ? fp : ''
  }
  function toolLabel(m: AgentMsg): string {
    if (m.description) return m.description
    const f = fileOf(m)
    if (m.tool === 'Bash') return 'Ran command'
    if (m.tool === 'Read') return f ? `Read ${baseName(f)}` : 'Read file'
    return m.tool ?? 'Tool'
  }
  function bashCommand(m: AgentMsg): string {
    const c = parsedInput(m)?.command
    return typeof c === 'string' ? c : (m.input ?? '')
  }
  function groupSummary(rows: Row[]): string {
    const files = new Set(rows.map((r) => fileOf(r.m)).filter(Boolean))
    const parts = [`${rows.length} tool call${rows.length === 1 ? '' : 's'}`]
    if (files.size) parts.push(`${files.size} file${files.size === 1 ? '' : 's'}`)
    return parts.join(' · ')
  }

  // formatDuration renders a finished turn's wall time. Sub-minute turns read
  // better in seconds ("12.4s") than as 0:12.
  function formatDuration(ms: number): string {
    if (ms <= 0) return ''
    const sec = ms / 1000
    if (sec < 60) return `${sec < 10 ? sec.toFixed(1) : Math.round(sec)}s`
    const total = Math.round(sec)
    const m = Math.floor(total / 60)
    const s = String(total % 60).padStart(2, '0')
    if (m >= 60) return `${Math.floor(m / 60)}h ${m % 60}m ${s}s`
    return `${m}m ${s}s`
  }

  let copied = $state('')
  function copy(text: string, key: string, e: Event) {
    e.stopPropagation()
    void navigator.clipboard.writeText(text).then(() => {
      copied = key
      setTimeout(() => { if (copied === key) copied = '' }, 1200)
    })
  }

  let chatScrollEl = $state<HTMLDivElement | undefined>(undefined)
  let chatColEl = $state<HTMLDivElement | undefined>(undefined)
  let nearBottom = $state(true)
  function onChatScroll() {
    const el = chatScrollEl!
    nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24
  }
  $effect(() => {
    const col = chatColEl
    if (!col) return
    const ro = new ResizeObserver(() => {
      if (nearBottom) chatScrollEl?.scrollTo({ top: chatScrollEl.scrollHeight })
    })
    ro.observe(col)
    return () => ro.disconnect()
  })
  const fadeChatScroll = fadeScrollEdges({ botSlack: 24, pinnedToBottom: () => nearBottom })

  let composerEl = $state<HTMLDivElement | undefined>(undefined)
  let input = $state('')
  function send() {
    const text = input.trim()
    if (!text) return
    onSend?.(text)
    input = ''
    if (composerEl) composerEl.textContent = ''
    nearBottom = true
  }
</script>

{#snippet toolCard(m: AgentMsg, rowKey: string)}
  {@const open = !!openRows[rowKey]}
  <div class="tc" class:err={m.resultError}>
    <div
      class="tc-head"
      role="button"
      tabindex="0"
      aria-expanded={open}
      onclick={() => (openRows = { ...openRows, [rowKey]: !open })}
      onkeydown={(e) => { if (e.key === 'Enter') openRows = { ...openRows, [rowKey]: !open } }}
    >
      <span class="tc-ico" aria-hidden="true">
        <span class="ico"><Icon name={m.tool === 'Bash' ? 'chevrons-right' : m.tool === 'Read' ? 'file' : 'flask'} /></span>
        <span class="ex">{open ? '−' : '+'}</span>
      </span>
      <span class="tc-label">{toolLabel(m)}</span>
      {#if m.resultError}<span class="tc-errbadge">failed</span>{/if}
      {#if m.tool === 'Bash'}
        <button
          type="button"
          class="tc-copy"
          class:copied={copied === rowKey}
          title="Copy command"
          aria-label="Copy command"
          onclick={(e) => copy(bashCommand(m), rowKey, e)}
        >
          <Icon name={copied === rowKey ? 'check' : 'copy'} strokeWidth={copied === rowKey ? 2.4 : undefined} />
        </button>
      {/if}
    </div>
    {#if open}
      <div class="tc-body">
        <div class="tc-card">
          {#if m.tool === 'Bash'}
            <pre class="tc-pre"><span class="sh-prompt">$ </span>{@html highlightBash(bashCommand(m))}{#if m.result}{'\n\n'}<span class="tc-outtext" class:tc-outerr={m.resultError}>{m.result}</span>{/if}</pre>
          {:else if m.tool === 'Read' && m.result}
            <pre class="tc-pre tc-out">{m.result}</pre>
          {:else}
            <pre class="tc-pre tc-cmd">{m.input}</pre>
            {#if m.result}<pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{m.result}</pre>{/if}
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/snippet}

<div class="chat" class:compact>
  <div class="chat-main">
    <div class="chat-scroll" bind:this={chatScrollEl} onscroll={onChatScroll} {@attach fadeChatScroll}>
      <div class="chat-col" bind:this={chatColEl}>
        {#if messages.length === 0}
          <div class="chat-empty">{emptyText}</div>
        {/if}
        {#each blocks as b (resetKey + ':' + (b.kind === 'steps' ? 's' + b.start : 'm' + b.i))}
          <div class="cblk">
            {#if b.kind === 'msg'}
              {#if b.m.log?.kind === 'fact'}
                <div class="tc loop-fact" class:loop-card={!!b.m.log.card} role="status">
                  {#if b.m.log.card === 'fanout' && b.m.log.data}
                    <FanOutCard data={b.m.log.data} />
                  {:else}
                    <div class="tc-head static">
                      <span class="tc-ico" aria-hidden="true"><span class="ico"><Icon name="activity" /></span></span>
                      <span class="tc-label">{b.m.log.name || b.m.log.code}</span>
                      <span class="tc-meta">{b.m.log.text}</span>
                    </div>
                  {/if}
                </div>
              {:else if b.m.log?.kind === 'session' || b.m.log?.kind === 'sessions'}
                {#if sessionCard}
                  <div class="fanout-card sesscard">
                    {#each b.m.log.kind === 'sessions' ? b.m.log.sessions : [b.m.log] as s (s.id || s.code)}
                      {@render sessionCard(s)}
                    {/each}
                  </div>
                {/if}
              {:else if b.m.turnEnd}
                {@const reply = lastAssistantTextInTurn(messages, b.i)}
                {@const copyKey = 'turn-' + (b.m.messageId || b.i)}
                <div class="turn-end-wrap">
                  <div class="turn-end">
                    <span>completed in {formatDuration(b.m.durationMs ?? 0)}</span>
                    {#if reply || (!readonly && b.m.messageId && b.m.checkpoint)}
                      <div class="turn-end-acts">
                        {#if reply}
                          <button
                            type="button"
                            class="turn-end-act"
                            class:copied={copied === copyKey}
                            title="Copy reply"
                            aria-label="Copy last assistant message"
                            onclick={(e) => copy(reply.body ?? '', copyKey, e)}
                          >
                            <Icon name={copied === copyKey ? 'check' : 'copy'} strokeWidth={copied === copyKey ? 2.4 : undefined} />
                          </button>
                        {/if}
                        {#if !readonly && b.m.messageId && b.m.checkpoint && onTurnDiff}
                          <button
                            type="button"
                            class="turn-end-act"
                            title="Changes this turn"
                            aria-label="Show worktree changes made during this turn"
                            onclick={() => onTurnDiff?.(b.m.messageId!)}
                          >
                            <Icon name="file-diff" />
                          </button>
                        {/if}
                      </div>
                    {/if}
                  </div>
                </div>
              {:else if b.m.me}
                {@const bubbleKey = 'm' + b.i}
                <div class="chat-row me">
                  <div class="chat-bubble">
                    {b.m.body}
                    <button
                      type="button"
                      class="chat-bubble-copy"
                      class:copied={copied === bubbleKey}
                      title="Copy message"
                      aria-label="Copy message"
                      onclick={(e) => copy(b.m.body ?? '', bubbleKey, e)}
                    >
                      <Icon name={copied === bubbleKey ? 'check' : 'copy'} strokeWidth={copied === bubbleKey ? 2.4 : undefined} />
                    </button>
                  </div>
                </div>
              {:else if b.m.body}
                <div class="chat-row assistant">
                  <MarkdownHtml text={b.m.body} />
                </div>
              {/if}
            {:else if b.rows.length === 1}
              <div class="steps">
                {@render toolCard(b.rows[0].m, stepRowKey(b.rows[0].m.toolId, b.rows[0].i))}
              </div>
            {:else}
              {@const open = !!openGroups[b.start]}
              <div class="steps" class:open>
                <button type="button" class="steps-head" onclick={() => (openGroups = { ...openGroups, [b.start]: !open })} aria-expanded={open}>
                  <span class="tc-chev" class:open aria-hidden="true">
                    <Icon name="chevron-right" strokeWidth={2.4} />
                  </span>
                  {groupSummary(b.rows)}
                </button>
                {#if open}
                  <div class="steps-rows">
                    {#each b.rows as r (stepRowKey(r.m.toolId, r.i))}
                      {@render toolCard(r.m, stepRowKey(r.m.toolId, r.i))}
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
        {#if statusText}
          <div class="chat-turn" role="status" aria-live="polite">
            <span class="chat-think-dots" aria-hidden="true"><i></i><i></i><i></i></span>
            <span class="chat-turn-status">{statusText}</span>
          </div>
        {/if}
      </div>
    </div>
  </div>
  {#if !readonly}
    <div class="chat-dock">
      <div class="chat-in">
        <div
          bind:this={composerEl}
          class="chat-composer"
          class:empty={!input.trim()}
          role="textbox"
          tabindex="0"
          aria-multiline="true"
          aria-label={placeholder}
          data-placeholder={placeholder}
          contenteditable="true"
          spellcheck="false"
          oninput={() => (input = composerEl?.innerText ?? '')}
          onkeydown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              void send()
            }
          }}
        ></div>
        <div class="chat-in-row">
          <div class="chat-in-tools">{@render toolbar?.()}</div>
          <div class="chat-in-actions">
            <ContextUsageMeter />
            {#if canStop}
              <button class="send stop" type="button" aria-label="Stop agent" title="Stop (⌘⇧⌫)">
                <Icon name="octagon-x" />
              </button>
            {/if}
            <button class="send" type="button" aria-label="Send message" title="Send" disabled={!input.trim()} onclick={send}>
              <Icon name="send" />
            </button>
          </div>
        </div>
      </div>
    </div>
  {/if}
</div>
