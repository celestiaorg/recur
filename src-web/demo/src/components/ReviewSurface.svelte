<script lang="ts">
  import { service } from '../lib/api'
  import type { Commit } from '../lib/types'
  import type { ReviewTab } from '../lib/review-diff'
  import { commitSha } from '../lib/commit-meta'
  import { canStepOnTape, reviewTape, reviewTapeIndex, stepTape, tapeStepLabel } from '../lib/review-scope'
  import FilesChangedView from './FilesChangedView.svelte'
  import CommitsView from './CommitsView.svelte'

  let {
    projectId = '',
    workspaceId = '',
    baseLabel = '',
    active = false,
    tab = 'changes',
    count = 0,
    excludeMatchers = undefined as readonly RegExp[] | undefined,
    onTabChange,
    onOpenFile,
    dirtyPaths = undefined as ReadonlySet<string> | undefined,
    workingDirtyCount = 0,
    commitCount = $bindable(0),
  }: {
    projectId?: string
    workspaceId?: string
    baseLabel?: string
    active?: boolean
    tab?: ReviewTab
    count?: number
    excludeMatchers?: readonly RegExp[]
    onTabChange?: (tab: ReviewTab) => void
    onOpenFile?: (path: string, line?: number) => void
    dirtyPaths?: ReadonlySet<string>
    workingDirtyCount?: number
    commitCount?: number
  } = $props()

  let commitScope = $state.raw<Commit | null>(null)
  let workingOpen = $state(false)
  let commits = $state<Commit[]>([])

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
  $effect(() => {
    if (workingOpen && workingDirtyCount === 0) clearScope()
  })

  function openCommit(c: Commit) {
    workingOpen = false
    commitScope = c
    onTabChange?.('commits')
  }

  function openWorking() {
    commitScope = null
    workingOpen = true
    onTabChange?.('commits')
  }

  function clearScope() {
    commitScope = null
    workingOpen = false
  }

  export function clearCommitScope() {
    clearScope()
  }

  /** Show a caller-supplied diff (e.g. a chat turn's checkpoint diff) as a
   *  commit scope. `c.files` must be prefilled: its hash is not on the branch. */
  export function openCommitScope(c: Commit) {
    openCommit(c)
  }

  function stepScope(dir: -1 | 1) {
    const next = stepTape(tape, scopeIdx, dir)
    if (!next) return
    if (next.kind === 'working') openWorking()
    else openCommit(next.commit)
  }

  $effect(() => {
    if (!active) return
    void service.ListCommits(projectId, workspaceId).then((got) => {
      commits = got ?? []
      commitCount = commits.length
    })
  })
</script>

<div class="obs-review-surface">
  <div class="obs-review-slot" class:off={!filesOpen} inert={!filesOpen || undefined}>
    <FilesChangedView
      {projectId}
      {workspaceId}
      {baseLabel}
      active={active && filesOpen}
      {commitScope}
      workingScope={workingOpen}
      {count}
      {excludeMatchers}
      onClearScope={clearScope}
      {onOpenFile}
      {dirtyPaths}
      onStepScope={stepScope}
      {canStepOlder}
      {canStepNewer}
      {stepOlderLabel}
      {stepNewerLabel}
    />
  </div>
  <div class="obs-review-slot" class:off={!commitsListOpen} inert={!commitsListOpen || undefined}>
    <CommitsView
      {commits}
      {baseLabel}
      {workingDirtyCount}
      onOpenCommit={openCommit}
      onOpenWorking={openWorking}
    />
  </div>
</div>
