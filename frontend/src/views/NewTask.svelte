<script lang="ts">
  import { tick, untrack } from 'svelte'
  import { get } from 'svelte/store'
  import {
    projects, workspaces, openNewProject, newTaskRepoId,
    newTaskOpen, closeNewTask, newTaskDraft, settings, pushToast,
  } from '../lib/stores'
  import { WorkspacesService, ProjectsService, call, refreshProjects, upsertWorkspace, dropWorkspace, bridgeErrorMessage, linkLinearProject, linkWorkspaceIssue } from '../lib/api'
  import * as BundleService from '../../bindings/recur/internal/app/bundleservice'
  import { simplifyEnabled } from '../lib/simplify'
  import type { Workspace } from '../../bindings/recur/internal/domain/models'
  import {
    chatModeOptions, cycleChatMode, normalizeChatMode,
    resolvedDefaultForMode, isModeDefaultSelection, type ChatMode,
  } from '../lib/modes'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import {
    loadModelCatalog, peekModelCatalog, normalizeFlags, flagsForModel,
    type CatalogModel,
  } from '../lib/models'
  import { openingChatSeed, openingSeedText, revealCreatedWorkspace, eagerCreateInput, startEagerCreate, endEagerCreate, takeEagerWorkspace } from '../lib/new-workspace'
  import { stripAttachmentMarkers, toSendAttachments } from '../lib/agentchat'
  import { waitForWorktree } from '../lib/wait-for-worktree'
  import { cycleDialogTab, clearKeyboardFocus, composerOwnsDialogTab } from '../lib/dialog-focus'
  import {
    buildPromptContext, findPromptBySlash, listPromptActionsUnfiltered, runPromptAction,
    promptActionErrorMessage, promptEnabled, paramsFromArgs,
  } from '../lib/promptActions'
  import AgentChat, { type ChatAttachmentDraft } from '../components/AgentChat.svelte'
  import Select from '../components/Select.svelte'
  import LoopPolicyPickers from '../components/LoopPolicyPickers.svelte'
  import ModelPickers from '../components/ModelPickers.svelte'
  import Icon from '../components/Icon.svelte'
  import ProjectPic from '../components/ProjectPic.svelte'

  import {
    autonomyFromPolicyId, createWorkspaceWith, defaultLoopId, defaultPolicyId,
    enabledLoops, enabledPolicies, fallbackLoops, fallbackPolicies,
    listLoops, listPolicies, type LoopInfo, type PolicyInfo,
  } from '../lib/loops'

  // One workspace noun; the MODE decides what starts inside it: Loop engages
  // the autopilot immediately, every other
  // mode opens the workspace with its first chat tab in that mode. The last
  // choice is remembered (default Agent).
  const MODE_KEY = 'recur.newWorkspaceMode'
  let mode = $state<ChatMode>(normalizeChatMode(localStorage.getItem(MODE_KEY)))
  // Eager create is identical for every non-loop mode; only Loop vs not should
  // abandon the pending worktree (Agent↔Plan↔Review must not re-run git add).
  const isLoopMode = $derived(mode === 'loop')

  let task = $state('')
  let seedWire = ''
  let seedDisplay = ''
  let attachments = $state<ChatAttachmentDraft[]>([])
  let starting = $state(false)
  let linearKey = $state('')
  let linkLinear = $state(true)
  let issueRef: { source: string; id: string; key?: string } | undefined
  let repoId = $state('')
  let baseBranch = $state('')
  let branches = $state<{ name: string; group: string }[]>([])
  let loadingBranches = $state(false)
  // Eager create: as soon as repo+base are known (non-loop), persist a workspace
  // and start `git worktree add` so Create can close the modal immediately.
  let pendingId = $state('')
  let pendingPromise: Promise<Workspace | null> | null = null
  let claimed = false
  let loopId = $state(defaultLoopId())
  let policyId = $state(defaultPolicyId())
  let catalogLoops = $state<LoopInfo[]>(fallbackLoops())
  let catalogPolicies = $state<PolicyInfo[]>(fallbackPolicies())
  const loops = $derived(enabledLoops(catalogLoops, $settings))
  const policies = $derived(enabledPolicies(catalogPolicies, $settings))
  let composer = $state<AgentChat | undefined>(undefined)
  let modalEl = $state<HTMLDivElement | null>(null)
  let promptBusy = $state(false)
  let promptCtl: AbortController | null = null

  // Model + flags for the first chat (non-loop modes) — same T3 pair as the
  // chat dock; defaults from that mode's Settings → Models slot. A loop's
  // specialists take Loop/Agent/Review models, so the picker hides in Loop mode.
  let catalogModels = $state<CatalogModel[]>(peekModelCatalog())
  let model = $state('')
  let flags = $state<Record<string, string>>({})
  const pickersReady = $derived(catalogModels.length > 0 && !!model)
  let simplifyOn = $state(true)
  let simplifySeeded = false

  function toggleSimplify() {
    simplifyOn = !simplifyOn
    simplifySeeded = true
  }

  function onModelChange(ref: string, nextFlags: Record<string, string>) {
    model = ref
    flags = nextFlags
  }
  function onFlagsChange(next: Record<string, string>) {
    flags = next
  }

  // Git projects only — non-git folders can host a working copy but cannot create
  // worktree-based workspaces (git worktree add would fail).
  const gitProjects = $derived($projects.filter((p) => p.isGit))

  // Re-probe isGit when the modal opens so a mid-session `git init` shows up
  // without requiring an app reload.
  $effect(() => {
    if (!$newTaskOpen) return
    void refreshProjects()
  })

  // Prefer openNewTask's preselect (sidebar "+" or focused project/workspace),
  // else the first git project. Pref always wins over a sticky prior picker choice;
  // a non-git pref is ignored so the modal never lands on an unusable host.
  $effect(() => {
    if (!$newTaskOpen) return
    const pref = $newTaskRepoId
    if (pref && gitProjects.some((p) => p.id === pref)) {
      repoId = pref
      newTaskRepoId.set('')
      return
    }
    if (pref) newTaskRepoId.set('')
    if (repoId && !gitProjects.some((p) => p.id === repoId)) repoId = ''
    if (!repoId && gitProjects.length) repoId = gitProjects[0].id
  })

  // Default autonomy from global policy each time the modal opens; the user can
  // override per loop without changing Settings. The mode keeps the remembered
  // last choice; model/flags reset to that mode's Settings-role default.
  $effect(() => {
    if (!$newTaskOpen) return
    loopId = defaultLoopId($settings)
    policyId = defaultPolicyId($settings)
    // Snapshot mode at open — mid-modal mode changes are handled by pickMode
    // (follow the new default only when still on the previous mode's default).
    const d = resolvedDefaultForMode(untrack(() => mode), $settings, catalogModels)
    model = d.model
    flags = d.flags
    setTimeout(() => composer?.focusComposer(), 0)
  })

  $effect(() => {
    if (!$newTaskOpen) {
      simplifySeeded = false
      return
    }
    // Snapshot once Settings has loaded. Untracking would freeze the helper's
    // on-default if the modal opened before loadInitial; tracking after the
    // snapshot would flip the chip when the user later changes Settings.
    if (simplifySeeded) return
    const s = $settings
    if (!s) return
    simplifyOn = simplifyEnabled(s)
    simplifySeeded = true
  })

  $effect(() => {
    if (!$newTaskOpen || catalogModels.length) return
    let cancelled = false
    loadModelCatalog()
      .then((models) => {
        if (cancelled) return
        catalogModels = models
        const d = resolvedDefaultForMode(mode, $settings, models)
        // Only refresh flags from the catalogue when still on the mode default
        // (an explicit pick while the catalogue was loading is kept).
        if (!model || model === d.model) {
          model = d.model
          flags = d.flags
        } else {
          flags = normalizeFlags(flagsForModel(models, model), flags)
        }
      })
      .catch(() => {})
    return () => { cancelled = true }
  })

  function pickMode(m: ChatMode) {
    if (isModeDefaultSelection(mode, model, flags, $settings, catalogModels)) {
      const d = resolvedDefaultForMode(m, $settings, catalogModels)
      model = d.model
      flags = d.flags
    }
    mode = m
    localStorage.setItem(MODE_KEY, m)
  }

  $effect(() => {
    if (!$newTaskOpen) return
    const onKey = (e: KeyboardEvent) => {
      // ⌘. / Ctrl+. — cycle Ask → Plan → Agent → Review → Loop.
      if (isCmdOrCtrl(e) && !e.shiftKey && !e.altKey &&
          (e.code === 'Period' || e.key === '.')) {
        e.preventDefault()
        pickMode(cycleChatMode(mode))
        return
      }
      if (e.key === 'Tab' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        // Lightbox / image menu own their own focus; don't steal Tab.
        if (document.querySelector('.chat-att-lb, .att-ctx')) return
        const modal = modalEl ?? document.querySelector<HTMLElement>('#v-newtask .nt-modal')
        if (!modal) return
        if (composerOwnsDialogTab(modal, e.target)) return
        e.preventDefault()
        cycleDialogTab(modal, e.shiftKey)
        return
      }
      if (e.key === 'Enter' && isCmdOrCtrl(e) && !e.shiftKey && !e.altKey) {
        e.preventDefault()
        e.stopPropagation()
        submit()
        return
      }
      if (e.key !== 'Escape') return
      // Capture phase: skip while a Select menu is open so Escape closes the
      // menu first (Select's own handler) instead of dismissing the modal.
      if (document.querySelector('#v-newtask .sel.open')) return
      // Slash / @ menus and attachment lightbox own Escape while open.
      if (document.querySelector('#v-newtask .chat-mention, .chat-att-lb, .att-ctx')) return
      e.preventDefault()
      closeNewTask()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  })

  $effect(() => {
    if (!$newTaskOpen) return
    let cancelled = false
    void Promise.all([listLoops(), listPolicies()]).then(([ls, ps]) => {
      if (cancelled) return
      if (ls.length) catalogLoops = ls
      if (ps.length) catalogPolicies = ps
    })
    return () => { cancelled = true }
  })

  const repo = $derived(gitProjects.find((p) => p.id === repoId) ?? gitProjects[0])
  // Loop needs a task description (it becomes the run's goal); every other mode
  // only needs a repo + base (an opening prompt is optional — the agent names
  // the workspace via MCP on the first session).
  const canStart = $derived(
    !!repo?.id && !!baseBranch && !loadingBranches && !starting &&
      (mode !== 'loop' || !!task.trim()),
  )
  const repoOptions = $derived(gitProjects.map((p) => ({ value: p.id, label: p.name })))
  const branchOptions = $derived(
    branches.map((b) => ({ value: b.name, label: b.name, group: b.group })),
  )

  // Load local + remote branches whenever the selected repo changes; default to
  // the project's configured base branch (or the first listed branch).
  // Primitives, not `repo`: the open-time refreshProjects() hands back new
  // objects, which must not reload branches (disabling Create) or reset a pick.
  const branchRepoId = $derived(repo?.id ?? '')
  const branchFallback = $derived(repo?.baseBranch || 'main')
  $effect(() => {
    const id = branchRepoId
    if (!id) {
      branches = []
      baseBranch = ''
      return
    }
    let cancelled = false
    loadingBranches = true
    const fallback = branchFallback
    baseBranch = fallback
    ProjectsService.ListBranches(id)
      .then((list) => {
        if (cancelled) return
        const next = list?.length ? list : [{ name: fallback, group: 'Local' }]
        branches = next
        baseBranch = next.some((b) => b.name === fallback) ? fallback : next[0].name
      })
      .catch(() => {
        if (cancelled) return
        branches = [{ name: fallback, group: 'Local' }]
        baseBranch = fallback
      })
      .finally(() => {
        if (!cancelled) loadingBranches = false
      })
    return () => { cancelled = true }
  })

  $effect.pre(() => {
    if ($newTaskOpen) claimed = false
  })

  function abandonPending(id: string) {
    if (!id) return
    void WorkspacesService.DeleteWorkspace(id).catch(() => {})
    dropWorkspace(id)
  }

  // Start the worktree as soon as the modal has a usable repo + base. Loop mode
  // still creates on submit (it needs the goal). Changing repo/branch, or
  // switching into Loop, abandons the unused pending workspace.
  $effect(() => {
    const input = eagerCreateInput($newTaskOpen, isLoopMode, repoId, baseBranch, claimed)
    if (!input) return
    const handle = startEagerCreate(
      () => WorkspacesService.CreateWorkspace(input.projectId, '', input.branch, 'agent', '', '', null),
      (ws) => {
        pendingId = ws.id
        upsertWorkspace(ws)
      },
      abandonPending,
    )
    pendingPromise = handle.promise
    return () => {
      const next = endEagerCreate(claimed, handle.cancel)
      pendingId = next.pendingId
      pendingPromise = next.pendingPromise
    }
  })

  $effect(() => {
    if (!$newTaskOpen) return
    const d = $newTaskDraft
    if (!d) return
    if (d.mode) untrack(() => pickMode(d.mode!))
    seedDisplay = (d.display ?? '').trim()
    seedWire = seedDisplay ? d.text : ''
    task = seedDisplay || d.text
    linearKey = d.linearKey ?? ''
    linkLinear = true
    newTaskDraft.set(null)
    queueMicrotask(() => composer?.focusComposer())
  })

  $effect(() => {
    if ($newTaskOpen) return
    promptCtl?.abort()
    linearKey = ''
    issueRef = undefined
  })

  function goAddProject() {
    openNewProject({ resumeNewTask: true })
  }

  function submit() {
    if (promptBusy || starting) return
    if (task.trim() || attachments.length) {
      composer?.send()
      return
    }
    void start()
  }

  async function invokePromptAction(slash: string, args = '') {
    if (promptBusy || starting || !repo?.id) return
    const slashName = slash.replace(/^\//, '').trim()
    const projectId = repo.id
    promptCtl?.abort()
    const ac = new AbortController()
    promptCtl = ac
    promptBusy = true
    try {
      const list = await listPromptActionsUnfiltered()
      if (ac.signal.aborted) return
      const info = findPromptBySlash(slashName, list)
      if (!info) {
        pushToast('error', `Unknown prompt /${slashName}`)
        return
      }
      if (!promptEnabled(info.id)) {
        pushToast('error', `${info.name} is disabled in Settings → Bundles`)
        return
      }
      if (info.mode) pickMode(normalizeChatMode(info.mode))
      const loop = info.loop?.trim()
      if (loop) loopId = loop
      await tick()
      if (ac.signal.aborted || !get(newTaskOpen) || repo?.id !== projectId) return
      let ws = await takeEagerWorkspace(pendingId, pendingPromise, get(workspaces))
      if (ac.signal.aborted || !get(newTaskOpen) || repo?.id !== projectId) return
      if (ws?.id && !(ws.worktreePath ?? '').trim()) {
        if (ws.lastError || ws.phase === 'failed') {
          pushToast('error', 'Could not create the workspace')
          return
        }
        const ready = await waitForWorktree(ws.id, ac.signal)
        if (ac.signal.aborted || !get(newTaskOpen) || repo?.id !== projectId || pendingId !== ws.id) return
        if (!ready) {
          pushToast('error', 'Could not create the workspace')
          return
        }
        ws = ready
      }
      // simplify: loop mode has no pending worktree; shebang cwd is the project
      // checkout. Upgrade: allocate a worktree before prompts that need isolation.
      const invokedPendingId = ws?.id ?? pendingId
      const ctx = buildPromptContext(
        {
          id: invokedPendingId,
          projectId,
          worktreePath: (ws?.worktreePath ?? '').trim() || (repo.repoPath ?? ''),
          port: ws?.port,
        },
        null,
        { surface: 'workspace' },
        args,
        paramsFromArgs(info, args),
      )
      const result = await runPromptAction(info, ctx, ac.signal)
      if (ac.signal.aborted || !get(newTaskOpen) || repo?.id !== projectId || pendingId !== invokedPendingId) return
      const display = result.display || info.name
      if (result.issue) issueRef = result.issue
      if (!result.prompt.trim()) {
        pushToast('success', display)
        return
      }
      seedWire = result.prompt
      seedDisplay = display
      task = display
      await start()
    } catch (err) {
      if (ac.signal.aborted) return
      const msg = promptActionErrorMessage(err)
      if (msg !== 'prompt cancelled') pushToast('error', msg)
    } finally {
      if (promptCtl === ac) promptCtl = null
      promptBusy = false
    }
  }

  // One creation path: mode=loop engages the
  // autopilot with the task as its goal; any other mode creates a hand-driven
  // workspace and seeds the prompt (and mode) to its first chat tab. Titles
  // start as "New Workspace" until a cheap LLM turn names them from the first
  // message. The worktree is allocated in the background — this only waits for
  // the persisted record so the modal can close immediately.
  async function start() {
    if (!canStart) return
    claimed = true
    starting = true
    try {
      const marked = task.trim()
      let prompt = stripAttachmentMarkers(marked)
      if (!prompt && mode === 'loop' && attachments.length) prompt = 'image attachment'
      let display = ''
      if (prompt) {
        try {
          const expanded = seedWire && prompt === seedDisplay
            ? { prompt: seedWire, display: seedDisplay }
            : await BundleService.ExpandGoal(prompt)
          if ('issue' in expanded && expanded.issue) issueRef = expanded.issue
          const out = openingChatSeed(prompt, { wire: seedWire, display: seedDisplay }, expanded)
          prompt = out.text
          display = out.display
        } catch (err) {
          pushToast('error', bridgeErrorMessage(err) || 'Could not fetch Linear issue')
          claimed = false
          return
        }
      }
      const drafts = attachments
      let ws: Workspace | null = null
      if (mode !== 'loop') {
        ws = await takeEagerWorkspace(pendingId, pendingPromise, $workspaces)
        if (ws) {
          const live = $workspaces.find((w) => w.id === ws!.id)
          if (live) ws = live
        }
        if (ws && (ws.lastError || ws.phase === 'failed')) {
          const failed = ws.id
          ws = null
          pendingId = ''
          pendingPromise = null
          abandonPending(failed)
        }
      }
      if (!ws) {
        const auto = autonomyFromPolicyId(policyId)
        ws = await call(
          mode === 'loop'
            ? createWorkspaceWith(repo!.id, display, baseBranch, mode, prompt, auto, loopId, policyId, simplifyOn, drafts.length ? toSendAttachments(drafts) : [])
            : WorkspacesService.CreateWorkspace(repo!.id, '', baseBranch, mode, prompt, auto, null),
          mode === 'loop' ? 'Could not start the loop' : 'Could not create the workspace',
        )
      }
      if (!ws) {
        claimed = false
        return // create failed — stay on the modal
      }
      upsertWorkspace(ws)
      if (linearKey && linkLinear) void linkLinearProject(repo!.id, linearKey)
      if (issueRef) void linkWorkspaceIssue(ws.id, issueRef)
      task = ''
      attachments = []
      closeNewTask()
      pushToast('success', mode === 'loop' ? 'Loop started' : 'Workspace created')
      void revealCreatedWorkspace({
        id: ws.id,
        text: openingSeedText(marked, prompt, drafts),
        display: display || undefined,
        mode,
        model,
        flags,
        simplify: simplifyOn,
        attachments: drafts.length ? drafts : undefined,
      }, { loop: mode === 'loop' })
    } finally {
      starting = false
    }
  }
</script>

{#if $newTaskOpen}
  <div
    class="nt-bg"
    id="v-newtask"
    onclick={(e) => { if (e.target === e.currentTarget) closeNewTask() }}
    role="presentation"
  >
    <div
      class="nt-modal"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label="New workspace"
      bind:this={modalEl}
      onpointerdown={() => { if (modalEl) clearKeyboardFocus(modalEl) }}
    >
      <div class="nt-head">
        {#if gitProjects.length}
          <div class="nt-pickers">
            <Select bind:value={repoId} options={repoOptions} label="Repository">
              {#snippet icon()}
                {#if repo}
                  <ProjectPic class="nt-pic" pic={repo.pic} color={repo.color} name={repo.name} />
                {:else}
                  <Icon name="folder" />
                {/if}
              {/snippet}
            </Select>
            <Select bind:value={baseBranch} options={branchOptions} disabled={loadingBranches || !branchOptions.length} label="Base branch" searchable>
              {#snippet icon()}
                <Icon name="git-branch" />
              {/snippet}
            </Select>
            {#if linearKey && repo}
              <button
                type="button"
                class="nt-link"
                class:on={linkLinear}
                aria-pressed={linkLinear}
                title="{linkLinear ? 'Will send' : 'Send'} Linear “{linearKey}” issues to {repo.name || repo.id} from now on"
                onclick={() => (linkLinear = !linkLinear)}
              >
                <Icon name="link" />Remember for {linearKey}
              </button>
            {/if}
          </div>
        {:else}
          <div class="nt-empty">No git projects yet — <button type="button" class="lnk" onclick={goAddProject}>add a repository first</button></div>
        {/if}
        <button type="button" class="nt-x" tabindex="-1" onclick={closeNewTask} aria-label="Close">
          <span class="esc">esc</span>
        </button>
      </div>

      <div class="nt-body">
        <AgentChat
          bind:this={composer}
          composerOnly
          bind:input={task}
          bind:attachments
          sending={starting}
          promptBusy={promptBusy || starting}
          projectId={repo?.id ?? ''}
          workspaceId={pendingId}
          rootPath={repo?.repoPath ?? ''}
          placeholder={mode === 'loop'
            ? 'What do you want to work on?'
            : 'What should the agent do? (optional) — e.g. review PR 123, or add a dark-mode toggle'}
          onSend={() => void start()}
          onPromptAction={invokePromptAction}
        />
      </div>

      <!-- Footer mirrors the chat composer: mode + model (or loop autonomy)
           as compact ghost controls, plus the shared Simplify chip. -->
      <div class="nt-foot">
        <Select
          ghost up
          label="Mode"
          options={chatModeOptions()}
          value={mode}
          onchange={(v) => pickMode(normalizeChatMode(v))}
        />
        {#if mode === 'loop'}
        <LoopPolicyPickers
          bind:loopId
          bind:policyId
          {loops}
          {policies}
          showTip
        />
        {:else if pickersReady}
          <ModelPickers
            ghost up
            models={catalogModels}
            {model}
            {flags}
            {onModelChange}
            {onFlagsChange}
          />
        {/if}
        <button
          type="button"
          class="wsv-simplify"
          class:on={simplifyOn}
          aria-pressed={simplifyOn}
          aria-label="Simplify mode"
          title={simplifyOn ? 'Simplify on' : 'Simplify off'}
          onclick={() => toggleSimplify()}
        >
          <Icon name="feather" />
          <span class="wsv-simplify-label">Simplify</span>
        </button>
        <button type="button" class="nt-go" onclick={submit} disabled={!canStart || promptBusy}>
          {#if starting}
            {mode === 'loop' ? 'Starting…' : 'Creating…'}
          {:else}
            {mode === 'loop' ? 'Start' : 'Create workspace'}
            <span class="nt-ret">⌘↵</span>
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}
