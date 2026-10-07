<script lang="ts">
  // One file's diff — the df-head + line-numbered, syntax-highlighted hunks with
  // inline comments / risk flags (PRD §12.2). Reused by Files-changed and per-commit
  // views (one diff renderer, per PRD §12). Seed CodeHtml/ExtraHtml are trusted.
  import { tick, untrack } from 'svelte'
  import type { FileDiff, Finding, PendingLineComment, PRComment } from '../../bindings/recur/internal/domain/models'
  import { selectedPRId, pushToast, prs } from '../lib/stores'
  import { PRsService, call, patchPRPendingComments } from '../lib/api'
  import { copyText } from '../lib/clipboard'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import { emojiShortcodes } from '../lib/emoji'
  import { userMentions } from '../lib/user-mention'
  import { splitDiffRows, slotOfSplitHighlight, slotOfUnifiedHighlight, splitRowDataLine } from '../lib/split-diff'
  import { isLineNoteRoot, lineNotesCoveringRow, lineNotesOnRow, sameCommentPath } from '../lib/comment-thread'
  import { commentHunkLineLabel } from '../lib/comment-hunk'
  import { extraHtmlWithoutFindingFlags, findingsCoveringLine, findingsEndingLine, findingsOnLine } from '../lib/finding-paths'
  import { fileDiffPathLabel } from '../lib/review-diff'
  import { findingSevIcon } from '../lib/findings-ui'
  import { paneScrollTo } from '../lib/review-scroll'
  import {
    applySlotPin, coverNear, DIFF_LINE_PX, DIFF_LINE_OVERSCAN, DIFF_LINE_WINDOW_MIN, jumpLineWindow, lineWindow, readSlotHeights, slotPad, type LineWin,
  } from '../lib/review-window'
  import { prConversationLocked } from '../lib/colors'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'
  import LineNoteThread from './LineNoteThread.svelte'

  let {
    file,
    highlightLine = 0,
    /** When false, hide inline comment affordances (observatory worktree diffs). */
    comments = true,
    noteMode,
    layout = 'unified',
    /** Commit diffs can fold a file; Files changed shows one file at a time. */
    collapsible = false,
    /** Controlled collapse when `onToggleCollapse` is also passed. */
    collapsed = false,
    onToggleCollapse,
    onOpenFile = undefined,
    openFileTitle = 'Open in editor — ⌘-click any line number to jump there',
    onAddFinding,
    findings = [] as Finding[],
    onOpenFinding,
    onPostFinding,
    postingFindingIds = new Set<string>(),
    postableFindingIds = new Set<string>(),
  }: {
    file: FileDiff
    highlightLine?: number
    comments?: boolean
    noteMode?: 'pr' | 'finding' | 'none'
    layout?: 'unified' | 'split'
    collapsible?: boolean
    collapsed?: boolean
    onToggleCollapse?: (next: boolean) => void
    onOpenFile?: (path: string, line?: number) => void
    openFileTitle?: string
    onAddFinding?: (path: string, line: number, body: string, endLine?: number) => Promise<boolean>
    findings?: Finding[]
    onOpenFinding?: (f: Finding) => void
    onPostFinding?: (f: Finding) => void
    postingFindingIds?: Set<string>
    postableFindingIds?: ReadonlySet<string>
  } = $props()

  let internalCollapsed = $state(false)
  const isCollapsed = $derived(onToggleCollapse ? collapsed : internalCollapsed)
  let openLine = $state<{ oldN: number; newN: number } | null>(null)
  let openSide = $state<'left' | 'right'>('right')
  let openStart = $state(0)
  let selecting = $state<{ side: 'left' | 'right'; anchor: number; tip: number } | null>(null)
  let draft = $state('')
  let posting = $state(false)
  let rootEl: HTMLDivElement | undefined = $state()
  let collapsedHunks = $state<Set<number>>(new Set())
  let winStart = $state(0)
  let winEnd = $state(DIFF_LINE_WINDOW_MIN)
  let slotH = $state.raw<number[]>([])
  let slotSig = ''
  let lastJump = 0

  // Reset per-file UI only when the path changes — a poll that replaces
  // `file` with the same path must not dismiss an open composer.
  let loadedPath = $state('')
  $effect(() => {
    const path = file.path
    if (path === loadedPath) return
    loadedPath = path
    internalCollapsed = false
    openLine = null
    draft = ''
    collapsedHunks = new Set()
    winStart = 0
    winEnd = DIFF_LINE_WINDOW_MIN
    slotH = []
    slotSig = ''
    lastJump = 0
  })

  const allLines = $derived(isCollapsed ? [] as NonNullable<FileDiff['lines']> : (file.lines ?? []))
  const splitRows = $derived(layout === 'split' && !isCollapsed ? splitDiffRows(allLines) : [])
  const mode = $derived(noteMode ?? (comments === false ? 'none' : 'pr'))
  const pendingAll = $derived.by(() => {
    if (mode !== 'pr') return [] as PendingLineComment[]
    return ($prs.find((p) => p.id === $selectedPRId)?.pendingComments ?? []) as PendingLineComment[]
  })
  const pendingHere = $derived(pendingAll.filter((pc) => pc.file === file.path))
  const prRow = $derived.by(() => {
    if (mode !== 'pr') return undefined
    return $prs.find((p) => p.id === $selectedPRId)
  })
  const postedNotes = $derived((prRow?.comments ?? []) as PRComment[])
  const canComment = $derived(mode !== 'none' && (mode === 'finding' || !prConversationLocked(prRow)))
  /** Findings are new-side only; left/deleted numbers would be stored as current-file lines. */
  function canNoteSide(side: 'left' | 'right'): boolean {
    return canComment && (mode !== 'finding' || side === 'right')
  }
  function notesOn(line: { old: string; new: string; type?: string }, side?: 'left' | 'right'): PRComment[] {
    if (mode !== 'pr') return []
    return lineNotesOnRow(postedNotes, file.path, line, side)
  }
  function notesCovering(line: { old: string; new: string; type?: string }, side?: 'left' | 'right'): PRComment[] {
    if (mode !== 'pr') return []
    return lineNotesCoveringRow(postedNotes, file.path, line, side)
  }
  function pendingCovering(line: { old: string; new: string; type?: string }, side?: 'left' | 'right'): number {
    if (mode !== 'pr') return 0
    return lineNotesCoveringRow(
      pendingHere.map((pc) => ({ id: pc.id, path: pc.file, line: pc.line, startLine: pc.startLine, side: pc.side })),
      file.path,
      line,
      side,
    ).length
  }
  function rowSide(line: { type?: string }): 'left' | 'right' {
    return line.type === 'del' ? 'left' : 'right'
  }
  function rowNum(line: { old?: string; new?: string }, side: 'left' | 'right'): number {
    return parseInt((side === 'left' ? line.old : line.new) || '0', 10) || 0
  }
  const draftSel = $derived.by(() => {
    if (selecting) {
      return {
        side: selecting.side,
        start: Math.min(selecting.anchor, selecting.tip),
        end: Math.max(selecting.anchor, selecting.tip),
      }
    }
    if (!openLine) return null
    const end = openSide === 'left' ? openLine.oldN : openLine.newN
    if (!end) return null
    const start = openStart > 0 && openStart < end ? openStart : end
    return { side: openSide, start, end }
  })
  function draftCovers(line: { old?: string; new?: string; type?: string }, side?: 'left' | 'right'): boolean {
    const r = draftSel
    const s = side ?? rowSide(line)
    if (!r || r.side !== s) return false
    const n = rowNum(line, s)
    return n >= r.start && n <= r.end
  }
  function draftEnds(line: { old?: string; new?: string; type?: string }, side?: 'left' | 'right'): boolean {
    const r = draftSel
    const s = side ?? rowSide(line)
    if (!r || r.side !== s) return false
    return rowNum(line, s) === r.end
  }
  function draftTip(line: { old?: string; new?: string; type?: string }, side?: 'left' | 'right'): boolean {
    if (!selecting) return false
    const s = side ?? rowSide(line)
    if (selecting.side !== s) return false
    const n = rowNum(line, s)
    return n === selecting.tip || n === selecting.anchor
  }
  function findLine(n: number, side: 'left' | 'right'): { old: string; new: string; type?: string } | undefined {
    return (file.lines ?? []).find((l) => {
      if (l.type === 'hunk') return false
      return rowNum(l, side) === n && (side === 'left' ? l.type !== 'add' : l.type !== 'del')
    })
  }
  function commentLineAt(x: number, y: number, side: 'left' | 'right'): number {
    const nodes = rootEl?.querySelectorAll(`[data-cmt-side="${side}"][data-cmt-line]`)
    if (!nodes) return 0
    for (const row of nodes) {
      if (!(row instanceof HTMLElement)) continue
      const box = row.getBoundingClientRect()
      if (y < box.top || y >= box.bottom) continue
      const addc = row.querySelector('.addc')
      if (!(addc instanceof HTMLElement)) return 0
      const a = addc.getBoundingClientRect()
      const pad = 10
      if (x < a.left - pad || x > a.right + pad) return 0
      return Number(row.dataset.cmtLine) || 0
    }
    return 0
  }
  function beginRange(e: PointerEvent, line: { old: string; new: string; type?: string }, side: 'left' | 'right') {
    if (!canNoteSide(side) || e.button !== 0) return
    const n = rowNum(line, side)
    if (!n) return
    e.preventDefault()
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    selecting = { side, anchor: n, tip: n }
  }
  function moveRange(e: PointerEvent) {
    if (!selecting) return
    const n = commentLineAt(e.clientX, e.clientY, selecting.side)
    if (!n || n === selecting.tip) return
    selecting = { ...selecting, tip: n }
  }
  function endRange(e: PointerEvent, line: { old: string; new: string; type?: string }, side: 'left' | 'right') {
    if (!selecting) return
    if ((e.currentTarget as HTMLElement).hasPointerCapture(e.pointerId)) {
      (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId)
    }
    const start = Math.min(selecting.anchor, selecting.tip)
    const end = Math.max(selecting.anchor, selecting.tip)
    const selSide = selecting.side
    selecting = null
    const target = findLine(end, selSide) ?? line
    openComposer(target, selSide, start < end ? start : 0)
  }
  function cancelRange() {
    selecting = null
  }
  function pendingOn(line: { old: string; new: string; type?: string }, side?: 'left' | 'right'): PendingLineComment[] {
    const oldN = parseInt(line.old || '0', 10) || 0
    const newN = parseInt(line.new || '0', 10) || 0
    if (side) {
      const n = side === 'left' ? oldN : newN
      if (!n) return []
      const want = side === 'left' ? 'LEFT' : 'RIGHT'
      return pendingHere.filter((pc) => pc.line === n && (pc.side || 'RIGHT') === want)
    }
    // Unified: one card per comment. LEFT stays on the deleted/base row;
    // RIGHT stays on the added/head row. Context matches the comment's side.
    return pendingHere.filter((pc) => {
      const want = pc.side || 'RIGHT'
      if (line.type === 'del') return want === 'LEFT' && oldN === pc.line
      if (line.type === 'add') return want === 'RIGHT' && newN === pc.line
      return want === 'LEFT' ? oldN === pc.line : newN === pc.line
    })
  }
  function findingsOn(line: { old: string; new: string; type?: string }): Finding[] {
    return findingsOnLine(findings, line)
  }
  function findingsCovering(line: { old: string; new: string; type?: string }): Finding[] {
    return findingsCoveringLine(findings, line)
  }
  function findingsEnding(line: { old: string; new: string; type?: string }): Finding[] {
    return findingsEndingLine(findings, line)
  }
  function extraHTML(line: { extraHtml?: string }): string {
    const html = line.extraHtml ?? ''
    return findings.length ? extraHtmlWithoutFindingFlags(html) : html
  }
  function lineNums(line: { old?: string; new?: string }): { oldN: number; newN: number } {
    return {
      oldN: parseInt(line.old || '0', 10) || 0,
      newN: parseInt(line.new || '0', 10) || 0,
    }
  }
  function composerOn(line: { old?: string; new?: string }, side?: 'left' | 'right'): boolean {
    if (!openLine) return false
    if (side && openSide !== side) return false
    const n = lineNums(line)
    if (openLine.newN && n.newN) return openLine.newN === n.newN
    if (openLine.oldN && n.oldN) return openLine.oldN === n.oldN
    return false
  }
  function apiLine(line: { old: string; new: string }, side: 'left' | 'right'): number {
    const n = parseInt(line.new || line.old || '0', 10) || 0
    return side === 'left' ? -n : n
  }
  function openComposer(line: { old: string; new: string }, side: 'left' | 'right' = 'right', start = 0) {
    if (!canNoteSide(side)) return
    const n = lineNums(line)
    const end = side === 'left' ? n.oldN : n.newN
    if (composerOn(line, side) && openStart === (start > 0 && start < end ? start : 0)) return
    if (!openLine || openLine.newN !== n.newN || openLine.oldN !== n.oldN) draft = ''
    openLine = n
    openSide = side
    openStart = start > 0 && start < end ? start : 0
    tick().then(() => {
      rootEl?.querySelector<HTMLTextAreaElement>('.icmt-compose .finding-note-input')?.focus()
    })
  }
  function composePlaceholder(line: { old?: string; new?: string }): string {
    const end = openSide === 'left' ? rowNum(line, 'left') : rowNum(line, 'right')
    const loc = commentHunkLineLabel(end, openStart)
    if (!loc) return 'Comment…'
    return loc.includes('-') ? `Comment on lines ${loc.replace('-', '–')}…` : `Comment on line ${loc}…`
  }
  function dismissComposer() {
    openLine = null
    openStart = 0
    draft = ''
  }

  async function submit(line: { old: string; new: string; type?: string }) {
    const body = draft.trim()
    if (!body || posting || !canComment) return
    if (mode === 'finding') {
      const lineNo = parseInt(line.new || '0', 10) || 0
      if (!lineNo) return
      posting = true
      try {
        const start = openStart > 0 && openStart < lineNo ? openStart : lineNo
        const endLine = start < lineNo ? lineNo : 0
        const ok = await onAddFinding?.(file.path, start, body, endLine)
        if (ok) {
          draft = ''
          openLine = null
          openStart = 0
          pushToast('success', 'Noted')
        }
      } finally {
        posting = false
      }
      return
    }
    const side = layout === 'split' ? openSide : line.type === 'del' ? 'left' : 'right'
    const signed = apiLine(line, side)
    const lineNoAbs = Math.abs(signed)
    posting = true
    try {
      const start = openStart > 0 && openStart < lineNoAbs ? openStart : 0
      const startSigned = start ? (side === 'left' ? -start : start) : 0
      const pc: PendingLineComment = {
        id: `pend-local-${Date.now()}`,
        file: file.path,
        line: lineNoAbs,
        side: side === 'left' ? 'LEFT' : 'RIGHT',
        startLine: start || undefined,
        startSide: start ? (side === 'left' ? 'LEFT' : 'RIGHT') : undefined,
        body,
      }
      patchPRPendingComments($selectedPRId, (prev) => [...prev, pc])
      draft = ''
      openLine = null
      openStart = 0
      const ok = await call(PRsService.AddPRComment($selectedPRId, file.path, signed, body, startSigned, ''), 'Comment failed to save')
      if (ok === null) {
        patchPRPendingComments($selectedPRId, (prev) => prev.filter((x) => x.id !== pc.id))
        return
      }
      pushToast('success', 'Saved — submit your review to post')
    } finally {
      posting = false
    }
  }
  async function removePending(pc: PendingLineComment) {
    const signed = (pc.side || 'RIGHT') === 'LEFT' ? -pc.line : pc.line
    const start = pc.startLine ?? 0
    const startSigned = start > 0 && start < pc.line
      ? ((pc.startSide || pc.side || 'RIGHT') === 'LEFT' ? -start : start)
      : 0
    patchPRPendingComments($selectedPRId, (prev) => prev.filter((x) => x.id !== pc.id))
    const ok = await call(PRsService.AddPRComment($selectedPRId, pc.file, signed, '', startSigned, pc.findingId || pc.id), 'Could not remove pending comment')
    if (ok === null) patchPRPendingComments($selectedPRId, (prev) => [...prev, pc])
  }

  let copiedPath = $state(false)
  let copyPathTimer: ReturnType<typeof setTimeout> | undefined
  function copyPath() {
    void copyText(file.path)
      .then(() => {
        copiedPath = true
        clearTimeout(copyPathTimer)
        copyPathTimer = setTimeout(() => { copiedPath = false }, 1200)
      })
      .catch((err: any) => pushToast('error', `Copy failed: ${err?.message ?? String(err)}`))
  }

  function jumpToFile(e: MouseEvent, line?: number) {
    e.stopPropagation()
    onOpenFile?.(file.path, line)
  }

  function onLnClick(e: MouseEvent, n: number) {
    if (!onOpenFile || !n || !isCmdOrCtrl(e)) return
    e.preventDefault()
    e.stopPropagation()
    onOpenFile(file.path, n)
  }
  /** Working-tree line for ⌘-click. Old-only (deleted) rows have no current line. */
  function openLineNo(line: { old?: string; new?: string }): number {
    return parseInt(line.new || '0', 10) || 0
  }

  const hunkOf = $derived.by<number[]>(() => {
    if (isCollapsed) return []
    const out: number[] = []
    let cur = -1
    ;(file.lines ?? []).forEach((l, i) => {
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

  const unifiedSlots = $derived.by(() => {
    const lines = allLines
    const out: number[] = []
    for (let i = 0; i < lines.length; i++) {
      if (lines[i].type === 'hunk' || visible(i)) out.push(i)
    }
    return out
  })
  const splitSlots = $derived.by(() => {
    if (layout !== 'split' || isCollapsed) return [] as typeof splitRows
    return splitRows.filter((row) => row.kind === 'hunk' || visible(row.i))
  })
  const slotCount = $derived(layout === 'split' ? splitSlots.length : unifiedSlots.length)
  const windowedUnified = $derived.by(() => {
    const slots = unifiedSlots
    const a = Math.max(0, Math.min(winStart, slots.length))
    const b = Math.max(a, Math.min(winEnd, slots.length))
    const lines = allLines
    return slots.slice(a, b).map((i) => ({ i, l: lines[i] }))
  })
  const windowedSplit = $derived.by(() => {
    const a = Math.max(0, Math.min(winStart, splitSlots.length))
    const b = Math.max(a, Math.min(winEnd, splitSlots.length))
    return splitSlots.slice(a, b)
  })
  const padTop = $derived(slotPad(slotH, 0, winStart))
  const padBot = $derived(slotPad(slotH, winEnd, slotCount))

  function slotOfLine(n: number): number {
    if (!n) return -1
    if (layout === 'split') {
      for (let s = 0; s < splitSlots.length; s++) {
        const row = splitSlots[s]
        if (row.kind === 'hunk') continue
        if ((row.left && rowNum(row.left.line, 'left') === n) || (row.right && rowNum(row.right.line, 'right') === n)) return s
      }
      return -1
    }
    const lines = allLines
    for (let s = 0; s < unifiedSlots.length; s++) {
      const l = lines[unifiedSlots[s]]
      if (l && l.type !== 'hunk' && lineNo(l) === n) return s
    }
    return -1
  }

  function coverPinned(win: LineWin, count: number): LineWin {
    const pin: number[] = []
    if (openLine) {
      if (openLine.oldN) pin.push(openLine.oldN)
      if (openLine.newN) pin.push(openLine.newN)
    }
    if (selecting) {
      pin.push(selecting.anchor)
      pin.push(selecting.tip)
    }
    for (const n of pin) win = coverNear(win, slotOfLine(n), count)
    return win
  }

  function refreshLineWindow() {
    const root = rootEl
    if (!root || isCollapsed) return
    const count = slotCount
    const pane = root.closest('.diffscroll')
    if (!(pane instanceof HTMLElement) || count <= DIFF_LINE_WINDOW_MIN) {
      winStart = 0
      winEnd = count
      return
    }
    const sig = `${file.path}:${layout}:${count}:${root.clientWidth}`
    if (sig !== slotSig) {
      slotSig = sig
      slotH = []
    }
    const measure = () => {
      const el = root.querySelector('.df-lines')
      if (!el) return
      const next = slotH.slice()
      if (readSlotHeights(el.children as HTMLCollectionOf<HTMLElement>, winStart, next)) slotH = next
    }
    measure()
    const head = root.querySelector('.df-head')
    const headH = head instanceof HTMLElement ? head.offsetHeight : 40
    const pr = pane.getBoundingClientRect()
    const rr = root.getBoundingClientRect()
    const bodyTop = pane.scrollTop + (rr.top - pr.top) + headH
    let win = lineWindow(count, bodyTop, pane.scrollTop, pane.clientHeight, DIFF_LINE_PX, DIFF_LINE_OVERSCAN, slotH)
    const jumping = highlightLine > 0 && lastJump !== highlightLine
    const fileThreads = jumping
      ? postedNotes.filter((c) => isLineNoteRoot(c) && sameCommentPath(c.path, file.path))
      : []
    const jumpSlot = jumping
      ? (layout === 'split'
        ? slotOfSplitHighlight(splitSlots, highlightLine, fileThreads)
        : slotOfUnifiedHighlight(allLines, unifiedSlots, highlightLine, fileThreads))
      : -1
    win = jumpLineWindow(win, count, highlightLine, lastJump, jumpSlot)
    if (!jumping) win = coverPinned(win, count)
    const linesEl = root.querySelector('.df-lines')
    const anchor = jumping ? null : linesEl?.querySelector(':scope > .hline')
    const pinSlot = anchor instanceof HTMLElement ? Number(anchor.dataset.slot) : NaN
    const pinY = anchor instanceof HTMLElement ? anchor.getBoundingClientRect().top : null
    const pinScroll = pane.scrollTop
    if (win.start === winStart && win.end === winEnd) return
    winStart = win.start
    winEnd = win.end
    void tick().then(() => {
      measure()
      requestAnimationFrame(() => {
        if (!Number.isFinite(pinSlot)) return
        const el = root.querySelector(`[data-slot="${pinSlot}"]`)
        applySlotPin(pane, el instanceof HTMLElement ? el : null, pinY, pinScroll)
      })
    })
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
    const root = rootEl
    void slotCount
    void layout
    void collapsedHunks
    if (!root || isCollapsed) return
    const pane = root.closest('.diffscroll')
    if (!(pane instanceof HTMLElement)) {
      winStart = 0
      winEnd = slotCount
      return
    }
    let raf = 0
    const onScroll = () => {
      if (raf) return
      raf = requestAnimationFrame(() => {
        raf = 0
        refreshLineWindow()
      })
    }
    pane.addEventListener('scroll', onScroll, { passive: true })
    const ro = new ResizeObserver(onScroll)
    ro.observe(root)
    void openLine
    void selecting
    void highlightLine
    untrack(() => refreshLineWindow())
    return () => {
      cancelAnimationFrame(raf)
      pane.removeEventListener('scroll', onScroll)
      ro.disconnect()
    }
  })

  // Jump when highlightLine changes. Do not read window state here — that retriggers paneScrollTo on scroll.
  $effect(() => {
    const target = highlightLine
    if (!target) {
      lastJump = 0
      return
    }
    if (!file?.lines?.length) return
    untrack(() => {
      if (onToggleCollapse) onToggleCollapse(false)
      else internalCollapsed = false
      file.lines.forEach((l, i) => {
        if (l.type === 'hunk') return
        if (lineNo(l) === target) {
          const h = hunkOf[i]
          if (h >= 0 && collapsedHunks.has(h)) {
            const next = new Set(collapsedHunks)
            next.delete(h)
            collapsedHunks = next
          }
        }
      })
    })
    tick().then(() => {
      const el = rootEl?.querySelector<HTMLElement>(`.icmt-thread[data-note-line="${target}"]`)
        ?? rootEl?.querySelector<HTMLElement>(`[data-line="${target}"]`)
      const pane = rootEl?.closest<HTMLElement>('.diffscroll')
      if (el && pane) paneScrollTo(pane, el)
      lastJump = target
    })
  })
</script>

<div class="diff-file" class:selecting={!!selecting} bind:this={rootEl}>
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
    {#snippet composerBox(l: { old: string; new: string })}
      <div class="icmt icmt-compose">
        <div class="finding-note-compose">
          <textarea
            class="finding-note-input"
            rows="2"
            placeholder={composePlaceholder(l)}
            disabled={posting}
            bind:value={draft}
            {@attach emojiShortcodes((v) => { draft = v })}
            {@attach userMentions((v) => { draft = v })}
            onkeydown={(e) => {
              if (e.key === 'Enter' && isCmdOrCtrl(e)) {
                e.preventDefault()
                void submit(l)
              }
              if (e.key === 'Escape') {
                e.preventDefault()
                dismissComposer()
              }
            }}
          ></textarea>
          <div class="finding-note-compose-foot">
            <span class="finding-note-hint"><Kbd text="⌘↵" /> {mode === 'finding' ? 'saves a finding on this line' : 'pending until you submit the review'}</span>
            <div class="finding-note-compose-actions">
              <button type="button" class="finding-note-btn" disabled={posting} onclick={dismissComposer}>Cancel</button>
              <button type="button" class="finding-note-btn primary" disabled={posting || !draft.trim()} onclick={() => void submit(l)}>{posting ? 'Saving…' : 'Add'}</button>
            </div>
          </div>
        </div>
      </div>
    {/snippet}
    {#snippet addCommentBtn(line: { old: string; new: string; type?: string }, side: 'left' | 'right')}
      {#if canNoteSide(side)}
        <button
          type="button"
          class="addc"
          aria-label="Comment on this line"
          title="Comment on this line — drag to select a range"
          onpointerdown={(e) => beginRange(e, line, side)}
          onpointermove={moveRange}
          onpointerup={(e) => endRange(e, line, side)}
          onpointercancel={cancelRange}
          onclick={(e) => { if (e.detail === 0) openComposer(line, side) }}
        >+</button>
      {/if}
    {/snippet}
    {#snippet pendingCards(line: { old: string; new: string }, side?: 'left' | 'right')}
      {#each pendingOn(line, side) as pc (pc.id)}
        <div class="icmt icmt-pending">
          <div class="icmt-pending-card">
            <div class="icmt-pending-meta">
              <span class="pending-pill">Pending</span>
              <button type="button" class="finding-note-del" title="Remove pending comment" aria-label="Remove pending comment" onclick={() => void removePending(pc)}>
                <Icon name="trash" />
              </button>
            </div>
            <div class="icmt-pending-body">{pc.body}</div>
          </div>
        </div>
      {/each}
    {/snippet}
    {#snippet postedThreads(line: { old: string; new: string; type?: string }, side?: 'left' | 'right')}
      {#each notesOn(line, side) as note (note.id)}
        <div class="icmt icmt-thread" data-note-line={note.line}>
          <LineNoteThread root={note} items={postedNotes} pr={prRow} />
        </div>
      {/each}
    {/snippet}
    {#snippet findingCards(line: { old: string; new: string; type?: string })}
      {#each findingsOn(line) as f (f.id)}
        <div class="icmt icmt-finding">
          <div class="icmt-finding-card">
            <span class="finding-sev {f.severity || 'info'}"><Icon name={findingSevIcon(f.severity)} /></span>
            <div class="icmt-finding-main">
              <div class="icmt-finding-msg">{f.message}</div>
              <div class="icmt-finding-meta">
                {#if (f.endLine ?? 0) > f.line}lines {f.line}–{f.endLine}{/if}
                {#if f.notes?.length}{f.notes.length} note{f.notes.length === 1 ? '' : 's'}{/if}
              </div>
            </div>
            {#if onPostFinding || onOpenFinding}
              <div class="icmt-finding-actions">
                {#if onPostFinding && postableFindingIds.has(f.id)}
                  <button
                    type="button"
                    class="finding-note-btn"
                    title="Queue as a GitHub review comment"
                    disabled={postingFindingIds.has(f.id)}
                    onclick={() => onPostFinding(f)}
                  >Post as comment</button>
                {/if}
                {#if onOpenFinding}
                  <button type="button" class="finding-note-btn" onclick={() => onOpenFinding(f)}>Open in Findings</button>
                {/if}
              </div>
            {/if}
          </div>
        </div>
      {/each}
    {/snippet}
    {#snippet commentBlock(l: { old: string; new: string; extraHtml?: string; type?: string }, showComposer = true)}
      {#if extraHTML(l)}{@html extraHTML(l)}{/if}
      {#if showComposer}{@render findingCards(l)}{/if}
      {#if mode !== 'none'}
        {#if showComposer && mode === 'pr'}{@render postedThreads(l)}{/if}
        {#if showComposer && mode === 'pr'}{@render pendingCards(l)}{/if}
        {#if showComposer && canComment && composerOn(l)}
          {@render composerBox(l)}
        {/if}
      {/if}
    {/snippet}
    {#if layout === 'split'}
      <div class="df-lines" style:padding-top="{padTop}px" style:padding-bottom="{padBot}px">
      {#each windowedSplit as row, s (`${row.kind}-${row.i}`)}
        {#if row.kind === 'hunk'}
          <div class="hline hunk" data-slot={winStart + s} role="button" tabindex="0" onclick={() => toggleHunk(row.i)} onkeydown={(e) => e.key === 'Enter' && toggleHunk(row.i)}>
            <span class="car">{collapsedHunks.has(row.i) ? '▸' : '▾'}</span><span class="cd">{@html hunkLabel(row.line.codeHtml)}</span>
          </div>
        {:else}
          {@const left = row.left}
          {@const right = row.right}
          {@const ln = splitRowDataLine(row)}
          {@const leftNotes = left ? notesCovering(left.line, 'left') : []}
          {@const rightNotes = right ? notesCovering(right.line, 'right') : []}
          {@const leftN = left ? rowNum(left.line, 'left') : 0}
          {@const rightN = right ? rowNum(right.line, 'right') : 0}
          <div class="hline split" data-slot={winStart + s} data-line={ln || undefined}>
            <div
              class="side {left ? left.line.type : 'blank'}"
              class:noted={leftNotes.length > 0 || (left ? pendingCovering(left.line, 'left') > 0 : false)}
              class:note-end={(left ? notesOn(left.line, 'left').length > 0 : false) || (left ? pendingOn(left.line, 'left').length > 0 : false)}
              class:drafting={left ? draftCovers(left.line, 'left') : false}
              class:draft-end={left ? draftEnds(left.line, 'left') : false}
              class:draft-tip={left ? draftTip(left.line, 'left') : false}
              data-cmt-side={left && leftN ? 'left' : undefined}
              data-cmt-line={left && leftN ? String(leftN) : undefined}
            >
              {#if left}
                <span class="ln" onclick={(e) => onLnClick(e, openLineNo(left.line))}>{left.line.old}</span>
                <span class="cd">{@html left.line.codeHtml}</span>
                {@render addCommentBtn(left.line, 'left')}
              {:else}
                <span class="ln"></span>
                <span class="cd"></span>
              {/if}
            </div>
            <div
              class="side {right ? right.line.type : 'blank'}"
              class:noted={rightNotes.length > 0 || (right ? pendingCovering(right.line, 'right') > 0 : false) || (right ? findingsCovering(right.line).length > 0 : false)}
              class:note-end={(right ? notesOn(right.line, 'right').length > 0 : false) || (right ? pendingOn(right.line, 'right').length > 0 : false) || (right ? findingsEnding(right.line).length > 0 : false)}
              class:drafting={right ? draftCovers(right.line, 'right') : false}
              class:draft-end={right ? draftEnds(right.line, 'right') : false}
              class:draft-tip={right ? draftTip(right.line, 'right') : false}
              data-cmt-side={right && rightN ? 'right' : undefined}
              data-cmt-line={right && rightN ? String(rightN) : undefined}
            >
              {#if right}
                <span class="ln" onclick={(e) => onLnClick(e, openLineNo(right.line))}>{right.line.new}</span>
                <span class="cd">{@html right.line.codeHtml}</span>
                {@render addCommentBtn(right.line, 'right')}
              {:else}
                <span class="ln"></span>
                <span class="cd"></span>
              {/if}
            </div>
          </div>
          {#if left}{@render commentBlock(left.line, false)}{/if}
          {#if right && right.i !== left?.i}{@render commentBlock(right.line, false)}{/if}
          {#if mode !== 'none' && (composerOn(left?.line ?? {}, 'left') || composerOn(right?.line ?? {}, 'right') || (mode === 'pr' && left && pendingOn(left.line, 'left').length) || (mode === 'pr' && right && pendingOn(right.line, 'right').length) || (mode === 'pr' && left && notesOn(left.line, 'left').length) || (mode === 'pr' && right && notesOn(right.line, 'right').length) || (right && findingsOn(right.line).length))}
            <div class="icmt-split">
              <div>
                {#if left}{@render postedThreads(left.line, 'left')}{/if}
                {#if left}{@render pendingCards(left.line, 'left')}{/if}
                {#if left && canComment && composerOn(left.line, 'left')}
                  {@render composerBox(left.line)}
                {/if}
              </div>
              <div>
                {#if right}{@render postedThreads(right.line, 'right')}{/if}
                {#if right}{@render pendingCards(right.line, 'right')}{/if}
                {#if right}{@render findingCards(right.line)}{/if}
                {#if right && canComment && composerOn(right.line, 'right')}
                  {@render composerBox(right.line)}
                {/if}
              </div>
            </div>
          {/if}
        {/if}
      {/each}
      </div>
    {:else}
      <div class="df-lines" style:padding-top="{padTop}px" style:padding-bottom="{padBot}px">
    {#each windowedUnified as { l, i }, s (i)}
      {#if l.type === 'hunk'}
        <div class="hline hunk" data-slot={winStart + s} role="button" tabindex="0" onclick={() => toggleHunk(i)} onkeydown={(e) => e.key === 'Enter' && toggleHunk(i)}>
          <span class="car">{collapsedHunks.has(i) ? '▸' : '▾'}</span><span class="cd">{@html hunkLabel(l.codeHtml)}</span>
        </div>
      {:else}
        {@const ln = lineNo(l)}
        {@const covering = notesCovering(l)}
        {@const ends = notesOn(l)}
        {@const cmtSide = rowSide(l)}
        {@const cmtN = rowNum(l, cmtSide)}
        <div
          class="hline {l.type}"
          class:noted={covering.length > 0 || pendingCovering(l) > 0 || findingsCovering(l).length > 0}
          class:note-end={ends.length > 0 || pendingOn(l).length > 0 || findingsEnding(l).length > 0}
          class:drafting={draftCovers(l)}
          class:draft-end={draftEnds(l)}
          class:draft-tip={draftTip(l)}
          data-slot={winStart + s}
          data-line={ln || undefined}
          data-cmt-side={cmtN ? cmtSide : undefined}
          data-cmt-line={cmtN ? String(cmtN) : undefined}
        >
          <span class="ln" onclick={(e) => onLnClick(e, openLineNo(l))}>{l.old}</span>
          <span class="ln" onclick={(e) => onLnClick(e, openLineNo(l))}>{l.new}</span>
          <span class="cd">{@html l.codeHtml}</span>
          {@render addCommentBtn(l, cmtSide)}
        </div>
        {@render commentBlock(l)}
      {/if}
    {/each}
      </div>
    {/if}
  {/if}
</div>
