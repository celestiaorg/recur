<script lang="ts">
  // Review scenes (`review` in scenes.json): ReviewSurface as the Observatory mounts it over the
  // fixtures in `content/review.json` (capture.spec answers DiffVsBase / ListCommits with them),
  // and a PR-mode DiffFile in PR detail's files pane for the line-note thread. The large scene
  // generates its files the way `recur_view_review::gallery` does.
  import { onMount, tick } from 'svelte'
  import ReviewSurface from '../../../../../frontend/src/components/ReviewSurface.svelte'
  import DiffFile from '../../../../../frontend/src/components/DiffFile.svelte'
  import { prs, selectedPRId } from '../../../../../frontend/src/lib/stores'
  import seedJson from '../../../../../frontend/e2e/fixtures/seed.json'
  import fixtures from '../../content/review.json'

  let { name, onready }: { name: string; onready: () => void } = $props()
  const variant = name.split('/').slice(1).join('/').replace(/-paper$/, '')
  let mounted = $state(false)
  let tab = $state<'changes' | 'commits'>(variant === 'commits' ? 'commits' : 'changes')

  const findingsByPath = new Map<string, any[]>()
  for (const f of fixtures.findings) findingsByPath.set(f.file, [...(findingsByPath.get(f.file) ?? []), f])
  const dirtyPaths = new Set<string>(fixtures.dirty)

  const seed = seedJson as any
  const pr = structuredClone(seed.prById['482'])
  pr.comments = [...(pr.comments ?? []), ...fixtures.threadReplies]
  const oauth = pr.files.find((f: any) => f.path === 'src/auth/oauth.ts')

  const frames = (n: number) =>
    new Promise<void>((r) => {
      const step = () => (--n > 0 ? requestAnimationFrame(step) : r())
      requestAnimationFrame(step)
    })

  onMount(async () => {
    if (variant === 'split') localStorage.setItem('recur.prDiffLayout', 'split')
    if (variant === 'filelist') localStorage.setItem('recur.reviewFileListMode', 'flat')
    if (variant === 'thread') {
      prs.set([pr])
      selectedPRId.set('482')
    }
    await document.fonts.ready
    mounted = true
    await tick()
    await new Promise((r) => setTimeout(r, 300))
    await frames(3)
    onready()
  })
</script>

<div class="review-scene">
  {#if !mounted}
  {:else if variant === 'thread'}
    <div class="prpane flexpane on">
      <div class="diffwrap no-sidebar">
        <div class="diffscroll">
          <div class="cfile">
            <DiffFile file={oauth} layout="unified" collapsible />
          </div>
        </div>
      </div>
    </div>
  {:else}
    <div class="obs-review-host">
      <ReviewSurface
        projectId="storefront"
        workspaceId="LOOP-2231"
        baseLabel="main"
        active
        {tab}
        count={variant === 'large' ? fixtures.large.files : fixtures.files.length}
        onTabChange={(t) => (tab = t)}
        onOpenFile={() => {}}
        onAddFinding={async () => true}
        findingsByPath={variant === 'large' ? undefined : findingsByPath}
        onOpenFinding={() => {}}
        onShowFindings={() => {}}
        dirtyPaths={variant === 'large' ? undefined : dirtyPaths}
        workingDirtyCount={variant === 'commits' ? 2 : 0}
      />
    </div>
  {/if}
</div>

<style>
  .review-scene { position: absolute; inset: 0; display: flex; flex-direction: column; }
  .review-scene > :global(*) { flex: 1; min-height: 0; }
</style>
