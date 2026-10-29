<script lang="ts">
  import { tick, untrack } from 'svelte'
  import { CodeService } from '../lib/api'
  import type { Commit, FileDiff, FileDiffResult, Finding } from '../../bindings/recur/internal/domain/models'
  import { isExcludedBy } from '../lib/glob'
  import {
    readDiffLayout, writeDiffLayout,
    readFileListMode, writeFileListMode,
    readReviewSidebar, writeReviewSidebar,
    reuseFileDiffs,
    fileDiffPathLabel,
    type DiffLayout, type FileListMode,
  } from '../lib/review-diff'
  import { applyViewportPin, captureViewportPin, fileAtScrollTop, stepPath, type ViewportPin } from '../lib/spy-pick'
  import { findingsForPath } from '../lib/finding-paths'
  import { sortFilesByChangedTree } from '../lib/changed-tree'
  import { inViewPaths, pickHydrated, recordStubHeight, reservedCardHeight, samePathList, samePathSet, syncStuckHeaders } from '../lib/review-window'
  import { commitByline, commitSha } from '../lib/commit-meta'
  import { stickyFolderFades } from '../lib/tree-sticky'
  import { overlayScrollbar } from '../lib/overlay-scrollbar'
  import DiffFile from './DiffFile.svelte'
  import ResizeHandle from './ResizeHandle.svelte'
  import ReviewFileList from './ReviewFileList.svelte'
  import Icon from './Icon.svelte'

  const SPLIT_MIN_PX = 720

  let {
    projectId = '',
    workspaceId = '',
    baseLabel = '',
    refresh = 0,
    active = false,
    commitScope = null,
    workingScope = false,
    count = undefined as number | undefined,
    excludeMatchers = undefined as readonly RegExp[] | undefined,
    onClearScope,
    onCommitFiles,
    onOpenFile,
    noteMode = 'none',
    onAddFinding,
    findingsByPath = undefined as ReadonlyMap<string, Finding[]> | undefined,
    onOpenFinding,
    onShowFindings,
    dirtyPaths = undefined as ReadonlySet<string> | undefined,
    onStepScope,
    canStepOlder = false,
    canStepNewer = false,
    stepOlderLabel = 'Previous commit',
    stepNewerLabel = 'Next commit',
    restore = null as { selectedPath: string; scrollTop: number } | null,
    diffTotals = $bindable({ add: 0, del: 0 }),
    fileCount = $bindable(0),
  }: {
    projectId?: string
    workspaceId?: string
    baseLabel?: string
    refresh?: number
    active?: boolean
    commitScope?: Commit | null
    workingScope?: boolean
    count?: number
    excludeMatchers?: readonly RegExp[]
    onClearScope?: () => void
    onCommitFiles?: (c: Commit, files: FileDiff[]) => void
    onOpenFile?: (path: string, line?: number) => void
    noteMode?: 'pr' | 'finding' | 'none'
    onAddFinding?: (path: string, line: number, body: string, endLine?: number) => Promise<boolean>
    findingsByPath?: ReadonlyMap<string, Finding[]>
    onOpenFinding?: (f: Finding) => void
    onShowFindings?: () => void
    dirtyPaths?: ReadonlySet<string>
    onStepScope?: (dir: -1 | 1) => void
    canStepOlder?: boolean
    canStepNewer?: boolean
    stepOlderLabel?: string
    stepNewerLabel?: string
    restore?: { selectedPath: string; scrollTop: number } | null
    diffTotals?: { add: number; del: number }
    fileCount?: number
  } = $props()

  let files = $state.raw<FileDiff[]>([])
  let filesTruncated = $state(false)
  let inView = $state.raw<string[]>([])
  let hydrated = $state.raw<Set<string>>(new Set())
  let loading = $state(false)
  let error = $state(false)
  let diffLayout = $state<DiffLayout>(readDiffLayout())
  let fileListMode = $state<FileListMode>(readFileListMode())
  let sidebarOpen = $state<boolean>(readReviewSidebar())
  let selectedPath = $state('')
  let collapsedFiles = $state<Set<string>>(new Set())
  let findingsOnly = $state(false)
  let diffscrollEl = $state<HTMLDivElement | undefined>(undefined)
  const fileEls = new Map<string, HTMLElement>()
  const measuredH = new Map<string, number>()
  let filterText = $state('')
  let tocReveal = $state(0)
  let paneWidth = $state(0)
  let spySuppressUntil = 0
  let spyObserver: IntersectionObserver | undefined
  let lastPin: ViewportPin | null = null
  let pinGen = 0
  let restoredOnce = false
  let restoreRoot = ''

  const vsLabel = $derived(baseLabel.trim() || 'HEAD')
  const visibleFiles = $derived.by(() => {
    // Commit and working-tree stacks are complete vs that baseline. Explorer
    // excludes apply only to the branch-vs-base Changes list.
    if (commitScope || workingScope || !excludeMatchers?.length) return files
    return files.filter((f) => !isExcludedBy(f.path, excludeMatchers))
  })
  const filterQ = $derived(filterText.trim().toLowerCase())
  const hasOpenFindings = $derived.by(() => {
    if (!findingsByPath) return false
    for (const fs of findingsByPath.values()) {
      if (fs.length) return true
    }
    return false
  })
  const matchedFiles = $derived.by(() => {
    let list = filterQ ? visibleFiles.filter((f) => f.path.toLowerCase().includes(filterQ) || (f.oldPath ?? '').toLowerCase().includes(filterQ)) : visibleFiles
    if (findingsOnly) list = list.filter((f) => findingsForPath(findingsByPath, f.path).length > 0)
    return sortFilesByChangedTree(list)
  })
  const add = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.add ?? 0), 0))
  const del = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.del ?? 0), 0))
  const headerCount = $derived.by(() => {
    const scoped = !!(commitScope || workingScope)
    const n = scoped ? visibleFiles.length : (count ?? visibleFiles.length)
    if (filterQ || findingsOnly) return `${matchedFiles.length}/${n}`
    return String(scoped ? matchedFiles.length : n)
  })
  const findingCount = $derived.by(() => {
    if (!findingsByPath) return 0
    let n = 0
    for (const f of visibleFiles) n += findingsForPath(findingsByPath, f.path).length
    return n
  })
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

  $effect(() => {
    diffTotals = { add, del }
    fileCount = matchedFiles.length
  })

  function setDiffLayout(next: DiffLayout) {
    diffLayout = next
    writeDiffLayout(next)
  }
  function setFileListMode(next: FileListMode) {
    fileListMode = next
    writeFileListMode(next)
  }
  function setSidebarOpen(open: boolean) {
    sidebarOpen = open
    writeReviewSidebar(open)
  }

  function toggleCollapsed(path: string, next: boolean) {
    const set = new Set(collapsedFiles)
    if (next) set.add(path)
    else set.delete(path)
    collapsedFiles = set
    void tick().then(refreshWindow)
  }

  function toggleFoldAll() {
    collapsedFiles = anyExpanded ? new Set(matchedFiles.map((f) => f.path)) : new Set()
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

  function selectPath(path: string) {
    selectedPath = path
    if (collapsedFiles.has(path)) {
      const set = new Set(collapsedFiles)
      set.delete(path)
      collapsedFiles = set
    }
    spySuppressUntil = performance.now() + 700
    pinGen++
    void tick().then(() => {
      refreshWindow()
      const root = diffscrollEl
      const el = fileEls.get(path)
      if (!root || !el) return
      root.scrollTo({ top: el.offsetTop, behavior: 'instant' })
      void tick().then(() => {
        const el2 = fileEls.get(path)
        if (el2) root.scrollTo({ top: el2.offsetTop, behavior: 'instant' })
        lastPin = captureViewportPin(fileRanges(), root.scrollTop, root.clientHeight, root.scrollHeight)
      })
    })
  }

  function spyPick(path: string) {
    if (!path || path === selectedPath) return
    selectedPath = path
    tocReveal++
  }

  function spyFromOffsets() {
    const root = diffscrollEl
    if (!root || performance.now() < spySuppressUntil) return
    const ranges = fileRanges()
    spyPick(fileAtScrollTop(ranges, root.scrollTop))
  }

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
      if (next) selectPath(next)
    } else if ((e.key === 'k' && !e.metaKey && !e.ctrlKey && !e.altKey) || (e.key === 'ArrowUp' && e.altKey)) {
      e.preventDefault()
      const prev = stepPath(paths, selectedPath, -1)
      if (prev) selectPath(prev)
    }
  }

  $effect(() => {
    const root = diffscrollEl
    if (!root || !active) return
    // Layout-only trigger (hydrate/resize). Do not spy from the callback
    // entries — they are a delta, so the next file looks selected while the
    // current file still owns scrollTop.
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
    untrack(refreshWindow)
    return () => {
      cancelAnimationFrame(raf)
      root.removeEventListener('scroll', onScroll)
      obs.disconnect()
      if (spyObserver === obs) spyObserver = undefined
    }
  })

  $effect(() => {
    const root = diffscrollEl
    if (!root || !active) return
    const ro = new ResizeObserver((entries) => {
      paneWidth = entries[0]?.contentRect.width ?? 0
    })
    ro.observe(root)
    return () => ro.disconnect()
  })

  $effect(() => {
    if (!active) return
    void tick().then(() => {
      const el = document.activeElement
      if (el instanceof HTMLElement && (el.closest('.icmt-compose') || el.tagName === 'TEXTAREA' || el.tagName === 'INPUT')) return
      diffscrollEl?.focus({ preventScroll: true })
    })
  })

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
    if (!active) return
    void tick().then(refreshWindow)
  })

  function applyRestore(got: FileDiff[]) {
    const root = `${projectId}\0${workspaceId}`
    if (restoreRoot !== root) {
      restoreRoot = root
      restoredOnce = false
    }
    if (restoredOnce || !restore?.selectedPath) return
    if (!got.some((f) => f.path === restore.selectedPath)) return
    restoredOnce = true
    selectedPath = restore.selectedPath
    const top = restore.scrollTop
    void tick().then(() => {
      if (diffscrollEl) diffscrollEl.scrollTop = top
    })
  }

  let loadGen = 0
  let loadRunning = false
  let loadQueued = false
  let filesScope = ''
  let loadScope = ''
  function reviewFilesScope() {
    const root = `${projectId}\0${workspaceId}`
    if (workingScope) return `${root}\0working`
    const sha = commitScope ? (commitSha(commitScope) || commitScope.hash || '') : ''
    if (sha) return `${root}\0commit:${sha}`
    return `${root}\0base`
  }
  function adoptFilesScope(scope: string) {
    loadScope = scope
    if (filesScope !== scope) {
      files = []
      filesTruncated = false
      loading = true
      error = false
    } else if (!files.length) {
      loading = true
    }
  }
  function dropWorkingSnapshot() {
    if (!filesScope.endsWith('\0working') && !loadScope.endsWith('\0working')) return
    files = []
    filesTruncated = false
    filesScope = ''
    loadScope = ''
    error = false
    loading = false
    loadQueued = false
    loadGen += 1
  }
  $effect(() => {
    if (workingScope) return
    untrack(() => dropWorkingSnapshot())
  })
  async function load() {
    if (!active) return
    const scope = reviewFilesScope()
    if (loadRunning) {
      loadQueued = true
      if (loadScope !== scope) {
        adoptFilesScope(scope)
        loadGen += 1
      }
      return
    }
    loadRunning = true
    loadQueued = false
    const gen = ++loadGen
    adoptFilesScope(scope)
    error = false
    const t0 = performance.now()
    try {
      let got: FileDiff[] = []
      let truncated = false
      if (workingScope) {
        const res: FileDiffResult | null = await CodeService.DiffWorking(projectId, workspaceId)
        got = res?.files ?? []
        truncated = !!res?.truncated
      } else if (commitScope) {
        if (commitScope.files?.length) {
          got = commitScope.files
        } else {
          const sha = commitSha(commitScope) || commitScope.hash
          got = (await CodeService.CommitDiff(projectId, workspaceId, sha)) ?? []
          if (gen === loadGen) onCommitFiles?.(commitScope, got)
        }
      } else {
        const res: FileDiffResult | null = await CodeService.DiffVsBase(projectId, workspaceId)
        got = res?.files ?? []
        truncated = !!res?.truncated
      }
      if (gen !== loadGen || !active) return
      const ms = performance.now() - t0
      if (ms >= 250) {
        const lines = got.reduce((n, f) => n + (f.lines?.length ?? 0), 0)
        const kind = workingScope ? 'DiffWorking' : commitScope ? 'CommitDiff' : 'DiffVsBase'
        console.warn(`[review] ${kind} ${got.length} files / ${lines} lines in ${Math.round(ms)}ms`)
      }
      const next = reuseFileDiffs(files, got)
      filesScope = scope
      filesTruncated = truncated
      if (next === files) return
      files = next
      applyRestore(got)
      void tick().then(refreshWindow)
    } catch {
      if (gen !== loadGen || !active) return
      error = true
      if (filesScope !== scope) {
        files = []
        filesTruncated = false
      }
    } finally {
      if (gen === loadGen) loading = false
      loadRunning = false
      if (loadQueued && active) void load()
    }
  }

  $effect(() => {
    if (!active) return
    void projectId
    void workspaceId
    const scopeKey = commitScope?.hash || commitScope?.sha || ''
    void workingScope
    if (scopeKey) void scopeKey
    else void refresh
    untrack(() => { void load() })
  })

  export function getViewState(): { selectedPath: string; scrollTop: number } {
    return { selectedPath, scrollTop: diffscrollEl?.scrollTop ?? 0 }
  }
</script>

<div class="obs-review prpane on flexpane" data-review="changes">
  <div class="fl-head review-toolbar" class:no-sidebar={!sidebarOpen}>
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
    <div class="fl-head-main">
      {headerCount} file{headerCount === '1' ? '' : 's'}
      {#if workingScope}
        <span class="obs-review-vs" title="Compared to HEAD">vs HEAD</span>
      {:else if !commitScope}
        <span class="obs-review-vs" title="Compared to {vsLabel}">vs {vsLabel}</span>
      {/if}
      {#if findingCount}
        <button type="button" class="linkish st-findings" onclick={() => onShowFindings?.()}>{findingCount} finding{findingCount === 1 ? '' : 's'}</button>
      {/if}
      <span class="v"><span class="a">+{add}</span><span class="r">−{del}</span></span>
      {@render diffLayoutSeg()}
      <button
        type="button"
        class="obs-icon-btn"
        title={anyExpanded ? 'Collapse all files' : 'Expand all files'}
        aria-label={anyExpanded ? 'Collapse all files' : 'Expand all files'}
        onclick={toggleFoldAll}
      ><Icon name={anyExpanded ? 'fold-vertical' : 'unfold-vertical'} /></button>
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
          {#if hasOpenFindings}
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
        </div>
        <div class="overlay-sb-shell">
        <div class="fl-body" {@attach overlayScrollbar()} {@attach stickyFolderFades('.fl-tree.dir')}>
          <ReviewFileList
            files={matchedFiles}
            {selectedPath}
            onSelect={selectPath}
            mode={fileListMode}
            {findingsByPath}
            {dirtyPaths}
            reveal={tocReveal}
          />
        </div>
        </div>
      </div>
    {/if}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="diffscroll" tabindex="-1" bind:this={diffscrollEl} onkeydown={onDiffKeydown}>
      {#if commitScope}
        {@render commitScopeHeader(commitScope)}
      {:else if workingScope}
        {@render workingScopeHeader()}
      {/if}
      {#if filesTruncated}
        <div class="empty" role="status"><div class="et">Untracked file list truncated — more files exist vs HEAD</div></div>
      {/if}
      {#if loading && !matchedFiles.length && !filterQ}
        <div class="empty"><div class="et">Loading diff…</div></div>
      {:else if error && !matchedFiles.length && !filterQ}
        <div class="empty"><div class="et">{workingScope ? 'Could not load uncommitted diff' : commitScope ? "Could not load this commit's diff" : 'Could not load files changed'}</div></div>
      {:else if (filterQ || findingsOnly) && !matchedFiles.length}
        <div class="empty">
          <div class="et">No files match — <button type="button" class="linkish" onclick={() => { filterText = ''; findingsOnly = false }}>clear filter</button></div>
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
                layout={effectiveLayout}
                comments={noteMode !== 'none'}
                {noteMode}
                collapsible
                collapsed={collapsedFiles.has(f.path)}
                onToggleCollapse={(c) => toggleCollapsed(f.path, c)}
                {onOpenFile}
                {onAddFinding}
                findings={findingsForPath(findingsByPath, f.path)}
                {onOpenFinding}
              />
            {:else}
              <div class="diff-file">
                <div
                  class="df-head collapsible collapsed"
                  role="button"
                  tabindex="0"
                  onclick={() => selectPath(f.path)}
                  onkeydown={(e) => e.key === 'Enter' && selectPath(f.path)}
                >
                  <Icon name="chevron-down" class="df-chev" strokeWidth={2.4} />
                  <span class="fp">{fileDiffPathLabel(f)}</span>
                  <div class="fd"><span class="a">+{f.diff?.add ?? 0}</span><span class="r">−{f.diff?.del ?? 0}</span></div>
                </div>
              </div>
            {/if}
          </div>
        {:else}
          <div class="empty"><div class="et">{workingScope ? 'No uncommitted changes' : commitScope ? 'No files in this commit' : `No files changed since ${vsLabel}`}</div></div>
        {/each}
      {/if}
    </div>
  </div>
</div>

{#snippet scopeNav()}
  <div class="commit-nav">
    <button type="button" class="cback" onclick={() => onClearScope?.()}>← all commits</button>
    {#if onStepScope}
      <div class="commit-scope-step">
        <button
          type="button"
          class="obs-icon-btn"
          disabled={!canStepOlder}
          title={stepOlderLabel}
          aria-label={stepOlderLabel}
          onclick={() => onStepScope?.(-1)}
        ><Icon name="chevron-left" /></button>
        <button
          type="button"
          class="obs-icon-btn"
          disabled={!canStepNewer}
          title={stepNewerLabel}
          aria-label={stepNewerLabel}
          onclick={() => onStepScope?.(1)}
        ><Icon name="chevron-right" /></button>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet workingScopeHeader()}
  {@render scopeNav()}
  <div class="commit-hd working-hd">
    <div class="commit-hd-top">
      <div class="cav wip" aria-hidden="true">WIP</div>
      <div class="cmsg">
        <div class="cm-t">Uncommitted changes</div>
        <div class="cm-by">Working tree vs HEAD{#if !loading || visibleFiles.length} · {visibleFiles.length} file{visibleFiles.length === 1 ? '' : 's'}{/if}</div>
      </div>
    </div>
  </div>
{/snippet}

{#snippet commitScopeHeader(c: Commit)}
  {@render scopeNav()}
  <!-- A synthetic scope (chat turn checkpoint diff) has no message: nothing to summarize. -->
  {#if c.message}
  <div class="commit-hd">
    <div class="commit-hd-top">
      <div class="cav" style="background:{c.color}">
        {#if c.avatarUrl}<img src={c.avatarUrl} alt="" />{:else}{c.agentCode}{/if}
      </div>
      <div class="cmsg">
        <div class="cm-t">{c.message}</div>
        <div class="cm-by">
          {#if c.authorRole && c.authorRole !== 'Author'}
            {c.authorRole}{#if c.iterLabel} · <span class="iter">{c.iterLabel}</span>{/if}{#if !loading || visibleFiles.length} · {visibleFiles.length} file{visibleFiles.length === 1 ? '' : 's'}{/if}
          {:else}
            {commitByline(c)}{#if !loading || visibleFiles.length} · {visibleFiles.length} file{visibleFiles.length === 1 ? '' : 's'}{/if}
          {/if}
        </div>
      </div>
      <span class="chash">{c.hash}</span>
    </div>
    {#if c.body}
      <pre class="cm-body">{c.body}</pre>
    {/if}
  </div>
  {/if}
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

{#snippet fileListModeSeg()}
  <div class="seg commit-diff-layout" role="group" aria-label="File list mode">
    <button type="button" class:on={fileListMode === 'flat'} title="Flat list" aria-label="Flat list" aria-pressed={fileListMode === 'flat'} onclick={() => setFileListMode('flat')}><Icon name="list" /></button>
    <button type="button" class:on={fileListMode === 'tree'} title="Tree" aria-label="Tree" aria-pressed={fileListMode === 'tree'} onclick={() => setFileListMode('tree')}><Icon name="workflow" /></button>
  </div>
{/snippet}
