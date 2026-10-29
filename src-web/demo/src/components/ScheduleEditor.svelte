<script lang="ts">
  // New / Edit schedule modal, mounted once in App so the sidebar "+" and the
  // Schedules lists can open it from any view. Opened via the scheduleEditor
  // store; saves bump schedulesVersion so lists refresh.
  import { onMount, tick } from 'svelte'
  import { ProjectsService } from '../../bindings/recur/internal/app'
  import { ScheduledPrompt } from '../../bindings/recur/internal/domain/models'
  import { loadModelCatalog, coerceFlags, normalizeFlags, flagsForModel, type CatalogModel } from '../lib/models'
  import { settings, projects, scheduleEditor, schedulesVersion, type ScheduleEditorRequest } from '../lib/stores'
  import { chatModeOptions, cycleChatMode, normalizeChatMode, resolvedDefaultForMode, isModeDefaultSelection } from '../lib/modes'
  import { simplifyEnabled } from '../lib/simplify'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import ModelPickers from './ModelPickers.svelte'
  import Icon from './Icon.svelte'
  import ProjectPic from './ProjectPic.svelte'
  import { cycleDialogTab, clearKeyboardFocus } from '../lib/dialog-focus'
  import { portalToBody } from '../lib/menu-space'
  import { listLoops, listPolicies, enabledLoops, enabledPolicies, defaultLoopId, defaultPolicyId, type LoopInfo, type PolicyInfo } from '../lib/loops'
  import LoopPolicyPickers from './LoopPolicyPickers.svelte'
  import Select from './Select.svelte'

  let models = $state.raw<CatalogModel[]>([])
  let loops = $state.raw<LoopInfo[]>([])
  let policies = $state.raw<PolicyInfo[]>([])
  let preview = $state.raw<number[]>([])
  let previewError = $state('')
  let previewBusy = $state(false)
  const availableLoops = $derived(enabledLoops(loops, $settings))
  const availablePolicies = $derived(enabledPolicies(policies, $settings))
  let branchesLoading = $state(false)
  let branches = $state.raw<{ name: string; group: string }[]>([])
  let draft = $state<ScheduledPrompt | null>(null)
  let busy = $state(false)
  let error = $state('')
  let modalEl = $state<HTMLDivElement | null>(null)
  let promptEl = $state<HTMLTextAreaElement | null>(null)
  let cronHelp = $state(false)
  let returnFocus: HTMLElement | null = null
  let mounted = false
  const draftProject = $derived($projects.find((p) => p.id === draft?.projectId))
  // IANA zones from the runtime, grouped by region for the searchable picker.
  // Saved zones the runtime does not list (or Local/UTC) still appear so an edit round-trips.
  const timeZoneOptions = $derived.by(() => {
    const listed: string[] = typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : []
    const zones = new Set([...listed, 'UTC', Intl.DateTimeFormat().resolvedOptions().timeZone])
    if (draft?.timeZone) zones.add(draft.timeZone)
    return [...zones].sort().map((z) => ({ value: z, label: z.replace(/_/g, ' '), group: z.includes('/') ? z.split('/')[0] : 'Other' }))
  })
  const projectOptions = $derived($projects.map((p) => ({ value: p.id, label: p.name })))
  const branchOptions = $derived(branches.map((b) => ({ value: b.name, label: b.name, group: b.group })))

  function refreshCatalogs() {
    void Promise.all([listLoops(), listPolicies()]).then(([l, p]) => {
      if (mounted) { loops = l; policies = p }
    })
    void loadModelCatalog()
      .then((m) => {
        if (!mounted) return
        models = m
        if (draft) draft.flags = normalizeFlags(flagsForModel(m, draft.model), draft.flags ?? {})
      })
      .catch((e) => { if (mounted && draft) error = String(e) })
  }

  onMount(() => {
    mounted = true
    refreshCatalogs()
    return () => { mounted = false }
  })

  $effect(() => {
    const req = $scheduleEditor
    if (!req) return
    scheduleEditor.set(null)
    open(req)
  })

  $effect(() => {
    const id = draft?.projectId
    if (!id) return
    let cancelled = false
    branches = []
    branchesLoading = true
    void ProjectsService.ListBranches(id)
      .then((rows) => { if (!cancelled) branches = rows })
      .catch((e) => { if (!cancelled) error = String(e) })
      .finally(() => { if (!cancelled) branchesLoading = false })
    return () => { cancelled = true }
  })

  $effect(() => {
    if (!draft) return
    const timing = new ScheduledPrompt({ cadence: draft.cadence, cron: draft.cron,
      date: draft.date, time: draft.time, timeZone: draft.timeZone })
    let cancelled = false
    preview = []
    previewError = ''
    previewBusy = true
    const timer = setTimeout(() => {
      void ProjectsService.PreviewScheduledPrompt(timing)
        .then((times) => { if (!cancelled) preview = times })
        .catch((e) => { if (!cancelled) previewError = String(e) })
        .finally(() => { if (!cancelled) previewBusy = false })
    }, 250)
    return () => { cancelled = true; clearTimeout(timer) }
  })

  $effect(() => {
    if (!draft) return
    void $settings
    refreshCatalogs()
  })

  $effect(() => {
    if (!draft) return
    const onKey = (e: KeyboardEvent) => {
      // ⌘. / Ctrl+. cycles the mode, same as New workspace and the chat dock.
      if (isCmdOrCtrl(e) && !e.shiftKey && !e.altKey && (e.code === 'Period' || e.key === '.')) {
        e.preventDefault()
        e.stopImmediatePropagation()
        if (draft) pickMode(cycleChatMode(draft.mode))
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopImmediatePropagation()
        const menu = modalEl?.querySelector<HTMLButtonElement>('.sel.open > .sel-btn')
        if (menu) menu.click()
        else if (cronHelp) cronHelp = false
        else closeEditor()
      } else if (e.key === 'Tab' && modalEl && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault()
        cycleDialogTab(modalEl, e.shiftKey)
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      returnFocus?.focus()
    }
  })

  function chooseProject(id: string) {
    if (!draft || draft.id) return
    draft.projectId = id
    draft.baseBranch = $projects.find((p) => p.id === id)?.baseBranch ?? ''
  }

  function closeEditor() {
    if (!busy) draft = null
  }

  function open(req: ScheduleEditorRequest) {
    const p = req.schedule
    returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
    error = ''
    cronHelp = false
    const target = $projects.find((x) => x.id === req.projectId) ?? $projects[0]
    const defaults = resolvedDefaultForMode('ask', $settings, models)
    draft = { ...new ScheduledPrompt(p ? { ...p } : {
      projectId: target?.id ?? '', name: '', prompt: '', baseBranch: target?.baseBranch ?? '',
      mode: 'ask', model: defaults.model, flags: defaults.flags, simplify: simplifyEnabled($settings),
      cadence: 'cron', cron: '0 9 * * MON-FRI', loopId: defaultLoopId($settings), policyId: defaultPolicyId($settings), date: '', time: '09:00', timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }) }
    draft.flags = normalizeFlags(flagsForModel(models, draft.model), draft.flags ?? {})
    draft.simplify ??= simplifyEnabled($settings)
    draft.loopId ||= defaultLoopId($settings)
    draft.policyId ||= defaultPolicyId($settings)
    void tick().then(() => promptEl?.focus())
    if (draft.cadence === 'daily' || draft.cadence === 'weekdays') {
      const [hour, minute] = draft.time.split(':').map(Number)
      draft.cron = `${minute} ${hour} * * ${draft.cadence === 'weekdays' ? 'MON-FRI' : '*'}`
      draft.cadence = 'cron'
    }
  }

  function pickMode(value: string) {
    if (!draft) return
    const mode = normalizeChatMode(value)
    if (isModeDefaultSelection(normalizeChatMode(draft.mode), draft.model, draft.flags ?? {}, $settings, models)) {
      const defaults = resolvedDefaultForMode(mode, $settings, models)
      draft.model = defaults.model
      draft.flags = defaults.flags
    }
    draft.mode = mode
  }

  const canSave = $derived(!!draft && !!draft.prompt.trim() && !!draft.model && !!draft.baseBranch
    && !branchesLoading && !previewBusy && !previewError
    && (draft.mode !== 'loop' || (!!draft.loopId && !!draft.policyId)))

  /** A schedule keeps its first name; a new one is titled from the prompt's first line. */
  function scheduleName(prompt: string) {
    return (prompt.split('\n').map((l) => l.trim()).find(Boolean) ?? '').slice(0, 60)
  }

  function submit() {
    if (!draft || !canSave || busy) return
    const p = new ScheduledPrompt({ ...draft, name: draft.name.trim() || scheduleName(draft.prompt) })
    busy = true
    error = ''
    ProjectsService.SaveScheduledPrompt(p)
      .then(() => { if (mounted) { draft = null; schedulesVersion.update((n) => n + 1) } })
      .catch((e) => { if (mounted) error = String(e) })
      .finally(() => { if (mounted) busy = false })
  }

  function onPromptKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && isCmdOrCtrl(e)) {
      e.preventDefault()
      submit()
    }
  }

  const nextLabel = $derived.by(() => {
    if (!draft) return ''
    if (previewError) return previewError.replace(/^Error:\s*/, '')
    if (previewBusy) return 'Checking…'
    if (draft.cadence === 'cron' && draft.cron.trim() === '@reboot') return 'Runs when Recur starts'
    if (!preview.length) return ''
    return 'Next ' + when(preview[0], draft.timeZone)
  })

  function when(at: number, zone: string) {
    try { return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short', timeZone: zone }).format(at) }
    catch { return '' }
  }
</script>

{#if draft}
  <div class="nt-bg" role="presentation" {@attach portalToBody}
    onclick={(e) => { if (e.target === e.currentTarget) closeEditor() }}>
    <div class="nt-modal" role="dialog" tabindex="-1" aria-modal="true"
      aria-label={draft.id ? 'Edit schedule' : 'New schedule'} bind:this={modalEl}
      onkeydown={(e) => e.stopPropagation()}
      onpointerdown={() => { if (modalEl) clearKeyboardFocus(modalEl) }}>
      <div class="nt-head">
        <div class="nt-pickers">
          <Select label="Repository" value={draft.projectId} options={projectOptions} onchange={chooseProject} disabled={!!draft.id}>
            {#snippet icon()}
              {#if draftProject}<ProjectPic class="nt-pic" pic={draftProject.pic} color={draftProject.color} name={draftProject.name} />{:else}<Icon name="folder" />{/if}
            {/snippet}
          </Select>
          <Select label="Base branch" bind:value={draft.baseBranch} options={branchOptions} searchable disabled={branchesLoading || !branchOptions.length}>
            {#snippet icon()}<Icon name="git-branch" />{/snippet}
          </Select>
        </div>
        <button type="button" class="nt-x" tabindex="-1" onclick={closeEditor} aria-label="Close"><span class="esc">esc</span></button>
      </div>

      <div class="nt-body">
        <textarea class="nt-in" aria-label="Prompt" bind:this={promptEl} bind:value={draft.prompt} onkeydown={onPromptKey}
          placeholder={draft.mode === 'loop' ? 'What should the loop work on each run?' : 'What should the agent do each run? — e.g. review yesterday\'s commits for missing tests'}
          rows={draft.mode === 'loop' ? 5 : 3}></textarea>
      </div>

      <!-- Schedule row: same ghost chrome as the composer footer. -->
      <div class="nt-foot sc-when">
        <Select ghost label="Repeat" bind:value={draft.cadence}
          options={[{ value: 'cron', label: 'Cron', icon: 'rotate-cw-fading-clock', hint: 'Repeat on a cron expression' }, { value: 'once', label: 'Once', icon: 'clock-fading', hint: 'Run once at a date and time' }]} />
        {#if draft.cadence === 'once'}
          <input class="sc-in" type="date" aria-label="Date" required bind:value={draft.date} />
          <input class="sc-in" type="time" aria-label="Time" required bind:value={draft.time} />
        {:else}
          <input class="sc-in sc-cron" aria-label="Cron expression" bind:value={draft.cron} spellcheck="false" placeholder="0 9 * * MON-FRI" />
          <button type="button" class="nt-help" class:on={cronHelp} aria-label="Cron format help" aria-expanded={cronHelp} aria-controls="cron-tip" onclick={() => (cronHelp = !cronHelp)}>
            <Icon name="info" />
          </button>
        {/if}
        <Select ghost searchable label="Time zone" bind:value={draft.timeZone} options={timeZoneOptions} />
        <span class="sc-preview" class:err={!!previewError} role={previewError ? 'alert' : 'status'}>{nextLabel}</span>
      </div>

      <div class="nt-foot">
        <Select ghost up label="Mode" options={chatModeOptions()} value={draft.mode} onchange={pickMode} />
        {#if draft.mode === 'loop'}
          <LoopPolicyPickers bind:loopId={draft.loopId} bind:policyId={draft.policyId} loops={availableLoops} policies={availablePolicies} showTip />
        {:else}
          <ModelPickers ghost up {models} model={draft.model} flags={coerceFlags(draft.flags) ?? {}}
            onModelChange={(model, flags) => { if (draft) { draft.model = model; draft.flags = flags } }}
            onFlagsChange={(flags) => { if (draft) draft.flags = flags }} />
        {/if}
        <button type="button" class="wsv-simplify" class:on={draft.simplify}
          aria-pressed={draft.simplify ?? true} aria-label="Simplify mode"
          title={draft.simplify ? 'Simplify on' : 'Simplify off'}
          onclick={() => { if (draft) draft.simplify = !draft.simplify }}>
          <Icon name="feather" /><span class="wsv-simplify-label">Simplify</span>
        </button>
        {#if error}<span class="sc-preview err" role="alert">{error}</span>{/if}
        <button type="button" class="nt-go" onclick={submit} disabled={!canSave || busy}>
          {#if busy}Saving…{:else}{draft.id ? 'Save schedule' : 'Schedule'}<span class="nt-ret">⌘↵</span>{/if}
        </button>
      </div>
      {#if cronHelp}
        <div class="nt-tip-pop sc-tip" id="cron-tip" role="tooltip">
          <b>Cron</b> is <code>minute hour day month weekday</code>; add a leading field for seconds.
          <code>*</code> any, <code>,</code> list, <code>-</code> range, <code>/</code> step, names like <code>MON</code> or <code>JAN</code>.
          <ul>
            <li><code>*/15 9-17 * * MON-FRI</code> every 15 min, weekday hours</li>
            <li><code>0 9,17 * * *</code> daily at 9am and 5pm</li>
            <li><code>0 9 1,15 * *</code> 1st and 15th at 9am</li>
            <li><code>@hourly</code> <code>@daily</code> <code>@weekly</code> <code>@monthly</code></li>
            <li><code>@every 2h30m</code> fixed interval</li>
            <li><code>@reboot</code> when Recur starts</li>
          </ul>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .nt-modal { position: relative; }
  /* Help pops below the row so it never covers the prompt. */
  .sc-when .nt-help { height: 24px; width: 24px; }
  .sc-tip { top: calc(100% + 8px); bottom: auto; left: 14px; width: 340px; }
  .sc-tip code { font-family: var(--mono); font-size: 11px; color: var(--text); }
  .sc-tip ul { margin: 8px 0 0; padding-left: 14px; }
  .sc-tip li { margin-top: 3px; }

  /* Schedule row: same ghost chrome as the composer footer. */
  .sc-when { padding-bottom: 0; gap: 2px; flex-wrap: wrap; }
  .sc-in {
    appearance: none; -webkit-appearance: none; height: 24px; padding: 0 6px; border: none; border-radius: 6px;
    background: transparent; box-shadow: none; color: var(--muted); -webkit-text-fill-color: currentColor;
    font: inherit; font-size: 12px; min-width: 0; transition: color .15s ease, background .15s ease;
  }
  .sc-in::placeholder { color: var(--faint); }
  .sc-in:hover, .sc-in:focus { background: var(--pane-2); color: var(--text); outline: none; }
  .sc-cron { font-family: var(--mono); width: 24ch; }
  .sc-in[type='date'], .sc-in[type='time'] { color-scheme: dark; }
  .sc-preview { margin-left: auto; font-family: var(--mono); font-size: 10px; font-variant-numeric: tabular-nums; color: var(--faint); text-align: right; min-width: 0; white-space: nowrap; }
  .sc-preview.err { color: var(--coral); }
</style>
