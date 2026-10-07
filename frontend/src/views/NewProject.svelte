<script lang="ts">
  import { untrack } from 'svelte'
  import { Dialogs } from '@wailsio/runtime'
  import {
    newProjectOpen, closeNewProject, takeNewProjectResumeNewTask, openNewTask, pushToast,
  } from '../lib/stores'
  import { ProjectsService, call, loadInitial } from '../lib/api'
  import type { Project } from '../../bindings/recur/internal/domain/models'
  import { projectPicFromName } from '../lib/colors'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import { cycleDialogTab, clearKeyboardFocus } from '../lib/dialog-focus'
  import Select, { type SelectOption } from '../components/Select.svelte'
  import Icon from '../components/Icon.svelte'

  let modalEl = $state<HTMLElement | undefined>()
  let pathEl = $state<HTMLInputElement | undefined>()
  let repoPath = $state('')
  let name = $state('')
  let baseBranch = $state('')
  let suggestedForPath = $state('')
  let branchChoices = $state<{ name: string; group: string }[]>([])
  let picking = $state(false)
  let saving = $state(false)
  let autoName = ''

  const repoPathOk = $derived(!!repoPath.trim())
  const canSave = $derived(repoPathOk && !saving)

  const branchOptions = $derived<SelectOption[]>(
    branchChoices.map((b) => ({ value: b.name, label: b.name, group: b.group })),
  )

  function reset() {
    repoPath = ''
    name = ''
    baseBranch = ''
    suggestedForPath = ''
    branchChoices = []
    picking = false
    saving = false
    autoName = ''
  }

  function applyRepoPath(path: string) {
    repoPath = path
    suggestedForPath = ''
    const base = path.replace(/[/\\]+$/, '').split(/[/\\]/).pop() || ''
    if (!name.trim() || name === autoName) {
      name = base
      autoName = base
    }
  }

  $effect(() => {
    if (!$newProjectOpen) return
    reset()
    setTimeout(() => pathEl?.focus(), 0)
  })

  $effect(() => {
    if (!$newProjectOpen) return
    const path = repoPath.trim()
    if (!path || path === suggestedForPath) return
    baseBranch = ''
    let cancelled = false
    ProjectsService.SuggestBaseBranch(path)
      .then((b) => {
        if (cancelled || !b) return
        suggestedForPath = path
        if (!baseBranch) baseBranch = b
      })
      .catch(() => {
        if (!cancelled) {
          suggestedForPath = path
          if (!baseBranch) baseBranch = 'main'
        }
      })
    return () => { cancelled = true }
  })

  $effect(() => {
    if (!$newProjectOpen) return
    const path = repoPath.trim()
    void suggestedForPath
    if (!path) {
      branchChoices = []
      return
    }
    let cancelled = false
    const preferred = untrack(() => baseBranch.trim())
    ProjectsService.ListBranchesForPath(path, preferred)
      .then((list) => {
        if (!cancelled) branchChoices = list?.length ? list : []
      })
      .catch(() => {
        if (!cancelled) branchChoices = []
      })
    return () => { cancelled = true }
  })

  $effect(() => {
    if (!$newProjectOpen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Tab' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const modal = modalEl ?? document.querySelector<HTMLElement>('#v-newproject .nt-modal')
        if (!modal) return
        e.preventDefault()
        cycleDialogTab(modal, e.shiftKey)
        return
      }
      if (e.key === 'Enter' && isCmdOrCtrl(e)) {
        e.preventDefault()
        void create()
        return
      }
      if (e.key !== 'Escape') return
      if (document.querySelector('#v-newproject .sel.open')) return
      e.preventDefault()
      closeNewProject()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  })

  async function pickRepo() {
    if (picking) return
    picking = true
    try {
      const path = await Dialogs.OpenFile({
        CanChooseDirectories: true,
        CanChooseFiles: false,
        AllowsMultipleSelection: false,
        Title: 'Choose repository',
        Message: 'Select a local git repository',
        ButtonText: 'Select',
        Directory: repoPath.trim() || undefined,
      })
      if (typeof path === 'string' && path) applyRepoPath(path)
    } catch {
      /* cancelled */
    } finally {
      picking = false
    }
  }

  async function create() {
    if (!canSave) return
    saving = true
    try {
      const path = repoPath.trim()
      const label = name.trim() || path.replace(/[/\\]+$/, '').split(/[/\\]/).pop() || ''
      const saved = await call(ProjectsService.AddProject({
        name: label,
        repoPath: path,
        baseBranch: baseBranch.trim(),
        pic: projectPicFromName(label),
      } as Project), 'Could not create project')
      if (!saved) return
      await loadInitial()
      pushToast('success', 'Project created')
      const resume = takeNewProjectResumeNewTask()
      closeNewProject()
      if (resume && saved.id) openNewTask(saved.id)
    } finally {
      saving = false
    }
  }
</script>

{#if $newProjectOpen}
  <div
    class="nt-bg"
    id="v-newproject"
    onclick={(e) => { if (e.target === e.currentTarget) closeNewProject() }}
    role="presentation"
  >
    <div
      class="nt-modal np-modal"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label="New project"
      bind:this={modalEl}
      onpointerdown={() => { if (modalEl) clearKeyboardFocus(modalEl) }}
    >
      <div class="nt-head">
        <div class="np-title">New project</div>
        <button type="button" class="nt-x" tabindex="-1" onclick={closeNewProject} aria-label="Close">
          <span class="esc">esc</span>
        </button>
      </div>
      <div class="np-body">
        <label class="np-row">
          <span class="np-lbl">Repository</span>
          <div class="path-pick">
            <input
              class="inp"
              bind:this={pathEl}
              value={repoPath}
              placeholder="~/code/my-app"
              aria-label="Repository path"
              oninput={(e) => applyRepoPath(e.currentTarget.value)}
            />
            <button type="button" class="tbtn" onclick={pickRepo} disabled={picking} aria-label="Browse for repository">
              <Icon name="folder" />
              {picking ? 'Opening…' : 'Browse'}
            </button>
          </div>
        </label>
        <label class="np-row">
          <span class="np-lbl">Name</span>
          <input class="inp" bind:value={name} placeholder="my-app" aria-label="Project name" />
        </label>
        {#if branchOptions.length}
          <div class="np-row">
            <span class="np-lbl">Default branch</span>
            <Select
              value={baseBranch || branchOptions[0]?.value || ''}
              options={branchOptions}
              label="Default branch"
              searchable
              onchange={(v) => (baseBranch = v)}
            >
              {#snippet icon()}
                <Icon name="git-branch" />
              {/snippet}
            </Select>
          </div>
        {/if}
      </div>
      <div class="nt-foot">
        <button type="button" class="nt-go" onclick={() => void create()} disabled={!canSave}>
          {saving ? 'Creating…' : 'Create project'}
          {#if !saving}<span class="nt-ret">⌘↵</span>{/if}
        </button>
      </div>
    </div>
  </div>
{/if}
