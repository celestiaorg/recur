<script lang="ts">
  // Code observatory: agent dock, file tabs, CodeMirror editor, branch review
  // and the file tree.
  import { tick, type Snippet } from 'svelte'
  import { call, service } from '../lib/api'
  import {
    observatoryJump, workspaces, projects, CODE_PANEL_LABELS,
    explorerPrefs, theme, attention, type CodePanelKey,
  } from '../lib/stores'
  import { fileIconClass } from '../lib/file-icons'
  import { isExcludedBy, compileExcludes } from '../lib/glob'
  import { buildTreeFromPaths } from '../lib/changed-tree'
  import { stickyFolderFades } from '../lib/tree-sticky'
  import { overlayScrollbar } from '../lib/overlay-scrollbar'
  import { scmGutterBase } from '../lib/editor-view'
  import { findingFlagForPath, findingTitle, severityRank, type FindingFlag } from '../lib/findings-ui'
  import type { EditorFindingMark } from '../lib/cm-findings-gutter'
  import type { ReviewTab } from '../lib/review-diff'
  import CodeEditor from './CodeEditor.svelte'
  import MergeEditor from './MergeEditor.svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import ResizeHandle from './ResizeHandle.svelte'
  import TabStrip from './TabStrip.svelte'
  import TabChip from './TabChip.svelte'
  import FindingsPanel, { type FindingsStatusFilter } from './FindingsPanel.svelte'
  import ReviewSurface from './ReviewSurface.svelte'
  import Icon, { type IconName } from './Icon.svelte'
  import type { TreeEntry, FileContent, Finding, Commit } from '../lib/types'

  let {
    projectId = '',
    workspaceId = '',
    loopScoped = false,
    leftPanel,
    leftHead,
    leftTitle = '',
  }: {
    projectId?: string
    workspaceId?: string
    loopScoped?: boolean
    leftPanel?: Snippet
    leftHead?: Snippet<[lit: boolean]>
    leftTitle?: string
  } = $props()

  const PANEL_ICONS: Partial<Record<CodePanelKey, IconName>> = {
    maestro: 'message',
    findings: 'hat-glasses',
  }
  const leftPanels: CodePanelKey[] = ['maestro', 'findings']
  const panelLabel = (key: CodePanelKey) => (key === 'maestro' && leftTitle ? leftTitle : CODE_PANEL_LABELS[key])

  let leftKind = $state<CodePanelKey>('maestro')
  let showSide = $state(false)
  let focusZone = $state<'editor' | 'agent' | 'other'>('editor')
  let leftAsideEl = $state<HTMLElement | undefined>(undefined)

  let gitStatus = $state<Record<string, string>>({})
  let workingStatus = $state<Record<string, string>>({})
  let findings = $state<Finding[]>([])
  let findingsFilter = $state<FindingsStatusFilter>('open')

  let openFiles = $state<string[]>([])
  let drafts = $state<Record<string, string>>({})
  let selectedPath = $state('')
  let treePath = $state('')
  let file = $state<FileContent | null>(null)
  let editorPath = $state('')
  let draft = $state('')
  let savedContent = $state('')
  let viewMode = $state<'code' | 'diff' | 'preview'>('code')
  let viewModes: Record<string, 'code' | 'diff' | 'preview'> = {}
  let baseDocs = $state<Record<string, string>>({})
  let fileLoading = $state(false)
  let findPanelOpen = $state(false)

  let editor = $state<CodeEditor | undefined>(undefined)
  let mergeEditor = $state<MergeEditor | undefined>(undefined)

  let mainSurface = $state<'editor' | 'review'>('editor')
  let reviewTab = $state<ReviewTab>('changes')
  let reviewCommitCount = $state(0)
  let reviewSurface = $state<{ clearCommitScope: () => void; openCommitScope: (c: Commit) => void } | undefined>()

  let tree = $state<TreeEntry[]>([])
  let expanded = $state<Set<string>>(new Set())
  let loadedDirs = $state<Set<string>>(new Set(['']))
  let changedCollapsed = $state<Set<string>>(new Set())
  let browseAll = $state(!loopScoped)

  const ws = $derived($workspaces.find((w) => w.id === workspaceId))
  const isGit = $derived(!!$projects.find((p) => p.id === projectId)?.isGit)
  const reviewBase = $derived(
    !ws || ws.isMain ? '' : (ws.baseBranch || $projects.find((p) => p.id === projectId)?.baseBranch || '').trim(),
  )
  const excludeMatchers = $derived(compileExcludes($explorerPrefs.exclude))
  const reviewPaths = $derived(Object.keys(gitStatus).sort().filter((p) => !isExcludedBy(p, excludeMatchers)))
  const reviewCount = $derived(reviewPaths.length)
  const workingDirtyCount = $derived(Object.keys(workingStatus).length)
  const filterChanged = $derived(!browseAll && reviewPaths.length > 0)
  const visibleTree = $derived(
    filterChanged ? buildTreeFromPaths(reviewPaths) : tree.filter((e) => !isExcludedBy(e.path, excludeMatchers)),
  )
  const selectedGitStatus = $derived(gitStatus[selectedPath] || '')
  const canShowDiff = $derived(!!selectedGitStatus)
  const previewKind = $derived(!!file && /\.(md|mdx|markdown)$/i.test(file.path))
  const showPreview = $derived(!!previewKind && viewMode === 'preview')
  const showDiff = $derived(viewMode === 'diff' && canShowDiff && selectedPath in baseDocs && editorPath === selectedPath)
  const pathCrumbs = $derived(selectedPath ? selectedPath.split('/').filter(Boolean) : [])
  const openFindingsCount = $derived(findings.filter((f) => (f.status || 'open') === 'open').length)
  const unseenChatCount = $derived(workspaceId ? ($attention.byWorkspace[workspaceId] ?? 0) : 0)

  const findingsFilterCounts = $derived({
    open: openFindingsCount,
    ignored: findings.filter((f) => f.status === 'ignored').length,
    invalid: findings.filter((f) => f.status === 'invalid').length,
    resolved: findings.filter((f) => f.status === 'resolved').length,
    all: findings.length,
  })
  const FINDINGS_FILTERS: { key: FindingsStatusFilter; label: string }[] = [
    { key: 'open', label: 'Open' },
    { key: 'ignored', label: 'Ignored' },
    { key: 'invalid', label: 'Invalid' },
    { key: 'resolved', label: 'Resolved' },
    { key: 'all', label: 'All' },
  ]

  function findingRelPath(p: string): string {
    const wt = ws?.worktreePath?.replace(/\/$/, '') ?? ''
    p = p.replace(/^\.\//, '')
    return wt && p.startsWith(wt + '/') ? p.slice(wt.length + 1) : p
  }
  const openFindingsByPath = $derived.by(() => {
    const map = new Map<string, FindingFlag>()
    for (const f of findings) {
      if ((f.status || 'open') !== 'open' || !f.file) continue
      const p = findingRelPath(f.file)
      const cur = map.get(p)
      const sev = String(f.severity || 'info')
      if (!cur) map.set(p, { count: 1, severity: sev })
      else {
        cur.count++
        if (severityRank(sev) > severityRank(cur.severity)) cur.severity = sev
      }
    }
    return map
  })
  const findingMarks = $derived.by((): EditorFindingMark[] =>
    findings
      .filter((f) => (f.status || 'open') === 'open' && f.id && f.file && f.line && findingRelPath(f.file) === selectedPath)
      .map((f) => ({
        id: f.id,
        line: f.line!,
        endLine: f.endLine && f.endLine > f.line! ? f.endLine : undefined,
        severity: String(f.severity || 'info'),
        title: findingTitle(f.message || ''),
      })),
  )

  function paneBadgeCount(key: CodePanelKey): number {
    return key === 'maestro' ? unseenChatCount : key === 'findings' ? openFindingsCount : 0
  }

  function selectCodePanel(next: CodePanelKey) {
    if (next === 'files') showSide = true
    else leftKind = next
  }

  async function load() {
    const [st, wst, f, sess] = await Promise.all([
      call(service.GitStatus(projectId, workspaceId), 'Git status failed'),
      call(service.WorkingStatus(projectId, workspaceId), 'Working status failed'),
      call(service.ListFindings(workspaceId), 'Findings failed'),
      service.GetEditorSession(workspaceId).catch(() => null),
    ])
    const compact = (m: Record<string, string | undefined> | null) =>
      Object.fromEntries(Object.entries(m ?? {}).filter(([, v]) => v)) as Record<string, string>
    gitStatus = compact(st)
    workingStatus = compact(wst)
    findings = f ?? []
    const t = await call(service.ListTree(projectId, workspaceId, ''), 'Tree failed')
    tree = t ?? []
    if (!selectedPath && sess?.selectedPath) {
      openFiles = sess.openFiles ?? []
      void openPath(sess.selectedPath)
    }
  }
  $effect(() => {
    if (workspaceId) void load()
  })

  async function loadBase(path: string) {
    if (!isGit || path in baseDocs) return
    const b = await service.FileAtBase(projectId, workspaceId, path).catch(() => null)
    if (b != null) baseDocs = { ...baseDocs, [path]: b }
  }

  function gutterBase(path: string): string | null {
    return path in baseDocs ? scmGutterBase(baseDocs[path], gitStatus[path] || '') : null
  }

  async function openPath(path: string, line = 0, preferDiff = false) {
    if (!path) return
    mainSurface = 'editor'
    treePath = path
    if (!openFiles.includes(path)) {
      const i = openFiles.indexOf(selectedPath)
      openFiles = i >= 0 ? [...openFiles.slice(0, i + 1), path, ...openFiles.slice(i + 1)] : [...openFiles, path]
    }
    if (selectedPath) viewModes[selectedPath] = viewMode
    viewMode = preferDiff && gitStatus[path] ? 'diff' : (viewModes[path] ?? 'code')
    if (path !== selectedPath || !file) {
      selectedPath = path
      fileLoading = true
      const f = await call(service.ReadFile(projectId, workspaceId, path), 'Read failed')
      if (selectedPath !== path) return
      fileLoading = false
      file = f
      savedContent = f?.content ?? ''
      draft = drafts[path] ?? savedContent
      editorPath = f ? path : ''
      void loadBase(path)
    }
    await tick()
    if (line > 0) {
      if (showDiff) mergeEditor?.scrollToLine(line)
      else editor?.scrollToLine(line)
    }
  }

  function onDocChange(v: string) {
    draft = v
    const { [selectedPath]: _, ...rest } = drafts
    drafts = v === savedContent ? rest : { ...rest, [selectedPath]: v }
  }

  async function setViewMode(mode: 'code' | 'diff' | 'preview') {
    if (mode === 'diff') await loadBase(selectedPath)
    viewMode = mode
    if (mode === 'code') {
      await tick()
      editor?.focus()
      editor?.resync()
    }
  }

  function setReviewTab(next: ReviewTab) {
    if (next === 'changes') reviewSurface?.clearCommitScope()
    reviewTab = next
  }

  function showReview() {
    if (isGit) mainSurface = 'review'
  }

  /** Show a chat turn's checkpoint diff in the review surface as a commit scope. */
  export function openTurnReview(c: Commit) {
    showReview()
    setReviewTab('commits')
    reviewSurface?.openCommitScope(c)
  }

  function searchable() {
    return showDiff ? mergeEditor : showPreview ? undefined : editor
  }
  function syncFindPanelOpen() {
    findPanelOpen = searchable()?.isSearchOpen() ?? false
  }
  function toggleFindPanel() {
    searchable()?.toggleSearch()
    syncFindPanelOpen()
  }
  $effect(() => {
    void [showDiff, showPreview, editor, mergeEditor]
    syncFindPanelOpen()
  })

  async function loadDirs(dirs: string[]) {
    const missing = dirs.filter((d) => !loadedDirs.has(d))
    if (missing.length) {
      const batch = await service.ListTrees(projectId, workspaceId, missing).catch(() => null)
      const patch = (nodes: TreeEntry[], d: string, kids: TreeEntry[]): TreeEntry[] =>
        nodes.map((n) => n.path === d ? { ...n, children: kids } : n.children?.length ? { ...n, children: patch(n.children, d, kids) } : n)
      let next = tree
      for (const d of missing) if (batch?.[d]) next = patch(next, d, batch[d]!)
      tree = next
      loadedDirs = new Set([...loadedDirs, ...missing])
    }
    expanded = new Set([...expanded, ...dirs])
  }

  function ancestorDirs(path: string): string[] {
    const parts = path.split('/').slice(0, -1)
    return parts.map((_, i) => parts.slice(0, i + 1).join('/'))
  }

  async function setBrowse(all: boolean) {
    browseAll = all
    if (all) await loadDirs([...new Set(reviewPaths.flatMap(ancestorDirs))])
  }

  async function revealInTree(path: string) {
    const dirs = ancestorDirs(path)
    if (filterChanged) changedCollapsed = new Set([...changedCollapsed].filter((d) => !dirs.includes(d)))
    else await loadDirs(dirs)
  }

  async function toggleDir(path: string) {
    const set = filterChanged ? changedCollapsed : expanded
    const next = new Set(set)
    if (filterChanged) {
      if (next.has(path)) next.delete(path)
      else next.add(path)
      changedCollapsed = next
      return
    }
    if (next.has(path)) {
      next.delete(path)
      expanded = next
    } else {
      await loadDirs([path])
    }
  }

  function entryStatus(e: TreeEntry): string {
    if (gitStatus[e.path]) return gitStatus[e.path]
    if (!e.dir) return e.status || ''
    const rank: Record<string, number> = { d: 4, a: 3, u: 2, m: 1 }
    let best = ''
    for (const [p, st] of Object.entries(gitStatus)) {
      if (p.startsWith(e.path + '/') && (rank[st] ?? 0) > (rank[best] ?? 0)) best = st
    }
    return best
  }

  const statusTitle: Record<string, string> = { m: 'Modified', a: 'Added', d: 'Deleted', u: 'Untracked' }

  function onNodeClick(e: TreeEntry) {
    if (e.dir) {
      treePath = e.path
      void toggleDir(e.path)
    } else {
      void openPath(e.path, 0, filterChanged || entryStatus(e) === 'd')
    }
  }

  function setFindingStatus(f: Finding, status: string) {
    findings = findings.map((x) => (x.id === f.id ? { ...x, status } : x))
  }

  function onObsFocusIn(e: FocusEvent) {
    const el = e.target as Element
    if (el.closest('.obs-files')) focusZone = 'editor'
    else if (el.closest('.obs-left')) focusZone = 'agent'
    else if (el.closest('.obs-main, .obs-tb-center')) focusZone = 'editor'
    else focusZone = 'other'
  }

  function onLeftMouseDown(e: MouseEvent) {
    if (e.button !== 0) return
    if ((e.target as Element).closest('textarea, input, select, button, a, [role="button"], [role="tab"], [contenteditable="true"]')) return
    leftAsideEl?.focus({ preventScroll: true })
  }

  $effect(() => {
    const jump = $observatoryJump
    if (!jump || (jump.workspaceId && jump.workspaceId !== workspaceId)) return
    observatoryJump.set(null)
    if (jump.panel === 'review') {
      showReview()
      setReviewTab('changes')
    } else if (jump.panel === 'findings') {
      selectCodePanel('findings')
    } else if (jump.path) {
      void openPath(jump.path, jump.line ?? 0, !!jump.preferDiff)
    }
  })
</script>

<div class="obs" class:loop-scoped={loopScoped} onfocusin={onObsFocusIn}>
  <div class="obs-toolbar" class:no-side={!showSide} class:has-left={true}>
    <div class="obs-tb-left">
      <div class="obs-panel-icons" role="toolbar" aria-label="Left panels">
        {#each leftPanels as key (key)}
          {@const n = paneBadgeCount(key)}
          <button
            type="button"
            class="obs-pane-toggle"
            class:on={leftKind === key}
            title={panelLabel(key)}
            aria-label={panelLabel(key)}
            aria-pressed={leftKind === key}
            onclick={() => selectCodePanel(key)}
          >
            <span class="obs-pane-icon-wrap">
              <Icon name={PANEL_ICONS[key]!} />
              {#if n}<span class="obs-pane-badge">{n > 9 ? '9+' : n}</span>{/if}
            </span>
          </button>
        {/each}
      </div>
    </div>
    <TabStrip
      class="obs-tb-center"
      label={mainSurface === 'review' ? 'Review' : 'Open files'}
      activeKey={mainSurface === 'review' ? reviewTab : selectedPath}
    >
      {#if mainSurface === 'review'}
        <TabChip
          class="obs-reviewchip"
          label="Changes"
          title={reviewCount ? `Changes (${reviewCount})` : 'Changes'}
          icon="file-diff"
          badge={reviewCount}
          active={reviewTab === 'changes'}
          lit={focusZone === 'editor'}
          onclick={() => setReviewTab('changes')}
        />
        <TabChip
          class="obs-reviewchip"
          label="Commits"
          title={reviewCommitCount ? `Commits (${reviewCommitCount})` : 'Commits'}
          icon="git-commit"
          badge={reviewCommitCount}
          dirty={workingDirtyCount > 0}
          active={reviewTab === 'commits'}
          lit={focusZone === 'editor'}
          onclick={() => setReviewTab('commits')}
        />
      {:else}
        {#each openFiles as p (p)}
          {@const name = p.split('/').pop() || p}
          <TabChip
            class="obs-filechip"
            tabKey={p}
            label={name}
            title={p}
            iconClass={fileIconClass(name, $theme)}
            active={selectedPath === p}
            lit={focusZone === 'editor'}
            dirty={p in drafts}
            onclick={() => openPath(p)}
          />
        {/each}
      {/if}
      {#snippet trailing()}
        <button
          type="button"
          class="obs-pane-toggle obs-editor-toggle"
          class:on={mainSurface === 'editor'}
          onclick={() => (mainSurface = 'editor')}
          title="Show editor (⌥⌘E)"
          aria-label="Show editor"
          aria-pressed={mainSurface === 'editor'}
        >
          <Icon name="code" />
        </button>
        {#if isGit}
          <button
            type="button"
            class="obs-pane-toggle obs-review-toggle"
            class:on={mainSurface === 'review'}
            title="Review changes (⌥⌘R)"
            aria-label="Review changes"
            aria-pressed={mainSurface === 'review'}
            onclick={() => (mainSurface === 'review' ? (mainSurface = 'editor') : showReview())}
          >
            <Icon name="file-diff" />
          </button>
        {/if}
      {/snippet}
    </TabStrip>
  </div>

  <div class="obs-body" class:no-side={!showSide} class:has-left={true}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <aside
      class="obs-left"
      aria-label={panelLabel(leftKind)}
      tabindex="-1"
      bind:this={leftAsideEl}
      onmousedown={onLeftMouseDown}
    >
      <div class="obs-side-head" class:tabbed={leftKind === 'maestro' && !!leftHead && leftTitle === 'Agent'}>
        {#if leftKind === 'maestro' && leftHead}
          {@render leftHead(focusZone === 'agent')}
        {:else}
          <span class="obs-side-title">{panelLabel(leftKind)}</span>
          <div class="obs-file-tools">
            <div class="seg obs-scope findings-scope" role="group" aria-label="Finding status">
              {#each FINDINGS_FILTERS as f (f.key)}
                <button
                  type="button"
                  class:on={findingsFilter === f.key}
                  aria-pressed={findingsFilter === f.key}
                  onclick={() => (findingsFilter = f.key)}
                >
                  {f.label}
                  {#if findingsFilterCounts[f.key]}<span class="cnt">{findingsFilterCounts[f.key]}</span>{/if}
                </button>
              {/each}
            </div>
          </div>
        {/if}
      </div>
      <div class="obs-pane-body obs-left-body">
        {#if leftKind === 'maestro'}
          {@render leftPanel?.()}
        {:else}
          <FindingsPanel
            {findings}
            filter={findingsFilter}
            onOpen={(f) => f.file && openPath(findingRelPath(f.file), f.line ?? 0)}
            onSetStatus={setFindingStatus}
          />
        {/if}
      </div>
      <ResizeHandle panel="maestroDock" edge="end" />
    </aside>

    <div class="obs-work">
      <main class="obs-main" tabindex="-1">
        {#if selectedPath}
          <div class="obs-crumbbar" class:off={mainSurface !== 'editor'} aria-label="File path" inert={mainSurface !== 'editor' || undefined}>
            {#each pathCrumbs as part, i}
              {#if i > 0}<span class="sep">/</span>{/if}
              <button
                type="button"
                class="crumb"
                class:file={i === pathCrumbs.length - 1}
                title={pathCrumbs.slice(0, i + 1).join('/')}
                onclick={() => {
                  if (i === pathCrumbs.length - 1) return
                  void revealInTree(pathCrumbs.slice(0, i + 1).join('/') + '/_')
                  selectCodePanel('files')
                }}
              >{part}</button>
            {/each}
            <div class="sp"></div>
            <div class="obs-crumb-actions">
              {#if canShowDiff || previewKind}
                <div class="seg obs-view-seg" role="group" aria-label="View mode">
                  <button type="button" class:on={viewMode === 'code'} disabled={!file} onclick={() => setViewMode('code')}>Code</button>
                  {#if canShowDiff}
                    <button type="button" class:on={viewMode === 'diff'} title={reviewBase ? `Diff vs ${reviewBase} (merge-base)` : 'Diff vs HEAD'} onclick={() => setViewMode('diff')}>Diff</button>
                  {/if}
                  {#if previewKind}
                    <button type="button" class:on={viewMode === 'preview'} disabled={!file} onclick={() => setViewMode('preview')}>Preview</button>
                  {/if}
                </div>
              {/if}
              {#if isGit && viewMode === 'diff'}
                <button
                  type="button"
                  class="obs-icon-btn"
                  title="All diffs — review changes since {reviewBase || 'HEAD'} (⌥⌘R)"
                  aria-label="Review all diffs"
                  onclick={() => { setReviewTab('changes'); showReview() }}
                >
                  <Icon name="rows-3" />
                </button>
              {/if}
              {#if !showPreview}
                <button
                  type="button"
                  class="obs-icon-btn"
                  class:on={findPanelOpen}
                  title="Find (⌘F)"
                  aria-label="Find"
                  aria-pressed={findPanelOpen}
                  onclick={toggleFindPanel}
                >
                  <Icon name="search" />
                </button>
              {/if}
            </div>
          </div>
        {/if}
        <div class="obs-editors" class:off={mainSurface !== 'editor'} inert={mainSurface !== 'editor' || undefined}>
          <div class="obs-editor-pane focused" class:lit={focusZone === 'editor'} role="group" aria-label="Main editor">
            {#if editorPath}
              <div class="obs-surface" class:off={showPreview || viewMode === 'diff'}>
                <CodeEditor
                  bind:this={editor}
                  doc={draft}
                  path={editorPath}
                  readOnly={!file?.writable || showPreview || viewMode === 'diff'}
                  {findingMarks}
                  baseDoc={gutterBase(editorPath)}
                  {onDocChange}
                  onSearchOpenChange={syncFindPanelOpen}
                  onOpenFinding={() => selectCodePanel('findings')}
                />
              </div>
            {/if}
            {#if previewKind}
              <div class="obs-surface" class:off={!showPreview} inert={!showPreview || undefined}>
                <div class="file-preview-host">
                  <div class="file-preview file-preview-md">
                    <MarkdownHtml text={draft} class="md-doc" />
                  </div>
                </div>
              </div>
            {/if}
            {#if showDiff}
              <div class="obs-diff">
                {#key selectedPath}
                  <MergeEditor
                    bind:this={mergeEditor}
                    path={selectedPath}
                    doc={draft}
                    original={baseDocs[selectedPath]}
                    readOnly={!file?.writable}
                    {onDocChange}
                    onSearchOpenChange={syncFindPanelOpen}
                  />
                {/key}
              </div>
            {:else if !editorPath}
              <div class="obs-empty pad" aria-busy={fileLoading || undefined}>{fileLoading ? '' : 'Select a file to view/edit.'}</div>
            {/if}
          </div>
        </div>
        {#if isGit}
          <div class="obs-review-host" class:off={mainSurface === 'editor'} inert={mainSurface === 'editor' || undefined} tabindex="-1">
            <ReviewSurface
              bind:this={reviewSurface}
              {projectId}
              {workspaceId}
              baseLabel={reviewBase}
              active={mainSurface === 'review'}
              tab={reviewTab}
              onTabChange={setReviewTab}
              onOpenFile={(path, line) => { void openPath(path, line ?? 0, true) }}
              dirtyPaths={new Set(Object.keys(workingStatus))}
              {workingDirtyCount}
              count={reviewCount}
              {excludeMatchers}
              bind:commitCount={reviewCommitCount}
            />
          </div>
        {/if}
      </main>

      {#if showSide}
        <aside class="obs-side">
          <ResizeHandle panel="obsSide" edge="start" />
          <div class="obs-side-head">
            <span class="obs-side-title">{CODE_PANEL_LABELS.files}</span>
            {#if reviewPaths.length}
              <div class="obs-file-tools">
                <div class="seg obs-scope" role="group" aria-label="Explorer scope">
                  <button type="button" class:on={!browseAll} aria-pressed={!browseAll} title="Show files changed since {reviewBase || 'HEAD'}" onclick={() => setBrowse(false)}>
                    Changed
                    <span class="cnt">{reviewCount}</span>
                  </button>
                  <button type="button" class:on={browseAll} aria-pressed={browseAll} title="Browse the full repository tree" onclick={() => setBrowse(true)}>All files</button>
                </div>
              </div>
            {/if}
          </div>
          <div class="obs-pane-body">
            <div class="obs-files-panel">
              <div class="overlay-sb-shell">
                <div
                  class="obs-files"
                  tabindex="0"
                  role="tree"
                  aria-label="File explorer"
                  {@attach overlayScrollbar()}
                  {@attach stickyFolderFades('.obs-node.dir')}
                >
                  {#each visibleTree as e (e.path)}
                    {@render node(e, 0)}
                  {:else}
                    <div class="obs-empty">No files</div>
                  {/each}
                </div>
              </div>
            </div>
          </div>
        </aside>
      {/if}
    </div>
  </div>
</div>

{#snippet node(e: TreeEntry, depth: number)}
  {@const isOpen = filterChanged ? !changedCollapsed.has(e.path) : expanded.has(e.path)}
  {@const st = entryStatus(e)}
  {@const findingFlag = findingFlagForPath(e.path, e.dir, openFindingsByPath)}
  {#snippet nodeRow()}
    <button
      class="obs-node"
      class:dir={e.dir}
      class:open={e.dir && isOpen}
      class:ignored={e.ignored}
      class:on={treePath === e.path}
      class:deleted={st === 'd' && !e.dir}
      style="padding-left:{10 + depth * 12}px"
      data-tree-path={e.path}
      role="treeitem"
      aria-selected={treePath === e.path}
      aria-expanded={e.dir ? isOpen : undefined}
      onclick={() => onNodeClick(e)}
    >
      {#if e.dir}
        <Icon class="car" name="chevron-down" strokeWidth={2.4} />
      {:else}
        <span class="fico {fileIconClass(e.name, $theme)}" aria-hidden="true"></span>
      {/if}
      <span class="label-row">
        <span class="label" class:st-m={st === 'm'} class:st-a={st === 'a'} class:st-d={st === 'd'} class:st-u={st === 'u'}>{e.name}</span>
      </span>
      {#if findingFlag.count}
        <span class="obs-finding-flag {findingFlag.severity}" class:dir={e.dir} title="{findingFlag.count} open finding{findingFlag.count === 1 ? '' : 's'}">
          <Icon name="hat-glasses" strokeWidth={2.2} />
        </span>
      {/if}
      {#if st && !e.dir}
        <span class="fst {st}" class:wip={!!workingStatus[e.path]} title={statusTitle[st] ?? st}>{st.toUpperCase()}</span>
      {/if}
    </button>
  {/snippet}
  {#if e.dir}
    <div class="obs-branch" style="--d:{depth}">
      {@render nodeRow()}
      {#if isOpen}
        {#each e.children ?? [] as c (c.path)}
          {@render node(c, depth + 1)}
        {/each}
      {/if}
    </div>
  {:else}
    {@render nodeRow()}
  {/if}
{/snippet}
