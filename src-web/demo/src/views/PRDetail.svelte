<script lang="ts">
  import { tick, untrack } from 'svelte'
  import { get } from 'svelte/store'
  import { view, prs, selectedPRId, go, openWorkspace, pushToast, prDetailTab, prComposerDraft, prDiffJump, appOverlayOpen, prAgentDockHost, observatoryJump, prAgentDockOpen, prDetailFocus, workspaces, workspacesRunning } from '../lib/stores'
  import { PRsService, WorkspacesService, ObservatoryService, onPRComment, onPRRefresh, onFindingsUpdated, call, callOk, patchPRPendingComments, postedFindingIds, markPostedFinding, forgetPostedFinding, resetPostedFindings } from '../lib/api'
  import { keepCommitFiles } from '../lib/pr-commits'
  import type { Commit, FileDiff, Finding, PendingLineComment, PRComment } from '../../bindings/recur/internal/domain/models'
  import { copyText } from '../lib/clipboard'
  import DiffFile from '../components/DiffFile.svelte'
  import CommitRow from '../components/CommitRow.svelte'
  import ReviewFileList from '../components/ReviewFileList.svelte'
  import ResizeHandle from '../components/ResizeHandle.svelte'
  import MarkdownHtml from '../components/MarkdownHtml.svelte'
  import Icon, { type IconName } from '../components/Icon.svelte'
  import Kbd from '../components/Kbd.svelte'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import { commentPermalink, githubBlobUrl, githubCommitUrl } from '../lib/comment-permalink'
  import { commentHeaderVerb, reviewDecision } from '../lib/comment-header'
  import { applyThreadResolved, applyThreadResolvedHolds, canDeletePRComment, canEditPRComment, canReactPRComment, canResolveLineNote, commentSource, commentsForPath, isDroppedComment, isLineNote, isPendingComment, isTimelineRoot, keepPendingReplies, lineThreadKey, PENDING_REPLY_PREFIX, reviewChildren, sameThreadReply, settlePostedReply, threadReplies, unresolvedLineNotesByPath } from '../lib/comment-thread'
  import { applyReactionHolds, commentReactionHold, resetCommentReactionHolds } from '../lib/comment-reactions'
  import { resetThreadResolvedHolds, setPRThreadResolved, threadResolvedHold } from '../lib/pr-thread-resolved'
  import CommentReactions from '../components/CommentReactions.svelte'
  import { clipCommentHunk, commentHunkPathLabel, parseDiffHunk } from '../lib/comment-hunk'
  import { openExternalUrl } from '../lib/open-external'
  import { followMoreMenu, moreMenuFixedBox, portalToBody } from '../lib/menu-space'
  import { autosizeTextarea, PR_COMPOSER_AUTOSIZE, PR_REPLY_AUTOSIZE } from '../lib/autosize-textarea'
  import { emojiShortcodes } from '../lib/emoji'
  import { userMentions } from '../lib/user-mention'
  import { liveCheckDetail, prunePendingClocks, type PendingClock } from '../lib/check-duration'
  import { checkGroupCollapsed, checkRunHref, checksBoxByline, checksHeading, checksRollup, githubPRChecksTabUrl, groupChecks, isPendingCheck, isSkippedCheck } from '../lib/pr-checks'
  import { prChromeColor, prConversationLocked, prIconName, prLifecycle, prStatusLabel, keepMergedPR } from '../lib/colors'
  import { prAttentionLabels } from '../lib/pr-attention'
  import PRPeopleStack from '../components/PRPeopleStack.svelte'
  import { prAgentIsRunning, prAgentWorkspaceId } from '../lib/pr-workspace'
  import { requestFixChecks } from '../lib/pr-fix-checks'
  import { prChecksPending, prCommitsPending, prConvPending, prFilesPending } from '../lib/pr-detail-pending'
  import { shortBaseName } from '../lib/pr-merge-action'
  import {
    readDiffLayout, writeDiffLayout,
    readFileListMode, writeFileListMode,
    readReviewSidebar, writeReviewSidebar,
    fileDiffPathLabel,
    type DiffLayout, type FileListMode,
  } from '../lib/review-diff'
  import { applyViewportPin, captureViewportPin, fileAtScrollTop, stepPath, type ViewportPin } from '../lib/spy-pick'
  import { findingsForPath, matchFindingPath } from '../lib/finding-paths'
  import { findingCommentBody, findingHasPendingComment, findingReadyToSettle, findingReviewAnchor } from '../lib/findings-ui'
  import { sortFilesByChangedTree } from '../lib/changed-tree'
  import { inViewPaths, pickHydrated, recordStubHeight, reservedCardHeight, samePathList, samePathSet, syncStuckHeaders } from '../lib/review-window'
  import { paneScrollTo, pickDiffJumpEl } from '../lib/review-scroll'
  import { commitByline, commitSha } from '../lib/commit-meta'
  import { fadeScrollTop } from '../lib/scroll-fade'
  import { stickyFolderFades } from '../lib/tree-sticky'
  import { overlayScrollbar } from '../lib/overlay-scrollbar'

  // shotURL maps an on-disk evidence screenshot path
  // (…/workspaces/<id>/screenshots/x.png) to the /workspaces/... URL served by
  // main.go's asset middleware. '' when unmappable.
  function shotURL(p?: string): string {
    if (!p) return ''
    const i = p.indexOf('/workspaces/')
    return i >= 0 ? p.slice(i) : ''
  }

  type PRTab = 'conv' | 'commits' | 'files' | 'checks'
  const prtab = $derived($prDetailTab)
  let commitIdx = $state<number | null>(null)
  let commitFiles = $state.raw<FileDiff[] | null>(null)
  let commitFilesByHash = $state.raw<Record<string, FileDiff[]>>({})
  let commitDiffLoading = $state(false)
  let commitDiffError = $state(false)
  let commitSelected = $state('')
  let commitInView = $state.raw<string[]>([])
  let commitHydrated = $state.raw<Set<string>>(new Set())
  const commitEls = new Map<string, HTMLElement>()
  const commitMeasuredH = new Map<string, number>()
  let commitSpyUntil = 0
  let commitLastPin: ViewportPin | null = null
  let commitPinGen = 0
  let commitSpyObserver: IntersectionObserver | undefined
  let commitViewObserver: IntersectionObserver | undefined
  let commitsPaneEl = $state<HTMLDivElement | undefined>(undefined)
  let composerEl = $state<HTMLTextAreaElement | null>(null)
  let editingTitle = $state(false)
  let editingDesc = $state(false)
  let titleDraft = $state('')
  let descDraft = $state('')
  let editingCommentId = $state('')
  let commentDraft = $state('')
  let savingField = $state('')
  let moreMenuId = $state('')
  let moreMenuBox = $state<{ top: number; right: number } | null>(null)
  let moreTrigger: HTMLElement | null = $state(null)
  const pinMore = followMoreMenu(() => moreTrigger, (box) => { moreMenuBox = box }, () => closeMore())
  let replyFor = $state('')
  let replyDraft = $state('')
  let collapsedCtx = $state<Record<string, boolean>>({})
  let openResolved = $state<Record<string, boolean>>({})
  let diffLayout = $state<DiffLayout>(readDiffLayout())
  function setDiffLayout(next: DiffLayout) {
    diffLayout = next
    writeDiffLayout(next)
  }

  let fileListMode = $state<FileListMode>(readFileListMode())
  function setFileListMode(next: FileListMode) {
    fileListMode = next
    writeFileListMode(next)
  }
  let sidebarOpen = $state(readReviewSidebar())
  function setSidebarOpen(open: boolean) {
    sidebarOpen = open
    writeReviewSidebar(open)
  }
  let selectedPath = $state('')
  let collapsedFiles = $state<Set<string>>(new Set())
  let diffscrollEl = $state<HTMLDivElement | undefined>(undefined)
  const fileEls = new Map<string, HTMLElement>()
  const measuredH = new Map<string, number>()
  let spySuppressUntil = 0
  let spyObserver: IntersectionObserver | undefined
  let lastPin: ViewportPin | null = null
  let pinGen = 0
  let tocReveal = $state(0)
  let filterText = $state('')
  let paneWidth = $state(0)
  let inView = $state.raw<string[]>([])
  let hydrated = $state.raw<Set<string>>(new Set())
  const SPLIT_MIN_PX = 720

  function openInWorkspace(path: string, line?: number) {
    if (!pr?.workspaceId) return
    observatoryJump.set({ path, line, workspaceId: pr.workspaceId, preferDiff: true })
    openWorkspace(pr.workspaceId)
  }

  function openFinding(f: Finding) {
    if (pr?.workspaceId && f.id) {
      observatoryJump.set({
        workspaceId: pr.workspaceId,
        panel: 'findings',
        findingId: f.id,
        path: f.file || undefined,
        line: f.line && f.line > 0 ? f.line : undefined,
      })
      openWorkspace(pr.workspaceId)
      return
    }
    setTab('conv')
  }

  let findings = $state<Finding[]>([])
  let highlightLine = $state(0)
  let findingsOnly = $state(false)
  let commentsOnly = $state(false)
  let postingFindingIds = $state<Set<string>>(new Set())
  let settlingFindingIds = $state<Set<string>>(new Set())
  let settleFailedIds = $state<Set<string>>(new Set())
  let settleFor = ''

  const openFindings = $derived(findings.filter((f) => f.status === 'open' || !f.status))
  const reviewFiles = $derived.by(() => pr?.files ?? [])
  const listedFindings = $derived(
    openFindings.filter((f) =>
      !findingHasPendingComment(f, pr?.pendingComments ?? [], reviewFiles)
      && !settlingFindingIds.has(f.id)
    ),
  )
  const postableFindings = $derived(listedFindings.filter((f) => findingReviewAnchor(f, reviewFiles) && !$postedFindingIds.has(f.id)))
  const postableFindingIds = $derived(new Set(postableFindings.map((f) => f.id)))
  const findingsByPath = $derived.by(() => {
    const m = new Map<string, Finding[]>()
    const paths = reviewFiles.map((f) => f.path)
    for (const f of listedFindings) {
      if (!f.file) continue
      const p = matchFindingPath(f.file, paths) || f.file
      m.set(p, [...(m.get(p) ?? []), f])
    }
    return m
  })
  const commentsByPath = $derived.by(() => unresolvedLineNotesByPath(threadItems, reviewFiles.map((f) => f.path)))
  const openCommentFiles = $derived(commentsByPath.size)
  const filterQ = $derived(filterText.trim().toLowerCase())
  const matchedFiles = $derived.by(() => {
    let list = filterQ ? reviewFiles.filter((f) => f.path.toLowerCase().includes(filterQ) || (f.oldPath ?? '').toLowerCase().includes(filterQ)) : reviewFiles
    if (findingsOnly) list = list.filter((f) => findingsForPath(findingsByPath, f.path).length > 0)
    if (commentsOnly) list = list.filter((f) => commentsForPath(commentsByPath, f.path).length > 0)
    return sortFilesByChangedTree(list)
  })
  const matchedAdd = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.add ?? 0), 0))
  const matchedDel = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.del ?? 0), 0))
  const anyExpanded = $derived(matchedFiles.some((f) => !collapsedFiles.has(f.path)))
  const splitFallback = $derived(diffLayout === 'split' && paneWidth > 0 && paneWidth < SPLIT_MIN_PX)
  const effectiveLayout = $derived<DiffLayout>(splitFallback ? 'unified' : diffLayout)

  function fileRanges() {
    return [...fileEls.entries()]
      .map(([path, el]) => ({ path, top: el.offsetTop, bottom: el.offsetTop + el.offsetHeight }))
      .sort((a, b) => a.top - b.top)
  }

  function pinIfAtEnd() {
    const root = diffscrollEl
    if (!root || !lastPin?.atEnd || performance.now() < spySuppressUntil) return
    root.scrollTop = Math.max(0, root.scrollHeight - root.clientHeight)
  }

  function refreshWindow() {
    const root = diffscrollEl
    if (!root) return
    const ranges = fileRanges()
    const pin = captureViewportPin(ranges, root.scrollTop, root.clientHeight, root.scrollHeight)
    lastPin = pin
    const nextInView = inViewPaths(ranges, root.scrollTop, root.clientHeight)
    const nextHydrated = pickHydrated(matchedFiles.map((f) => f.path), selectedPath, nextInView)
    const swapped = !samePathSet(hydrated, nextHydrated)
    if (!samePathList(inView, nextInView)) inView = nextInView
    if (swapped) hydrated = nextHydrated
    syncStuckHeaders(fileEls.values(), root.scrollTop)
    if (!swapped || performance.now() < spySuppressUntil) return
    const capturedTop = root.scrollTop
    const token = ++pinGen
    void tick().then(() => {
      requestAnimationFrame(() => {
        if (token !== pinGen) return
        applyViewportPin(root, fileRanges(), pin, capturedTop)
        lastPin = captureViewportPin(fileRanges(), root.scrollTop, root.clientHeight, root.scrollHeight)
      })
    })
  }

  function toggleCollapsed(path: string, next: boolean) {
    const set = new Set(collapsedFiles)
    if (next) set.add(path)
    else set.delete(path)
    collapsedFiles = set
    void tick().then(refreshWindow)
  }

  function registerFileEl(path: string) {
    return (el: HTMLElement) => {
      fileEls.set(path, el)
      spyObserver?.observe(el)
      const stopMeasure = recordStubHeight(el, path, measuredH, pinIfAtEnd)
      return () => {
        stopMeasure()
        spyObserver?.unobserve(el)
        fileEls.delete(path)
      }
    }
  }

  function spyPick(path: string) {
    if (!path || path === selectedPath) return
    selectedPath = path
    tocReveal++
  }

  function spyFromOffsets() {
    const root = diffscrollEl
    if (!root || performance.now() < spySuppressUntil) return
    spyPick(fileAtScrollTop(fileRanges(), root.scrollTop))
  }

  $effect(() => {
    const root = diffscrollEl
    if (!root || prtab !== 'files') return
    const obs = new IntersectionObserver(() => spyFromOffsets(), { root, threshold: 0 })
    spyObserver = obs
    for (const el of fileEls.values()) obs.observe(el)
    let raf = 0
    const onScroll = () => {
      if (raf) return
      raf = requestAnimationFrame(() => {
        raf = 0
        spyFromOffsets()
        refreshWindow()
      })
    }
    root.addEventListener('scroll', onScroll, { passive: true })
    refreshWindow()
    return () => {
      cancelAnimationFrame(raf)
      root.removeEventListener('scroll', onScroll)
      obs.disconnect()
      if (spyObserver === obs) spyObserver = undefined
    }
  })

  $effect(() => {
    const root = diffscrollEl
    if (!root || prtab !== 'files') return
    const ro = new ResizeObserver((entries) => {
      paneWidth = entries[0]?.contentRect.width ?? 0
    })
    ro.observe(root)
    return () => ro.disconnect()
  })

  $effect(() => {
    if (prtab !== 'files') return
    void tick().then(() => {
      const el = document.activeElement
      if (el instanceof HTMLElement && (el.closest('.icmt-compose') || el.tagName === 'TEXTAREA' || el.tagName === 'INPUT')) return
      diffscrollEl?.focus({ preventScroll: true })
    })
  })

  function onDiffKeydown(e: KeyboardEvent) {
    const t = e.target
    if (t instanceof HTMLElement) {
      const tag = t.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || t.closest('.icmt-compose')) return
    }
    const paths = matchedFiles.map((f) => f.path)
    if ((e.key === 'j' && !e.metaKey && !e.ctrlKey && !e.altKey) || (e.key === 'ArrowDown' && e.altKey)) {
      e.preventDefault()
      const next = stepPath(paths, selectedPath, 1)
      if (next) void selectPath(next)
    } else if ((e.key === 'k' && !e.metaKey && !e.ctrlKey && !e.altKey) || (e.key === 'ArrowUp' && e.altKey)) {
      e.preventDefault()
      const prev = stepPath(paths, selectedPath, -1)
      if (prev) void selectPath(prev)
    }
  }

  function toggleFoldAll() {
    collapsedFiles = anyExpanded ? new Set(matchedFiles.map((f) => f.path)) : new Set()
    void tick().then(refreshWindow)
  }

  $effect(() => {
    const paths = matchedFiles.map((f) => f.path)
    if (!paths.length) {
      selectedPath = ''
      return
    }
    if (!paths.includes(selectedPath)) selectedPath = paths[0]
  })

  $effect(() => {
    void matchedFiles
    void selectedPath
    if (prtab !== 'files') return
    void tick().then(refreshWindow)
  })

  $effect(() => {
    if (prtab !== 'commits' || commitIdx === null) return
    const pane = commitsPaneEl
    void detailFiles
    if (!pane) return
    const vis = new Set<string>()
    const viewObs = new IntersectionObserver((entries) => {
      let changed = false
      for (const e of entries) {
        const path = (e.target as HTMLElement).dataset.path ?? ''
        if (!path) continue
        if (e.isIntersecting) {
          if (!vis.has(path)) { vis.add(path); changed = true }
        } else if (vis.delete(path)) changed = true
      }
      if (changed) commitInView = [...vis]
    }, { root: pane, rootMargin: '900px 0px', threshold: 0 })
    const spyObs = new IntersectionObserver(() => spyCommitFromOffsets(), { root: pane, threshold: 0 })
    commitViewObserver = viewObs
    commitSpyObserver = spyObs
    for (const el of commitEls.values()) {
      viewObs.observe(el)
      spyObs.observe(el)
    }
    let raf = 0
    const onScroll = () => {
      if (raf) return
      raf = requestAnimationFrame(() => {
        raf = 0
        spyCommitFromOffsets()
        syncCommitHydrated()
      })
    }
    pane.addEventListener('scroll', onScroll, { passive: true })
    syncCommitHydrated()
    return () => {
      cancelAnimationFrame(raf)
      pane.removeEventListener('scroll', onScroll)
      viewObs.disconnect()
      spyObs.disconnect()
      if (commitViewObserver === viewObs) commitViewObserver = undefined
      if (commitSpyObserver === spyObs) commitSpyObserver = undefined
    }
  })

  $effect(() => {
    void detailFiles
    void commitSelected
    if (prtab !== 'commits') return
    syncCommitHydrated()
  })

  async function selectPath(path: string, opts?: { highlight?: number }) {
    selectedPath = path
    highlightLine = opts?.highlight ?? 0
    if (collapsedFiles.has(path)) {
      const set = new Set(collapsedFiles)
      set.delete(path)
      collapsedFiles = set
    }
    spySuppressUntil = performance.now() + 1400
    pinGen++
    await tick()
    refreshWindow()
    const root = diffscrollEl
    const fileEl = fileEls.get(path)
    if (!root || !fileEl) return
    root.scrollTo({ top: fileEl.offsetTop, behavior: 'instant' })
    const line = highlightLine
    if (line <= 0) {
      await tick()
      const el2 = fileEls.get(path)
      if (el2) root.scrollTo({ top: el2.offsetTop, behavior: 'instant' })
      lastPin = captureViewportPin(fileRanges(), root.scrollTop, root.clientHeight, root.scrollHeight)
      return
    }
    for (let i = 0; i < 12; i++) {
      await tick()
      const target = pickDiffJumpEl(fileEl, line)
      if (target && target !== fileEl) {
        paneScrollTo(root, target)
        lastPin = captureViewportPin(fileRanges(), root.scrollTop, root.clientHeight, root.scrollHeight)
        return
      }
      refreshWindow()
      await new Promise<void>((r) => requestAnimationFrame(() => r()))
    }
    lastPin = captureViewportPin(fileRanges(), root.scrollTop, root.clientHeight, root.scrollHeight)
  }

  function matchFilePath(path: string): string {
    return matchFindingPath(path, (pr?.files ?? []).map((f) => f.path))
  }

  function jumpToFinding(f: Finding) {
    jumpToImpact(f.file, f.line)
  }

  /** Files changed when the PR touches `path`; otherwise the file on GitHub at the PR head. */
  function jumpToImpact(path: string, line?: number) {
    if (!path || filesPending) return
    const resolved = matchFilePath(path)
    if (resolved) {
      setTab('files')
      void selectPath(resolved, { highlight: line || 0 })
      return
    }
    const root = ($workspaces.find((w) => w.id === prAgentWsId)?.worktreePath ?? '').replace(/\/+$/, '')
    const rel = root && path.startsWith(root + '/') ? path.slice(root.length + 1) : path
    if (rel.startsWith('/')) return
    const href = githubBlobUrl(pr?.url, pr?.headSha ?? '', rel, line)
    if (href) void openExternalUrl(href)
  }

  $effect(() => {
    const target = $prDiffJump
    if (!target) return
    jumpToImpact(target.file, target.line)
    prDiffJump.set(null)
  })

  // Live thread entries appended by pr.comment events, keyed so they survive re-renders.
  let liveComments = $state<PRComment[]>([])

  const pr = $derived($prs.find((p) => p.id === $selectedPRId))
  const prAgentWsId = $derived(prAgentWorkspaceId(pr, $workspaces))
  const prAgentRunning = $derived(prAgentIsRunning(pr, $workspaces, $workspacesRunning))
  const updatingBase = $derived(pr?.updatingBase ?? '')
  const isUpdating = $derived(!!updatingBase)
  const updateBaseName = $derived(shortBaseName(pr?.base))
  // Store / git log order is newest-first; the Commits tab keeps that.
  const prCommits = $derived(pr?.commits ?? [])
  const detailFiles = $derived.by(() => {
    const raw = commitFiles?.length
      ? commitFiles
      : (commitIdx != null ? prCommits[commitIdx]?.files : undefined) ?? []
    return sortFilesByChangedTree(raw)
  })

  function pinIfCommitAtEnd() {
    const pane = commitsPaneEl
    if (!pane || !commitLastPin?.atEnd || performance.now() < commitSpyUntil) return
    pane.scrollTop = Math.max(0, pane.scrollHeight - pane.clientHeight)
  }

  function syncCommitHydrated() {
    const pane = commitsPaneEl
    if (!pane) return
    const ranges = [...commitEls.entries()]
      .map(([path, el]) => ({ path, top: el.offsetTop, bottom: el.offsetTop + el.offsetHeight }))
      .sort((a, b) => a.top - b.top)
    const pin = captureViewportPin(ranges, pane.scrollTop, pane.clientHeight, pane.scrollHeight)
    commitLastPin = pin
    const nextInView = inViewPaths(ranges, pane.scrollTop, pane.clientHeight)
    const nextHydrated = pickHydrated(
      detailFiles.map((f) => f.path),
      commitSelected || detailFiles[0]?.path || '',
      nextInView,
    )
    const swapped = !samePathSet(commitHydrated, nextHydrated)
    if (!samePathList(commitInView, nextInView)) commitInView = nextInView
    if (swapped) commitHydrated = nextHydrated
    syncStuckHeaders(commitEls.values(), pane.scrollTop)
    if (!swapped || performance.now() < commitSpyUntil) return
    const capturedTop = pane.scrollTop
    const token = ++commitPinGen
    void tick().then(() => {
      requestAnimationFrame(() => {
        if (token !== commitPinGen) return
        const after = [...commitEls.entries()]
          .map(([path, el]) => ({ path, top: el.offsetTop, bottom: el.offsetTop + el.offsetHeight }))
          .sort((a, b) => a.top - b.top)
        applyViewportPin(pane, after, pin, capturedTop)
        commitLastPin = captureViewportPin(after, pane.scrollTop, pane.clientHeight, pane.scrollHeight)
      })
    })
  }

  async function openCommit(i: number) {
    commitIdx = i
    const c = prCommits[i]
    if (!c) return
    await loadCommitDiff(c)
  }

  let commitDiffGen = 0
  function discardCommitDiff() {
    commitDiffGen++
    commitFiles = null
    commitSelected = ''
    commitInView = []
    commitHydrated = new Set()
    commitDiffLoading = false
    commitDiffError = false
  }

  function spyCommit(path: string) {
    if (!path || path === commitSelected) return
    commitSelected = path
  }

  function spyCommitFromOffsets() {
    const pane = commitsPaneEl
    if (!pane || performance.now() < commitSpyUntil) return
    const ranges = [...commitEls.entries()]
      .map(([path, el]) => ({ path, top: el.offsetTop, bottom: el.offsetTop + el.offsetHeight }))
      .sort((a, b) => a.top - b.top)
    spyCommit(fileAtScrollTop(ranges, pane.scrollTop))
  }

  function registerCommitEl(path: string) {
    return (el: HTMLElement) => {
      commitEls.set(path, el)
      commitViewObserver?.observe(el)
      commitSpyObserver?.observe(el)
      const stopMeasure = recordStubHeight(el, path, commitMeasuredH, pinIfCommitAtEnd)
      return () => {
        stopMeasure()
        commitViewObserver?.unobserve(el)
        commitSpyObserver?.unobserve(el)
        commitEls.delete(path)
      }
    }
  }

  function cacheCommitFiles(hash: string, files: FileDiff[]) {
    if (!hash || !files.length) return
    commitFilesByHash = { ...commitFilesByHash, [hash]: files }
  }

  let commitCachePR = ''
  let findingsGen = 0
  $effect(() => {
    const id = $selectedPRId
    if (id === commitCachePR) return
    commitCachePR = id
    commitFilesByHash = {}
    filterText = ''
    findingsOnly = false
    commentsOnly = false
    findingsGen++
    findings = []
    postingFindingIds = new Set()
    settlingFindingIds = new Set()
    settleFailedIds = new Set()
    resetPostedFindings()
  })

  $effect(() => {
    for (const pc of pr?.pendingComments ?? []) markPostedFinding(pc.findingId ?? '')
  })

  async function loadCommitDiff(c: Commit) {
    const hash = (c.hash || c.sha || '').trim()
    if (c.files?.length) {
      commitDiffGen++
      commitFiles = c.files
      commitSelected = c.files[0]?.path ?? ''
      commitDiffLoading = false
      commitDiffError = false
      cacheCommitFiles(hash, c.files)
      return
    }
    const cached = hash ? commitFilesByHash[hash] : undefined
    if (cached?.length) {
      commitDiffGen++
      commitFiles = cached
      commitSelected = cached[0]?.path ?? ''
      commitDiffLoading = false
      commitDiffError = false
      return
    }
    const id = $selectedPRId
    if (!id || !hash) return
    const token = ++commitDiffGen
    commitDiffLoading = true
    commitDiffError = false
    commitFiles = null
    try {
      const full = await PRsService.GetCommitDiff(id, hash)
      if (token !== commitDiffGen) return
      const files = full?.files ?? []
      commitFiles = files
      commitSelected = files[0]?.path ?? ''
      cacheCommitFiles(hash, files)
    } catch {
      if (token !== commitDiffGen) return
      commitFiles = []
      commitDiffError = true
    } finally {
      if (token === commitDiffGen) commitDiffLoading = false
    }
  }

  function attachAgentHost(node: HTMLElement) {
    prAgentDockHost.set(node)
    return () => {
      if (get(prAgentDockHost) === node) prAgentDockHost.set(null)
    }
  }

  let prColEl = $state<HTMLElement | undefined>(undefined)
  const prTabsLit = $derived(!prAgentWsId || !$prAgentDockOpen || $prDetailFocus === 'pr')

  // Chat transcript / empty PR chrome aren't focusable, so a click there never
  // fires focusin. Park on the column (same pattern as Observatory's left dock)
  // so the review tabs light up.
  function onPRChromeMouseDown(e: MouseEvent) {
    if (e.button !== 0) return
    const t = e.target
    if (!(t instanceof Element)) return
    if (t.closest('#prd-agent, .obs-left')) return
    if (
      t.closest(
        'textarea, input, select, button, a, [role="button"], [role="tab"], [contenteditable="true"]',
      )
    ) {
      return
    }
    prColEl?.focus({ preventScroll: true })
  }

  async function showPRAgentDock() {
    prAgentDockOpen.set(true)
    await tick()
    document.querySelector<HTMLElement>('#prd-agent button.prd-chat-toggle')?.focus()
  }

  // Refresh the full PR from Go when the detail opens or its id changes. Engine-
  // created PRs carry their real diff/commits/thread only on the full record, so we
  // pull GetPR and merge it into the prs store (the list rows carry the summary).
  // silent: background GH-check poll — no loading flash, no thread reset.
  let refreshing = $state(false)
  async function refresh(id: string, opts?: { silent?: boolean; findings?: boolean }) {
    if (!id) return
    const silent = !!opts?.silent
    if (!silent) refreshing = true
    const full = await PRsService.GetPR(id).catch(() => null)
    if (!silent) refreshing = false
    if (!full || !full.id) {
      // Unknown / deleted PR — bounce to the list rather than rendering blank.
      // Poll failures stay put (gh blips should not kick you out).
      if (!silent && $view === 'prdetail' && $selectedPRId === id) go('pr')
      return
    }
    // Upsert: engine-created PRs may not be in the list yet when Inbox/palette opens them.
    // Keep in-flight optimistic replies so a silent GetPR cannot drop them before GH echoes.
    prs.update((arr) => {
      const i = arr.findIndex((p) => p.id === full.id)
      const prev = i >= 0 ? arr[i] : undefined
      const merged = keepMergedPR(prev, {
        ...full,
        comments: applyReactionHolds(applyThreadResolvedHolds(keepPendingReplies(prev?.comments ?? [], full.comments ?? []), threadResolvedHold), commentReactionHold),
        commits: keepCommitFiles(prev?.commits, full.commits ?? []),
      })
      if (i >= 0) { const next = arr.slice(); next[i] = merged; return next }
      return [...arr, merged]
    })
    // Drop live rows GetPR now has so pr.comment + pr.updated cannot double-render.
    // Keep not-yet-persisted live rows on silent polls (GetPR can lag the event).
    const persisted = new Set((full.comments ?? []).map((c) => c.id).filter(Boolean))
    liveComments = silent
      ? liveComments.filter((c) => !persisted.has(c.id))
      : []
    if (!silent || opts?.findings) {
      const token = ++findingsGen
      PRsService.ListPRFindings(id).then((f) => {
        if (token !== findingsGen) return
        findings = f ?? []
      }).catch(() => {
        if (token !== findingsGen) return
        findings = []
      })
    }
  }

  $effect(() => {
    if ($view !== 'prdetail') return
    const id = $selectedPRId
    if (!id) { go('pr'); return }
    // Always GetPR — do not require list membership (engine-created PRs arrive via event).
    refresh(id)
  })

  // Live GitHub checks while this PR is open (gh pr checks on GetPR).
  // Interval reads the store via get() so check updates do not reset the timer.
  const GH_CHECKS_POLL_MS = 15_000
  $effect(() => {
    if ($view !== 'prdetail') return
    const id = $selectedPRId
    if (!id) return
    let inFlight = false
    const tick = async () => {
      if (inFlight || (typeof document !== 'undefined' && document.hidden)) return
      const p = get(prs).find((x) => x.id === id)
      if (!p || p.source !== 'gh' || p.state === 'Merged' || p.group === 'merged') return
      inFlight = true
      try {
        await refresh(id, { silent: true })
      } finally {
        inFlight = false
      }
    }
    const t = setInterval(tick, GH_CHECKS_POLL_MS)
    return () => clearInterval(t)
  })

  // Live thread updates: append a pr.comment for THIS PR.
  $effect(() => onPRComment((prId, comment) => {
    if (prId !== $selectedPRId) return
    const row = comment as PRComment
    if (!row.id) return
    if ((pr?.comments ?? []).some((c) => c.id === row.id)) return
    if (liveComments.some((c) => c.id === row.id)) return
    const pending = (pr?.comments ?? []).find((c) => isPendingComment(c) && sameThreadReply(c, row))
    if (pending) {
      patchPRComments(prId, (comments) => settlePostedReply(comments, pending.id, row))
      return
    }
    liveComments = [...liveComments, row]
  }))

  // When a pr.updated lands for this PR (e.g. rework re-requested review, merge),
  // re-pull the full record so the diff/commits/checks reflect the new state.
  $effect(() => onPRRefresh((prId) => {
    if (prId === $selectedPRId) refresh($selectedPRId, { silent: true, findings: true })
  }))

  // Review-mode report_finding lands on the chat host (GitHub-only) or linked workspace.
  $effect(() => onFindingsUpdated((wsId) => {
    if ($view !== 'prdetail' || !wsId || wsId !== prAgentWsId || !$selectedPRId) return
    refresh($selectedPRId, { silent: true, findings: true })
  }))

  function setTab(t: PRTab) {
    prDetailTab.set(t)
    if (t === 'commits') {
      commitIdx = null
      discardCommitDiff()
      refresh($selectedPRId, { silent: true })
    }
    if (t === 'checks' && pr?.source === 'gh') refresh($selectedPRId, { silent: true })
  }

  function localCommitIndex(sha?: string): number {
    const s = (sha ?? '').toLowerCase()
    if (!s) return -1
    return prCommits.findIndex((c) => {
      const h = (c.hash ?? '').toLowerCase()
      const full = (c.sha ?? '').toLowerCase()
      return (!!h && (h === s || h.startsWith(s) || s.startsWith(h))) ||
        (!!full && (full === s || full.startsWith(s) || s.startsWith(full)))
    })
  }

  function openLocalCommit(i: number) {
    prDetailTab.set('commits')
    void openCommit(i)
  }

  function checkIcon(c: { passed: boolean; status?: string; detail?: string }): { name: IconName; color: string; spin?: boolean } {
    if (isSkippedCheck(c)) return { name: 'circle-slash', color: 'var(--muted)' }
    if (c.passed) return { name: 'check', color: 'var(--emerald)' }
    if (isPendingCheck(c)) return { name: 'loader', color: 'var(--amber)', spin: true }
    return { name: 'x', color: 'var(--coral)' }
  }

  const isGitHubPR = $derived(pr?.source === 'gh')
  let hydrateGiveUp = $state(false)
  const convPending = $derived(prConvPending(pr, hydrateGiveUp))
  const filesPending = $derived(prFilesPending(pr, refreshing, hydrateGiveUp))
  const commitsPending = $derived(prCommitsPending(pr, refreshing))
  const checksPending = $derived(prChecksPending(pr, refreshing))
  $effect(() => {
    if ($view !== 'prdetail') return
    const id = $selectedPRId
    if (!id) return
    hydrateGiveUp = false
    const t = setTimeout(() => { hydrateGiveUp = true }, 25_000)
    return () => clearTimeout(t)
  })
  const attnLines = $derived(prAttentionLabels(pr))
  const threadItems = $derived([...(pr?.comments ?? []), ...liveComments])
  const description = $derived(threadItems.find((c) => c.kind === 'description'))
  const timeline = $derived(threadItems.filter(isTimelineRoot))
  const viewerLogin = $derived((pr?.viewerLogin ?? '').toLowerCase())
  const isPRAuthor = $derived(!!viewerLogin && viewerLogin === (pr?.authorLogin ?? '').toLowerCase())
  const conversationLocked = $derived(prConversationLocked(pr))
  const canPostFindings = $derived(isGitHubPR && !conversationLocked)
  const canEditPR = $derived(!conversationLocked && (!isGitHubPR || isPRAuthor || !!pr?.viewerCanWrite))

  async function postFindings(list: Finding[]) {
    if (!pr?.id || !canPostFindings) return
    const prId = pr.id
    const files = reviewFiles
    const pending = pr.pendingComments ?? []
    const jobs = list
      .filter((f) => !postingFindingIds.has(f.id) && !$postedFindingIds.has(f.id) && !findingHasPendingComment(f, pending, files))
      .map((f) => ({ f, a: findingReviewAnchor(f, files), body: findingCommentBody(f.message) }))
      .filter((x): x is { f: Finding; a: NonNullable<ReturnType<typeof findingReviewAnchor>>; body: string } => !!x.a && !!x.body)
    if (!jobs.length) return
    postingFindingIds = new Set([...postingFindingIds, ...jobs.map((j) => j.f.id)])
    let n = 0
    try {
      for (const { f, a, body } of jobs) {
        const pc: PendingLineComment = {
          id: `pend-local-${Date.now()}-${f.id}`,
          file: a.file,
          line: a.line,
          side: 'RIGHT',
          startLine: a.startLine || undefined,
          startSide: a.startLine ? 'RIGHT' : undefined,
          body,
          findingId: f.id,
        }
        patchPRPendingComments(prId, (prev) => [...prev, pc])
        const ok = await call(PRsService.AddPRComment(prId, a.file, a.line, body, a.startLine, f.id), 'Could not queue the comment')
        if (ok === null) {
          patchPRPendingComments(prId, (prev) => prev.filter((x) => x.id !== pc.id))
          continue
        }
        markPostedFinding(f.id)
        n++
      }
      if (n === 1) pushToast('success', 'Saved — submit your review to post')
      else if (n > 1) pushToast('success', `${n} comments pending — submit a review to post them`)
    } finally {
      const left = new Set(postingFindingIds)
      for (const { f } of jobs) left.delete(f.id)
      postingFindingIds = left
    }
  }

  $effect(() => {
    const prId = pr?.id
    if (!prId) return
    void prAgentWsId
    const pending = pr?.pendingComments ?? []
    const comments = threadItems
    const files = reviewFiles
    const posted = $postedFindingIds
    const failed = settleFailedIds
    if (!openFindings.some((f) => !failed.has(f.id) && findingReadyToSettle(f, pending, comments, files, posted))) return
    untrack(() => { void settlePostedFindings(prId) })
  })

  async function settlePostedFindings(prId: string, retryIds?: string[]) {
    if (settleFor === prId) return
    settleFor = prId
    const files = reviewFiles
    const pending = pr?.pendingComments ?? []
    const comments = threadItems
    const posted = get(postedFindingIds)
    const retry = retryIds ? new Set(retryIds) : null
    const due = openFindings.filter((f) => {
      if (retry ? !retry.has(f.id) : settleFailedIds.has(f.id)) return false
      return findingReadyToSettle(f, pending, comments, files, posted)
    })
    if (!due.length) {
      if (settleFor === prId) settleFor = ''
      return
    }
    settlingFindingIds = new Set([...settlingFindingIds, ...due.map((f) => f.id)])
    try {
      for (const f of due) {
        if (pr?.id !== prId) return
        const wsId = (f.workspaceId || prAgentWsId || '').trim()
        if (!wsId) continue
        const ok = await call(ObservatoryService.SetFindingStatus(wsId, f.id, 'resolved'), 'Could not resolve the finding')
        if (pr?.id !== prId) return
        if (ok === null) {
          settleFailedIds = new Set([...settleFailedIds, f.id])
          continue
        }
        const leftFailed = new Set(settleFailedIds)
        leftFailed.delete(f.id)
        settleFailedIds = leftFailed
        forgetPostedFinding(f.id)
        findings = findings.map((x) => x.id === f.id ? { ...x, status: 'resolved' } : x)
      }
    } finally {
      if (settleFor === prId) {
        settleFor = ''
        const left = new Set(settlingFindingIds)
        for (const f of due) left.delete(f.id)
        settlingFindingIds = left
      }
    }
  }
  const prLife = $derived(prLifecycle(pr))
  const checkGroups = $derived(groupChecks(pr?.checks ?? []))
  const checksSummary = $derived(checksRollup(pr?.checks, { checksOk: pr?.checksOk, hasChecks: pr?.hasChecks }))
  const checksPassedCount = $derived((pr?.checks ?? []).filter((c) => c.passed).length)
  const checksTotal = $derived(pr?.checks?.length ?? 0)
  const checksSummaryHref = $derived(isGitHubPR ? githubPRChecksTabUrl(pr?.url) : '')
  const canFixChecks = $derived(
    isGitHubPR && prLife !== 'merged' && prLife !== 'closed' && checksSummary.kind === 'fail',
  )
  let collapsedCheckGroups = $state<Record<string, boolean>>({})
  const pendingCheckClocks = new Map<string, PendingClock>()
  let checkNow = $state(Date.now())
  const hasPendingChecks = $derived((pr?.checks ?? []).some((c) => isPendingCheck(c)))
  $effect(() => {
    prunePendingClocks(pendingCheckClocks, pr?.checks ?? [])
  })
  $effect(() => {
    if ($view !== 'prdetail' || !hasPendingChecks) return
    checkNow = Date.now()
    const t = setInterval(() => { checkNow = Date.now() }, 1000)
    return () => clearInterval(t)
  })
  function toggleCheckGroup(key: string, compact: boolean) {
    const next = !checkGroupCollapsed(key, collapsedCheckGroups, compact)
    collapsedCheckGroups = { ...collapsedCheckGroups, [key]: next }
  }

  function commitTitle(c: PRComment): string {
    let t = (c.bodyMd ?? '').replace(/^committed\s+/i, '')
    t = t.replace(/\s+\([0-9a-f]{7,}\)\s*$/i, '')
    return t
  }
  function shortSha(sha: string): string {
    return sha.length > 7 ? sha.slice(0, 7) : sha
  }

  function actionIcon(c: PRComment): IconName {
    const t = (c.bodyMd ?? '').toLowerCase()
    if (t.startsWith('committed')) return 'git-commit'
    if (t.startsWith('merged')) return 'git-merge'
    if (t.includes('deleted the') && t.includes('branch')) return 'git-branch'
    if (t.includes('restored the') && t.includes('branch')) return 'git-branch'
    if (t.startsWith('closed')) return 'circle-slash'
    if (t.includes('started review') || t.includes('started work') || t.includes('finished work') || t.includes('finished review') || t.includes('stopped work')) return 'git-branch'
    if (t.includes('review')) return 'eye'
    if (t.includes('force-pushed') || t.includes('assigned')) return 'git-branch'
    return 'git-pull-request'
  }

  function isMergeAction(c: PRComment): boolean {
    return (c.bodyMd ?? '').toLowerCase().startsWith('merged ')
  }

  function mergeCommitLink(c: PRComment): { sha: string; rest: string; href: string } | null {
    const m = /^merged commit ([0-9a-f]{7,40})(.*)$/i.exec(c.bodyMd ?? '')
    if (!m) return null
    const full = (c.commitSha ?? '').trim() || m[1]
    return { sha: shortSha(m[1]), rest: m[2].trim(), href: githubCommitUrl(pr?.url, full) }
  }

  let copiedSha = $state('')
  let copyShaTimer: ReturnType<typeof setTimeout> | undefined
  async function copyCommitSha(e: MouseEvent, c: Commit) {
    e.stopPropagation()
    const sha = commitSha(c)
    if (!sha) return
    try {
      await copyText(sha)
      copiedSha = sha
      clearTimeout(copyShaTimer)
      copyShaTimer = setTimeout(() => { if (copiedSha === sha) copiedSha = '' }, 1200)
    } catch {
      pushToast('error', 'Could not copy SHA')
    }
  }

  function actionBehalf(c: PRComment): { verb: string; whom: string } | null {
    const m = /^(started reviewing|started work|finished reviewing|finished work|stopped work) on behalf of (.+?)(?: due to an error)?$/.exec(c.bodyMd ?? '')
    if (!m) return null
    return { verb: m[1], whom: m[2] }
  }

  $effect(() => {
    $selectedPRId
    collapsedCheckGroups = {}
    editingTitle = false
    editingDesc = false
    editingCommentId = ''
    savingField = ''
    replyFor = ''
    replyDraft = ''
    resetThreadResolvedHolds()
    resetCommentReactionHolds()
    closeMore()
    commitIdx = null
    discardCommitDiff()
    prDetailTab.set('conv')
    prComposerDraft.set('')
  })

  function canEditComment(c: PRComment) {
    return canEditPRComment(pr, c)
  }

  function canDeleteComment(c: PRComment) {
    return canDeletePRComment(pr, c)
  }

  function toggleCtx(id: string) {
    collapsedCtx[id] = !collapsedCtx[id]
  }

  function toggleKidHunk(child: PRComment) {
    if (child.resolved) openResolved[child.id] = !openResolved[child.id]
    else toggleCtx(child.id)
  }

  function canResolveNote(c: PRComment) {
    return canResolveLineNote(pr, c)
  }

  async function setThreadResolved(c: PRComment, resolved: boolean) {
    const key = lineThreadKey(c)
    if (resolved) openResolved[c.id] = false
    liveComments = applyThreadResolved(liveComments, key, resolved)
    const ok = await setPRThreadResolved(c, resolved)
    if (!ok && threadResolvedHold[key] !== resolved) {
      liveComments = applyThreadResolved(liveComments, key, !resolved)
    }
  }

  function openReply(id: string) {
    if (conversationLocked) return
    replyFor = id
    replyDraft = ''
    queueMicrotask(() => document.getElementById(`reply-${id}`)?.focus())
  }

  function patchPRComments(prId: string, next: (comments: PRComment[]) => PRComment[]) {
    prs.update((arr) => arr.map((p) => (p.id !== prId ? p : { ...p, comments: next(p.comments ?? []) })))
  }

  function optimisticReply(rootId: string, body: string): PRComment {
    const root = threadItems.find((c) => c.id === rootId)
    const mine = threadItems.find((c) => c.kind !== 'description' && canEditComment(c))
    const login = pr?.viewerLogin || mine?.authorLogin || ''
    const author = mine?.author || login || 'You'
    return {
      id: `${PENDING_REPLY_PREFIX}${Date.now()}`,
      code: mine?.code || author.slice(0, 2).toUpperCase(),
      author,
      authorLogin: login,
      avatarCss: mine?.avatarCss || '',
      avatarUrl: mine?.avatarUrl,
      time: 'now',
      bodyHtml: '',
      bodyMd: body,
      verb: 'commented',
      kind: 'comment',
      inReplyToId: rootId,
      parentId: root?.parentId,
      path: root?.path,
      line: root?.line,
      side: root?.side,
      threadId: root?.threadId || rootId,
      resolved: root?.resolved,
    }
  }

  async function submitReply(rootId: string) {
    const body = replyDraft.trim()
    if (!body || savingField || conversationLocked) return
    const prId = $selectedPRId
    const pending = optimisticReply(rootId, body)
    // Close the composer before the GH write returns — a hung `gh api` used to
    // leave Saving… up even after the 5s poll already showed the posted note.
    patchPRComments(prId, (comments) => [...comments, pending])
    replyFor = ''
    replyDraft = ''
    const posted = await call(PRsService.ReplyPRComment(prId, rootId, body), 'Could not reply')
    if (posted?.id) {
      patchPRComments(prId, (rows) => settlePostedReply(rows, pending.id, posted))
      liveComments = liveComments.filter((c) => c.id !== posted.id)
      pushToast('success', 'Reply posted')
      return
    }
    patchPRComments(prId, (rows) => settlePostedReply(rows, pending.id, null))
  }

  async function deleteComment(id: string) {
    if (savingField || !id) return
    const prId = $selectedPRId
    const fromPR = (pr?.comments ?? []).filter((c) => isDroppedComment(c, id))
    const fromLive = liveComments.filter((c) => isDroppedComment(c, id))
    if (!fromPR.length && !fromLive.length) return
    savingField = `del-${id}`
    prs.update((arr) => arr.map((p) => p.id !== prId ? p : { ...p, comments: (p.comments ?? []).filter((c) => !isDroppedComment(c, id)) }))
    liveComments = liveComments.filter((c) => !isDroppedComment(c, id))
    if (editingCommentId === id) editingCommentId = ''
    if (replyFor === id) replyFor = ''
    const updated = await call(PRsService.DeletePRComment(prId, id), 'Could not delete comment')
    savingField = ''
    if (!updated) {
      prs.update((arr) => arr.map((p) => p.id !== prId ? p : { ...p, comments: [...(p.comments ?? []), ...fromPR] }))
      liveComments = [...liveComments, ...fromLive]
      return
    }
    prs.update((arr) => arr.map((p) => (p.id === updated.id ? updated : p)))
    pushToast('success', 'Comment deleted')
  }

  function quoteReply(c: PRComment) {
    if (conversationLocked) return
    const src = commentSource(c)
    if (!src) return
    const quoted = src.split('\n').map((line) => `> ${line}`).join('\n') + '\n\n'
    const threadRoot = isLineNote(c) ? ((c.inReplyToId ?? '').trim() || c.id) : ''
    if (threadRoot) {
      const keep = replyFor === threadRoot ? replyDraft : ''
      replyFor = threadRoot
      replyDraft = quoted + keep
      setTab('conv')
      queueMicrotask(() => document.getElementById(`reply-${threadRoot}`)?.focus())
      return
    }
    prComposerDraft.set(quoted + $prComposerDraft)
    setTab('conv')
    queueMicrotask(() => composerEl?.focus())
  }

  function closeMore() {
    moreMenuId = ''
    moreMenuBox = null
    moreTrigger = null
  }

  function toggleMore(id: string, e: MouseEvent) {
    e.stopPropagation()
    if (moreMenuId === id) {
      closeMore()
      return
    }
    moreTrigger = e.currentTarget as HTMLElement
    moreMenuId = id
    moreMenuBox = moreMenuFixedBox(moreTrigger.getBoundingClientRect())
  }

  function moreTarget(): PRComment | undefined {
    if (!moreMenuId) return
    if (description && moreMenuId === description.id) return description
    if (moreMenuId === 'gh-description') return description
    return threadItems.find((c) => c.id === moreMenuId)
  }

  function hasMoreActions(c: PRComment | undefined, canEdit: boolean) {
    if (canEdit) return true
    if (c && canDeleteComment(c)) return true
    if (c && commentSource(c)) return true
    return !!commentPermalink(pr?.url, c ?? { id: 'gh-description', kind: 'description' })
  }

  function moreEdit() {
    const c = moreTarget()
    closeMore()
    if (!c) return
    if (c.kind === 'description') startEditDesc()
    else startEditComment(c)
  }

  function moreQuote() {
    const c = moreTarget()
    closeMore()
    if (c) quoteReply(c)
  }

  function morePermalink() {
    return commentPermalink(pr?.url, moreTarget() ?? { id: 'gh-description', kind: 'description' })
  }

  async function moreCopyLink() {
    const link = morePermalink()
    closeMore()
    if (!link) return
    await copyText(link)
    pushToast('info', 'Copied link')
  }

  async function moreOpenGitHub() {
    const link = morePermalink()
    closeMore()
    if (!link) return
    await openExternalUrl(link)
  }

  function moreDelete() {
    const c = moreTarget()
    closeMore()
    if (c) void deleteComment(c.id)
  }

  function startEditTitle() {
    if (!canEditPR) return
    titleDraft = pr?.title ?? ''
    editingTitle = true
    editingDesc = false
    editingCommentId = ''
  }

  function startEditDesc() {
    if (!canEditPR) return
    descDraft = commentSource(description)
    editingDesc = true
    editingTitle = false
    editingCommentId = ''
  }

  async function savePRField(field: string, title: string, body: string, done: () => void, okMsg: string) {
    if (!canEditPR || savingField) return
    savingField = field
    const updated = await call(PRsService.UpdatePRDetails($selectedPRId, title, body), 'Could not update pull request')
    savingField = ''
    if (!updated) return
    prs.update((arr) => arr.map((p) => (p.id === updated.id ? updated : p)))
    done()
    pushToast('success', okMsg)
  }

  async function saveTitle() {
    const title = titleDraft.trim()
    if (!title || savingField) return
    await savePRField('title', title, commentSource(description), () => { editingTitle = false }, 'Title updated')
  }

  async function saveDesc() {
    if (savingField) return
    await savePRField('desc', pr?.title ?? '', descDraft, () => { editingDesc = false }, 'Description updated')
  }

  function startEditComment(c: PRComment) {
    editingCommentId = c.id
    commentDraft = commentSource(c)
    editingTitle = false
    editingDesc = false
  }

  async function saveComment(id: string) {
    const body = commentDraft.trim()
    if (!body || savingField) return
    savingField = id
    const updated = await call(PRsService.UpdatePRComment($selectedPRId, id, body), 'Could not update comment')
    savingField = ''
    if (!updated) return
    prs.update((arr) => arr.map((p) => {
      if (p.id !== $selectedPRId) return p
      return { ...p, comments: (p.comments ?? []).map((c) => c.id === id ? { ...c, ...updated, bodyMd: body } : c) }
    }))
    liveComments = liveComments.map((c) => c.id === id ? { ...c, ...updated, bodyMd: body } : c)
    editingCommentId = ''
    pushToast('success', 'Comment updated')
  }

  async function submitComposer() {
    const draft = $prComposerDraft.trim()
    if (!draft || conversationLocked || savingField) return
    savingField = 'composer'
    try {
      const posted = await call(PRsService.AddPRComment($selectedPRId, '', 0, draft, 0, ''), 'Comment failed')
      if (!posted) return
      prs.update((arr) => arr.map((p) => {
        if (p.id !== $selectedPRId) return p
        return { ...p, comments: [...(p.comments ?? []), posted] }
      }))
      prComposerDraft.set('')
      pushToast('success', 'Comment posted')
    } finally {
      savingField = ''
    }
  }

  function composeKey(e: KeyboardEvent, save: () => void | Promise<void>, cancel: () => void) {
    if (e.key === 'Enter' && isCmdOrCtrl(e)) {
      e.preventDefault()
      void save()
      return
    }
    if (e.key === 'Escape') {
      e.preventDefault()
      cancel()
    }
  }

  $effect(() => {
    if ($appOverlayOpen) closeMore()
  })

  $effect(() => {
    if (!moreMenuId) return
    const onDown = (ev: MouseEvent) => {
      const el = ev.target as HTMLElement | null
      if (el?.closest?.('.cmt-more-menu') || el?.closest?.('.cmt-more')) return
      closeMore()
    }
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== 'Escape') return
      ev.preventDefault()
      ev.stopPropagation()
      closeMore()
    }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey, true)
    }
  })
</script>

{#snippet shaCopy(c: Commit)}
  {@const sha = commitSha(c)}
  {@const ok = copiedSha === sha}
  <button type="button" class="chash-copy" class:copied={ok} title={ok ? 'Copied' : 'Copy full SHA'} aria-label={ok ? 'Copied' : 'Copy full SHA'} onclick={(e) => copyCommitSha(e, c)}>
    {#if ok}
      <Icon name="check" strokeWidth={2.4} />
    {:else}
      <Icon name="copy" />
    {/if}
  </button>
{/snippet}

{#snippet commitCheck(status: string | undefined)}
  {@const icon = status ? checkIcon({ passed: status === 'pass', status: status === 'pass' ? 'pass' : status }) : null}
  <span
    class="tl-ck"
    class:spin={!!icon?.spin}
    style={icon ? `color:${icon.color}` : undefined}
    title={status === 'pass' ? 'Checks passed' : status === 'fail' ? 'Checks failed' : status ? 'Checks pending' : undefined}
  >
    {#if icon}
      <Icon name={icon.name} class="ci" strokeWidth={2.4} />
    {/if}
  </span>
{/snippet}

{#snippet diffLayoutSeg()}
  <div class="seg commit-diff-layout">
    <button type="button" class:on={diffLayout === 'unified'} title="Unified" aria-label="Unified diff" aria-pressed={diffLayout === 'unified'} onclick={() => setDiffLayout('unified')}><Icon name="rows-2" /></button>
    <button
      type="button"
      class:on={diffLayout === 'split'}
      class:dim={splitFallback}
      title={splitFallback ? 'Too narrow for side-by-side — widen the pane or hide the file list' : 'Side by side'}
      aria-label="Side-by-side diff"
      aria-pressed={diffLayout === 'split'}
      aria-disabled={splitFallback}
      onclick={() => { if (!splitFallback) setDiffLayout('split') }}
    ><Icon name="columns-2" /></button>
  </div>
{/snippet}

{#snippet filesViewSeg()}
  <div class="review-sidebar-cluster">
    <button
      type="button"
      class="obs-pane-toggle"
      class:on={sidebarOpen}
      title={sidebarOpen ? 'Hide file list' : 'Show file list'}
      aria-label={sidebarOpen ? 'Hide file list' : 'Show file list'}
      aria-pressed={sidebarOpen}
      onclick={() => setSidebarOpen(!sidebarOpen)}
    ><Icon name="panel-left" /></button>
    {#if sidebarOpen}
      {@render fileListModeSeg()}
    {/if}
  </div>
{/snippet}

{#snippet fileListModeSeg()}
  <div class="seg commit-diff-layout" role="group" aria-label="File list mode">
    <button type="button" class:on={fileListMode === 'flat'} title="Flat list" aria-label="Flat list" aria-pressed={fileListMode === 'flat'} onclick={() => setFileListMode('flat')}><Icon name="list" /></button>
    <button type="button" class:on={fileListMode === 'tree'} title="Tree" aria-label="Tree" aria-pressed={fileListMode === 'tree'} onclick={() => setFileListMode('tree')}><Icon name="workflow" /></button>
  </div>
{/snippet}

{#snippet convSkel()}
  <div class="pr-skel" aria-hidden="true">
    <div class="cmt desc">
      <div class="skel pr-skel-ava"></div>
      <div class="cc">
        <div class="skel" style="width:7rem;height:11px;margin-bottom:12px"></div>
        <div class="skel" style="width:94%;height:10px;margin-bottom:7px"></div>
        <div class="skel" style="width:82%;height:10px;margin-bottom:7px"></div>
        <div class="skel" style="width:61%;height:10px"></div>
      </div>
    </div>
    <div class="thread pr-skel-thread">
      {#each [88, 64, 76] as w, i (i)}
        <div class="cmt">
          <div class="skel pr-skel-ava"></div>
          <div class="cc">
            <div class="skel" style="width:{i === 1 ? 5.5 : 6.75}rem;height:11px;margin-bottom:9px"></div>
            <div class="skel" style="width:{w}%;height:10px;margin-bottom:6px"></div>
            <div class="skel" style="width:{Math.max(38, w - 22)}%;height:10px"></div>
          </div>
        </div>
      {/each}
    </div>
  </div>
{/snippet}

{#snippet commitsSkel()}
  <div class="pr-skel pr-skel-commits" aria-hidden="true">
    {#each [72, 58, 81, 49] as w, i (i)}
      <div class="pr-skel-commit">
        <div class="skel" style="width:14px;height:14px;border-radius:4px"></div>
        <div class="skel" style="width:{w}%;height:12px"></div>
        <div class="skel" style="width:3.2rem;height:10px;margin-left:auto"></div>
      </div>
    {/each}
  </div>
{/snippet}

{#snippet filesSkel()}
  <div class="pr-skel pr-skel-files" aria-hidden="true">
    {#each [86, 64, 78, 51, 71] as w, i (i)}
      <div class="pr-skel-file">
        <div class="skel" style="width:14px;height:14px;border-radius:3px"></div>
        <div class="skel" style="width:{w}%;height:12px"></div>
        <div class="skel" style="width:2.6rem;height:10px;margin-left:auto"></div>
      </div>
    {/each}
  </div>
{/snippet}

{#snippet checksSkel()}
  <div class="pr-skel card" aria-hidden="true">
    <div class="card-h">
      <div class="skel" style="width:14px;height:14px;border-radius:99px"></div>
      <div class="skel" style="width:9rem;height:12px"></div>
    </div>
    {#each [54, 70, 46] as w, i (i)}
      <div class="pr-skel-check">
        <div class="skel" style="width:12px;height:12px;border-radius:99px"></div>
        <div class="skel" style="width:{w}%;height:11px"></div>
      </div>
    {/each}
  </div>
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<section class="view" class:on={$view === 'prdetail'} class:pr-with-agent={!!prAgentWsId} id="v-prdetail" onmousedown={onPRChromeMouseDown}>
  <div class="prh">
    {#if !pr}
      <div class="prh-t pr-skel" aria-hidden="true">
        <span class="skel" style="width:5.5rem;height:22px;border-radius:99px"></span>
        <span class="skel" style="width:3.2rem;height:16px"></span>
        <span class="skel" style="flex:1;max-width:22rem;height:18px"></span>
      </div>
      <div class="prh-side pr-skel" aria-hidden="true">
        <span class="skel" style="width:4.5rem;height:20px;border-radius:99px"></span>
        <span class="skel" style="width:9rem;height:20px;border-radius:6px"></span>
      </div>
    {:else}
    <div class="prh-t">
      {#if isUpdating}
        <span class="statepill rwk"><span class="d" style="background:var(--amber)"></span>Updating…</span>
      {:else}
        {@const chrome = prChromeColor(pr)}
        <span class="statepill" class:merged={prLife === 'merged'} class:closed={prLife === 'closed'} style="background:color-mix(in srgb, {chrome} 14%, transparent);color:{chrome}"><span class="d" style="background:{chrome}"></span>{prStatusLabel(pr)}</span>
      {/if}
      <span class="num">
        <span class="pr-src" title={isGitHubPR ? 'GitHub PR' : 'Local PR'}>
          {#if isGitHubPR}
            <Icon name="github" />
          {:else}
            <Icon name="hard-drive" />
          {/if}
        </span>
        #{pr.number}
      </span>
      {#if editingTitle}
        <input class="prh-title-input" bind:value={titleDraft} placeholder="Title" disabled={savingField === 'title'} onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void saveTitle() } }} />
        <button type="button" class="finding-note-btn" disabled={savingField === 'title'} onclick={() => (editingTitle = false)}>Cancel</button>
        <button type="button" class="finding-note-btn primary" disabled={!titleDraft.trim() || savingField === 'title'} onclick={() => void saveTitle()}>{savingField === 'title' ? 'Saving…' : 'Save'}</button>
      {:else}
        <span class="prh-title">{pr.title}</span>
        {#if canEditPR}
          <button type="button" class="cmt-act" title="Edit title" aria-label="Edit title" onclick={startEditTitle}><Icon name="pencil" /></button>
        {/if}
      {/if}
    </div>
    <div class="prh-side">
      <PRPeopleStack {pr} />
      {#if prLife !== 'merged' && prLife !== 'closed'}
        {#each attnLines as line (line)}
          <span class="pill2 pr-attn-pill">{line}</span>
        {/each}
      {/if}
      <span class="pill2 branch" title="{pr.branch} → {pr.base}"><Icon name="git-line" />{pr.branch} → {pr.base}</span>
    </div>
    {/if}
  </div>

  <div class="prd-split">
    {#if prAgentWsId}
      <div class="prd-agent" id="prd-agent" hidden={!$prAgentDockOpen} {@attach attachAgentHost}></div>
    {/if}
    <div class="prd-col" tabindex="-1" bind:this={prColEl}>
  <div class="prtabs">
    {#if prAgentWsId && !$prAgentDockOpen}
      <button
        type="button"
        class="obs-pane-toggle prd-chat-toggle"
        class:running={prAgentRunning}
        onclick={() => void showPRAgentDock()}
        title={prAgentRunning ? 'Show left panel — agent running (⌥⌘B)' : 'Show left panel (⌥⌘B)'}
        aria-label={prAgentRunning ? 'Show left panel. Agent running' : 'Show left panel'}
        aria-pressed={false}
      >
        <Icon name="panel-left" />
      </button>
    {/if}
    <button class="prtab" class:on={prtab === 'conv'} class:lit={prTabsLit} onclick={() => setTab('conv')}><Icon name="message" />Conversation{#if convPending}<span class="tc skel"></span>{:else}<span class="tc">{timeline.length + (description ? 1 : 0)}</span>{/if}</button>
    <button class="prtab" class:on={prtab === 'commits'} class:lit={prTabsLit} onclick={() => setTab('commits')}><Icon name="git-commit" />Commits{#if commitsPending}<span class="tc skel"></span>{:else}<span class="tc">{pr?.commits?.length ?? 0}</span>{/if}</button>
    <button class="prtab" class:on={prtab === 'files'} class:lit={prTabsLit} onclick={() => setTab('files')}><Icon name="file-diff" />Files changed<span class="tc">{pr?.files?.length || pr?.fileCount || 0}</span></button>
    <button class="prtab" class:on={prtab === 'checks'} class:lit={prTabsLit} onclick={() => setTab('checks')}><span class="prtab-check" class:spin={!checksPending && checksSummary.kind === 'pending'} data-kind={checksPending ? 'none' : checksSummary.kind} style:color={checksPending ? 'var(--faint)' : checksSummary.color}><Icon name={checksPending ? 'clock' : checksSummary.icon} /></span>Checks{#if checksPending}<span class="tc skel"></span>{:else}<span class="tc" style:color={checksSummary.kind === 'none' ? undefined : checksSummary.color}>{checksPassedCount}/{checksTotal}</span>{/if}</button>
  </div>

  <!-- Conversation -->
  <div class="prpane" class:on={prtab === 'conv'} aria-busy={convPending} {@attach fadeScrollTop}>
    <div class="conv-wrap">
      {#if isUpdating}
        <div class="rework">
          <div class="rw-ico"><Icon name="refresh-cw" class="spin" /></div>
          <div class="rw-tx"><div class="rw-t">Updating with latest {updateBaseName}</div><div class="rw-d">Integrating <code>{updateBaseName}</code> into this branch…</div></div>
        </div>
      {/if}

      {#if convPending}
        {@render convSkel()}
      {:else}

      {#if description}
        <div class="cmt desc">
          {@render personAvatar(description)}
          <div class="cc">
            <div class="ch">
              <span class="cn">{description.author}</span>
              {@render whenStamp(description, 'ct')}
              {#if !editingDesc && hasMoreActions(description, canEditPR)}
                <div class="cmt-more" class:open={moreMenuId === (description.id ?? 'gh-description')}>
                  <button type="button" class="cmt-act" title="More" aria-label="More" aria-haspopup="menu" aria-expanded={moreMenuId === (description.id ?? 'gh-description')} onclick={(e) => toggleMore(description.id ?? 'gh-description', e)}><Icon name="ellipsis" /></button>
                </div>
              {/if}
            </div>
            {#if editingDesc}
              <div class="finding-note-compose cmt-edit">
                <textarea class="finding-note-input" bind:value={descDraft} placeholder="Description" disabled={savingField === 'desc'} {@attach emojiShortcodes((v) => { descDraft = v })} {@attach userMentions((v) => { descDraft = v })} onkeydown={(e) => composeKey(e, saveDesc, () => { editingDesc = false })}></textarea>
                <div class="finding-note-compose-foot">
                  <span class="finding-note-hint"><Kbd text="⌘↵" /> to save</span>
                  <div class="finding-note-compose-actions">
                    <button type="button" class="finding-note-btn" disabled={savingField === 'desc'} onclick={() => (editingDesc = false)}>Cancel</button>
                    <button type="button" class="finding-note-btn primary" disabled={savingField === 'desc'} onclick={() => void saveDesc()}>{savingField === 'desc' ? 'Saving…' : 'Save'}</button>
                  </div>
                </div>
              </div>
            {:else if description?.bodyMd}
              <MarkdownHtml text={description.bodyMd} class="chat-canvas cm-md" />
            {:else}
              <div class="cm muted">No description provided.</div>
            {/if}
            {#if !editingDesc && description}
              <CommentReactions comment={description} canReact={canReactPRComment(pr, description)} />
            {/if}
          </div>
        </div>
      {/if}

      {#if !isGitHubPR && (!description || pr?.focus)}
        <div class="card">
          <div class="card-h"><Icon name="align-left" />Summary<span class="by">written by Maestro · review this first</span></div>
          <div class="card-b">
            {#if !description}
              {#each pr?.summary ?? [] as para (para.html ?? para.items?.join())}
                {#if para.kind === 'bullets'}
                  <ul>{#each para.items ?? [] as it}<li>{@html it}</li>{/each}</ul>
                {:else}
                  <p>{@html para.html}</p>
                {/if}
              {/each}
            {/if}
            {#if pr?.focus}
              <div class="focus">
                <Icon name="triangle-alert" />
                <div>
                  <div class="ft">{pr.focus.title}</div>
                  <div class="ff">{@html pr.focus.body}</div>
                  {#if (pr?.impact?.length ?? 0) > 0}
                    <div class="impact-chips">
                      {#each (pr?.impact ?? []).filter((it) => it.changed || it.kind === 'test').slice(0, 8) as it (it.kind + it.path + it.name)}
                        <button type="button" class="impact-chip" onclick={() => jumpToImpact(it.path, it.line)}>{it.kind}: {it.name}</button>
                      {/each}
                    </div>
                  {/if}
                  <span class="fl" role="button" tabindex="0" onclick={() => setTab('files')} onkeydown={(e) => e.key === 'Enter' && setTab('files')}>jump to the diff →</span>
                </div>
              </div>
            {/if}
          </div>
        </div>
      {/if}

        {#if listedFindings.length}
          <div class="card">
            <div class="card-h">
              <Icon name="search" />Reviewer findings
              <div class="card-h-end">
                <span class="by">{listedFindings.length} open</span>
                {#if canPostFindings && postableFindings.length}
                  <button
                    type="button"
                    class="finding-note-btn"
                    disabled={postableFindings.some((f) => postingFindingIds.has(f.id))}
                    onclick={() => void postFindings(postableFindings)}
                  >Post all</button>
                {/if}
              </div>
            </div>
            <div class="card-b">
              {#each listedFindings as f (f.id)}
                <div class="finding-row-actions">
                  <button class="finding-row" onclick={() => jumpToFinding(f)}>
                    <span class="sev {f.severity}">{f.severity}</span>
                    <span class="fm">{f.message}</span>
                    <span class="fl">{f.file}{f.line > 0 ? ((f.endLine ?? 0) > f.line ? `:${f.line}-${f.endLine}` : `:${f.line}`) : ''}</span>
                  </button>
                  {#if canPostFindings && postableFindingIds.has(f.id)}
                    <button
                      type="button"
                      class="finding-note-btn"
                      disabled={postingFindingIds.has(f.id)}
                      title="Queue as a GitHub review comment"
                      aria-label="Post as GitHub comment"
                      onclick={() => void postFindings([f])}
                    >Post</button>
                  {:else if $postedFindingIds.has(f.id)}
                    <button
                      type="button"
                      class="finding-note-btn"
                      disabled={settlingFindingIds.has(f.id)}
                      title="Mark the finding resolved"
                      aria-label="Resolve finding"
                      onclick={() => { if (pr?.id) void settlePostedFindings(pr.id, [f.id]) }}
                    >Resolve</button>
                  {/if}
                </div>
              {/each}
            </div>
          </div>
        {/if}

      {#if !isGitHubPR}
        <div class="card">
          <div class="card-h"><Icon name="refresh-ccw" />Verified in browser<span class="by">QA agent{pr?.when ? ` · ${pr.when}` : ''}</span></div>
          {#each pr?.verify ?? [] as v (v.html)}
            <div class="estep">
              <div class="ei" class:fail={!v.passed}>
                {#if v.passed}
                  <Icon name="check" strokeWidth={3} />
                {:else}
                  <Icon name="x" strokeWidth={3} />
                {/if}
              </div>
              <div class="ev2">{@html v.html}</div>
              {#if shotURL(v.screenshot)}
                <a class="shot has-shot" href={shotURL(v.screenshot)} target="_blank" title={v.screenshot}><img src={shotURL(v.screenshot)} alt="verification screenshot" loading="lazy" /></a>
              {:else}
                <div class="shot" title={v.screenshot ?? ''}></div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}

      <div class="thread" style="max-width:none;padding:6px 0 0">
        {#each timeline as c (c.id)}
          {#if c.kind === 'action' && c.commitSha && !isMergeAction(c)}
            {@const localIdx = localCommitIndex(c.commitSha)}
            <div class="tl-event tl-commit">
              <div class="tl-ico">
                {#if c.avatarUrl}
                  <img src={c.avatarUrl} alt="" />
                {:else}
                  <Icon name="git-commit" />
                {/if}
              </div>
              <div class="tl-tx">
                {#if localIdx >= 0}
                  <button type="button" class="tl-msg" title="View commit" onclick={() => openLocalCommit(localIdx)}>{commitTitle(c)}</button>
                {:else if c.url}
                  <a class="tl-msg" href={c.url} title="Open commit on GitHub">{commitTitle(c)}</a>
                {:else}
                  <div class="tl-msg">{commitTitle(c)}</div>
                {/if}
                {#if c.time}<span class="tl-when">{c.time}</span>{/if}
                <div class="tl-meta">
                  {#if c.verified}<span class="tl-verified">Verified</span>{/if}
                  {@render commitCheck(c.checkStatus)}
                  {#if localIdx >= 0}
                    <button type="button" class="tl-sha" title="View commit" onclick={() => openLocalCommit(localIdx)}>{shortSha(c.commitSha)}</button>
                  {:else if c.url}
                    <a class="tl-sha" href={c.url} title="Open commit on GitHub">{shortSha(c.commitSha)}</a>
                  {:else}
                    <span class="tl-sha">{shortSha(c.commitSha)}</span>
                  {/if}
                </div>
              </div>
            </div>
          {:else if c.kind === 'action'}
            {@const behalf = actionBehalf(c)}
            {@const mergeAct = isMergeAction(c)}
            {@const mergeLink = mergeCommitLink(c)}
            {@const decision = reviewDecision(c)}
            <div class="tl-event" class:merge={mergeAct} class:review-ok={decision === 'approved'} class:review-ask={decision === 'changes'}>
              <div class="tl-ico" class:merge={mergeAct} class:review-ok={decision === 'approved'} class:review-ask={decision === 'changes'}>
                {#if decision === 'approved'}
                  <Icon name="check" strokeWidth={3} />
                {:else if decision === 'changes'}
                  <Icon name="x" strokeWidth={3} />
                {:else if c.avatarUrl}
                  <img src={c.avatarUrl} alt="" />
                {:else}
                  <Icon name={actionIcon(c)} />
                {/if}
              </div>
              <div class="tl-tx">
                {#if behalf}
                  {@const href = commentPermalink(pr?.url, c)}
                  <b>{c.author}</b>
                  {#if href}
                    <a class="tl-verb" href={href} title="Open on GitHub">{behalf.verb}</a>
                  {:else}
                    {behalf.verb}
                  {/if}
                  on behalf of <b>{behalf.whom}</b>
                {:else if mergeLink}
                  <b>{c.author}</b>
                  <span>merged commit</span>
                  {#if mergeLink.href}
                    <a class="tl-sha" href={mergeLink.href} title="Open commit on GitHub">{mergeLink.sha}</a>
                  {:else}
                    <span class="tl-sha">{mergeLink.sha}</span>
                  {/if}
                  {#if mergeLink.rest}<span>{mergeLink.rest}</span>{/if}
                {:else if decision === 'approved'}
                  <b>{c.author}</b> approved these changes
                {:else if decision === 'changes'}
                  <b>{c.author}</b> requested changes
                {:else}
                  <b>{c.author}</b> {c.bodyMd}
                {/if}
                {@render whenStamp(c, 'tl-when')}
              </div>
            </div>
          {:else}
            {@const headerVerb = commentHeaderVerb(c)}
            {@const kids = reviewChildren(threadItems, c.id)}
            {@const decision = reviewDecision(c)}
            <div class="cmt" class:review-ok={decision === 'approved'} class:review-ask={decision === 'changes'}>
              <div class="cmt-face">
                {@render personAvatar(c)}
                {#if decision === 'approved'}
                  <span class="cmt-review-badge ok" title="Approved"><Icon name="check" strokeWidth={3} /></span>
                {:else if decision === 'changes'}
                  <span class="cmt-review-badge ask" title="Changes requested"><Icon name="x" strokeWidth={3} /></span>
                {/if}
              </div>
              <div class="cc">
                <div class="ch">
                  <span class="cn">{c.author}</span>
                  {#if headerVerb}{headerVerb}{/if}
                  {@render whenStamp(c, 'ct')}
                  {#if editingCommentId !== c.id && hasMoreActions(c, canEditComment(c))}
                    <div class="cmt-more" class:open={moreMenuId === c.id}>
                      <button type="button" class="cmt-act" title="More" aria-label="More" aria-haspopup="menu" aria-expanded={moreMenuId === c.id} onclick={(e) => toggleMore(c.id, e)}><Icon name="ellipsis" /></button>
                    </div>
                  {/if}
                </div>
                {@render commentBody(c)}
                {#if kids.length}
                  <div class="cmt-kids">
                    {#each kids as child (child.id)}
                      {@const replies = threadReplies(threadItems, child.id)}
                      {@const hunk = clipCommentHunk(parseDiffHunk(child.diffHunk ?? ''), { line: child.line, side: child.side, startLine: child.startLine, startSide: child.startSide })}
                      {@const resolvedOpen = !child.resolved || !!openResolved[child.id]}
                      {@const hideHunk = child.resolved ? !openResolved[child.id] : !!collapsedCtx[child.id]}
                      <div class="cmt-kid" class:resolved={!!child.resolved}>
                        {#if child.path || hunk.length}
                          <div class="cmt-ctx">
                            {#if child.path}
                              <div class="cmt-ctx-h">
                                <button type="button" class="cmt-ctx-toggle" class:collapsed={hideHunk} title={hideHunk ? 'Show context' : 'Hide context'} aria-label={hideHunk ? 'Show context' : 'Hide context'} aria-expanded={!hideHunk} onclick={() => toggleKidHunk(child)}><Icon name="chevron-down" class="cmt-ctx-chev" /></button>
                                <button type="button" class="cmt-ctx-path" title="View in Files changed" onclick={() => jumpToImpact(child.path ?? '', child.outdated ? 0 : child.line)}>{commentHunkPathLabel(child.path ?? '', child.line, child.startLine, child.outdated)}</button>
                                <button type="button" class="cmt-ctx-fill" tabindex="-1" aria-hidden="true" title={hideHunk ? 'Show context' : 'Hide context'} onclick={() => toggleKidHunk(child)}></button>
                              </div>
                            {/if}
                            {#if !hideHunk && hunk.length}
                              <div class="cmt-ctx-diff">
                                {#each hunk as hl, i (`${hl.type}-${hl.old}-${hl.new}-${i}`)}
                                  <div class="hline {hl.type}" class:target={!!hl.target} class:in-range={!!hl.inRange && !hl.target}>
                                    <span class="ln">{hl.old}</span>
                                    <span class="ln">{hl.new}</span>
                                    <span class="cd">{hl.text}</span>
                                  </div>
                                {/each}
                              </div>
                            {/if}
                          </div>
                        {:else if child.bodyHtml && resolvedOpen}
                          <div class="cm loc">{@html child.bodyHtml}</div>
                        {/if}
                        {#if resolvedOpen}
                          {@render lineNote(child)}
                          {#each replies as reply (reply.id)}
                            {@render lineNote(reply)}
                          {/each}
                          <div class="cmt-kid-foot">
                            <div class="cmt-reply">
                              {#if replyFor === child.id && !conversationLocked}
                                <div class="finding-note-compose cmt-edit">
                                  <textarea id={`reply-${child.id}`} class="finding-note-input" bind:value={replyDraft} placeholder="Reply…" disabled={savingField === `reply-${child.id}`} {@attach autosizeTextarea(replyDraft, PR_REPLY_AUTOSIZE)} {@attach emojiShortcodes((v) => { replyDraft = v })} {@attach userMentions((v) => { replyDraft = v })} onkeydown={(e) => composeKey(e, () => submitReply(child.id), () => { replyFor = '' })}></textarea>
                                  <div class="finding-note-compose-foot">
                                    <span class="finding-note-hint"><Kbd text="⌘↵" /> to save</span>
                                    <div class="finding-note-compose-actions">
                                      <button type="button" class="finding-note-btn" disabled={savingField === `reply-${child.id}`} onclick={() => (replyFor = '')}>Cancel</button>
                                      <button type="button" class="finding-note-btn primary" disabled={!replyDraft.trim() || savingField === `reply-${child.id}`} onclick={() => void submitReply(child.id)}>{savingField === `reply-${child.id}` ? 'Saving…' : 'Reply'}</button>
                                    </div>
                                  </div>
                                </div>
                              {:else if !conversationLocked}
                                <button type="button" class="cmt-reply-ph" onclick={() => openReply(child.id)}>Reply…</button>
                              {/if}
                            </div>
                            {@render resolveThreadBtn(child)}
                          </div>
                        {:else}
                          <div class="cmt-resolved-bar">
                            <span>Resolved</span>
                            {#if canResolveNote(child)}
                              <button type="button" class="finding-note-btn cmt-resolve" disabled={savingField === `res-${lineThreadKey(child)}`} onclick={() => void setThreadResolved(child, false)}>Unresolve</button>
                            {/if}
                          </div>
                        {/if}
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        {/each}
      </div>

      {#if conversationLocked}
        <div class="pr-settled" class:merged={prLife === 'merged'} class:closed={prLife === 'closed'}>
          <div class="pr-settled-ico"><Icon name={prIconName(pr)} /></div>
          <div class="pr-settled-copy">
            {#if prLife === 'merged'}
              <div class="pr-settled-h">Pull request successfully merged and closed</div>
              <div class="pr-settled-s">You're all set — the branch has been merged.</div>
            {:else}
              <div class="pr-settled-h">Pull request closed</div>
              <div class="pr-settled-s">This pull request was closed without merging.</div>
            {/if}
          </div>
        </div>
      {:else}
        {@render checksPanel(true)}

        <div class="composer finding-note-compose">
          <textarea
            class="finding-note-input"
            bind:this={composerEl}
            bind:value={$prComposerDraft}
            placeholder="Leave a comment…"
            {@attach autosizeTextarea($prComposerDraft, PR_COMPOSER_AUTOSIZE)}
            {@attach emojiShortcodes((v) => prComposerDraft.set(v))}
            {@attach userMentions((v) => prComposerDraft.set(v))}
            onkeydown={(e) => composeKey(e, submitComposer, () => {})}
          ></textarea>
          <div class="finding-note-compose-foot composer-foot">
            <button
              type="button"
              class="nt-go"
              disabled={!$prComposerDraft.trim() || !!savingField}
              onclick={() => void submitComposer()}
            >Comment<span class="nt-ret">⌘↵</span></button>
          </div>
        </div>
      {/if}
      {/if}
    </div>
  </div>

  <!-- Commits -->
  <div class="prpane" class:on={prtab === 'commits'} bind:this={commitsPaneEl} aria-busy={commitsPending} {@attach fadeScrollTop}>
    <div class="commits-wrap" class:detail={commitIdx !== null}>
      {#if commitIdx === null}
        {#if commitsPending}
          {@render commitsSkel()}
        {:else}
        <div>
          <div class="commits-list">
            {#each prCommits as c, i (c.hash)}
              <CommitRow commit={c} onOpen={() => void openCommit(i)}>
                {#snippet trailing()}
                  {#if c.verified}<span class="tl-verified">Verified</span>{/if}
                  {@render commitCheck(c.checkStatus)}
                {/snippet}
              </CommitRow>
            {/each}
          </div>
          <div class="hint" style="text-align:left;padding:14px 2px 0">{isGitHubPR ? 'Commits on this pull request. Click any to see its diff.' : 'Every commit is the loop\'s work — agent-authored, one per iteration. Click any to see its diff. The messages trace how it converged: build → fix on review → fix on review → tests.'}</div>
        </div>
        {/if}
      {:else}
        {@const c = prCommits[commitIdx]}
        {#if c}
          <div class="commit-nav">
            <button class="cback" onclick={() => { commitIdx = null; discardCommitDiff() }}>← all commits</button>
            {@render diffLayoutSeg()}
          </div>
          <div class="commit-hd">
            <div class="commit-hd-top">
              <div class="cav" style="background:{c.color}">
                {#if c.avatarUrl}<img src={c.avatarUrl} alt="" />{:else}{c.agentCode}{/if}
              </div>
              <div class="cmsg">
                <div class="cm-t">{c.message}</div>
                <div class="cm-by">
                  {#if c.authorRole && c.authorRole !== 'Author'}
                    {c.authorRole}{#if c.iterLabel} · <span class="iter">{c.iterLabel}</span>{/if}{#if !commitDiffLoading || detailFiles.length} · {detailFiles.length} file{detailFiles.length === 1 ? '' : 's'}{/if}
                  {:else}
                    {commitByline(c)}{#if !commitDiffLoading || detailFiles.length} · {detailFiles.length} file{detailFiles.length === 1 ? '' : 's'}{/if}
                  {/if}
                </div>
              </div>
              {#if c.verified}<span class="tl-verified">Verified</span>{/if}
              {@render commitCheck(c.checkStatus)}
              <span class="chash">{c.hash}</span>
              {@render shaCopy(c)}
            </div>
            {#if c.body}
              <pre class="cm-body">{c.body}</pre>
            {/if}
          </div>
          {#if commitDiffLoading}
            <div class="empty"><div class="et">Loading diff…</div></div>
          {:else if commitDiffError}
            <div class="empty"><div class="et">Could not load this commit's diff</div></div>
          {:else}
            {#each detailFiles as f (f.path)}
              {@const live = commitHydrated.has(f.path)}
              {@const reserved = reservedCardHeight(f, { live, collapsed: false, measured: commitMeasuredH.get(f.path) })}
              <div
                class="cfile"
                class:stub={!live}
                data-path={f.path}
                style:min-height={reserved != null ? `${reserved}px` : undefined}
                {@attach registerCommitEl(f.path)}
              >
                {#if live}
                  <DiffFile file={f} layout={diffLayout} collapsible onOpenFile={pr?.workspaceId ? openInWorkspace : undefined} openFileTitle="Open in workspace" />
                {:else}
                  <div class="diff-file">
                    <div
                      class="df-head collapsible collapsed"
                      role="button"
                      tabindex="0"
                      onclick={() => { commitSpyUntil = performance.now() + 700; commitPinGen++; commitSelected = f.path }}
                      onkeydown={(e) => e.key === 'Enter' && (commitSpyUntil = performance.now() + 700, commitPinGen++, commitSelected = f.path)}
                    >
                      <Icon name="chevron-down" class="df-chev" strokeWidth={2.4} />
                      <span class="fp">{fileDiffPathLabel(f)}</span>
                      <div class="fd"><span class="a">+{f.diff?.add ?? 0}</span><span class="r">−{f.diff?.del ?? 0}</span></div>
                    </div>
                  </div>
                {/if}
              </div>
            {:else}
              <div class="empty"><div class="et">No files in this commit</div></div>
            {/each}
          {/if}
        {/if}
      {/if}
    </div>
  </div>

  <!-- Files changed -->
  <div class="prpane flexpane" class:on={prtab === 'files'} aria-busy={filesPending} {@attach fadeScrollTop}>
    {#if filesPending}
      {@render filesSkel()}
    {:else}
    <div class="fl-head" class:no-sidebar={!sidebarOpen}>
      {@render filesViewSeg()}
      <div class="fl-head-main">
        {#if filterQ || findingsOnly || commentsOnly}
          {matchedFiles.length}/{reviewFiles.length} files
        {:else}
          {reviewFiles.length}/{pr?.files?.length ?? 0} files
        {/if}
        <span class="v"><span class="a">+{matchedAdd}</span><span class="r">−{matchedDel}</span></span>
        {@render diffLayoutSeg()}
        <button
          type="button"
          class="obs-icon-btn"
          title={anyExpanded ? 'Collapse all files' : 'Expand all files'}
          aria-label={anyExpanded ? 'Collapse all files' : 'Expand all files'}
          onclick={toggleFoldAll}
        ><Icon name={anyExpanded ? 'fold-vertical' : 'unfold-vertical'} /></button>
        {#if canPostFindings && postableFindings.length}
          <button
            type="button"
            class="finding-note-btn"
            disabled={postableFindings.some((f) => postingFindingIds.has(f.id))}
            onclick={() => void postFindings(postableFindings)}
          >Post all</button>
        {/if}
      </div>
    </div>
    <div class="diffwrap" class:no-sidebar={!sidebarOpen}>
      {#if sidebarOpen}
        <div class="filelist">
          <ResizeHandle panel="prFileList" edge="end" />
          <div class="fl-filter">
            <label class="fl-filter-q">
              <Icon name="search" size={12} />
              <input
                type="search"
                placeholder="Filter files"
                bind:value={filterText}
                onkeydown={(e) => { if (e.key === 'Escape') { e.preventDefault(); filterText = '' } }}
              />
            </label>
            {#if listedFindings.length}
              <button
                type="button"
                class="obs-icon-btn"
                class:on={findingsOnly}
                title={findingsOnly ? 'Show all files' : 'Show files with open findings'}
                aria-label={findingsOnly ? 'Show all files' : 'Show files with open findings'}
                aria-pressed={findingsOnly}
                onclick={() => (findingsOnly = !findingsOnly)}
              ><Icon name="hat-glasses" strokeWidth={2.2} /></button>
            {/if}
            {#if openCommentFiles}
              <button
                type="button"
                class="obs-icon-btn notes"
                class:on={commentsOnly}
                title={commentsOnly ? 'Show all files' : 'Show files with unresolved comments'}
                aria-label={commentsOnly ? 'Show all files' : 'Show files with unresolved comments'}
                aria-pressed={commentsOnly}
                onclick={() => (commentsOnly = !commentsOnly)}
              ><Icon name="message-square" strokeWidth={2.2} /></button>
            {/if}
          </div>
          <div class="overlay-sb-shell">
          <div class="fl-body" {@attach overlayScrollbar()} {@attach stickyFolderFades('.fl-tree.dir')}>
            <ReviewFileList
              files={matchedFiles}
              {selectedPath}
              mode={fileListMode}
              reveal={tocReveal}
              {findingsByPath}
              {commentsByPath}
              onSelect={(path, opts) => {
                const notes = commentsForPath(commentsByPath, path)
                const highlight = opts?.highlight ?? (commentsOnly ? (notes[0]?.line ?? 0) : 0)
                void selectPath(path, { highlight })
              }}
            />
          </div>
          </div>
        </div>
      {/if}
      <div class="diffscroll" tabindex="-1" bind:this={diffscrollEl} onkeydown={onDiffKeydown}>
        {#if (filterQ || findingsOnly || commentsOnly) && !matchedFiles.length}
          <div class="empty">
            <div class="et">No files match — <button type="button" class="linkish" onclick={() => { filterText = ''; findingsOnly = false; commentsOnly = false }}>clear filter</button></div>
          </div>
        {:else}
        {#each matchedFiles as f (f.path)}
          {@const live = hydrated.has(f.path)}
          {@const reserved = reservedCardHeight(f, { live, collapsed: collapsedFiles.has(f.path), measured: measuredH.get(f.path) })}
          <div
            class="cfile"
            class:stub={!live}
            data-path={f.path}
            style:min-height={reserved != null ? `${reserved}px` : undefined}
            {@attach registerFileEl(f.path)}
          >
            {#if live}
              <DiffFile
                file={f}
                highlightLine={f.path === selectedPath ? highlightLine : 0}
                layout={effectiveLayout}
                collapsible
                collapsed={collapsedFiles.has(f.path)}
                onToggleCollapse={(c) => toggleCollapsed(f.path, c)}
                onOpenFile={pr?.workspaceId ? openInWorkspace : undefined}
                openFileTitle="Open in workspace"
                findings={findingsForPath(findingsByPath, f.path)}
                onOpenFinding={pr?.workspaceId ? openFinding : undefined}
                onPostFinding={canPostFindings ? (finding) => void postFindings([finding]) : undefined}
                postingFindingIds={postingFindingIds}
                postableFindingIds={postableFindingIds}
              />
            {:else}
              <div class="diff-file">
                <div
                  class="df-head collapsible collapsed"
                  role="button"
                  tabindex="0"
                  onclick={() => { void selectPath(f.path) }}
                  onkeydown={(e) => e.key === 'Enter' && void selectPath(f.path)}
                >
                  <Icon name="chevron-down" class="df-chev" strokeWidth={2.4} />
                  <span class="fp">{fileDiffPathLabel(f)}</span>
                  <div class="fd"><span class="a">+{f.diff?.add ?? 0}</span><span class="r">−{f.diff?.del ?? 0}</span></div>
                </div>
              </div>
            {/if}
          </div>
        {/each}
        {/if}
      </div>
    </div>
    {/if}
  </div>

  <!-- Checks -->
  <div class="prpane" class:on={prtab === 'checks'} aria-busy={checksPending} {@attach fadeScrollTop}>
    <div class="conv-wrap" style="max-width:680px">
      {#if checksPending}
        {@render checksSkel()}
      {:else}
      {@render checksPanel()}
      <div class="hint" style="text-align:left;padding:0 2px">{pr?.source === 'gh' ? 'These are the GitHub status checks on this PR.' : 'Verify-in-browser is unique to Recur — the QA agent actually drove the running app. Evidence is in the Conversation tab.'}</div>
      {/if}
    </div>
  </div>

  {#if moreMenuId && moreMenuBox}
    {@const target = moreTarget()}
    {@const canEdit = target?.kind === 'description' ? canEditPR : !!target && canEditComment(target)}
    {@const canDelete = !!target && canDeleteComment(target)}
    {@const canQuote = !conversationLocked && !!target && !!commentSource(target)}
    {@const link = commentPermalink(pr?.url, target ?? { id: 'gh-description', kind: 'description' })}
    <div class="cmt-more-menu" style="top:{moreMenuBox.top}px;right:{moreMenuBox.right}px" role="menu" tabindex="-1" {@attach pinMore} {@attach portalToBody} onmousedown={(e) => e.stopPropagation()}>
      {#if link}
        <button type="button" class="cmt-more-item" role="menuitem" onclick={() => void moreOpenGitHub()}><Icon name="github" />Open in GitHub</button>
        <button type="button" class="cmt-more-item" role="menuitem" onclick={() => void moreCopyLink()}><Icon name="copy" />Copy link</button>
      {/if}
      {#if canQuote}
        <button type="button" class="cmt-more-item" role="menuitem" onclick={moreQuote}><Icon name="quote" />Quote reply</button>
      {/if}
      {#if canEdit}
        <button type="button" class="cmt-more-item" role="menuitem" onclick={moreEdit}><Icon name="pencil" />Edit</button>
      {/if}
      {#if canDelete}
        <button type="button" class="cmt-more-item danger" role="menuitem" onclick={moreDelete}><Icon name="trash" />Delete</button>
      {/if}
    </div>
  {/if}
    </div>
  </div>
</section>

{#snippet whenStamp(c: PRComment | undefined, cls: string)}
  {@const href = commentPermalink(pr?.url, c)}
  {#if href && c?.time}
    <a class={cls} href={href} title="Open on GitHub">{c.time}</a>
  {:else if c?.time}
    <span class={cls}>{c.time}</span>
  {/if}
{/snippet}

{#snippet resolveThreadBtn(c: PRComment)}
  {#if canResolveNote(c)}
    <button type="button" class="finding-note-btn cmt-resolve" disabled={savingField === `res-${lineThreadKey(c)}`} onclick={() => void setThreadResolved(c, !c.resolved)}>{c.resolved ? 'Unresolve' : 'Resolve conversation'}</button>
  {/if}
{/snippet}

{#snippet lineNote(c: PRComment)}
  <div class="cmt-kid-note">
    {@render personAvatar(c)}
    <div class="cmt-kid-cc">
      <div class="ch">
        <span class="cn">{c.author}</span>
        {@render whenStamp(c, 'ct')}
        {#if editingCommentId !== c.id && hasMoreActions(c, canEditComment(c))}
          <div class="cmt-more" class:open={moreMenuId === c.id}>
            <button type="button" class="cmt-act" title="More" aria-label="More" aria-haspopup="menu" aria-expanded={moreMenuId === c.id} onclick={(e) => toggleMore(c.id, e)}><Icon name="ellipsis" /></button>
          </div>
        {/if}
      </div>
      {@render commentBody(c, true)}
    </div>
  </div>
{/snippet}

{#snippet commentBody(c: PRComment, locShown = false)}
  {#if editingCommentId === c.id}
    <div class="finding-note-compose cmt-edit">
      <textarea class="finding-note-input" bind:value={commentDraft} placeholder="Edit comment" disabled={savingField === c.id} {@attach emojiShortcodes((v) => { commentDraft = v })} {@attach userMentions((v) => { commentDraft = v })} onkeydown={(e) => composeKey(e, () => saveComment(c.id), () => { editingCommentId = '' })}></textarea>
      <div class="finding-note-compose-foot">
        <span class="finding-note-hint"><Kbd text="⌘↵" /> to save</span>
        <div class="finding-note-compose-actions">
          <button type="button" class="finding-note-btn" disabled={savingField === c.id} onclick={() => (editingCommentId = '')}>Cancel</button>
          <button type="button" class="finding-note-btn primary" disabled={!commentDraft.trim() || savingField === c.id} onclick={() => void saveComment(c.id)}>{savingField === c.id ? 'Saving…' : 'Save'}</button>
        </div>
      </div>
    </div>
  {:else if c.bodyMd}
    {#if !locShown && c.bodyHtml}<div class="cm loc">{@html c.bodyHtml}</div>{/if}
    <MarkdownHtml text={c.bodyMd} class="chat-canvas cm-md" />
  {:else if c.bodyHtml && !locShown}
    <div class="cm">{@html c.bodyHtml}</div>
  {/if}
  {#if editingCommentId !== c.id}
    {#if c.reactHtml}<span class="react">{@html c.reactHtml}</span>{/if}
    <CommentReactions comment={c} canReact={canReactPRComment(pr, c)} />
  {/if}
{/snippet}

{#snippet personAvatar(c: PRComment | undefined, fallback = '')}
  <div class="ca" style={c?.avatarCss}>
    {c?.code || fallback}
    {#if c?.avatarUrl}<img src={c.avatarUrl} alt="" />{/if}
  </div>
{/snippet}

{#snippet checksPanel(scroll = false)}
  {@const g = checkGroups}
  <div class="card" class:checks-scroll={scroll}>
    <div class="card-h checks-h">
      <span class="checks-h-ico" class:spin={checksSummary.kind === 'pending'} data-kind={checksSummary.kind} style:color={checksSummary.color}><Icon name={checksSummary.icon} /></span>
      {#if checksSummaryHref}
        <a class="checks-h-link" href={checksSummaryHref} title="Open run on GitHub">{checksHeading(pr?.checks, pr?.source)}</a>
      {:else}
        {checksHeading(pr?.checks, pr?.source)}
      {/if}
      <span class="by">{checksBoxByline(g) || (pr?.source === 'gh' ? 'GitHub checks on this PR' : 'required before merge')}</span>
      {#if canFixChecks && pr?.id}
        <button type="button" class="finding-note-btn checks-fix" title="Ask an agent to fix failing checks" onclick={() => void requestFixChecks(pr.id)}>Fix checks</button>
      {/if}
    </div>
    <div class="checks-box-body">
      {#each [
        { key: 'fail', label: 'failed', items: g.fail },
        { key: 'pending', label: 'in progress', items: g.pending },
        { key: 'pass', label: 'successful', items: g.pass },
        { key: 'skip', label: 'skipped', items: g.skip },
      ] as grp (grp.key)}
        {#if grp.items.length}
          {@const collapsed = checkGroupCollapsed(grp.key, collapsedCheckGroups, scroll)}
          <button type="button" class="checks-group-h" class:collapsed={collapsed} onclick={() => toggleCheckGroup(grp.key, scroll)}>
            <Icon name="chevron-down" class="checks-group-chev" />
            {grp.items.length} {grp.label}
          </button>
          {#if !collapsed}
            {#each grp.items as c, i (c.name + '-' + i)}
              {@const icon = checkIcon(c)}
              {@const href = checkRunHref(c.url, { prUrl: pr?.url, prNumber: pr?.number, status: c.status })}
              <div class="checkrow">
                <span class="ci-wrap" class:spin={!!icon.spin} style="color:{icon.color}"><Icon name={icon.name} class="ci" strokeWidth={2.4} /></span>
                {#if href}
                  <a class="cn" href={href} title="Open on GitHub">{c.name}</a>
                {:else}
                  <span class="cn">{c.name}</span>
                {/if}
                <span class="cd">{liveCheckDetail(c, checkNow, pendingCheckClocks)}</span>
                {#if canFixChecks && grp.key === 'fail' && pr?.id}
                  <button type="button" class="finding-note-btn checks-fix" title="Ask an agent to fix {c.name}" onclick={() => void requestFixChecks(pr.id, c.name)}>Fix</button>
                {/if}
              </div>
            {/each}
          {/if}
        {/if}
      {/each}
    </div>
  </div>
{/snippet}
