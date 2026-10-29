<script lang="ts">
  import { untrack } from 'svelte'
  import { CodeService } from '../lib/api'
  import type { Commit, FileDiff, Finding } from '../../bindings/recur/internal/domain/models'
  import type { ReviewTab } from '../lib/review-diff'
  import { commitSha } from '../lib/commit-meta'
  import { canStepOnTape, reviewTape, reviewTapeIndex, stepTape, tapeStepLabel } from '../lib/review-scope'
  import FilesChangedView from './FilesChangedView.svelte'
  import CommitsView from './CommitsView.svelte'

  export type ReviewViewState = { tab: ReviewTab; selectedPath: string; scrollTop: number }

  let {
    projectId = '',
    workspaceId = '',
    baseLabel = '',
    refresh = 0,
    active = false,
    tab = 'changes',
    count = 0,
    excludeMatchers = undefined as readonly RegExp[] | undefined,
    onTabChange,
    onOpenFile,
    onAddFinding,
    findingsByPath = undefined as ReadonlyMap<string, Finding[]> | undefined,
    onOpenFinding,
    onShowFindings,
    dirtyPaths = undefined as ReadonlySet<string> | undefined,
    workingDirtyCount = 0,
    restore = null as { selectedPath: string; scrollTop: number } | null,
    commitCount = $bindable(0),
    commitScopeHash = $bindable(''),
    workingOpen = $bindable(false),
    diffTotals = $bindable({ add: 0, del: 0 }),
    fileCount = $bindable(0),
  }: {
    projectId?: string
    workspaceId?: string
    baseLabel?: string
    refresh?: number
    active?: boolean
    tab?: ReviewTab
    count?: number
    excludeMatchers?: readonly RegExp[]
    onTabChange?: (tab: ReviewTab) => void
    onOpenFile?: (path: string, line?: number) => void
    onAddFinding?: (path: string, line: number, body: string, endLine?: number) => Promise<boolean>
    findingsByPath?: ReadonlyMap<string, Finding[]>
    onOpenFinding?: (f: Finding) => void
    onShowFindings?: () => void
    dirtyPaths?: ReadonlySet<string>
    workingDirtyCount?: number
    restore?: { selectedPath: string; scrollTop: number } | null
    commitCount?: number
    commitScopeHash?: string
    workingOpen?: boolean
    diffTotals?: { add: number; del: number }
    fileCount?: number
  } = $props()

  let commitScope = $state.raw<Commit | null>(null)
  let commitFilesByHash = $state.raw<Record<string, FileDiff[]>>({})
  let commits = $state<Commit[]>([])
  let commitsLoading = $state(false)
  let commitsError = $state(false)
  let filesView: { getViewState: () => { selectedPath: string; scrollTop: number } } | undefined

  const showingCommit = $derived(!!commitScope)
  const filesOpen = $derived(tab === 'changes' || showingCommit || workingOpen)
  const commitsListOpen = $derived(tab === 'commits' && !showingCommit && !workingOpen)
  const tape = $derived(reviewTape(workingDirtyCount > 0, commits))
  const scopeIdx = $derived(reviewTapeIndex(tape, workingOpen, commitScope ? commitSha(commitScope) : ''))
  const stepFlags = $derived(canStepOnTape(scopeIdx, tape.length))
  const canStepOlder = $derived(stepFlags.older)
  const canStepNewer = $derived(stepFlags.newer)
  const stepOlderLabel = $derived(tapeStepLabel(stepTape(tape, scopeIdx, -1), -1))
  const stepNewerLabel = $derived(tapeStepLabel(stepTape(tape, scopeIdx, 1), 1))
  // workspaceId is re-passed (same string) when the workspaces store
  // refreshes — file edits fire workspace.updated. Resetting on that
  // kicked a commit / checkpoint drill-down back to the commits list.
  let scopeForWs = ''
  $effect(() => {
    const id = workspaceId
    if (id === scopeForWs) return
    scopeForWs = id
    commitScope = null
    commitFilesByHash = {}
    commitCount = 0
    commitScopeHash = ''
    workingOpen = false
    commits = []
    fileCount = 0
  })

  $effect(() => {
    if (workingOpen && workingDirtyCount === 0) clearScope()
  })

  function openCommit(c: Commit) {
    const hash = commitSha(c)
    const cached = hash ? commitFilesByHash[hash] : undefined
    workingOpen = false
    commitScope = cached?.length ? { ...c, files: cached } : c
    commitScopeHash = hash
    onTabChange?.('commits')
  }

  function openWorking() {
    commitScope = null
    commitScopeHash = ''
    workingOpen = true
    onTabChange?.('commits')
  }

  function clearScope() {
    commitScope = null
    commitScopeHash = ''
    workingOpen = false
  }

  export function clearCommitScope() {
    clearScope()
  }

  export function openWorkingScope() {
    openWorking()
  }

  /** Show a caller-supplied diff (e.g. a chat turn's checkpoint diff) as a
   *  commit scope. `c.files` must be prefilled — its hash is not on the branch. */
  export function openCommitScope(c: Commit) {
    openCommit(c)
  }

  function cacheCommitFiles(c: Commit, files: FileDiff[]) {
    const hash = commitSha(c)
    if (!hash) return
    commitFilesByHash = { ...commitFilesByHash, [hash]: files }
    if (commitScope && commitSha(commitScope) === hash) {
      commitScope = { ...commitScope, files }
    }
  }

  function stepScope(dir: -1 | 1) {
    const next = stepTape(tape, scopeIdx, dir)
    if (!next) return
    if (next.kind === 'working') openWorking()
    else openCommit(next.commit)
  }

  let loadGen = 0
  async function loadCommits() {
    if (!active) return
    const gen = ++loadGen
    commitsLoading = true
    commitsError = false
    try {
      const got = await CodeService.ListCommits(projectId, workspaceId)
      if (gen !== loadGen) return
      commits = got ?? []
      commitCount = commits.length
    } catch {
      if (gen !== loadGen) return
      commitsError = true
      if (!commits.length) commits = []
      commitCount = commits.length
    } finally {
      if (gen === loadGen) commitsLoading = false
    }
  }

  $effect(() => {
    if (!active) return
    void projectId
    void workspaceId
    void refresh
    untrack(() => { void loadCommits() })
  })

  export function getViewState(): ReviewViewState {
    const inner = filesView?.getViewState()
    return { tab, selectedPath: inner?.selectedPath ?? '', scrollTop: inner?.scrollTop ?? 0 }
  }
</script>

<div class="obs-review-surface">
  <div class="obs-review-slot" class:off={!filesOpen} inert={!filesOpen || undefined}>
    <FilesChangedView
      bind:this={filesView}
      {projectId}
      {workspaceId}
      {baseLabel}
      {refresh}
      active={active && filesOpen}
      {commitScope}
      workingScope={workingOpen}
      {count}
      {excludeMatchers}
      onClearScope={clearScope}
      onCommitFiles={cacheCommitFiles}
      {onOpenFile}
      noteMode="finding"
      {onAddFinding}
      {findingsByPath}
      {onOpenFinding}
      {onShowFindings}
      {dirtyPaths}
      onStepScope={stepScope}
      {canStepOlder}
      {canStepNewer}
      {stepOlderLabel}
      {stepNewerLabel}
      {restore}
      bind:diffTotals
      bind:fileCount
    />
  </div>
  <div class="obs-review-slot" class:off={!commitsListOpen} inert={!commitsListOpen || undefined}>
    <CommitsView
      {commits}
      loading={commitsLoading}
      error={commitsError}
      {baseLabel}
      {workingDirtyCount}
      onOpenCommit={openCommit}
      onOpenWorking={openWorking}
    />
  </div>
</div>
