<script lang="ts">
  import { prs, selectedPRId, projects, settings, workspaces, workspacesRunning, prDetailTab, pushToast, openPRReview, closePRReview, prReviewDraft, type PRReviewKind } from '../lib/stores'
  import { PRsService, WorkspacesService, call, onWorkspaceFs, onAppFocused } from '../lib/api'
  import { wsHasRun } from '../lib/modes'
  import { openExternalUrl } from '../lib/open-external'
  import { keepMergedPR, prLifecycle } from '../lib/colors'
  import { allowedMergeMethods, canApprovePRReview, canSetPRDraft, pickAllowedMergeMethod, prMergeAction, resolveUpdateBaseMethod, shortBaseName, showPRUpdate, UPDATE_BASE_METHODS, updateWithBaseLabel } from '../lib/pr-merge-action'
  import { requestFixChecks } from '../lib/pr-fix-checks'
  import { requestPromptAction } from '../lib/promptActions'
  import {
    EMPTY_WORKSPACE_LAND, getWorkspaceLand, putWorkspaceLand, workspaceLandFrom,
  } from '../lib/caches/workspace-land-cache'
  import Icon from './Icon.svelte'

  let confirmingMerge = $state(false)
  let mergeMenuOpen = $state(false)
  let reviewMenuOpen = $state(false)
  let reviewKind = $state<PRReviewKind>('approve')
  let awaitingUpdate = $state(false)
  let draftBusy = $state(false)
  let mergeBusy = $state(false)
  let mergeMethodOverride = $state<string | null>(null)
  let updateMethodOverride = $state<string | null>(null)
  let updateMenuOpen = $state(false)

  const mergeMethodLabels: Record<string, string> = {
    merge: 'Merge',
    squash: 'Squash and merge',
    rebase: 'Rebase and merge',
  }
  const updateMethodMenuLabels: Record<string, string> = {
    merge: 'Merge',
    rebase: 'Rebase',
  }
  const pr = $derived($prs.find((p) => p.id === $selectedPRId))
  const mergeMethodKeys = $derived(allowedMergeMethods(pr))
  const showMergeMenu = $derived(mergeMethodKeys.length > 1)
  const isLocal = $derived(pr?.source === 'local')
  const behindBase = $derived(pr?.behindBase ?? 0)
  const updatingBase = $derived(pr?.updatingBase ?? '')
  const isUpdating = $derived(awaitingUpdate || !!updatingBase)
  const merged = $derived((pr?.state ?? '') === 'Merged' || pr?.group === 'merged')
  const prWorkspace = $derived($workspaces.find((w) => w.id === pr?.workspaceId))
  const canSendBack = $derived(!!pr && !merged && wsHasRun(prWorkspace))
  const landWsId = $derived((pr?.workspaceId ?? '').trim())
  const githubOpen = $derived(pr?.source === 'gh' && prLifecycle(pr) !== 'merged' && prLifecycle(pr) !== 'closed')
  let land = $state({ ...EMPTY_WORKSPACE_LAND })
  let landSeq = 0
  let landTrackedId = ''
  let landKnown = $state(false)
  let pullingPR = $state(false)
  let pushingRemote = $state(false)
  const agentBusy = $derived(!!landWsId && $workspacesRunning.has(landWsId))
  const landPending = $derived(githubOpen && !!landWsId && !landKnown)
  const busy = $derived(isUpdating || pullingPR || pushingRemote)
  const remoteBusy = $derived(busy || agentBusy || landPending)
  const landPendingTitle = $derived(landPending ? 'Checking git status…' : '')
  const prDiverged = $derived(githubOpen && land.aheadRemote > 0 && land.behindRemote > 0)
  const canPullPR = $derived(githubOpen && !!landWsId && !land.dirty && !land.unmerged && land.behindRemote > 0)
  const canPushPR = $derived(githubOpen && !!landWsId && !land.dirty && !land.unmerged && land.aheadRemote > 0 && land.behindRemote === 0)
  const canForcePushPR = $derived(githubOpen && !!landWsId && !land.dirty && !land.unmerged && prDiverged)

  function applyLand(st: Parameters<typeof workspaceLandFrom>[0]) {
    land = workspaceLandFrom(st)
    landKnown = true
    if (landWsId) putWorkspaceLand(landWsId, land)
  }
  function refreshLandState() {
    const id = landWsId
    if (!id || !githubOpen) return
    const seq = ++landSeq
    void WorkspacesService.WorkspaceLandState(id)
      .then((st) => { if (seq === landSeq) applyLand(st) })
      .catch(() => {
        if (seq !== landSeq) return
        land = { ...EMPTY_WORKSPACE_LAND }
        landKnown = true
      })
  }
  $effect.pre(() => {
    const id = landWsId
    if (id === landTrackedId) return
    landTrackedId = id
    landSeq += 1
    land = (id && getWorkspaceLand(id)) || { ...EMPTY_WORKSPACE_LAND }
    landKnown = false
  })
  $effect(() => {
    void landWsId
    void githubOpen
    void prWorkspace?.diff
    refreshLandState()
  })
  $effect(() => {
    if (!githubOpen || !landWsId) return
    const id = landWsId
    let cancelled = false
    let inner = 0
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => {
        if (cancelled) return
        void WorkspacesService.FetchWorkingCopyUpstream(id)
          .then((st) => {
            if (cancelled) return
            landSeq++
            applyLand(st)
          })
          .catch(() => {})
      })
    })
    return () => {
      cancelled = true
      cancelAnimationFrame(outer)
      if (inner) cancelAnimationFrame(inner)
    }
  })
  const LAND_FS_DEBOUNCE_MS = 50
  let landFsTimer: ReturnType<typeof setTimeout> | undefined
  $effect(() => {
    const id = landWsId
    if (!id || !githubOpen) return
    const unsubFs = onWorkspaceFs((changedId) => {
      if (changedId !== id) return
      if (landFsTimer) return
      landFsTimer = setTimeout(() => {
        landFsTimer = undefined
        refreshLandState()
      }, LAND_FS_DEBOUNCE_MS)
    })
    const unsubFocus = onAppFocused(() => refreshLandState())
    return () => {
      unsubFs()
      unsubFocus()
      if (landFsTimer) {
        clearTimeout(landFsTimer)
        landFsTimer = undefined
      }
    }
  })

  async function pullWorkspacePR() {
    if (remoteBusy || !canPullPR || !landWsId) return
    const id = landWsId
    const n = pr?.number
    pullingPR = true
    try {
      const res = await call(WorkspacesService.PullWorkspacePR(id), 'Could not pull from the PR')
      if (!res) return
      if (res.workspace?.id) {
        workspaces.update((arr) => arr.map((l) => (l.id === res.workspace.id ? { ...l, ...res.workspace, behindBase: res.workspace.behindBase ?? 0 } : l)))
      }
      if (res.status === 'conflict') {
        requestPromptAction(id, 'update')
        return
      }
      if (res.status === 'updated') {
        pushToast('success', n ? `Pulled updates from PR #${n}` : 'Pulled updates from the PR')
      } else if (res.status === 'noop') {
        pushToast('info', n ? `Already up to date with PR #${n}` : 'Already up to date with the PR')
      }
    } finally {
      pullingPR = false
      if (landWsId === id) refreshLandState()
    }
  }

  async function pushWorkspacePR(force = false) {
    if (remoteBusy || (force ? !canForcePushPR : !canPushPR) || !landWsId) return
    const id = landWsId
    const n = pr?.number
    pushingRemote = true
    try {
      const prId = await call(WorkspacesService.OpenWorkspacePR(id, force), force ? 'Could not force-push to the PR' : 'Could not push to the PR')
      if (!prId) return
      if (force) {
        pushToast('success', n ? `Force-pushed to PR #${n}` : 'Force-pushed updates to the PR')
      } else {
        pushToast('success', n ? `Pushed to PR #${n}` : 'Pushed updates to the PR')
      }
    } finally {
      pushingRemote = false
      if (landWsId === id) refreshLandState()
    }
  }
  let sendingBack = $state(false)

  async function sendBack() {
    if (!pr?.id || sendingBack || busy) return
    sendingBack = true
    try {
      const updated = await call(PRsService.SendBackToLoop(pr.id, []), 'Could not send the review to the conductor')
      if (updated) pushToast('success', 'Sent the review to the conductor')
    } finally {
      sendingBack = false
    }
  }
  const pendingCount = $derived(pr?.pendingComments?.length ?? 0)
  const action = $derived(prMergeAction(pr, { awaitingUpdate }))
  const canDraft = $derived(canSetPRDraft(pr))
  const isDraft = $derived(!!pr?.draft || action.kind === 'ready')
  const canApprove = $derived(!isDraft && canApprovePRReview(pr))
  const showUpdate = $derived(showPRUpdate(pr, action))
  const githubHref = $derived.by(() => {
    if (!pr || pr.source !== 'gh' || !pr.number) return ''
    const stored = (pr.url ?? '').trim()
    if (stored) return stored
    for (const c of pr.checks ?? []) {
      const href = githubPRHrefFromCheck(c.url, pr.number)
      if (href) return href
    }
    return ''
  })

  function githubPRHrefFromCheck(url: string | undefined, n: number): string {
    if (!url) return ''
    try {
      const u = new URL(url)
      const parts = u.pathname.split('/').filter(Boolean)
      if (parts.length >= 2) return `${u.origin}/${parts[0]}/${parts[1]}/pull/${n}`
    } catch {
      /* ignore malformed check urls */
    }
    return ''
  }

  const resolvedMergeMethod = $derived.by(() => {
    const loop = $workspaces.find((l) => l.id === pr?.workspaceId)
    const proj = loop
      ? $projects.find((p) => p.id === loop.projectId)
      : $projects.find((p) => p.id === pr?.projectId)
    let preferred = 'merge'
    if (proj?.mergeMethod && mergeMethodLabels[proj.mergeMethod]) preferred = proj.mergeMethod
    else if ($settings?.defaultMergeMethod && mergeMethodLabels[$settings.defaultMergeMethod]) preferred = $settings.defaultMergeMethod
    return pickAllowedMergeMethod(preferred, mergeMethodKeys)
  })
  const mergeMethod = $derived(pickAllowedMergeMethod(mergeMethodOverride ?? resolvedMergeMethod, mergeMethodKeys))
  const mergeLabel = $derived(mergeMethodLabels[mergeMethod] ?? 'Merge')
  const resolvedUpdateMethod = $derived.by(() => {
    const loop = $workspaces.find((l) => l.id === pr?.workspaceId)
    const proj = loop
      ? $projects.find((p) => p.id === loop.projectId)
      : $projects.find((p) => p.id === pr?.projectId)
    return resolveUpdateBaseMethod(proj?.updateBaseMethod, $settings?.defaultUpdateBaseMethod)
  })
  const updateMethod = $derived(resolveUpdateBaseMethod(resolvedUpdateMethod, undefined, updateMethodOverride))
  const updateBaseName = $derived(shortBaseName(pr?.base))
  const updateLabel = $derived(updateWithBaseLabel(updateMethod, pr?.base))

  async function updateWithBase() {
    if (busy || !showUpdate) return
    awaitingUpdate = true
    updateMenuOpen = false
    prDetailTab.set('conv')
    const ok = await call(PRsService.UpdateWithBase($selectedPRId, updateMethod), 'Update with base failed')
    awaitingUpdate = false
    if (ok === null) return
    if (ok.updatingBase === 'coder') {
      pushToast('info', 'Coder resolving merge conflicts')
    } else if (!ok.behindBase) {
      pushToast('success', `Updated with latest ${updateBaseName}`)
    } else {
      pushToast('success', `Updating with latest ${updateBaseName}`)
    }
  }

  async function requestMerge() {
    if (busy || mergeBusy || action.kind !== 'merge' || !pr) return
    if (!confirmingMerge) {
      confirmingMerge = true
      return
    }
    confirmingMerge = false
    mergeMenuOpen = false
    const id = pr.id
    const prevState = pr.state
    const prevGroup = pr.group
    const method = mergeMethod
    mergeBusy = true
    prs.update((arr) => arr.map((p) => (p.id === id ? { ...p, state: 'Merged', group: 'merged' } : p)))
    const mergedPR = await call(PRsService.MergePR(id, method), 'Merge failed')
    mergeBusy = false
    if (mergedPR) {
      prs.update((arr) => arr.map((p) => (p.id === mergedPR.id ? keepMergedPR(p, mergedPR) : p)))
      pushToast('success', 'PR merged')
      return
    }
    const full = await PRsService.GetPR(id).catch(() => null)
    prs.update((arr) => arr.map((p) => {
      if (p.id !== id) return p
      if (full && (full.state === 'Merged' || full.group === 'merged')) {
        return { ...p, state: 'Merged', group: 'merged' }
      }
      return { ...p, state: prevState, group: prevGroup }
    }))
  }

  function pickMergeMethod(m: string) {
    mergeMethodOverride = m
    mergeMenuOpen = false
    confirmingMerge = false
  }

  function pickUpdateMethod(m: string) {
    updateMethodOverride = m
    updateMenuOpen = false
  }

  function openReview(kind: PRReviewKind) {
    reviewKind = kind
    reviewMenuOpen = false
    openPRReview(kind)
  }

  $effect(() => {
    if (!reviewMenuOpen) return
    const onDown = (e: PointerEvent) => {
      const t = e.target as HTMLElement | null
      if (t?.closest('.review-split')) return
      reviewMenuOpen = false
    }
    window.addEventListener('pointerdown', onDown, true)
    return () => window.removeEventListener('pointerdown', onDown, true)
  })

  $effect(() => {
    if (!updateMenuOpen) return
    const onDown = (e: PointerEvent) => {
      const t = e.target as HTMLElement | null
      if (t?.closest('.update-split')) return
      updateMenuOpen = false
    }
    window.addEventListener('pointerdown', onDown, true)
    return () => window.removeEventListener('pointerdown', onDown, true)
  })

  async function setDraft(draft: boolean) {
    if (busy || !canDraft || draftBusy || !pr) return
    const id = pr.id
    const prev = !!pr.draft
    draftBusy = true
    prs.update((arr) => arr.map((p) => (p.id === id ? { ...p, draft } : p)))
    const updated = await call(PRsService.SetPRDraft(id, draft), draft ? 'Convert to draft failed' : 'Mark ready failed')
    draftBusy = false
    if (updated) {
      prs.update((arr) => arr.map((p) => (p.id === updated.id ? { ...p, ...updated, draft } : p)))
      pushToast('success', draft ? 'Converted to draft' : 'Ready for review')
      return
    }
    prs.update((arr) => arr.map((p) => (p.id === id ? { ...p, draft: prev } : p)))
  }

  function fixChecks() {
    if (!pr?.id) return
    void requestFixChecks(pr.id)
  }

  let reviewPRId = $selectedPRId
  $effect(() => {
    const id = $selectedPRId
    if (id === reviewPRId) return
    reviewPRId = id
    confirmingMerge = false
    mergeMenuOpen = false
    reviewMenuOpen = false
    updateMenuOpen = false
    reviewKind = 'approve'
    mergeMethodOverride = null
    updateMethodOverride = null
    awaitingUpdate = false
    draftBusy = false
    mergeBusy = false
    prReviewDraft.set('')
    closePRReview()
  })
</script>

{#if pr && (githubHref || !merged)}
  <div class="review-actions">
    {#if githubHref}
      <button type="button" class="tbtn" title="Open in GitHub" onclick={() => void openExternalUrl(githubHref)}><Icon name="github" />Open in GitHub</button>
    {/if}
    {#if canSendBack}
      <button
        type="button"
        class="tbtn"
        disabled={busy || sendingBack}
        title="Talk to the loop conductor with this PR's comments and line notes"
        onclick={() => void sendBack()}
      ><Icon name="undo" />{sendingBack ? 'Sending…' : 'Send back'}</button>
    {/if}
    {#if !merged}
    {#if action.kind === 'updating'}
      <button class="tbtn solid" disabled><Icon name="refresh-cw" class="spin" />Updating…</button>
    {:else if showUpdate}
      <div class="update-split" class:menu-open={updateMenuOpen}>
        <button
          class="tbtn solid"
          disabled={busy}
          title={action.kind === 'update-base' ? action.title : `${behindBase} commit${behindBase === 1 ? '' : 's'} on ${updateBaseName} not in this branch`}
          onclick={updateWithBase}
        ><Icon name="arrow-up" />{updateLabel}</button>
        <button
          type="button"
          class="tbtn solid update-chev"
          aria-label="Choose update method"
          aria-haspopup="menu"
          aria-expanded={updateMenuOpen}
          onclick={() => { updateMenuOpen = !updateMenuOpen; mergeMenuOpen = false; reviewMenuOpen = false; confirmingMerge = false }}
        ><Icon name="chevron-down" /></button>
        {#if updateMenuOpen}
          <div class="merge-menu" role="menu">
            {#each UPDATE_BASE_METHODS as m (m)}
              <button class="merge-menu-item" class:on={updateMethod === m} role="menuitem" onclick={() => pickUpdateMethod(m)}><span class="mm-check">{updateMethod === m ? '✓' : ''}</span>{updateMethodMenuLabels[m]}</button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
    {#if canPullPR}
      <button
        class="tbtn solid"
        disabled={remoteBusy}
        title={landPendingTitle || 'Bring GitHub commits into this workspace'}
        onclick={() => void pullWorkspacePR()}
      ><Icon name="arrow-up" />{pullingPR ? 'Pulling…' : 'Pull updates'}</button>
    {/if}
    {#if canPushPR}
      <button
        class="tbtn solid"
        disabled={remoteBusy}
        title={landPendingTitle || 'Push new commits to the open pull request'}
        onclick={() => void pushWorkspacePR()}
      ><Icon name="upload" />{pushingRemote ? 'Pushing…' : 'Push updates'}</button>
    {/if}
    {#if canForcePushPR}
      <button
        class="tbtn solid"
        disabled={remoteBusy}
        title={landPendingTitle || 'Overwrite the GitHub head with this workspace (e.g. after rebase onto main). Remote-only commits will be dropped.'}
        onclick={() => void pushWorkspacePR(true)}
      ><Icon name="upload" />{pushingRemote ? 'Force-pushing…' : 'Force push'}</button>
    {/if}
    {#if pendingCount}
      <button
        type="button"
        class="pending-chip"
        title="Line comments post when you submit the review"
        onclick={() => { if (canApprove && !busy) openReview(reviewKind) }}
      >{pendingCount} pending</button>
    {/if}
    {#if canApprove}
      <div class="review-split" class:menu-open={reviewMenuOpen}>
        <button
          type="button"
          class="tbtn"
          disabled={busy}
          title={reviewKind === 'approve' ? 'Approve this pull request' : 'Request changes on this pull request'}
          onclick={() => openReview(reviewKind)}
        ><Icon name={reviewKind === 'approve' ? 'check' : 'octagon-x'} />{reviewKind === 'approve' ? 'Approve' : 'Request changes'}</button>
        <button
          type="button"
          class="tbtn review-chev"
          disabled={busy}
          aria-label="Choose review type"
          aria-haspopup="menu"
          aria-expanded={reviewMenuOpen}
          onclick={() => { reviewMenuOpen = !reviewMenuOpen; mergeMenuOpen = false; updateMenuOpen = false }}
        ><Icon name="chevron-down" /></button>
        {#if reviewMenuOpen}
          <div class="merge-menu" role="menu">
            <button class="merge-menu-item" class:on={reviewKind === 'approve'} role="menuitem" onclick={() => openReview('approve')}><span class="mm-check">{reviewKind === 'approve' ? '✓' : ''}</span>Approve</button>
            <button class="merge-menu-item" class:on={reviewKind === 'request'} role="menuitem" onclick={() => openReview('request')}><span class="mm-check">{reviewKind === 'request' ? '✓' : ''}</span>Request changes</button>
          </div>
        {/if}
      </div>
    {/if}
    {#if canDraft && !isDraft}
      <button class="tbtn" title="Convert this pull request to a draft on GitHub" disabled={busy || draftBusy} onclick={() => void setDraft(true)}><Icon name="git-pull-request-draft" />Convert to draft</button>
    {/if}
    {#if action.kind === 'ready'}
      <button class="tbtn solid" title={action.title} disabled={busy || draftBusy} onclick={() => void setDraft(false)}><Icon name="git-pull-request" />Ready for review</button>
    {:else if action.kind === 'checks'}
      <button class="tbtn solid" title={action.title} onclick={fixChecks}><Icon name="wrench" />Fix checks</button>
    {:else if action.kind === 'conflicts' || action.kind === 'blocked'}
      <button class="tbtn" disabled title={action.title}><Icon name="circle-alert" />{action.label}</button>
    {:else if action.kind === 'merge'}
    <div class="merge-split" class:menu-open={mergeMenuOpen} class:solo={!showMergeMenu}>
      <button
        class="rbtn merge"
        disabled={busy}
        title={action.title}
        onclick={requestMerge}
      ><Icon name="git-pull-request" />{confirmingMerge ? `Confirm ${mergeLabel.toLowerCase()}` : mergeLabel}</button>
      {#if showMergeMenu}
      <button
        class="rbtn merge-chev"
        disabled={busy}
        aria-label="Choose merge method"
        onclick={() => { mergeMenuOpen = !mergeMenuOpen; confirmingMerge = false; updateMenuOpen = false; reviewMenuOpen = false }}
      ><Icon name="chevron-down" /></button>
      {#if mergeMenuOpen}
        <div class="merge-menu" role="menu">
          {#each mergeMethodKeys as m (m)}
            <button class="merge-menu-item" class:on={mergeMethod === m} role="menuitem" onclick={() => pickMergeMethod(m)}><span class="mm-check">{mergeMethod === m ? '✓' : ''}</span>{mergeMethodLabels[m]}</button>
          {/each}
        </div>
      {/if}
      {/if}
    </div>
    {/if}
    {/if}
  </div>
{/if}
