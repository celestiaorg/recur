<script lang="ts">
  import { tick } from 'svelte'
  import { view, prs, selectedPRId, go, openWorkspace, prDetailTab, observatoryJump } from '../lib/stores'
  import { service } from '../lib/api'
  import type { Commit, PRComment } from '../lib/types'
  import DiffFile from '../components/DiffFile.svelte'
  import CommitRow from '../components/CommitRow.svelte'
  import ReviewFileList from '../components/ReviewFileList.svelte'
  import ResizeHandle from '../components/ResizeHandle.svelte'
  import MarkdownHtml from '../components/MarkdownHtml.svelte'
  import CommentReactions from '../components/CommentReactions.svelte'
  import Icon, { type IconName } from '../components/Icon.svelte'
  import { commentPermalink, githubCommitUrl } from '../lib/comment-permalink'
  import { commentHeaderVerb, reviewDecision } from '../lib/comment-header'
  import { commentsForPath, isTimelineRoot, reviewChildren, threadReplies, unresolvedLineNotesByPath } from '../lib/comment-thread'
  import { clipCommentHunk, commentHunkPathLabel, parseDiffHunk } from '../lib/comment-hunk'
  import { checkGroupCollapsed, checkRunHref, checksBoxByline, checksHeading, checksRollup, githubPRChecksTabUrl, groupChecks, isPendingCheck, isSkippedCheck } from '../lib/pr-checks'
  import { prChromeColor, prLifecycle, prStatusLabel } from '../lib/colors'
  import {
    readDiffLayout, writeDiffLayout,
    readFileListMode, writeFileListMode,
    readReviewSidebar, writeReviewSidebar,
    type DiffLayout, type FileListMode,
  } from '../lib/review-diff'
  import { matchFindingPath } from '../lib/finding-paths'
  import { sortFilesByChangedTree } from '../lib/changed-tree'
  import { paneScrollTo, pickDiffJumpEl } from '../lib/review-scroll'
  import { commitByline, commitSha } from '../lib/commit-meta'
  import { fadeScrollTop } from '../lib/scroll-fade'
  import { stickyFolderFades } from '../lib/tree-sticky'
  import { overlayScrollbar } from '../lib/overlay-scrollbar'

  type PRTab = 'conv' | 'commits' | 'files' | 'checks'
  const prtab = $derived($prDetailTab)
  let commitIdx = $state<number | null>(null)
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
  let highlightLine = $state(0)
  let collapsedFiles = $state<Set<string>>(new Set())
  let diffscrollEl = $state<HTMLDivElement | undefined>(undefined)
  const fileEls = new Map<string, HTMLElement>()
  let spySuppressUntil = 0
  let tocReveal = $state(0)
  let filterText = $state('')
  let commentsOnly = $state(false)

  const pr = $derived($prs.find((p) => p.id === $selectedPRId))
  const prCommits = $derived(pr?.commits ?? [])
  const detailFiles = $derived(sortFilesByChangedTree(commitIdx != null ? prCommits[commitIdx]?.files ?? [] : []))
  const reviewFiles = $derived(pr?.files ?? [])
  const threadItems = $derived(pr?.comments ?? [])
  const description = $derived(threadItems.find((c) => c.kind === 'description'))
  const timeline = $derived(threadItems.filter(isTimelineRoot))
  const commentsByPath = $derived(unresolvedLineNotesByPath(threadItems, reviewFiles.map((f) => f.path)))
  const openCommentFiles = $derived(commentsByPath.size)
  const filterQ = $derived(filterText.trim().toLowerCase())
  const matchedFiles = $derived.by(() => {
    let list = filterQ ? reviewFiles.filter((f) => f.path.toLowerCase().includes(filterQ) || (f.oldPath ?? '').toLowerCase().includes(filterQ)) : reviewFiles
    if (commentsOnly) list = list.filter((f) => commentsForPath(commentsByPath, f.path).length > 0)
    return sortFilesByChangedTree(list)
  })
  const matchedAdd = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.add ?? 0), 0))
  const matchedDel = $derived(matchedFiles.reduce((n, f) => n + (f.diff?.del ?? 0), 0))
  const anyExpanded = $derived(matchedFiles.some((f) => !collapsedFiles.has(f.path)))
  const isGitHubPR = $derived(pr?.source === 'gh')
  const prLife = $derived(prLifecycle(pr))
  const checkGroups = $derived(groupChecks(pr?.checks ?? []))
  const checksSummary = $derived(checksRollup(pr?.checks, { checksOk: pr?.checksOk, hasChecks: pr?.hasChecks }))
  const checksPassedCount = $derived((pr?.checks ?? []).filter((c) => c.passed).length)
  const checksTotal = $derived(pr?.checks?.length ?? 0)
  const checksSummaryHref = $derived(isGitHubPR ? githubPRChecksTabUrl(pr?.url) : '')
  let collapsedCheckGroups = $state<Record<string, boolean>>({})

  function openInWorkspace(path: string, line?: number) {
    if (!pr?.workspaceId) return
    observatoryJump.set({ path, line, workspaceId: pr.workspaceId, preferDiff: true })
    openWorkspace(pr.workspaceId)
  }

  function toggleCollapsed(path: string, next: boolean) {
    const set = new Set(collapsedFiles)
    if (next) set.add(path)
    else set.delete(path)
    collapsedFiles = set
  }

  function registerFileEl(path: string) {
    return (el: HTMLElement) => {
      fileEls.set(path, el)
      return () => fileEls.delete(path)
    }
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

  function toggleFoldAll() {
    collapsedFiles = anyExpanded ? new Set(matchedFiles.map((f) => f.path)) : new Set()
  }

  $effect(() => {
    const paths = matchedFiles.map((f) => f.path)
    if (!paths.includes(selectedPath)) selectedPath = paths[0] ?? ''
  })

  async function selectPath(path: string, opts?: { highlight?: number }) {
    selectedPath = path
    highlightLine = opts?.highlight ?? 0
    if (collapsedFiles.has(path)) toggleCollapsed(path, false)
    spySuppressUntil = performance.now() + 1400
    await tick()
    const root = diffscrollEl
    const fileEl = fileEls.get(path)
    if (!root || !fileEl) return
    root.scrollTo({ top: fileEl.offsetTop, behavior: 'instant' })
    if (highlightLine <= 0) return
    await tick()
    const target = pickDiffJumpEl(fileEl, highlightLine)
    if (target && target !== fileEl) paneScrollTo(root, target)
  }

  function jumpToImpact(path: string, line?: number) {
    const resolved = matchFindingPath(path, reviewFiles.map((f) => f.path))
    if (!resolved) return
    setTab('files')
    void selectPath(resolved, { highlight: line || 0 })
  }

  // Pull the full PR record (diff, commits, thread) into the prs store.
  $effect(() => {
    if ($view !== 'prdetail') return
    const id = $selectedPRId
    if (!id) {
      go('pr')
      return
    }
    void service.GetPR(id).then((full) => {
      if (full?.id) prs.update((arr) => arr.map((p) => (p.id === full.id ? { ...p, ...full } : p)))
    })
  })

  $effect(() => {
    void $selectedPRId
    collapsedCheckGroups = {}
    commitIdx = null
    filterText = ''
    commentsOnly = false
    prDetailTab.set('conv')
  })

  function setTab(t: PRTab) {
    prDetailTab.set(t)
    if (t === 'commits') commitIdx = null
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
    commitIdx = i
  }

  function checkIcon(c: { passed: boolean; status?: string; detail?: string }): { name: IconName; color: string; spin?: boolean } {
    if (isSkippedCheck(c)) return { name: 'circle-slash', color: 'var(--muted)' }
    if (c.passed) return { name: 'check', color: 'var(--emerald)' }
    if (isPendingCheck(c)) return { name: 'loader', color: 'var(--amber)', spin: true }
    return { name: 'x', color: 'var(--coral)' }
  }

  function toggleCheckGroup(key: string, compact: boolean) {
    const next = !checkGroupCollapsed(key, collapsedCheckGroups, compact)
    collapsedCheckGroups = { ...collapsedCheckGroups, [key]: next }
  }

  function commitTitle(c: PRComment): string {
    return (c.bodyMd ?? '').replace(/^committed\s+/i, '').replace(/\s+\([0-9a-f]{7,}\)\s*$/i, '')
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
  async function copyCommitSha(e: MouseEvent, c: Commit) {
    e.stopPropagation()
    const sha = commitSha(c)
    if (!sha) return
    await navigator.clipboard.writeText(sha)
    copiedSha = sha
    setTimeout(() => { if (copiedSha === sha) copiedSha = '' }, 1200)
  }

  function actionBehalf(c: PRComment): { verb: string; whom: string } | null {
    const m = /^(started reviewing|started work|finished reviewing|finished work|stopped work) on behalf of (.+?)(?: due to an error)?$/.exec(c.bodyMd ?? '')
    if (!m) return null
    return { verb: m[1], whom: m[2] }
  }

  function toggleKidHunk(child: PRComment) {
    if (child.resolved) openResolved[child.id] = !openResolved[child.id]
    else collapsedCtx[child.id] = !collapsedCtx[child.id]
  }
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
    <button type="button" class:on={diffLayout === 'split'} title="Side by side" aria-label="Side-by-side diff" aria-pressed={diffLayout === 'split'} onclick={() => setDiffLayout('split')}><Icon name="columns-2" /></button>
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

<section class="view" class:on={$view === 'prdetail'} id="v-prdetail">
  <div class="prh">
    {#if pr}
    {@const chrome = prChromeColor(pr)}
    <div class="prh-t">
      <span class="statepill" class:merged={prLife === 'merged'} class:closed={prLife === 'closed'} style="background:color-mix(in srgb, {chrome} 14%, transparent);color:{chrome}"><span class="d" style="background:{chrome}"></span>{prStatusLabel(pr)}</span>
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
      <span class="prh-title">{pr.title}</span>
    </div>
    {/if}
  </div>

  <div class="prd-split">
    <div class="prd-col" tabindex="-1">
  <div class="prtabs">
    <button class="prtab lit" class:on={prtab === 'conv'} onclick={() => setTab('conv')}><Icon name="message" />Conversation<span class="tc">{timeline.length + (description ? 1 : 0)}</span></button>
    <button class="prtab lit" class:on={prtab === 'commits'} onclick={() => setTab('commits')}><Icon name="git-commit" />Commits<span class="tc">{pr?.commits?.length ?? 0}</span></button>
    <button class="prtab lit" class:on={prtab === 'files'} onclick={() => setTab('files')}><Icon name="file-diff" />Files changed<span class="tc">{pr?.files?.length || pr?.fileCount || 0}</span></button>
    <button class="prtab lit" class:on={prtab === 'checks'} onclick={() => setTab('checks')}><span class="prtab-check" class:spin={checksSummary.kind === 'pending'} data-kind={checksSummary.kind} style:color={checksSummary.color}><Icon name={checksSummary.icon} /></span>Checks<span class="tc" style:color={checksSummary.kind === 'none' ? undefined : checksSummary.color}>{checksPassedCount}/{checksTotal}</span></button>
  </div>

  <!-- Conversation -->
  <div class="prpane" class:on={prtab === 'conv'} {@attach fadeScrollTop}>
    <div class="conv-wrap">
      {#if description}
        <div class="cmt desc">
          {@render personAvatar(description)}
          <div class="cc">
            <div class="ch">
              <span class="cn">{description.author}</span>
              {@render whenStamp(description, 'ct')}
            </div>
            {#if description.bodyMd}
              <MarkdownHtml text={description.bodyMd} class="chat-canvas cm-md" />
            {:else}
              <div class="cm muted">No description provided.</div>
            {/if}
            <CommentReactions comment={description} />
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
                  <span class="fl" role="button" tabindex="0" onclick={() => setTab('files')} onkeydown={(e) => e.key === 'Enter' && setTab('files')}>jump to the diff →</span>
                </div>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      {#if !isGitHubPR}
        <div class="card">
          <div class="card-h"><Icon name="refresh-ccw" />Verified in browser<span class="by">QA agent{pr?.when ? ` · ${pr.when}` : ''}</span></div>
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
                        {:else}
                          <div class="cmt-resolved-bar">
                            <span>Resolved</span>
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

      {@render checksPanel(true)}
    </div>
  </div>

  <!-- Commits -->
  <div class="prpane" class:on={prtab === 'commits'} {@attach fadeScrollTop}>
    <div class="commits-wrap" class:detail={commitIdx !== null}>
      {#if commitIdx === null}
        <div>
          <div class="commits-list">
            {#each prCommits as c, i (c.hash)}
              <CommitRow commit={c} onOpen={() => (commitIdx = i)}>
                {#snippet trailing()}
                  {#if c.verified}<span class="tl-verified">Verified</span>{/if}
                  {@render commitCheck(c.checkStatus)}
                {/snippet}
              </CommitRow>
            {/each}
          </div>
          <div class="hint" style="text-align:left;padding:14px 2px 0">{isGitHubPR ? 'Commits on this pull request. Click any to see its diff.' : 'Every commit is the loop\'s work — agent-authored, one per iteration. Click any to see its diff. The messages trace how it converged: build → fix on review → fix on review → tests.'}</div>
        </div>
      {:else}
        {@const c = prCommits[commitIdx]}
        {#if c}
          <div class="commit-nav">
            <button class="cback" onclick={() => (commitIdx = null)}>← all commits</button>
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
                    {c.authorRole}{#if c.iterLabel} · <span class="iter">{c.iterLabel}</span>{/if} · {detailFiles.length} file{detailFiles.length === 1 ? '' : 's'}
                  {:else}
                    {commitByline(c)} · {detailFiles.length} file{detailFiles.length === 1 ? '' : 's'}
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
          {#each detailFiles as f (f.path)}
            <div class="cfile" data-path={f.path}>
              <DiffFile file={f} layout={diffLayout} collapsible onOpenFile={pr?.workspaceId ? openInWorkspace : undefined} openFileTitle="Open in workspace" />
            </div>
          {:else}
            <div class="empty"><div class="et">No files in this commit</div></div>
          {/each}
        {/if}
      {/if}
    </div>
  </div>

  <!-- Files changed -->
  <div class="prpane flexpane" class:on={prtab === 'files'} {@attach fadeScrollTop}>
    <div class="fl-head" class:no-sidebar={!sidebarOpen}>
      {@render filesViewSeg()}
      <div class="fl-head-main">
        {#if filterQ || commentsOnly}
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
      <div class="diffscroll" tabindex="-1" bind:this={diffscrollEl} onscroll={onDiffScroll}>
        {#if (filterQ || commentsOnly) && !matchedFiles.length}
          <div class="empty">
            <div class="et">No files match — <button type="button" class="linkish" onclick={() => { filterText = ''; commentsOnly = false }}>clear filter</button></div>
          </div>
        {:else}
        {#each matchedFiles as f (f.path)}
          <div class="cfile" data-path={f.path} {@attach registerFileEl(f.path)}>
            <DiffFile
              file={f}
              highlightLine={f.path === selectedPath ? highlightLine : 0}
              layout={diffLayout}
              collapsible
              collapsed={collapsedFiles.has(f.path)}
              onToggleCollapse={(c) => toggleCollapsed(f.path, c)}
              onOpenFile={pr?.workspaceId ? openInWorkspace : undefined}
              openFileTitle="Open in workspace"
            />
          </div>
        {/each}
        {/if}
      </div>
    </div>
  </div>

  <!-- Checks -->
  <div class="prpane" class:on={prtab === 'checks'} {@attach fadeScrollTop}>
    <div class="conv-wrap" style="max-width:680px">
      {@render checksPanel()}
      <div class="hint" style="text-align:left;padding:0 2px">{pr?.source === 'gh' ? 'These are the GitHub status checks on this PR.' : 'Verify-in-browser is unique to Recur — the QA agent actually drove the running app. Evidence is in the Conversation tab.'}</div>
    </div>
  </div>

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

{#snippet lineNote(c: PRComment)}
  <div class="cmt-kid-note">
    {@render personAvatar(c)}
    <div class="cmt-kid-cc">
      <div class="ch">
        <span class="cn">{c.author}</span>
        {@render whenStamp(c, 'ct')}
      </div>
      {@render commentBody(c, true)}
    </div>
  </div>
{/snippet}

{#snippet commentBody(c: PRComment, locShown = false)}
  {#if c.bodyMd}
    {#if !locShown && c.bodyHtml}<div class="cm loc">{@html c.bodyHtml}</div>{/if}
    <MarkdownHtml text={c.bodyMd} class="chat-canvas cm-md" />
  {:else if c.bodyHtml && !locShown}
    <div class="cm">{@html c.bodyHtml}</div>
  {/if}
  {#if c.reactHtml}<span class="react">{@html c.reactHtml}</span>{/if}
  <CommentReactions comment={c} />
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
                <span class="cd">{c.detail}</span>
              </div>
            {/each}
          {/if}
        {/if}
      {/each}
    </div>
  </div>
{/snippet}
