<script lang="ts">
  // One file's diff: the df-head + line-numbered, syntax-highlighted hunks with
  // posted PR line comments. Reused by Files-changed and per-commit views.
  import { tick } from 'svelte'
  import type { FileDiff, PRComment } from '../lib/types'
  import { selectedPRId, prs } from '../lib/stores'
  import { splitDiffRows, splitRowDataLine } from '../lib/split-diff'
  import { lineNotesCoveringRow, lineNotesOnRow } from '../lib/comment-thread'
  import { fileDiffPathLabel } from '../lib/review-diff'
  import { paneScrollTo } from '../lib/review-scroll'
  import Icon from './Icon.svelte'
  import LineNoteThread from './LineNoteThread.svelte'

  let {
    file,
    highlightLine = 0,
    /** When false, hide posted PR line comments (observatory worktree diffs). */
    comments = true,
    layout = 'unified',
    /** Commit diffs can fold a file; Files changed shows one file at a time. */
    collapsible = false,
    /** Controlled collapse when `onToggleCollapse` is also passed. */
    collapsed = false,
    onToggleCollapse,
    onOpenFile = undefined,
    openFileTitle = 'Open in editor: ⌘-click any line number to jump there',
  }: {
    file: FileDiff
    highlightLine?: number
    comments?: boolean
    layout?: 'unified' | 'split'
    collapsible?: boolean
    collapsed?: boolean
    onToggleCollapse?: (next: boolean) => void
    onOpenFile?: (path: string, line?: number) => void
    openFileTitle?: string
  } = $props()

  let internalCollapsed = $state(false)
  const isCollapsed = $derived(onToggleCollapse ? collapsed : internalCollapsed)
  let rootEl: HTMLDivElement | undefined = $state()
  let collapsedHunks = $state<Set<number>>(new Set())

  const allLines = $derived(isCollapsed ? [] as NonNullable<FileDiff['lines']> : (file.lines ?? []))
  const splitRows = $derived(layout === 'split' && !isCollapsed ? splitDiffRows(allLines) : [])
  const prRow = $derived(comments ? $prs.find((p) => p.id === $selectedPRId) : undefined)
  const postedNotes = $derived((prRow?.comments ?? []) as PRComment[])
  function notesOn(line: { old: string; new: string; type?: string }, side?: 'left' | 'right'): PRComment[] {
    return comments ? lineNotesOnRow(postedNotes, file.path, line, side) : []
  }
  function notesCovering(line: { old: string; new: string; type?: string }, side?: 'left' | 'right'): PRComment[] {
    return comments ? lineNotesCoveringRow(postedNotes, file.path, line, side) : []
  }

  let copiedPath = $state(false)
  function copyPath() {
    void navigator.clipboard.writeText(file.path).then(() => {
      copiedPath = true
      setTimeout(() => { copiedPath = false }, 1200)
    })
  }

  function jumpToFile(e: MouseEvent, line?: number) {
    e.stopPropagation()
    onOpenFile?.(file.path, line)
  }

  function onLnClick(e: MouseEvent, n: number) {
    if (!onOpenFile || !n || !(e.metaKey || e.ctrlKey)) return
    e.preventDefault()
    e.stopPropagation()
    onOpenFile(file.path, n)
  }

  const hunkOf = $derived.by<number[]>(() => {
    const out: number[] = []
    let cur = -1
    allLines.forEach((l, i) => {
      if (l.type === 'hunk') cur = i
      out[i] = l.type === 'hunk' ? -1 : cur
    })
    return out
  })
  function toggleHunk(i: number) {
    const next = new Set(collapsedHunks)
    if (next.has(i)) next.delete(i)
    else next.add(i)
    collapsedHunks = next
  }
  function visible(i: number): boolean {
    const h = hunkOf[i]
    return h === -1 || !collapsedHunks.has(h)
  }

  function hunkLabel(html: string): string {
    const raw = (html ?? '').replace(/<[^>]+>/g, '')
    const m = raw.match(/^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@\s*(.*)$/)
    if (!m) return html
    const oldStart = +m[1], oldCount = m[2] != null ? +m[2] : 1
    const newStart = +m[3], newCount = m[4] != null ? +m[4] : 1
    const ctx = (m[5] ?? '').trim()
    let start = newStart, count = newCount
    if (count === 0) { start = oldStart; count = oldCount }
    const range = count <= 1 ? `Line ${start}` : `Lines ${start}–${start + count - 1}`
    const label = ctx ? `${range} · ${ctx}` : range
    return label.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  }

  function lineNo(l: { new: string; old: string }): number {
    return parseInt(l.new || l.old || '0', 10) || 0
  }

  function toggleCollapsed() {
    if (!collapsible) return
    const next = !isCollapsed
    if (onToggleCollapse) onToggleCollapse(next)
    else internalCollapsed = next
  }

  $effect(() => {
    const target = highlightLine
    if (!target) return
    void tick().then(() => {
      const el = rootEl?.querySelector<HTMLElement>(`.icmt-thread[data-note-line="${target}"]`)
        ?? rootEl?.querySelector<HTMLElement>(`[data-line="${target}"]`)
      const pane = rootEl?.closest<HTMLElement>('.diffscroll')
      if (el && pane) paneScrollTo(pane, el)
    })
  })
</script>

<div class="diff-file" bind:this={rootEl}>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="df-head"
    class:collapsible
    class:collapsed={isCollapsed}
    role={collapsible ? 'button' : undefined}
    tabindex={collapsible ? 0 : undefined}
    aria-expanded={collapsible ? !isCollapsed : undefined}
    onclick={collapsible ? toggleCollapsed : undefined}
    onkeydown={(e) => {
      if (!collapsible) return
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        toggleCollapsed()
      }
    }}
  >
    {#if collapsible}
      <Icon name="chevron-down" class="df-chev" strokeWidth={2.4} />
    {/if}
    {#if onOpenFile !== undefined}
      <button type="button" class="df-path" title={openFileTitle} onclick={(e) => jumpToFile(e)}>
        {fileDiffPathLabel(file)}
      </button>
    {:else}
      <span class="fp">{fileDiffPathLabel(file)}</span>
    {/if}
    <div class="df-head-actions">
      {#if onOpenFile !== undefined}
        <button type="button" class="obs-icon-btn df-open" title={openFileTitle} aria-label={openFileTitle} onclick={(e) => jumpToFile(e)}>
          <Icon name="code" />
        </button>
      {/if}
      <button type="button" class="obs-icon-btn" class:copied={copiedPath} title={copiedPath ? 'Copied' : 'Copy path'} aria-label={copiedPath ? 'Copied' : 'Copy path'} onclick={(e) => { e.stopPropagation(); copyPath() }}>
        {#if copiedPath}
          <Icon name="check" strokeWidth={2.4} />
        {:else}
          <Icon name="copy" />
        {/if}
      </button>
    </div>
    <div class="fd"><span class="a">+{file.diff.add}</span><span class="r">−{file.diff.del}</span></div>
  </div>
  {#if !isCollapsed}
    {#snippet postedThreads(line: { old: string; new: string; type?: string }, side?: 'left' | 'right')}
      {#each notesOn(line, side) as note (note.id)}
        <div class="icmt icmt-thread" data-note-line={note.line}>
          <LineNoteThread root={note} items={postedNotes} pr={prRow} />
        </div>
      {/each}
    {/snippet}
    {#if layout === 'split'}
      <div class="df-lines">
      {#each splitRows.filter((row) => row.kind === 'hunk' || visible(row.i)) as row (`${row.kind}-${row.i}`)}
        {#if row.kind === 'hunk'}
          <div class="hline hunk" role="button" tabindex="0" onclick={() => toggleHunk(row.i)} onkeydown={(e) => e.key === 'Enter' && toggleHunk(row.i)}>
            <span class="car">{collapsedHunks.has(row.i) ? '▸' : '▾'}</span><span class="cd">{@html hunkLabel(row.line.codeHtml)}</span>
          </div>
        {:else}
          {@const left = row.left}
          {@const right = row.right}
          {@const ln = splitRowDataLine(row)}
          <div class="hline split" data-line={ln || undefined}>
            <div
              class="side {left ? left.line.type : 'blank'}"
              class:noted={left ? notesCovering(left.line, 'left').length > 0 : false}
              class:note-end={left ? notesOn(left.line, 'left').length > 0 : false}
            >
              {#if left}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <span class="ln" onclick={(e) => onLnClick(e, parseInt(left.line.new || '0', 10))}>{left.line.old}</span>
                <span class="cd">{@html left.line.codeHtml}</span>
              {:else}
                <span class="ln"></span>
                <span class="cd"></span>
              {/if}
            </div>
            <div
              class="side {right ? right.line.type : 'blank'}"
              class:noted={right ? notesCovering(right.line, 'right').length > 0 : false}
              class:note-end={right ? notesOn(right.line, 'right').length > 0 : false}
            >
              {#if right}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <span class="ln" onclick={(e) => onLnClick(e, parseInt(right.line.new || '0', 10))}>{right.line.new}</span>
                <span class="cd">{@html right.line.codeHtml}</span>
              {:else}
                <span class="ln"></span>
                <span class="cd"></span>
              {/if}
            </div>
          </div>
          {#if left?.line.extraHtml}{@html left.line.extraHtml}{/if}
          {#if right && right.i !== left?.i && right.line.extraHtml}{@html right.line.extraHtml}{/if}
          {#if (left && notesOn(left.line, 'left').length) || (right && notesOn(right.line, 'right').length)}
            <div class="icmt-split">
              <div>{#if left}{@render postedThreads(left.line, 'left')}{/if}</div>
              <div>{#if right}{@render postedThreads(right.line, 'right')}{/if}</div>
            </div>
          {/if}
        {/if}
      {/each}
      </div>
    {:else}
      <div class="df-lines">
    {#each allLines as l, i (i)}
      {#if l.type === 'hunk'}
        <div class="hline hunk" role="button" tabindex="0" onclick={() => toggleHunk(i)} onkeydown={(e) => e.key === 'Enter' && toggleHunk(i)}>
          <span class="car">{collapsedHunks.has(i) ? '▸' : '▾'}</span><span class="cd">{@html hunkLabel(l.codeHtml)}</span>
        </div>
      {:else if visible(i)}
        {@const ln = lineNo(l)}
        <div
          class="hline {l.type}"
          class:noted={notesCovering(l).length > 0}
          class:note-end={notesOn(l).length > 0}
          data-line={ln || undefined}
        >
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <span class="ln" onclick={(e) => onLnClick(e, parseInt(l.new || '0', 10))}>{l.old}</span>
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <span class="ln" onclick={(e) => onLnClick(e, parseInt(l.new || '0', 10))}>{l.new}</span>
          <span class="cd">{@html l.codeHtml}</span>
        </div>
        {#if l.extraHtml}{@html l.extraHtml}{/if}
        {@render postedThreads(l)}
      {/if}
    {/each}
      </div>
    {/if}
  {/if}
</div>
