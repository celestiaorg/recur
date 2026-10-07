<script lang="ts">
  // Operator surface for reviewer findings: master–detail list + inspector.
  // Dense list selects a finding (jump to file:line); detail pane holds the full
  // message, notes, and Ignore / Mark invalid / Reopen actions.
  // Engine-owned "resolved" is read-only for status; notes remain editable.
  import { tick } from 'svelte'
  import type { Finding } from '../../bindings/recur/internal/domain/models'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'
  import { findingBodyNeeded, findingSevIcon, findingTitle } from '../lib/findings-ui'
  import { formatRelativeAge } from '../lib/format'
  import { isCmdOrCtrl } from '../lib/shortcut'

  export type FindingsStatusFilter = 'open' | 'ignored' | 'invalid' | 'resolved' | 'all'

  let {
    findings = [],
    filter = 'open',
    busyId = '',
    expandIds = [] as string[],
    focusNoteId = '',
    onOpen,
    onSetStatus,
    onAddNote,
    onDeleteNote,
  }: {
    findings?: Finding[]
    filter?: FindingsStatusFilter
    busyId?: string
    /** Finding ids that should be selected (e.g. deep-link from chat). */
    expandIds?: string[]
    /** When set, focus the note composer for this finding. */
    focusNoteId?: string
    onOpen: (f: Finding) => void
    onSetStatus: (f: Finding, status: string) => void
    onAddNote: (f: Finding, body: string) => void | Promise<void>
    onDeleteNote: (f: Finding, noteId: string) => void | Promise<void>
  } = $props()

  const SPLIT_KEY = 'recur.findingsSplitPct'
  const SPLIT_DEFAULT = 60

  let selectedId = $state('')
  let draftById = $state<Record<string, string>>({})
  /** Finding id with the note composer open (chat-style Add note toggle). */
  let composeFor = $state('')
  let panelEl = $state<HTMLDivElement | undefined>(undefined)
  let listEl = $state<HTMLDivElement | undefined>(undefined)
  let splitEl = $state<HTMLDivElement | undefined>(undefined)
  let scrollEl = $state<HTMLDivElement | undefined>(undefined)
  /** Bottom fade only while content continues past the viewport. */
  let fadeBot = $state(false)
  /** List pane height as % of the findings split (25–75). Default 60. */
  let splitPct = $state((() => {
    try {
      const n = Number(localStorage.getItem(SPLIT_KEY))
      if (Number.isFinite(n) && n >= 25 && n <= 75) return Math.round(n)
    } catch { /* ignore */ }
    return SPLIT_DEFAULT
  })())
  let splitResizing = $state(false)

  function clampSplitPct(n: number): number {
    return Math.min(75, Math.max(25, Math.round(n)))
  }
  function writeSplitPct() {
    try { localStorage.setItem(SPLIT_KEY, String(splitPct)) } catch { /* ignore */ }
  }
  function persistSplitPct(n: number) {
    splitPct = clampSplitPct(n)
    writeSplitPct()
  }
  function onSplitResizeDown(e: PointerEvent) {
    if (e.button !== 0) return
    e.preventDefault()
    e.stopPropagation()
    const el = e.currentTarget as HTMLElement
    const row = splitEl ?? el.closest('.findings-split') as HTMLDivElement | null
    if (!row) return
    el.setPointerCapture(e.pointerId)
    splitResizing = true
    document.body.classList.add('resizing-panels', 'resizing-rows')
    const rect = row.getBoundingClientRect()
    let pendingY = 0
    let raf = 0
    const apply = () => {
      raf = 0
      if (rect.height <= 0) return
      splitPct = clampSplitPct(((pendingY - rect.top) / rect.height) * 100)
    }
    const move = (ev: PointerEvent) => {
      pendingY = ev.clientY
      if (!raf) raf = requestAnimationFrame(apply)
    }
    const up = (ev: PointerEvent) => {
      try { el.releasePointerCapture(ev.pointerId) } catch { /* already released */ }
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      if (raf) {
        cancelAnimationFrame(raf)
        apply()
      }
      splitResizing = false
      document.body.classList.remove('resizing-panels', 'resizing-rows')
      writeSplitPct()
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }
  function onSplitResizeKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 5 : 2
    if (e.key === 'ArrowUp') { e.preventDefault(); persistSplitPct(splitPct - step) }
    else if (e.key === 'ArrowDown') { e.preventDefault(); persistSplitPct(splitPct + step) }
  }

  const visible = $derived(
    filter === 'all'
      ? findings
      : findings.filter((f) => (f.status || 'open') === filter),
  )

  const selected = $derived(visible.find((f) => f.id === selectedId) ?? null)

  $effect(() => {
    // Keep selection inside the filtered list; default to the first row.
    // Prefer an in-flight deep-link so it wins over the default pick.
    const keep = new Set(visible.map((f) => f.id))
    const jump = expandIds.find(Boolean) || focusNoteId
    if (jump && keep.has(jump)) {
      if (selectedId !== jump) selectedId = jump
    } else if (selectedId && !keep.has(selectedId)) {
      selectedId = visible[0]?.id ?? ''
    } else if (!selectedId && visible.length) {
      selectedId = visible[0].id
    }
    if (composeFor && !keep.has(composeFor)) composeFor = ''
  })

  $effect(() => {
    // Scroll once per expandIds change (selection applied in the keep effect).
    if (!expandIds.length) return
    const id = expandIds.find(Boolean) || ''
    if (!id) return
    void scrollSelectedIntoView(id)
  })

  $effect(() => {
    // Open the note composer once per focusNoteId change.
    const id = focusNoteId
    if (!id) return
    void openCompose(id)
    void scrollSelectedIntoView(id)
  })

  async function scrollSelectedIntoView(id: string) {
    await tick()
    const el = listEl?.querySelector(`[data-finding-id="${CSS.escape(id)}"]`) as HTMLElement | null
    el?.scrollIntoView({ block: 'nearest' })
  }

  function select(f: Finding) {
    if (composeFor && composeFor !== f.id) closeCompose(composeFor)
    selectedId = f.id
    if (f.file) onOpen(f)
  }

  async function focusNoteInput(_id: string) {
    await tick()
    const el = panelEl?.querySelector(
      `[data-finding-detail] .finding-note-input`,
    ) as HTMLTextAreaElement | null
    el?.focus()
  }

  async function openCompose(id: string) {
    composeFor = id
    if (!(id in draftById)) draftById = { ...draftById, [id]: '' }
    await focusNoteInput(id)
    await tick()
    updateScrollFade()
  }

  function closeCompose(id: string) {
    if (composeFor === id) composeFor = ''
    draftById = { ...draftById, [id]: '' }
    fadeBot = false
  }

  function updateScrollFade() {
    const el = scrollEl
    if (!el || composeFor !== selectedId) {
      fadeBot = false
      return
    }
    const maxScroll = el.scrollHeight - el.clientHeight
    fadeBot = maxScroll > 1 && el.scrollTop < maxScroll - 1
  }

  $effect(() => {
    // Remeasure when the selected finding or compose state changes layout.
    void selectedId
    void composeFor
    void selected?.message
    void selected?.notes?.length
    void tick().then(updateScrollFade)
  })

  // Compact age for list + status attribution; full stamp lives in the title tooltip.
  function statusWhen(iso: string | undefined): { rel: string; full: string } | null {
    if (!iso) return null
    return formatRelativeAge(Date.parse(iso))
  }

  // Who closed the finding (resolved / invalid / ignored). Actor labels match
  // the resolved-row copy the panel already used.
  function statusByLine(status: string, updatedBy: string | undefined): string {
    const by =
      updatedBy === 'coder' ? ' by coder'
      : updatedBy === 'reviewer' ? ' by reviewer'
      : updatedBy === 'engine' ? ' by the loop'
      : updatedBy === 'human' ? ' by you'
      : ''
    switch (status) {
      case 'resolved': return `Resolved${by || ''}`
      case 'invalid': return `Marked invalid${by}`
      case 'ignored': return `Ignored${by}`
      default: return ''
    }
  }

  function noteAuthor(author: string | undefined): string {
    if (author === 'human') return 'You'
    if (author === 'coder') return 'Coder'
    if (author === 'engine') return 'Loop'
    return author || 'Note'
  }

  function baseName(p: string): string {
    const i = p.lastIndexOf('/')
    return i >= 0 ? p.slice(i + 1) : p
  }

  function locSuffix(f: Finding): string {
    if (!f.line || f.line <= 0) return ''
    if (f.endLine && f.endLine > f.line) return `:${f.line}–${f.endLine}`
    return `:${f.line}`
  }

  function locLabel(f: Finding): string {
    const name = baseName(f.file || '')
    return name ? `${name}${locSuffix(f)}` : locSuffix(f).replace(/^:/, '') || '—'
  }

  /** Show status in the list only when the filter doesn't already imply it. */
  function showListStatus(status: string): boolean {
    return filter === 'all' || status !== filter
  }

  async function submitNote(f: Finding) {
    const body = (draftById[f.id] || '').trim()
    if (!body || busyId === f.id) return
    await onAddNote(f, body)
    closeCompose(f.id)
  }
</script>

<div class="findings-panel" bind:this={panelEl}>
  {#if !findings.length}
    <div class="obs-empty">No findings yet — run /review.</div>
  {:else if !visible.length}
    <div class="obs-empty">No {filter} findings.</div>
  {:else}
    <div
      class="findings-split"
      class:resizing={splitResizing}
      style={`--findings-split-pct:${splitPct}%`}
      bind:this={splitEl}
    >
      <div class="findings-list" role="list" aria-label="Findings" bind:this={listEl}>
        {#each visible as f (f.id)}
          {@const status = f.status || 'open'}
          {@const sel = f.id === selectedId}
          {@const when = statusWhen(f.updatedAt || f.createdAt)}
          {@const noteCount = f.notes?.length ?? 0}
          <div class="finding-row-wrap" role="listitem" data-finding-id={f.id}>
            <button
              type="button"
              class="finding-row-md"
              class:sel
              class:busy={busyId === f.id}
              aria-current={sel ? 'true' : undefined}
              onclick={() => select(f)}
            >
              <span class="finding-sev {f.severity}" aria-hidden="true">
                <Icon name={findingSevIcon(f.severity)} />
              </span>
              <span class="finding-row-main">
                <span class="finding-title">{findingTitle(f.message)}</span>
                <span class="finding-sub">
                  <span class="finding-loc" title={f.file}>{locLabel(f)}</span>
                  {#if showListStatus(status)}
                    <span class="finding-status {status}">{status}</span>
                  {/if}
                </span>
              </span>
              <span class="finding-row-trail">
                {#if noteCount > 0}
                  <span
                    class="finding-has-notes"
                    title={noteCount === 1 ? '1 note' : `${noteCount} notes`}
                    aria-label={noteCount === 1 ? '1 note' : `${noteCount} notes`}
                  >
                    <Icon name="notebook-pen" />
                  </span>
                {/if}
                {#if when}
                  <span class="finding-ago" title={when.full}>{when.rel}</span>
                {/if}
              </span>
            </button>
          </div>
        {/each}
      </div>

      <button
        type="button"
        class="hsplit findings-hsplit"
        class:dragging={splitResizing}
        aria-label="Resize findings list"
        title="Drag to resize"
        onpointerdown={onSplitResizeDown}
        onkeydown={onSplitResizeKey}
      ></button>

      <div class="finding-inspector" data-finding-detail class:busy={busyId === (selected?.id ?? '')}>
        {#if selected}
          {@const f = selected}
          {@const status = f.status || 'open'}
          {@const noting = composeFor === f.id}
          <div class="finding-inspector-head">
            <div class="finding-inspector-meta">
              <span class="tc-sev {f.severity}">{f.severity}</span>
              <h3 class="finding-inspector-title" title={findingTitle(f.message)}>{findingTitle(f.message)}</h3>
              {#if showListStatus(status) || status !== 'open'}
                <span class="finding-status {status}">{status}</span>
              {/if}
            </div>
            <div class="finding-actions">
              {#if status === 'open'}
                <button
                  type="button"
                  class="finding-act-btn"
                  disabled={busyId === f.id}
                  onclick={() => onSetStatus(f, 'ignored')}
                >
                  <Icon name="eye-off" />
                  Ignore
                </button>
                <button
                  type="button"
                  class="finding-act-btn danger"
                  disabled={busyId === f.id}
                  onclick={() => onSetStatus(f, 'invalid')}
                >
                  <Icon name="bug-off" />
                  Mark invalid
                </button>
              {:else if status === 'ignored' || status === 'invalid'}
                <button
                  type="button"
                  class="finding-act-btn"
                  disabled={busyId === f.id}
                  onclick={() => onSetStatus(f, 'open')}
                >
                  <Icon name="undo" />
                  Reopen
                </button>
              {/if}
              {#if !noting}
                <button
                  type="button"
                  class="finding-act-btn"
                  disabled={busyId === f.id}
                  onclick={() => openCompose(f.id)}
                >
                  <Icon name="notebook-pen" />
                  Add note
                </button>
              {/if}
            </div>
          </div>
          <div
            class="finding-inspector-scroll"
            class:composing={noting}
            class:fade-bot={noting && fadeBot}
            bind:this={scrollEl}
            onscroll={updateScrollFade}
          >
            <button
              type="button"
              class="finding-inspector-loc"
              title={f.file || undefined}
              disabled={!f.file}
              onclick={() => f.file && onOpen(f)}
            >{locLabel(f)}</button>
            {#if findingBodyNeeded(f.message || '')}
              <MarkdownHtml text={f.message} class="chat-canvas finding-body" />
            {/if}
            {#if f.statusReason}
              <p class="finding-reason"><span class="finding-reason-label">Reason</span> {f.statusReason}</p>
            {/if}
            {#if status !== 'open'}
              {@const when = statusWhen(f.updatedAt)}
              {@const by = statusByLine(status, f.updatedBy)}
              {#if by}
                <p class="finding-note" title={when?.full ?? undefined}>
                  {by}{#if when}<span class="finding-when"> · {when.rel}</span>{/if}
                </p>
              {/if}
            {/if}
            {#if f.notes?.length}
              <div class="finding-notes">
                {#each f.notes as n (n.id)}
                  {@const when = statusWhen(n.createdAt)}
                  <div class="finding-comment">
                    <p class="finding-comment-meta" title={when?.full ?? undefined}>
                      <span class="finding-reason-label">{noteAuthor(n.author)}</span>
                      {#if when}<span class="finding-when">{when.rel}</span>{/if}
                      {#if n.id}
                        <button
                          type="button"
                          class="finding-note-del"
                          title="Delete note"
                          aria-label="Delete note"
                          disabled={busyId === f.id}
                          onclick={() => void onDeleteNote(f, n.id)}
                        >
                          <Icon name="trash" />
                        </button>
                      {/if}
                    </p>
                    <MarkdownHtml text={n.body} class="chat-canvas finding-comment-body" />
                  </div>
                {/each}
              </div>
            {/if}
          </div>
          {#if noting}
            <div class="finding-note-dock">
              <div class="finding-note-compose">
                <textarea
                  class="finding-note-input"
                  rows="2"
                  placeholder="Add a note, clarification, or context…"
                  disabled={busyId === f.id}
                  value={draftById[f.id] ?? ''}
                  oninput={(e) => {
                    draftById = { ...draftById, [f.id]: e.currentTarget.value }
                  }}
                  onkeydown={(e) => {
                    if (e.key === 'Enter' && isCmdOrCtrl(e)) {
                      e.preventDefault()
                      void submitNote(f)
                    }
                    if (e.key === 'Escape') {
                      e.preventDefault()
                      closeCompose(f.id)
                    }
                  }}
                ></textarea>
                <div class="finding-note-compose-foot">
                  <span class="finding-note-hint"><Kbd text="⌘↵" /> to save</span>
                  <div class="finding-note-compose-actions">
                    <button
                      type="button"
                      class="finding-note-btn"
                      disabled={busyId === f.id}
                      onclick={() => closeCompose(f.id)}
                    >Cancel</button>
                    <button
                      type="button"
                      class="finding-note-btn primary"
                      disabled={busyId === f.id || !(draftById[f.id] || '').trim()}
                      onclick={() => void submitNote(f)}
                    >Save</button>
                  </div>
                </div>
              </div>
            </div>
          {/if}
        {:else}
          <div class="obs-empty">Select a finding.</div>
        {/if}
      </div>
    </div>
  {/if}
</div>
