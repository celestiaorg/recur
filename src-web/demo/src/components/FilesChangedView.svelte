<script lang="ts">
  import { tick } from 'svelte'
  import { service } from '../lib/api'
  import type { Commit, FileDiff } from '../lib/types'
  import { isExcludedBy } from '../lib/glob'
  import {
    readDiffLayout, writeDiffLayout,
    readFileListMode, writeFileListMode,
    readReviewSidebar, writeReviewSidebar,
    type DiffLayout, type FileListMode,
  } from '../lib/review-diff'
  import { sortFilesByChangedTree } from '../lib/changed-tree'
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
    active = false,
    commitScope = null,
    workingScope = false,
    count = undefined as number | undefined,
    excludeMatchers = undefined as readonly RegExp[] | undefined,
    onClearScope,
    onOpenFile,
    dirtyPaths = undefined as ReadonlySet<string> | undefined,
    onStepScope,
    canStepOlder = false,
    canStepNewer = false,
    stepOlderLabel = 'Previous commit',
    stepNewerLabel = 'Next commit',
  }: {
    projectId?: string
    workspaceId?: string
    baseLabel?: string
    active?: boolean
    commitScope?: Commit | null
    workingScope?: boolean
    count?: number
    excludeMatchers?: readonly RegExp[]
    onClearScope?: () => void
    onOpenFile?: (path: string, line?: number) => void
    dirtyPaths?: ReadonlySet<string>
    onStepScope?: (dir: -1 | 1) => void
    canStepOlder?: boolean
    canStepNewer?: boolean
    stepOlderLabel?: string
    stepNewerLabel?: string
  } = $props()

  let files = $state.raw<FileDiff[]>([])
  let loading = $state(false)
  let diffLayout = $state<DiffLayout>(readDiffLayout())
  let fileListMode = $state<FileListMode>(readFileListMode())
  let sidebarOpen = $state<boolean>(readReviewSidebar())
  let selectedPath = $state('')
  let collapsedFiles = $state<Set<string>>(new Set())
  let diffscrollEl = $state<HTMLDivElement | undefined>(undefined)
  const fileEls = new Map<string, HTMLElement>()
  let filterText = $state('')
  let tocReveal = $state(0)
  let paneWidth = $state(0)
  let spySuppressUntil = 0

  const vsLabel = $derived(baseLabel.trim() || 'HEAD')
  const visibleFiles = $derived.by(() => {
    // Commit and working-tree stacks are complete vs that baseline. Explorer
    // excludes apply only to the branch-vs-base Changes list.
    if (commitScope || workingScope || !excludeMatchers?.length) return files
    return files.filter((f) => !isExcludedBy(f.path, excludeMatchers))
  })
  const filterQ = $derived(filterText.trim().toLowerCase())
  const matchedFiles = $derived(sortFilesByChangedTree(
    filterQ ? visibleFiles.filter((f) => f.path.toLowerCase().includes(filterQ) || (f.oldPath ?? '').toLowerCase().includes(filterQ)) : visibleFiles,
  ))
  const add = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.add ?? 0), 0))
  const del = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.del ?? 0), 0))
  const headerCount = $derived.by(() => {
    const scoped = !!(commitScope || workingScope)
    const n = scoped ? visibleFiles.length : (count ?? visibleFiles.length)
    if (filterQ) return `${matchedFiles.length}/${n}`
    return String(scoped ? matchedFiles.length : n)
  })
  const anyExpanded = $derived(matchedFiles.some((f) => !collapsedFiles.has(f.path)))
  const splitFallback = $derived(diffLayout === 'split' && paneWidth > 0 && paneWidth < SPLIT_MIN_PX)
  const effectiveLayout = $derived<DiffLayout>(splitFallback ? 'unified' : diffLayout)

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
  }

  function toggleFoldAll() {
    collapsedFiles = anyExpanded ? new Set(matchedFiles.map((f) => f.path)) : new Set()
  }

  function registerFileEl(path: string) {
    return (el: HTMLElement) => {
      fileEls.set(path, el)
      return () => fileEls.delete(path)
    }
  }

  async function selectPath(path: string) {
    selectedPath = path
    if (collapsedFiles.has(path)) toggleCollapsed(path, false)
    spySuppressUntil = performance.now() + 700
    await tick()
    const el = fileEls.get(path)
    if (diffscrollEl && el) diffscrollEl.scrollTo({ top: el.offsetTop, behavior: 'instant' })
  }

  // Scroll spy: the file list follows the diff in view.
  function onDiffScroll() {
    const root = diffscrollEl
    if (!root || performance.now() < spySuppressUntil) return
    const hit = matchedFiles.find((f) => {
      const el = fileEls.get(f.path)
      return el && el.offsetTop + el.offsetHeight > root.scrollTop + 1
    })
    if (hit && hit.path !== selectedPath) {
      selectedPath = hit.path
      tocReveal++
    }
  }

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
    const paths = matchedFiles.map((f) => f.path)
    if (!paths.includes(selectedPath)) selectedPath = paths[0] ?? ''
  })

  let loadGen = 0
  async function load() {
    const gen = ++loadGen
    loading = true
    let got: FileDiff[] = []
    if (workingScope) {
      got = (await service.DiffWorking(projectId, workspaceId))?.files ?? []
    } else if (commitScope) {
      got = commitScope.files?.length
        ? commitScope.files
        : (await service.CommitDiff(projectId, workspaceId, commitSha(commitScope) || commitScope.hash)) ?? []
    } else {
      got = (await service.DiffVsBase(projectId, workspaceId))?.files ?? []
    }
    if (gen !== loadGen) return
    files = got
    loading = false
  }

  $effect(() => {
    if (!active) return
    void [projectId, workspaceId, commitScope, workingScope]
    void load()
  })
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
        </div>
        <div class="overlay-sb-shell">
        <div class="fl-body" {@attach overlayScrollbar()} {@attach stickyFolderFades('.fl-tree.dir')}>
          <ReviewFileList
            files={matchedFiles}
            {selectedPath}
            onSelect={selectPath}
            mode={fileListMode}
            {dirtyPaths}
            reveal={tocReveal}
          />
        </div>
        </div>
      </div>
    {/if}
    <div class="diffscroll" tabindex="-1" bind:this={diffscrollEl} onscroll={onDiffScroll}>
      {#if commitScope}
        {@render commitScopeHeader(commitScope)}
      {:else if workingScope}
        {@render workingScopeHeader()}
      {/if}
      {#if loading && !matchedFiles.length && !filterQ}
        <div class="empty"><div class="et">Loading diff…</div></div>
      {:else if filterQ && !matchedFiles.length}
        <div class="empty">
          <div class="et">No files match: <button type="button" class="linkish" onclick={() => (filterText = '')}>clear filter</button></div>
        </div>
      {:else}
        {#each matchedFiles as f (f.path)}
          <div class="cfile" data-path={f.path} {@attach registerFileEl(f.path)}>
            <DiffFile
              file={f}
              layout={effectiveLayout}
              comments={false}
              collapsible
              collapsed={collapsedFiles.has(f.path)}
              onToggleCollapse={(c) => toggleCollapsed(f.path, c)}
              {onOpenFile}
            />
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
      title={splitFallback ? 'Too narrow for side-by-side: widen the pane or hide the file list' : 'Side by side'}
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
