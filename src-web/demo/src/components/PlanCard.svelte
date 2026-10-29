<script lang="ts">
  // A filed plan as a compact card: summary plus the step checklist with file
  // chips and dependencies. Step status is read live from the workspace's
  // current plan (by step id) so the card ticks along as the board runs; the
  // full body lives in the Plan panel, not here.
  import { workspaces, observatoryJump, openWorkspace } from '../lib/stores'
  import Icon from './Icon.svelte'

  type PlanCardStep = { id: string; title: string; files?: string[]; dependsOn?: string[] }
  type PlanCardData = { summary: string; steps: PlanCardStep[]; source?: string }

  let { data, fallback = '', workspaceId = '' }: { data: string; fallback?: string; workspaceId?: string } = $props()

  const plan = $derived.by((): PlanCardData | null => {
    try {
      const p = JSON.parse(data) as PlanCardData
      return p && Array.isArray(p.steps) ? p : null
    } catch {
      return null
    }
  })

  const currentSteps = $derived.by(() => {
    const ws = workspaceId ? $workspaces.find((w) => w.id === workspaceId) : undefined
    return ws?.plan?.steps ?? []
  })
  /** Live status is only for the card that matches the current plan revision.
   *  Replans reuse S1..Sn, so older cards must not inherit the new ids. */
  const isCurrentPlan = $derived.by(() => {
    if (!plan) return false
    const cur = currentSteps
    if (cur.length !== plan.steps.length) return false
    return plan.steps.every((s) => cur.some((c) => c.id === s.id && c.title === s.title))
  })

  const liveStatus = $derived.by(() => {
    const m = new Map<string, string>()
    if (!isCurrentPlan) return m
    for (const s of currentSteps) m.set(s.id, s.status)
    return m
  })
  /** Step id → child workspace working it right now (still open). */
  const liveChild = $derived.by(() => {
    const m = new Map<string, string>()
    if (!isCurrentPlan) return m
    for (const s of currentSteps) {
      if (s.status !== 'doing' || !s.workspace) continue
      const child = $workspaces.find((w) => w.id === s.workspace)
      if (child && !child.archived) m.set(s.id, s.workspace)
    }
    return m
  })

  const doneCount = $derived(plan ? plan.steps.filter((s) => {
    const st = liveStatus.get(s.id)
    return st === 'done' || st === 'skipped'
  }).length : 0)

  /** The Plan panel shows the live plan for this workspace; link only when
   *  the card belongs to a workspace that still has one. */
  const canOpen = $derived(!!workspaceId && !!$workspaces.find((w) => w.id === workspaceId)?.plan)
  function openPlan(tab: 'details' | 'todos' = 'details') {
    if (canOpen) observatoryJump.set({ panel: 'plan', workspaceId, planTab: tab })
  }

  function fileName(p: string): string {
    const i = p.lastIndexOf('/')
    return i >= 0 ? p.slice(i + 1) : p
  }
</script>

{#if plan}
  <div class="plan-card">
    <div class="plan-card-head">
      <span class="ico" aria-hidden="true"><Icon name="list-todo" /></span>
      <span class="plan-card-title">Plan</span>
      <span class="plan-card-sum">{#if isCurrentPlan}{doneCount} of {plan.steps.length} step{plan.steps.length === 1 ? '' : 's'} done{:else}{plan.steps.length} step{plan.steps.length === 1 ? '' : 's'}{/if}</span>
      {#if canOpen}
        <button type="button" class="plan-card-open" title="Open the Plan panel" onclick={() => openPlan('details')}>Open plan</button>
      {/if}
    </div>
    {#if plan.summary}<p class="plan-card-summary">{plan.summary}</p>{/if}
    <ol class="plan-card-steps">
      {#each plan.steps as s (s.id)}
        {@const st = isCurrentPlan ? (liveStatus.get(s.id) ?? 'pending') : ''}
        <li class="plan-card-step {st}">
          <span class="tstep">{s.id}</span>
          <span class="plan-card-step-body">
            {#if canOpen}
              <button type="button" class="plan-card-step-title plan-card-step-link" title="Open the Todos pane" onclick={() => openPlan('todos')}>{s.title}</button>
            {:else}
              <span class="plan-card-step-title">{s.title}</span>
            {/if}
            {#if (s.files?.length ?? 0) > 0 || (s.dependsOn?.length ?? 0) > 0}
              <span class="plan-card-meta">
                {#each s.files ?? [] as f (f)}<span class="plan-card-file" title={f}>{fileName(f)}</span>{/each}
                {#if s.dependsOn?.length}<span class="plan-card-after">after {s.dependsOn.join(', ')}</span>{/if}
              </span>
            {/if}
          </span>
          <span class="plan-card-state" aria-label={st}>
            {#if st === 'done'}<Icon name="circle-check" />
            {:else if st === 'skipped'}<Icon name="circle-slash" />
            {:else if st === 'doing' && liveChild.has(s.id)}
              <button type="button" class="plan-card-child" title="Open the child workspace working this step" onclick={() => openWorkspace(liveChild.get(s.id)!)}><span class="sd"></span><Icon name="git-branch" /></button>
            {:else if st === 'doing'}<span class="sd"></span>
            {/if}
          </span>
        </li>
      {/each}
    </ol>
  </div>
{:else}
  <div class="tc-head static"><span class="tc-meta">{@html fallback}</span></div>
{/if}
