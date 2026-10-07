<script lang="ts">
  // Plan panel: Details (headline + brief; Edit lives in this tab's footer),
  // Attachments (same list as the Artifacts pane), and Todos (living checklist
  // plus agent TodoWrite lists). The Observatory pane header owns the tabs;
  // edits ride submit_plan / update_plan_step / edit_plan.
  import { workspaces, pushToast, setWorkspaceSeed, settings, openWorkspace, childTranscriptRequest } from '../lib/stores'
  import { WorkspacesService, call, setWorkspacesFromList } from '../lib/api'
  import { agColor } from '../lib/colors'
  import {
    engageLoopWith, workspaceLoopId, workspacePolicyId,
  } from '../lib/loops'
  import { latestTodosFromMessages } from '../lib/todos'
  import { fadeScrollEdges } from '../lib/scroll-fade'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import { tick, untrack } from 'svelte'
  import type { FeedSess } from '../lib/loop-cockpit'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import ArtifactsPanel from './ArtifactsPanel.svelte'
  import MiniScriptEditor from './MiniScriptEditor.svelte'

  let {
    workspaceId = '',
    sessions = [] as FeedSess[],
    onOpenArtifact,
    tab = $bindable('details'),
    editing = $bindable(false),
    selectedArtifactId = '',
  }: {
    workspaceId?: string
    sessions?: FeedSess[]
    onOpenArtifact?: (a: { id?: string; title?: string; kind?: string; path?: string }) => void
    /** Details / Attachments / Todos — owned by the Observatory pane header. */
    tab?: 'details' | 'todos' | 'attachments'
    editing?: boolean
    /** Artifact currently open in the editor — highlights the matching attachment row. */
    selectedArtifactId?: string
  } = $props()

  const ws = $derived($workspaces.find((w) => w.id === workspaceId))
  const plan = $derived(ws?.plan ?? null)
  const pickLoopId = $derived(workspaceLoopId(ws, $settings))
  const pickPolicyId = $derived(workspacePolicyId(ws, $settings))
  const planAtts = $derived(plan?.attachments ?? [])
  const planSteps = $derived(plan?.steps ?? [])

  const todoGroups = $derived(
    sessions
      .map((s) => ({ code: s.code, name: s.name, todos: latestTodosFromMessages(s.messages) }))
      .filter((g) => g.todos.length),
  )

  const stepsDone = $derived(planSteps.filter((s) => s.status === 'done').length)
  const stepsSkipped = $derived(planSteps.filter((s) => s.status === 'skipped').length)
  const stepsDoing = $derived(planSteps.filter((s) => s.status === 'doing').length)
  const stepsTotal = $derived(planSteps.length)
  const stepsResolved = $derived(stepsDone + stepsSkipped)
  function stepsShare(n: number): string {
    return stepsTotal ? `${(100 * n) / stepsTotal}%` : '0%'
  }
  const stepsBarLabel = $derived.by(() => {
    const parts = [`${stepsDone} done`]
    if (stepsSkipped) parts.push(`${stepsSkipped} skipped`)
    if (stepsDoing) parts.push(`${stepsDoing} in progress`)
    return `${parts.join(', ')} of ${stepsTotal}`
  })

  type StepDraft = { key: string; title: string; status: string; note: string; workspace: string }

  let summaryDraft = $state('')
  let bodyDraft = $state('')
  let todoDrafts = $state<StepDraft[]>([])
  let draftsReady = false
  let lastTodoWs = ''
  let lastTodoSig = ''
  let stepFocusKey = $state('')
  let stepsSave: Promise<boolean> | null = null
  let summaryEl = $state<HTMLTextAreaElement | undefined>(undefined)
  let bodyEditorEl = $state<{ focus: () => void } | undefined>(undefined)
  let stepKeySeq = 0
  const fadePlanBody = fadeScrollEdges()

  function planStepsSig(): string {
    return planSteps.map((s) => `${s.id}\0${s.title}\0${s.status}\0${s.note ?? ''}\0${s.workspace ?? ''}`).join('\n')
  }

  function draftsFromPlan(): StepDraft[] {
    return planSteps.map((s) => ({
      key: s.id,
      title: s.title,
      status: s.status,
      note: s.note ?? '',
      workspace: s.workspace ?? '',
    }))
  }

  todoDrafts = draftsFromPlan()
  lastTodoWs = workspaceId
  lastTodoSig = planStepsSig()
  draftsReady = true

  // Keep local drafts in sync with the workspace plan, but don't clobber an
  // in-progress title or an unsaved empty row the human just added.
  $effect(() => {
    const id = workspaceId
    const sig = planStepsSig()
    const focus = stepFocusKey
    if (id === lastTodoWs && sig === lastTodoSig) return
    if (focus && id === lastTodoWs) return
    const keepNew = id === lastTodoWs
      ? untrack(() => todoDrafts.filter((d) => {
          if (!d.key.startsWith('new-')) return false
          const t = d.title.trim()
          if (!t) return true
          return !planSteps.some((s) => s.title === t)
        }))
      : []
    lastTodoWs = id
    lastTodoSig = sig
    draftsReady = true
    todoDrafts = [...draftsFromPlan(), ...keepNew]
  })

  function newStepDraft(partial?: Partial<StepDraft>): StepDraft {
    stepKeySeq += 1
    return {
      key: partial?.key ?? `new-${stepKeySeq}`,
      title: partial?.title ?? '',
      status: partial?.status ?? 'pending',
      note: partial?.note ?? '',
      workspace: partial?.workspace ?? '',
    }
  }

  function stepsForSubmit(): { titles: string[]; ids: string[] } {
    if (draftsReady) {
      const rows = todoDrafts.filter((s) => s.title.trim())
      return {
        titles: rows.map((s) => s.title.trim()),
        ids: rows.map((s) => (s.key.startsWith('new-') ? '' : s.key)),
      }
    }
    return {
      titles: planSteps.map((s) => s.title),
      ids: planSteps.map((s) => s.id),
    }
  }

  function titlesUnchanged(titles: string[]): boolean {
    return titles.length === planSteps.length && titles.every((t, i) => t === planSteps[i]?.title)
  }

  function restoreTodoDrafts() {
    todoDrafts = draftsFromPlan()
  }

  async function persistStepTitles(): Promise<boolean> {
    while (stepsSave) await stepsSave
    const run = (async () => {
      if (!plan) return false
      const { titles, ids } = stepsForSubmit()
      if (titlesUnchanged(titles)) return true
      const wasApproved = plan.approved
      const status = await call(
        WorkspacesService.SubmitPlan(workspaceId, plan.summary, plan.body ?? '', titles, ids),
        'Could not update the checklist',
      )
      if (!status) {
        restoreTodoDrafts()
        return false
      }
      if (status.startsWith('Refused')) {
        pushToast('error', status)
        restoreTodoDrafts()
        return false
      }
      await refreshWorkspace()
      if (wasApproved) pushToast('success', 'Checklist updated — approval reset')
      return true
    })()
    stepsSave = run
    try {
      return await run
    } finally {
      if (stepsSave === run) stepsSave = null
    }
  }

  function focusStep(key: string) {
    void tick().then(() => {
      const el = document.querySelector<HTMLTextAreaElement>(`#plan-page [data-step-key="${CSS.escape(key)}"]`)
      el?.focus()
    })
  }

  function growSummary(text: string) {
    return (el: HTMLTextAreaElement) => {
      void text
      el.style.height = '0px'
      el.style.height = `${el.scrollHeight}px`
    }
  }

  function onSummaryKeydown(e: KeyboardEvent) {
    if (e.key !== 'Enter' || e.shiftKey || isCmdOrCtrl(e)) return
    e.preventDefault()
    bodyEditorEl?.focus()
  }

  function growStepTitle(title: string) {
    return (el: HTMLTextAreaElement) => {
      void title
      el.style.height = '0px'
      el.style.height = `${el.scrollHeight}px`
    }
  }

  function addStep(afterKey?: string) {
    if (!plan) {
      pushToast('error', 'File a plan first, then add steps.')
      return
    }
    const row = newStepDraft()
    if (afterKey) {
      const i = todoDrafts.findIndex((s) => s.key === afterKey)
      if (i >= 0) todoDrafts.splice(i + 1, 0, row)
      else todoDrafts.push(row)
    } else {
      todoDrafts.push(row)
    }
    focusStep(row.key)
  }

  function removeStep(key: string) {
    const i = todoDrafts.findIndex((s) => s.key === key)
    if (i < 0) return
    todoDrafts.splice(i, 1)
    const next = todoDrafts[Math.min(i, todoDrafts.length - 1)]
    if (next) focusStep(next.key)
    else stepFocusKey = ''
    if (!key.startsWith('new-')) void persistStepTitles()
  }

  function onStepKeydown(e: KeyboardEvent, key: string) {
    if (e.key === 'Enter' && !e.shiftKey && !isCmdOrCtrl(e)) {
      e.preventDefault()
      addStep(key)
      void persistStepTitles()
      return
    }
    if (e.key !== 'Backspace' || isCmdOrCtrl(e) || e.altKey) return
    const input = e.currentTarget
    if (!(input instanceof HTMLTextAreaElement) || input.value) return
    e.preventDefault()
    removeStep(key)
  }

  function startPlanEdit() {
    summaryDraft = plan?.summary ?? ''
    bodyDraft = plan?.body ?? (!plan ? (ws?.definitionOfDone ?? '') : '')
    tab = 'details'
    editing = true
    void tick().then(() => summaryEl?.focus())
  }
  function cancelPlanEdit() {
    editing = false
  }
  async function refreshWorkspace() {
    const fresh = await WorkspacesService.ListWorkspaces().catch(() => null)
    if (fresh) setWorkspacesFromList(fresh)
  }
  async function savePlan() {
    if (stepsSave) await stepsSave
    const summary = summaryDraft.trim() || bodyDraft.trim().split('\n').find((l) => l.trim())?.trim() || ''
    const body = bodyDraft.trim()
    const { titles, ids } = stepsForSubmit()
    if (!summary) {
      pushToast('error', 'The plan needs a summary.')
      return
    }
    if (plan) {
      const status = await call(WorkspacesService.SubmitPlan(workspaceId, summary, body, titles, ids), 'Could not update the plan')
      if (!status) return
      if (status.startsWith('Refused')) {
        pushToast('error', status)
        return
      }
      await refreshWorkspace()
      editing = false
      pushToast('success', 'Plan updated — approval reset')
      return
    }
    const prose = [summary, body].filter(Boolean).join('\n\n')
    const status = await call(WorkspacesService.UpdatePlan(workspaceId, prose), 'Could not update the plan')
    if (!status) return
    if (status.startsWith('Refused')) {
      pushToast('error', status)
      return
    }
    workspaces.update((arr) => arr.map((l) => (l.id === workspaceId ? { ...l, definitionOfDone: prose } : l)))
    editing = false
    pushToast('success', 'Plan updated — logged as a deviation')
  }

  const STEP_CYCLE: Record<string, string> = { pending: 'doing', doing: 'done', done: 'skipped', skipped: 'pending' }
  async function cycleStep(stepID: string, cur: string) {
    const next = STEP_CYCLE[cur] ?? 'doing'
    const status = await call(WorkspacesService.UpdatePlanStep(workspaceId, stepID, next), 'Could not update the step')
    if (!status) return
    if (status.startsWith('Refused')) {
      pushToast('error', status)
      return
    }
    await refreshWorkspace()
  }

  export async function approvePlan() {
    const status = await call(WorkspacesService.ApprovePlan(workspaceId), 'Could not approve the plan')
    if (!status) return
    if (status.startsWith('Refused')) {
      pushToast('error', status)
      return
    }
    await refreshWorkspace()
    pushToast('success', 'Plan approved')
  }

  export async function runAsLoop() {
    const goal = plan?.summary || ws?.definitionOfDone || ''
    const status = await call(engageLoopWith(workspaceId, goal, pickLoopId, pickPolicyId), 'Could not engage the loop')
    if (!status) return
    if (status.startsWith('Refused')) {
      pushToast('error', status)
      return
    }
    await refreshWorkspace()
    pushToast('success', status)
  }
  export function executeInChat() {
    setWorkspaceSeed({
      id: workspaceId,
      text: 'Implement the approved plan from the Plan panel — follow its steps and satisfy its definition of done.',
      mode: 'agent',
    })
  }

  function onEditKeydown(e: KeyboardEvent) {
    if (!editing) return
    if (e.key === 'Escape') {
      e.preventDefault()
      editing = false
      return
    }
    if (e.key === 'Enter' && isCmdOrCtrl(e)) {
      e.preventDefault()
      void savePlan()
    }
  }

  function stepStatusLabel(status: string): string {
    switch (status) {
      case 'doing': return 'in progress'
      case 'done': return 'done'
      case 'skipped': return 'skipped'
      default: return 'pending'
    }
  }
</script>

{#snippet stepMark(status: string)}
  <span
    class="plan-step-check"
    class:doing={status === 'doing'}
    class:done={status === 'done'}
    class:skipped={status === 'skipped'}
    aria-hidden="true"
  >
    {#if status === 'done'}
      <Icon name="check" strokeWidth={3} />
    {:else if status === 'skipped'}
      <Icon name="minus" />
    {/if}
  </span>
{/snippet}

{#snippet stepCheck(id: string, status: string)}
  <button
    type="button"
    class="plan-step-check"
    class:doing={status === 'doing'}
    class:done={status === 'done'}
    class:skipped={status === 'skipped'}
    title={`${stepStatusLabel(status)} — click to change`}
    aria-label={`Step ${id}: ${stepStatusLabel(status)} — click to change`}
    onclick={() => cycleStep(id, status)}
  >
    {#if status === 'done'}
      <Icon name="check" strokeWidth={3} />
    {:else if status === 'skipped'}
      <Icon name="minus" />
    {/if}
  </button>
{/snippet}

<div class="ovw plan-pane">
  <div
    class="plan-page"
    class:editing={editing && tab === 'details'}
    id="plan-page"
    role="tabpanel"
    aria-labelledby={`plan-tab-${tab}`}
  >
    {#if tab === 'details'}
      <div class="plan-details-pane" onkeydown={onEditKeydown}>
        {#if editing}
          <div class="plan-edit" role="form" aria-label="Edit plan">
            <div class="plan-edit-headline">
              <span class="plan-edit-headline-gutter" aria-hidden="true"></span>
              <textarea
                class="plan-ta plan-edit-summary"
                bind:this={summaryEl}
                bind:value={summaryDraft}
                rows="1"
                aria-label="Headline"
                placeholder="What this change does, and why"
                {@attach growSummary(summaryDraft)}
                onkeydown={onSummaryKeydown}
              ></textarea>
            </div>
            <div class="plan-edit-body">
              <MiniScriptEditor
                bind:this={bodyEditorEl}
                lang="md"
                value={bodyDraft}
                ariaLabel="Plan"
                placeholder="Relevant files, approach, edge cases, definition of done"
                minLines={6}
                fill
                onChange={(v) => (bodyDraft = v)}
                onModEnter={() => void savePlan()}
              />
            </div>
          </div>
        {:else}
          {#if plan?.summary}
            <div class="plan-summary">{plan.summary}</div>
          {/if}
          <div class="plan-body-scroll" {@attach fadePlanBody}>
            {#if plan?.body}
              <MarkdownHtml text={plan.body} class="chat-canvas plan-md" />
            {:else if !plan && ws?.definitionOfDone}
              <MarkdownHtml text={ws.definitionOfDone} class="chat-canvas plan-md" />
            {:else if !plan}
              <div class="plan-empty">No plan yet — plan it in a Plan-mode chat, or let the loop’s planning turn write one.</div>
            {/if}
          </div>
        {/if}
        <div class="plan-edit-steps-foot">
          {#if editing}
            <span class="plan-foot-hint"><Kbd text="⌘↵" /> to save</span>
            <div class="plan-foot-actions">
              <button type="button" class="plan-edit-add" onclick={cancelPlanEdit}>Cancel</button>
              <button
                type="button"
                class="plan-foot-save"
                title="Save plan (⌘⏎)"
                onclick={() => void savePlan()}
              >Save</button>
            </div>
          {:else}
            <button type="button" class="plan-edit-add" onclick={startPlanEdit}>
              <Icon name="pencil" />Edit
            </button>
          {/if}
        </div>
      </div>
    {:else if tab === 'attachments'}
      <ArtifactsPanel
        {workspaceId}
        items={planAtts}
        selectedId={selectedArtifactId}
        empty="No attachments yet — pin artifacts or pasted images when you submit the plan."
        onOpen={(a) => onOpenArtifact?.(a)}
      />
    {:else}
      <div class="plan-todos-pane">
        {#if stepsTotal}
          <div class="plan-prog">
            <span class="plan-prog-label">{stepsResolved} of {stepsTotal} done</span>
            <div class="plan-prog-track" role="img" aria-label={stepsBarLabel} title={stepsBarLabel}>
              {#if stepsDone}
                <div class="plan-prog-fill done" style:width={stepsShare(stepsDone)}></div>
              {/if}
              {#if stepsSkipped}
                <div class="plan-prog-fill skipped" style:width={stepsShare(stepsSkipped)}></div>
              {/if}
              {#if stepsDoing}
                <div class="plan-prog-fill doing" style:width={stepsShare(stepsDoing)}></div>
              {/if}
            </div>
          </div>
        {/if}
        <div class="plan-todos-scroll">
          {#if todoDrafts.length}
            <ol class="plan-todo-list">
              {#each todoDrafts as row (row.key)}
                <li
                  class="plan-todo-row"
                  class:doing={row.status === 'doing'}
                  class:done={row.status === 'done'}
                  class:skipped={row.status === 'skipped'}
                >
                  {#if row.key.startsWith('new-')}
                    {@render stepMark(row.status)}
                  {:else}
                    {@render stepCheck(row.key, row.status)}
                  {/if}
                  <div class="plan-edit-step-copy">
                    <textarea
                      class="plan-step-title plan-todo-title"
                      data-step-key={row.key}
                      bind:value={row.title}
                      rows="1"
                      aria-label="Step title"
                      placeholder="What needs to happen"
                      {@attach growStepTitle(row.title)}
                      onfocus={() => (stepFocusKey = row.key)}
                      onblur={() => {
                        if (stepFocusKey === row.key) stepFocusKey = ''
                        void persistStepTitles()
                      }}
                      onkeydown={(e) => onStepKeydown(e, row.key)}
                    ></textarea>
                    {#if row.note}<span class="plan-step-note">{row.note}</span>{/if}
                    {#if row.workspace}
                      {@const childWs = $workspaces.find((w) => w.id === row.workspace)}
                      {@const childGone = !childWs || childWs.archived}
                      <button
                        type="button"
                        class="plan-step-child"
                        title={childGone ? 'Show the child workspace transcript in the loop chat' : 'Open the child workspace working this step'}
                        onmousedown={(e) => e.preventDefault()}
                        onclick={() => { if (childGone) childTranscriptRequest.set(row.workspace); else openWorkspace(row.workspace) }}
                      >
                        <Icon name={childGone ? 'git-merge' : 'git-branch'} />
                        {row.status === 'doing' ? 'Running in child workspace' : childGone ? 'Child transcript' : 'Child workspace'}
                      </button>
                    {/if}
                  </div>
                  <button
                    type="button"
                    class="exclude-rm"
                    title="Remove step"
                    aria-label="Remove step"
                    onmousedown={(e) => e.preventDefault()}
                    onclick={() => removeStep(row.key)}
                  >
                    <Icon name="trash" />
                  </button>
                </li>
              {/each}
            </ol>
          {:else}
            <div class="plan-empty">
              {#if plan}
                No checklist yet — add a step below.
              {:else}
                No checklist yet — file a plan first, then add steps here.
              {/if}
            </div>
          {/if}

          {#if todoGroups.length}
            <div class="plan-todo-sec">
              <h4>From agents</h4>
              {#each todoGroups as g (`${g.code}:${g.name}`)}
                <div class="plan-todo-ag">
                  <span class="sess-ava" style="background:{agColor[g.code] ?? 'var(--cyan)'}">{g.code}</span>
                  <span class="plan-todo-name">{g.name || g.code}</span>
                </div>
                <ul class="todos plan-todos">
                  {#each g.todos as td, ti (`${g.code}:${td.content}:${ti}`)}
                    <li class="todo-row" class:done={td.status === 'completed'} class:active={td.status === 'in_progress'}>
                      <span class="todo-box" aria-hidden="true">
                        {#if td.status === 'completed'}
                          <Icon name="check" strokeWidth={3} />
                        {/if}
                      </span>
                      <span class="todo-text">{td.content}</span>
                    </li>
                  {/each}
                </ul>
              {/each}
            </div>
          {/if}
        </div>
        {#if plan}
          <div class="plan-edit-steps-foot">
            <button type="button" class="plan-edit-add" onclick={() => addStep()}>
              <Icon name="plus" />Add step
            </button>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>
