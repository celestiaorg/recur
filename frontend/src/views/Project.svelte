<script lang="ts">
  import { Dialogs } from '@wailsio/runtime'
  import { view, projects, projectGroups, projectFormId, go, openSettings, pushToast, settings } from '../lib/stores'
  import { ProjectsService, CodeService, loadInitial, refreshProjects, upsertProject, setProjectHidden, call, callOk } from '../lib/api'
  import type { Project } from '../../bindings/recur/internal/domain/models'
  import ScheduledPrompts from '../components/ScheduledPrompts.svelte'
  import MiniScriptEditor from '../components/MiniScriptEditor.svelte'
  import Select, { type SelectOption } from '../components/Select.svelte'
  import Icon, { type IconName } from '../components/Icon.svelte'
  import SettingsGroup from '../components/SettingsGroup.svelte'
  import SettingsGroups from '../components/SettingsGroups.svelte'
  import {
    skillLocation, mapSkillRows, skillKind,
    effectiveSkillEnabled, projectSkillOverrideLists, skillOverrideHint,
    skillKindOverrideCount, skillKindHasOverrides,
    type SkillRow, type SkillKind,
  } from '../lib/skills'
  import { LINEAR_MCP_ID, customMCPServers, mcpGlobalDisabled, type MCPServerRow } from '../lib/mcp'
  import { projectRuns } from '../lib/run-commands'
  import { projectColors, projectPicFromName } from '../lib/colors'
  import {
    DEFAULT_LUCIDE_PIC, colorToHex, formatLucidePic, isProjectPresetColor,
    parseHexColor, parseProjectPic, picFromImageFile,
  } from '../lib/project-pic'
  import ProjectPic from '../components/ProjectPic.svelte'
  import IconPicker from '../components/IconPicker.svelte'

  // The project being edited, or undefined when creating a NEW one ($projectFormId === '').
  const selected = $derived($projects.find((p) => p.id === $projectFormId))
  const isNew = $derived(!selected)
  // String only — upsertProject replaces the project object on every autosave,
  // and reading `selected?.repoPath` in the preview effect would rescan each time.
  const copyPreviewRepo = $derived((selected?.repoPath ?? '').trim())

  $effect(() => {
    if ($view === 'project' && !$projectFormId) go('briefing')
  })

  // Local editable copy; (re)seeded whenever the target project changes.
  let form = $state<Partial<Project>>({ baseBranch: '' })
  let lastFormId = $state<string | null>(null)
  let saving = $state(false)
  let deleting = $state(false)
  let confirmingDelete = $state(false)
  let picking = $state(false)
  let branchChoices = $state<{ name: string; group: string }[]>([])
  let projectSkills = $state<SkillRow[]>([])
  let projectSkillsLoading = $state(false)
  let projectSkillsErr = $state('')
  let savingSkills = $state(false)
  // Set only after a successful load so an effect re-run can't cancel the
  // in-flight fetch and then early-return forever on "Loading skills…".
  let skillsLoadedKey = ''
  // Recur / Your skills lists stay collapsed unless overriding global defaults.
  let overrideRecur = $state(false)
  let overrideUser = $state(false)
  let runRowSeq = 0
  type RunRow = { id: number; name: string; cmd: string }
  function runRow(name = '', cmd = ''): RunRow {
    return { id: ++runRowSeq, name, cmd }
  }
  let runRows = $state<RunRow[]>([runRow('dev')])
  let persistSnap = ''
  let persistPending = false
  const projectFileError = $derived(selected?.projectFileError || '')

  type Section = 'general' | 'schedules' | 'run' | 'prs' | 'skills' | 'mcp' | 'danger'
  let section = $state<Section>('general')
  let query = $state('')

  $effect(() => {
    const fid = $projectFormId
    if (fid !== lastFormId) {
      lastFormId = fid
      form = selected ? { ...selected } : { baseBranch: '' }
      const seeded = selected ? projectRuns(selected) : []
      runRows = seeded.length ? seeded.map((r) => runRow(r.name, r.cmd)) : [runRow('dev')]
      confirmingDelete = false
      persistSnap = ''
      persistPending = false
      skillsLoadedKey = ''
      overrideRecur = false
      overrideUser = false
      section = 'general'
      query = ''
      // Refresh isGit when focusing a project (e.g. after `git init` outside Recur).
      if (fid) void refreshProjects()
    }
  })

  $effect(() => {
    const id = $projectFormId
    // Backend resolves skills from the *persisted* project repo — key off that,
    // not the draft form.repoPath (avoids keystroke refetch + stale list after save).
    const repo = selected?.repoPath?.trim() || ''
    const key = `${id}\0${repo}`
    if (!id) {
      skillsLoadedKey = ''
      projectSkills = []
      projectSkillsErr = ''
      projectSkillsLoading = false
      return
    }
    if (key === skillsLoadedKey) return

    let cancelled = false
    projectSkillsLoading = true
    projectSkillsErr = ''
    CodeService.ListAllSkills(id, '')
      .then((list) => {
        if (cancelled) return
        const rows = mapSkillRows(list)
        projectSkills = rows
        skillsLoadedKey = key
        // Expand override panels only when this project already customizes that kind.
        overrideRecur = skillKindHasOverrides(rows, 'recur', form.disabledSkills, form.enabledSkills)
        overrideUser = skillKindHasOverrides(rows, 'user', form.disabledSkills, form.enabledSkills)
      })
      .catch((err) => {
        if (cancelled) return
        projectSkills = []
        projectSkillsErr = err instanceof Error ? err.message : String(err)
        skillsLoadedKey = key
      })
      .finally(() => {
        if (!cancelled) projectSkillsLoading = false
      })
    return () => { cancelled = true }
  })

  type SkillGroup = { kind: SkillKind; label: string; blurb: string; skills: SkillRow[] }

  const skillsByKind = $derived.by((): Record<SkillKind, SkillRow[]> => {
    const buckets: Record<SkillKind, SkillRow[]> = { recur: [], project: [], user: [] }
    for (const s of projectSkills) {
      buckets[skillKind(s)].push(s)
    }
    for (const k of Object.keys(buckets) as SkillKind[]) {
      buckets[k].sort((a, b) => a.name.localeCompare(b.name))
    }
    return buckets
  })

  const repoSkillGroup = $derived.by((): SkillGroup | null => {
    const skills = skillsByKind.project
    if (!skills.length) return null
    return { kind: 'project', label: 'In this project', blurb: 'Skills in the repository.', skills }
  })

  function kindSkillNames(kind: 'recur' | 'user'): Set<string> {
    return new Set(skillsByKind[kind].map((s) => s.name))
  }

  function kindOverrideCount(kind: 'recur' | 'user'): number {
    return skillKindOverrideCount(skillsByKind[kind], form.disabledSkills, form.enabledSkills)
  }

  function projectSkillOn(name: string): boolean {
    return effectiveSkillEnabled(
      name,
      $settings?.disabledSkills,
      form.disabledSkills,
      form.enabledSkills,
    )
  }

  async function persistSkillLists(disabledSkills: string[] | undefined, enabledSkills: string[] | undefined) {
    if (!selected?.id || savingSkills) return false
    const prevDisabled = form.disabledSkills
    const prevEnabled = form.enabledSkills
    form.disabledSkills = disabledSkills
    form.enabledSkills = enabledSkills
    savingSkills = true
    try {
      // Persist only skill lists over the last-saved record — never the unsaved form draft.
      const payload = { ...selected, disabledSkills, enabledSkills } as Project
      const saved = await call(ProjectsService.UpdateProjectRunConfig(payload as any), 'Could not save skills')
      if (!saved) {
        form.disabledSkills = prevDisabled
        form.enabledSkills = prevEnabled
        return false
      }
      form.disabledSkills = saved.disabledSkills
      form.enabledSkills = saved.enabledSkills
      projects.update((list) => list.map((p) => (p.id === saved.id ? { ...p, ...saved } : p)))
      return true
    } finally {
      savingSkills = false
    }
  }

  async function setProjectSkillEnabled(name: string, enabled: boolean) {
    if (!selected?.id) return
    const next = projectSkillOverrideLists(
      name,
      enabled,
      $settings?.disabledSkills,
      form.disabledSkills,
      form.enabledSkills,
    )
    await persistSkillLists(next.disabledSkills, next.enabledSkills)
  }

  const mcpGlobalOff = $derived(mcpGlobalDisabled($settings?.mcpServers as MCPServerRow[] | undefined))
  const mcpRows = $derived.by(() => {
    const rows: { id: string; name: string; detail: string }[] = [
      { id: LINEAR_MCP_ID, name: 'Linear', detail: 'Uses the API key in Settings → Integrations' },
    ]
    for (const s of customMCPServers($settings?.mcpServers as MCPServerRow[] | undefined)) {
      const id = (s.id ?? '').trim()
      if (!id) continue
      rows.push({ id, name: s.name || id, detail: s.transport === 'stdio' ? (s.command || 'stdio') : (s.url || 'http') })
    }
    return rows
  })

  function projectMCPOn(id: string): boolean {
    return effectiveSkillEnabled(id, mcpGlobalOff, form.disabledMCP, form.enabledMCP)
  }

  async function persistMCPLists(disabledMCP: string[] | undefined, enabledMCP: string[] | undefined) {
    if (!selected?.id || savingSkills) return false
    const prevDisabled = form.disabledMCP
    const prevEnabled = form.enabledMCP
    form.disabledMCP = disabledMCP
    form.enabledMCP = enabledMCP
    savingSkills = true
    try {
      const payload = { ...selected, disabledMCP, enabledMCP } as Project
      const saved = await call(ProjectsService.UpdateProjectRunConfig(payload as any), 'Could not save MCP')
      if (!saved) {
        form.disabledMCP = prevDisabled
        form.enabledMCP = prevEnabled
        return false
      }
      form.disabledMCP = saved.disabledMCP
      form.enabledMCP = saved.enabledMCP
      projects.update((list) => list.map((p) => (p.id === saved.id ? { ...p, ...saved } : p)))
      return true
    } finally {
      savingSkills = false
    }
  }

  async function setProjectMCPEnabled(id: string, enabled: boolean) {
    if (!selected?.id) return
    const next = projectSkillOverrideLists(id, enabled, mcpGlobalOff, form.disabledMCP, form.enabledMCP)
    await persistMCPLists(next.disabledSkills, next.enabledSkills)
  }

  async function setKindOverride(kind: 'recur' | 'user', on: boolean) {
    if (kind === 'recur') overrideRecur = on
    else overrideUser = on
    if (on || !selected?.id) return
    // Turning override off restores global defaults for that kind.
    const names = kindSkillNames(kind)
    if (!names.size) return
    const disabledSkills = (form.disabledSkills ?? []).filter((n) => !names.has(n))
    const enabledSkills = (form.enabledSkills ?? []).filter((n) => !names.has(n))
    if (
      disabledSkills.length === (form.disabledSkills ?? []).length &&
      enabledSkills.length === (form.enabledSkills ?? []).length
    ) {
      return
    }
    const ok = await persistSkillLists(disabledSkills, enabledSkills)
    if (!ok) {
      // Persist failed — overrides still apply; keep the panel open.
      if (kind === 'recur') overrideRecur = true
      else overrideUser = true
    }
  }

  // Load remote + local branches for the default-branch picker.
  $effect(() => {
    const id = $projectFormId
    let cancelled = false
    if (id) {
      void selected?.repoPath
      ProjectsService.ListBranches(id)
        .then((list) => {
          if (!cancelled) branchChoices = list?.length ? list : []
        })
        .catch(() => {
          if (!cancelled) branchChoices = []
        })
    } else {
      branchChoices = []
    }
    return () => { cancelled = true }
  })

  const repoPathOk = $derived(!!form.repoPath && form.repoPath.trim().length > 0)
  const canSave = $derived(repoPathOk)
  const title = $derived(form.name || 'Project')
  const identColor = $derived(form.color || projectColors[0])
  const identPic = $derived(form.pic || projectPicFromName(form.name ?? '') || '?')
  const identParsed = $derived(parseProjectPic(identPic))
  const identKind = $derived(identParsed.kind)
  const identIcon = $derived(identParsed.kind === 'icon' ? identParsed.name : 'box')
  const customColorOn = $derived(!isProjectPresetColor(identColor))
  const pickerHex = $derived(colorToHex(identColor) || '#4cc6d6')
  let picFileEl: HTMLInputElement | undefined = $state()

  function setName(next: string) {
    const prevLetter = projectPicFromName(form.name ?? '')
    form.name = next
    if (!form.pic || form.pic === prevLetter) form.pic = projectPicFromName(next)
  }

  function setLetterPic(next: string) {
    form.pic = projectPicFromName(next)
  }

  function setAvatarKind(next: 'letter' | 'icon' | 'image') {
    if (next === 'letter') {
      form.pic = projectPicFromName(form.name ?? '') || 'P'
      return
    }
    if (next === 'icon') {
      form.pic = identKind === 'icon' ? identPic : DEFAULT_LUCIDE_PIC
      return
    }
    if (identKind === 'image') return
    picFileEl?.click()
  }

  function setLucideName(name: string) {
    form.pic = formatLucidePic(name)
  }

  async function onPicFile(e: Event) {
    const input = e.currentTarget as HTMLInputElement
    const file = input.files?.[0]
    input.value = ''
    if (!file) return
    try {
      form.pic = await picFromImageFile(file)
    } catch {
      pushToast('error', 'Could not read that image')
    }
  }

  function setCustomColor(next: string) {
    const hex = parseHexColor(next)
    if (hex) form.color = hex
  }
  const mergeShort: Record<string, string> = { merge: 'Merge', squash: 'Squash', rebase: 'Rebase' }
  const squashMsgShort: Record<string, string> = {
    title: 'PR title',
    'title-and-commits': 'PR title and commits',
    'title-and-description': 'PR title and description',
  }
  const mergeMsgShort: Record<string, string> = {
    title: 'PR title',
    'title-and-description': 'PR title and description',
  }
  const globalMergeLabel = $derived(mergeShort[$settings?.defaultMergeMethod ?? 'merge'] ?? 'Merge')
  const globalUpdateLabel = $derived(mergeShort[$settings?.defaultUpdateBaseMethod ?? 'merge'] ?? 'Merge')
  const globalSquashMsgLabel = $derived(squashMsgShort[$settings?.defaultSquashCommitMessage || 'title-and-commits'] ?? 'PR title and commits')
  const globalMergeMsgLabel = $derived(mergeMsgShort[$settings?.defaultMergeCommitMessage || 'title'] ?? 'PR title')

  const navIcons: Record<string, IconName> = {
    general: 'sliders-horizontal',
    schedules: 'clock',
    run: 'play',
    prs: 'git-pull-request',
    skills: 'slash',
    mcp: 'plug',
    danger: 'trash',
  }
  const titles: Record<Section, string> = {
    general: 'General',
    schedules: 'Schedules',
    run: 'Run',
    prs: 'Pull requests',
    skills: 'Skills',
    mcp: 'MCP',
    danger: 'Danger zone',
  }
  type NavItem = { id: Section; label: string; icon: string; keywords: string }
  type NavGroup = { group: string; items: NavItem[] }
  const nav = $derived.by((): NavGroup[] => {
    const groups: NavGroup[] = [
      {
        group: 'Project',
        items: [
          { id: 'general', label: 'General', icon: 'general', keywords: 'name avatar pic color icon identity repository path git repo checkout worktree default branch group sidebar issues linear github' },
          { id: 'run', label: 'Run', icon: 'run', keywords: 'setup teardown script app url env environment verify browser project.toml copy files glob dotenv workspace' },
          { id: 'schedules', label: 'Schedules', icon: 'schedules', keywords: 'schedule automation prompt daily weekdays recurring timer' },
          { id: 'prs', label: 'Pull requests', icon: 'prs', keywords: 'github merge squash rebase update base delete branch commit message title description' },
          { id: 'skills', label: 'Skills', icon: 'skills', keywords: 'skills slash recur user repository override' },
          { id: 'mcp', label: 'MCP', icon: 'mcp', keywords: 'mcp linear tools http stdio server override' },
        ],
      },
    ]
    if (!isNew) {
      groups.push({
        group: 'Danger',
        items: [
          { id: 'danger', label: 'Danger zone', icon: 'danger', keywords: 'delete remove wipe project' },
        ],
      })
    }
    return groups
  })
  const filteredNav = $derived.by(() => {
    const q = query.trim().toLowerCase()
    if (!q) return nav
    return nav
      .map((g) => ({
        ...g,
        items: g.items.filter((i) =>
          i.label.toLowerCase().includes(q) || i.keywords.includes(q),
        ),
      }))
      .filter((g) => g.items.length > 0)
  })

  $effect(() => {
    const ids = filteredNav.flatMap((g) => g.items.map((i) => i.id))
    if (ids.length && !ids.includes(section)) section = ids[0]
  })

  const groupOptions = $derived<SelectOption[]>([
    { value: '', label: 'Ungrouped' },
    ...$projectGroups.map((g) => ({ value: g.id, label: g.name })),
  ])
  const branchOptions = $derived.by((): SelectOption[] => {
    const opts = branchChoices.map((b) => ({ value: b.name, label: b.name, group: b.group }))
    const cur = (form.baseBranch || '').trim()
    // Surface a configured base even if the ref is gone.
    if (cur && !opts.some((o) => o.value === cur)) {
      const slash = cur.indexOf('/')
      const remote = slash > 0 ? cur.slice(0, slash) : ''
      const group = remote && opts.some((o) => o.group === remote) ? remote : 'Local'
      let insertAt = opts.length
      if (group === 'Local') {
        const firstLocal = opts.findIndex((o) => o.group === 'Local')
        insertAt = firstLocal >= 0 ? firstLocal : opts.length
      } else {
        const first = opts.findIndex((o) => o.group === group)
        insertAt = first >= 0 ? first : opts.findIndex((o) => o.group === 'Local')
        if (insertAt < 0) insertAt = opts.length
      }
      opts.splice(insertAt, 0, { value: cur, label: cur, group })
    }
    if (!opts.length && cur) return [{ value: cur, label: cur, group: 'Local' }]
    return opts
  })

  async function pickRepo(): Promise<void> {
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
        Directory: form.repoPath?.trim() || undefined,
      })
      if (typeof path === 'string' && path) {
        form.repoPath = path
      }
    } catch {
      // Cancelled or dialog unavailable (e.g. browser e2e) — leave the field alone.
    } finally {
      picking = false
    }
  }

  function addRunRow() {
    runRows = [...runRows, runRow()]
  }

  function removeRunRow(id: number) {
    runRows = runRows.filter((r) => r.id !== id)
    if (runRows.length === 0) runRows = [runRow('dev')]
  }

  function runConfigDirty(runs: { name: string; cmd: string }[]): boolean {
    if ((form.repoPath ?? '').trim() !== (selected?.repoPath ?? '').trim()) return true
    if ((form.setupCmd ?? '') !== (selected?.setupCmd ?? '')) return true
    if ((form.teardownCmd ?? '') !== (selected?.teardownCmd ?? '')) return true
    if ((form.appURL ?? '') !== (selected?.appURL ?? '')) return true
    if ((form.env ?? '') !== (selected?.env ?? '')) return true
    if ((form.copyFiles ?? '') !== (selected?.copyFiles ?? '')) return true
    const prev = projectRuns(selected)
    const next = projectRuns({ runCommands: runs })
    if (prev.length !== next.length) return true
    return prev.some((r, i) => r.name !== next[i]!.name || r.cmd !== next[i]!.cmd)
  }

  function persistSnapshot(): string {
    return JSON.stringify({
      repoPath: form.repoPath ?? '',
      name: form.name ?? '',
      pic: form.pic ?? '',
      color: form.color ?? '',
      baseBranch: form.baseBranch ?? '',
      groupId: form.groupId ?? '',
      setupCmd: form.setupCmd ?? '',
      teardownCmd: form.teardownCmd ?? '',
      appURL: form.appURL ?? '',
      env: form.env ?? '',
      copyFiles: form.copyFiles ?? '',
      prOnGithub: !!form.prOnGithub,
      deleteBranchOnRemove: !!form.deleteBranchOnRemove,
      linearProjects: form.linearProjects ?? '',
      mergeMethod: form.mergeMethod ?? '',
      updateBaseMethod: form.updateBaseMethod ?? '',
      squashCommitMessage: form.squashCommitMessage ?? '',
      mergeCommitMessage: form.mergeCommitMessage ?? '',
      runs: runRows.map((r) => ({ name: r.name, cmd: r.cmd })),
    })
  }

  async function persist(): Promise<Project | null> {
    if (!canSave) return null
    if (saving) {
      persistPending = true
      return null
    }
    saving = true
    let last: Project | null = null
    try {
      do {
        persistPending = false
        const snap = persistSnapshot()
        if (snap === persistSnap) break
        const runs = runRows
          .map((r) => ({ name: r.name.trim(), cmd: r.cmd.trim() }))
          .filter((r) => r.cmd)
        const payload = {
          ...(selected ?? {}),
          ...form,
          hidden: selected?.hidden,
          runCommands: runs,
          writeProjectFile: runConfigDirty(runs),
        } as Project
        const saved = await call(ProjectsService.UpdateProjectRunConfig(payload as any), 'Save failed')
        if (!saved) return null
        persistSnap = snap
        upsertProject(saved)
        if (saved.id) projectFormId.set(saved.id)
        last = saved
      } while (persistPending || persistSnapshot() !== persistSnap)
      if (last) pushToast('success', 'Project saved', { replace: 'project-saved' })
      return last
    } finally {
      saving = false
    }
  }

  $effect(() => {
    if (isNew || !selected?.id) return
    const snap = persistSnapshot()
    if (!persistSnap) {
      persistSnap = snap
      return
    }
    if (snap === persistSnap) return
    if (!canSave) return
    const t = setTimeout(() => { void persist() }, 450)
    return () => clearTimeout(t)
  })

  type CopyPreview = { root: string; files: string[]; count: number; truncated: boolean; error: string; loading: boolean }
  let copyPreview = $state<CopyPreview>({ root: '', files: [], count: 0, truncated: false, error: '', loading: false })
  let copyRefreshTick = $state(0)

  $effect(() => {
    if (section !== 'run') return
    const id = $projectFormId
    const patterns = form.copyFiles ?? ''
    void copyRefreshTick
    void copyPreviewRepo
    if (!id) return
    let cancelled = false
    const t = setTimeout(() => {
      if (cancelled) return
      if (!patterns.trim()) {
        copyPreview = { root: (form.repoPath ?? '').trim(), files: [], count: 0, truncated: false, error: '', loading: false }
        return
      }
      copyPreview.loading = true
      void ProjectsService.PreviewCopyFiles(id, patterns)
        .then((got) => {
          if (cancelled) return
          copyPreview = {
            root: got?.root ?? '',
            files: got?.files ?? [],
            count: got?.count ?? 0,
            truncated: !!got?.truncated,
            error: got?.error ?? '',
            loading: false,
          }
        })
        .catch((err) => {
          if (cancelled) return
          copyPreview = {
            root: '',
            files: [],
            count: 0,
            truncated: false,
            error: err instanceof Error ? err.message : String(err),
            loading: false,
          }
        })
    }, 280)
    return () => {
      cancelled = true
      clearTimeout(t)
    }
  })

  const copyFilesMeta = $derived.by(() => {
    if (copyPreview.loading) return 'Scanning…'
    if (copyPreview.error) return copyPreview.error
    if (!(form.copyFiles ?? '').trim()) {
      return 'Nothing will be copied'
    }
    if (isNew) return 'Preview available after the project is created'
    const n = copyPreview.count
    const root = copyPreview.root || (form.repoPath ?? '').trim()
    if (n === 0) {
      return root ? `No matching files in ${root}` : 'No matching files'
    }
    const noun = n === 1 ? 'file' : 'files'
    return root ? `${n} ${noun} will be copied from ${root}` : `${n} ${noun} will be copied`
  })

  async function deleteProject() {
    if (!selected) return
    deleting = true
    try {
      if (!(await callOk(ProjectsService.DeleteProject(selected.id), 'Delete failed'))) return
      await loadInitial()
      confirmingDelete = false
      pushToast('success', 'Project deleted')
      go($projects.length ? 'briefing' : 'welcome')
    } finally {
      deleting = false
    }
  }
</script>

<section class="view" class:on={$view === 'project'} id="v-project">
  <div class="set-shell">
    <aside class="set-nav">
      <label class="set-search">
        <Icon name="search" />
        <input type="search" placeholder="Search settings" bind:value={query} autocomplete="off" spellcheck="false" />
      </label>
      <nav class="set-nav-list" aria-label="Project settings">
        {#each filteredNav as g (g.group)}
          <div class="set-group">{g.group}</div>
          {#each g.items as item (item.id)}
            <button
              type="button"
              class="set-nav-item"
              class:on={section === item.id}
              aria-current={section === item.id ? 'page' : undefined}
              onclick={() => (section = item.id)}
            >
              <span class="set-nav-ico"><Icon name={navIcons[item.icon]} /></span>
              {item.label}
            </button>
          {/each}
        {/each}
        {#if !filteredNav.length}
          <div class="set-nav-empty">No matching settings</div>
        {/if}
      </nav>
    </aside>

    <div class="set-main">
      <header class="set-head">
        <div class="set-head-copy">
          <h2>{titles[section]}</h2>
          <p class="set-head-sub">{title}</p>
        </div>
      </header>
      <div class="set-body">

        {#if section === 'general'}
          <SettingsGroups>
            <SettingsGroup title="Identity" blurb="How this project appears in Recur.">
              <div class="set-rows">
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Name</div>
                  </div>
                  <div class="set-ctrl">
                    <input
                      class="inp proj-name"
                      value={form.name ?? ''}
                      placeholder="my-app"
                      aria-label="Project name"
                      oninput={(e) => setName(e.currentTarget.value)}
                    />
                  </div>
                </div>
                <div class="set-row proj-ident-row">
                  <div class="set-copy proj-ident-label">
                    <div class="set-title">Avatar</div>
                    <ProjectPic class="proj-ident-pic" pic={identPic} color={identColor} name={form.name ?? ''} />
                  </div>
                  <div class="set-ctrl proj-ident">
                    <div class="seg proj-ident-kind" role="group" aria-label="Avatar kind">
                      <button type="button" class:on={identKind === 'letter'} onclick={() => setAvatarKind('letter')}>Letter</button>
                      <button type="button" class:on={identKind === 'icon'} onclick={() => setAvatarKind('icon')}>Icon</button>
                      <button type="button" class:on={identKind === 'image'} onclick={() => setAvatarKind('image')}>Image</button>
                    </div>
                    <div class="proj-ident-slot">
                      {#if identKind === 'letter'}
                        <input
                          class="inp proj-ident-letter"
                          value={identParsed.kind === 'letter' ? identParsed.text : ''}
                          maxlength="1"
                          aria-label="Avatar letter"
                          oninput={(e) => setLetterPic(e.currentTarget.value)}
                        />
                      {:else if identKind === 'icon'}
                        <IconPicker value={identIcon} onchange={setLucideName} />
                      {:else}
                        <div class="proj-ident-img-btns">
                          <button type="button" class="tbtn" onclick={() => picFileEl?.click()}>
                            <Icon name="photo" />Browse
                          </button>
                          <button type="button" class="tbtn" onclick={() => (form.pic = projectPicFromName(form.name ?? '') || 'P')}>
                            Clear
                          </button>
                        </div>
                      {/if}
                    </div>
                    <input
                      bind:this={picFileEl}
                      type="file"
                      accept="image/png,image/jpeg,image/webp,image/gif"
                      hidden
                      onchange={onPicFile}
                    />
                    <div class="proj-ident-colors" role="group" aria-label="Avatar color">
                      {#each projectColors as c (c)}
                        <button
                          type="button"
                          class="proj-ident-swatch"
                          class:on={identColor === c}
                          style:background={c}
                          aria-label={c.replace('var(--', '').replace(')', '')}
                          aria-pressed={identColor === c}
                          onclick={() => (form.color = c)}
                        ></button>
                      {/each}
                      <label
                        class="proj-ident-swatch proj-ident-custom"
                        class:on={customColorOn}
                        style:background={customColorOn ? identColor : 'var(--pane)'}
                      >
                        {#if !customColorOn}
                          <Icon name="plus" />
                        {/if}
                        <input
                          type="color"
                          value={pickerHex}
                          aria-label="Custom color"
                          oninput={(e) => setCustomColor(e.currentTarget.value)}
                        />
                      </label>
                    </div>
                  </div>
                </div>
              </div>
            </SettingsGroup>
            <SettingsGroup title="Repository" blurb="Local git repo. Workspaces run in isolated git worktrees off this checkout.">
              <div class="set-rows">
                <div class="set-row set-row-stack">
                  <div class="path-pick">
                    <input class="inp" bind:value={form.repoPath} placeholder="~/code/my-app" aria-label="Repository path" />
                    <button type="button" class="tbtn" onclick={pickRepo} disabled={picking} aria-label="Browse for repository">
                      <Icon name="folder" />
                      {picking ? 'Opening…' : 'Browse'}
                    </button>
                  </div>
                </div>
              </div>
            </SettingsGroup>
            <SettingsGroup title="Workspace" blurb="Where new worktrees branch from, and where this project sits in the sidebar.">
              <div class="set-rows">
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Default branch</div>
                    <div class="set-desc">Workspaces spin off this base. Prefer a remote tip (e.g. origin/main) when you want a fresh fetch at create.</div>
                  </div>
                  <div class="set-ctrl">
                    <Select
                      class="proj-sel"
                      value={form.baseBranch || branchOptions[0]?.value || ''}
                      options={branchOptions}
                      label="Default branch"
                      searchable
                      onchange={(v) => (form.baseBranch = v)}
                    >
                      {#snippet icon()}
                        <Icon name="git-branch" />
                      {/snippet}
                    </Select>
                  </div>
                </div>
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Group</div>
                    <div class="set-desc">Sidebar section this project appears under. Create groups from the Projects rail.</div>
                  </div>
                  <div class="set-ctrl">
                    <Select
                      class="proj-sel"
                      value={form.groupId ?? ''}
                      options={groupOptions}
                      label="Project group"
                      onchange={(v) => (form.groupId = v || undefined)}
                    >
                      {#snippet icon()}
                        <Icon name="folder" />
                      {/snippet}
                    </Select>
                  </div>
                </div>
                {#if selected}
                  <div class="set-row">
                    <div class="set-copy">
                      <div class="set-title">Hide from sidebar</div>
                      <div class="set-desc">Leaves this project out of the Projects rail. The eye button in the rail header shows hidden projects.</div>
                    </div>
                    <div class="set-ctrl">
                      <button type="button" class="tog" class:on={!!selected.hidden} aria-label="toggle hide from sidebar" aria-pressed={!!selected.hidden} onclick={() => void setProjectHidden(selected.id, !selected.hidden)}></button>
                    </div>
                  </div>
                {/if}
              </div>
            </SettingsGroup>
            <SettingsGroup title="Issues" blurb="Which assigned issues belong to this project. GitHub issues match this repo's git remotes automatically.">
              <div class="set-rows">
                <div class="set-row">
                  <div class="set-copy">
                    <div class="set-title">Linear projects</div>
                    <div class="set-desc">Comma-separated Linear project names or team keys. A project name match wins over a team key.</div>
                  </div>
                  <div class="set-ctrl">
                    <input class="inp" bind:value={form.linearProjects} placeholder="Mobile App, ENG" aria-label="Linear projects" />
                  </div>
                </div>
              </div>
            </SettingsGroup>
          </SettingsGroups>
        {/if}

        {#if section === 'schedules' && selected}
          {#key selected.id}<ScheduledPrompts project={selected} />{/key}
        {/if}

        {#if section === 'run'}
          {#if projectFileError}
            <div class="set-note warn" role="status">
              <Icon name="info" />
              <div>
                <div class="set-title">project.toml could not be parsed — editing run settings will replace it</div>
                <div class="set-desc">{projectFileError}</div>
              </div>
            </div>
          {/if}
          <SettingsGroups>
            <p class="set-lead">Run settings are saved to <code>.recur/project.toml</code>.</p>
            <SettingsGroup title="Files to copy" blurb="Copied into each new workspace before setup, skipping files that already exist in the worktree. Leave empty to copy nothing. A pattern without a slash matches at any depth. Heavy dirs (node_modules, dist, build, …) are skipped unless the pattern names them.">
              <div class="copy-files">
                <textarea
                  class="inp copy-files-pat"
                  value={form.copyFiles ?? ''}
                  placeholder={'# .env*\n# **/.env.local'}
                  spellcheck="false"
                  aria-label="Files to copy"
                  oninput={(e) => (form.copyFiles = e.currentTarget.value)}
                ></textarea>
                <div class="copy-files-meta">
                  <span
                    class={['copy-files-meta-text', copyPreview.error && !copyPreview.loading && 'err']}
                    title={copyFilesMeta}
                  >{copyFilesMeta}</span>
                  <button
                    type="button"
                    class="exclude-rm"
                    aria-label="Refresh matching files"
                    title="Refresh matching files"
                    disabled={copyPreview.loading || isNew}
                    onclick={() => (copyRefreshTick += 1)}
                  >
                    <Icon name="refresh" />
                  </button>
                </div>
                {#if copyPreview.files.length}
                  <ul class="copy-files-list" aria-label="Matching files">
                    {#each copyPreview.files as f (f)}
                      <li title={f}>{f}</li>
                    {/each}
                    {#if copyPreview.truncated}
                      <li class="copy-files-more">and {copyPreview.count - copyPreview.files.length} more</li>
                    {/if}
                  </ul>
                {/if}
              </div>
            </SettingsGroup>
            <SettingsGroup title="Setup script" blurb={'Bash script, once per worktree create. Gets $RECUR_PORT, $RECUR_WORKSPACE_ID, $RECUR_WORKTREE, $RECUR_PROJECT_ID, $RECUR_BRANCH, and project env.'}>
              <div class="set-rows">
                <div class="set-row set-row-stack">
                  <MiniScriptEditor
                    lang="shell"
                    value={form.setupCmd ?? ''}
                    placeholder={"# runs once when the worktree is created\npnpm install"}
                    minLines={4}
                    maxLines={14}
                    onChange={(v) => (form.setupCmd = v)}
                  />
                </div>
              </div>
            </SettingsGroup>
            <SettingsGroup title="Teardown script" blurb="Bash script when the worktree is removed (best-effort). Same env as setup.">
              <div class="set-rows">
                <div class="set-row set-row-stack">
                  <MiniScriptEditor
                    lang="shell"
                    value={form.teardownCmd ?? ''}
                    placeholder={"# runs once on remove\nrm -rf \"$HOME/Library/Application Support/Recur-$RECUR_WORKSPACE_ID\""}
                    minLines={3}
                    maxLines={12}
                    onChange={(v) => (form.teardownCmd = v)}
                  />
                </div>
              </div>
            </SettingsGroup>
            <SettingsGroup title="Run commands" blurb="Named scripts for the titlebar Run button. A command named dev is what verify-in-browser boots — without one a loop can't verify and won't open a PR. Pin the default from that dropdown — it stays on this machine.">
              <div class="run-list">
                {#each runRows as row (row.id)}
                  <div class="run-row">
                    <input class="inp run-name" bind:value={row.name} placeholder="dev" aria-label="Command name" />
                    <div class="run-cmd">
                      <MiniScriptEditor
                        lang="shell"
                        value={row.cmd}
                        placeholder={"pnpm dev --port $RECUR_PORT"}
                        minLines={1}
                        maxLines={10}
                        onChange={(v) => (row.cmd = v)}
                      />
                    </div>
                    <button type="button" class="exclude-rm" aria-label="Remove command" onclick={() => removeRunRow(row.id)}>
                      <Icon name="trash" />
                    </button>
                  </div>
                {/each}
                <div class="exclude-actions">
                  <button type="button" class="tbtn" onclick={addRunRow}><Icon name="plus" />Add command</button>
                </div>
              </div>
            </SettingsGroup>
            <SettingsGroup title="App URL" blurb={'Where the running app is reachable. $RECUR_PORT is filled in for each workspace.'}>
              <div class="set-rows">
                <div class="set-row set-row-stack">
                  <input class="inp cmd-in" bind:value={form.appURL} placeholder="http://localhost:$RECUR_PORT" aria-label="App URL" />
                </div>
              </div>
            </SettingsGroup>
            <SettingsGroup title="Environment" blurb="Dotenv script (KEY=value, # comments) injected into setup, teardown, runs, and terminals. Do not put secrets here. They are written into .recur/project.toml, which is meant to be committed. Keep API keys in gitignored .env files; copy them into worktrees and let the process load them. Recur does not auto-export those keys.">
              <div class="set-rows">
                <div class="set-row set-row-stack">
                  <MiniScriptEditor
                    lang="env"
                    value={form.env ?? ''}
                    placeholder={"# injected into setup, teardown, dev, and terminals\nWAILS_VITE_PORT=$RECUR_PORT\nRECUR_DATA_DIR=$HOME/Library/Application Support/Recur-$RECUR_WORKSPACE_ID"}
                    minLines={4}
                    maxLines={14}
                    onChange={(v) => (form.env = v)}
                  />
                  <p class="set-desc">Values may use <code>$RECUR_PORT</code>, <code>$RECUR_WORKSPACE_ID</code>, <code>$RECUR_WORKTREE</code>, <code>$RECUR_PROJECT_ID</code>, <code>$RECUR_BRANCH</code>, <code>$HOME</code>, and keys from gitignored <code>.env</code> files (expansion only). <code>project.toml</code> env ignores <code>PATH</code>, <code>HOME</code>, <code>SHELL</code>, <code>LD_*</code>, and <code>DYLD_*</code>.</p>
                </div>
              </div>
            </SettingsGroup>
          </SettingsGroups>
        {/if}

        {#if section === 'prs'}

          <div class="set-rows">
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Open PRs on GitHub</div>
                <div class="set-desc">Push the loop branch and open a real GitHub PR (needs the gh CLI signed in and an origin remote). Off = Local PRs reviewed in-app. Falls back to Local if GitHub isn't reachable.</div>
              </div>
              <div class="set-ctrl">
                <button type="button" class="tog" class:on={form.prOnGithub} aria-label="toggle GitHub PRs" aria-pressed={form.prOnGithub} onclick={() => (form.prOnGithub = !form.prOnGithub)}></button>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Delete branch when removing a workspace</div>
                <div class="set-desc">Deletes the workspace’s local git branch after the worktree is removed. Remote branches are left alone. Off by default.</div>
              </div>
              <div class="set-ctrl">
                <button type="button" class="tog" class:on={!!form.deleteBranchOnRemove} aria-label="toggle delete branch on remove" aria-pressed={!!form.deleteBranchOnRemove} onclick={() => (form.deleteBranchOnRemove = !form.deleteBranchOnRemove)}></button>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Merge method</div>
                <div class="set-desc">Override the global default for this project. Applies when merging PRs from any loop.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  <button type="button" class="mm-radio" class:on={(form.mergeMethod ?? '') === ''} onclick={() => (form.mergeMethod = '' as any)}>
                    <span class="mm-dot"></span>Use global default ({globalMergeLabel})
                  </button>
                  {#each [['merge', 'Create a merge commit'], ['squash', 'Squash and merge'], ['rebase', 'Rebase and merge']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={form.mergeMethod === val} onclick={() => (form.mergeMethod = val as any)}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Update with base</div>
                <div class="set-desc">How a PR branch catches up when you click Update with base.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  <button type="button" class="mm-radio" class:on={(form.updateBaseMethod ?? '') === ''} onclick={() => (form.updateBaseMethod = '' as any)}>
                    <span class="mm-dot"></span>Use global default ({globalUpdateLabel})
                  </button>
                  {#each [['merge', 'Merge base into branch'], ['rebase', 'Rebase onto base']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={form.updateBaseMethod === val} onclick={() => (form.updateBaseMethod = val as any)}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Squash commit message</div>
                <div class="set-desc">Presented when squash-merging a pull request.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  <button type="button" class="mm-radio" class:on={(form.squashCommitMessage ?? '') === ''} onclick={() => (form.squashCommitMessage = '' as any)}>
                    <span class="mm-dot"></span>Use global default ({globalSquashMsgLabel})
                  </button>
                  {#each [['title', 'Pull request title'], ['title-and-commits', 'Pull request title and commit details'], ['title-and-description', 'Pull request title and description']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={form.squashCommitMessage === val} onclick={() => (form.squashCommitMessage = val as any)}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
            <div class="set-row">
              <div class="set-copy">
                <div class="set-title">Merge commit message</div>
                <div class="set-desc">Presented when merging a pull request with a merge commit.</div>
              </div>
              <div class="set-ctrl">
                <div class="merge-method-radios">
                  <button type="button" class="mm-radio" class:on={(form.mergeCommitMessage ?? '') === ''} onclick={() => (form.mergeCommitMessage = '' as any)}>
                    <span class="mm-dot"></span>Use global default ({globalMergeMsgLabel})
                  </button>
                  {#each [['title', 'Pull request title'], ['title-and-description', 'Pull request title and description']] as [val, label]}
                    <button type="button" class="mm-radio" class:on={form.mergeCommitMessage === val} onclick={() => (form.mergeCommitMessage = val as any)}>
                      <span class="mm-dot"></span>{label}
                    </button>
                  {/each}
                </div>
              </div>
            </div>
          </div>
        {/if}

        {#if section === 'skills'}
          <SettingsGroups>
            <p class="set-lead">
              Repository skills for this project. Recur and personal skills follow
              <button type="button" class="lnk" style="display:inline;padding:0;border:none;background:none;font:inherit" onclick={() => openSettings('skills')}>Settings → Skills</button>
              unless you override them below.
            </p>
            {#if projectSkillsLoading}
              <div class="set-ctrl-text" style="max-width:none;text-align:left">Loading skills…</div>
            {:else if projectSkillsErr}
              <div class="set-ctrl-text" style="max-width:none;text-align:left">{projectSkillsErr}</div>
            {:else}
              {#if repoSkillGroup}
                <SettingsGroup title={repoSkillGroup.label} blurb={repoSkillGroup.blurb}>
                  <div class="skill-list">
                    {#each repoSkillGroup.skills as sk (sk.name + sk.path)}
                      {@const on = projectSkillOn(sk.name)}
                      {@const loc = skillLocation(sk.path, sk.scope)}
                      {@const hint = skillOverrideHint(sk.name, $settings?.disabledSkills, form.disabledSkills, form.enabledSkills)}
                      <div class="skill-row">
                        <div class="skill-copy">
                          <div class="skill-name">/{sk.name}</div>
                          {#if sk.description}
                            <div class="skill-desc">{sk.description}</div>
                          {/if}
                          {#if hint}
                            <div class="skill-desc">{hint}</div>
                          {/if}
                          {#if loc}
                            <div class="skill-path" title={sk.path}>{loc}</div>
                          {/if}
                        </div>
                        <button
                          type="button"
                          class="tog"
                          class:on={on}
                          aria-label="{on ? 'Disable' : 'Enable'} {sk.name}"
                          aria-pressed={on}
                          disabled={savingSkills}
                          onclick={() => setProjectSkillEnabled(sk.name, !on)}
                        ></button>
                      </div>
                    {/each}
                  </div>
                </SettingsGroup>
              {:else}
                <div class="set-ctrl-text" style="max-width:none;text-align:left">No skills found in this repository.</div>
              {/if}

              {#if skillsByKind.recur.length}
                {@const recurCount = kindOverrideCount('recur')}
                <SettingsGroup
                  title="Recur"
                  override={overrideRecur}
                  overrideLabel="Override Recur skills"
                  overrideDisabled={savingSkills}
                  onOverrideChange={(on) => setKindOverride('recur', on)}
                >
                  {#snippet blurbContent()}
                    {#if overrideRecur}
                      Customize for this project. Turn off to use Settings defaults{recurCount ? ` and clear ${recurCount} override${recurCount === 1 ? '' : 's'}` : ''}.
                    {:else}
                      Using Settings → Skills. Turn on to override for this project.
                    {/if}
                  {/snippet}
                  <div class="skill-list">
                    {#each skillsByKind.recur as sk (sk.name + sk.path)}
                      {@const on = projectSkillOn(sk.name)}
                      {@const hint = skillOverrideHint(sk.name, $settings?.disabledSkills, form.disabledSkills, form.enabledSkills)}
                      <div class="skill-row">
                        <div class="skill-copy">
                          <div class="skill-name">/{sk.name}</div>
                          {#if sk.description}
                            <div class="skill-desc">{sk.description}</div>
                          {/if}
                          {#if hint}
                            <div class="skill-desc">{hint}</div>
                          {/if}
                        </div>
                        <button
                          type="button"
                          class="tog"
                          class:on={on}
                          aria-label="{on ? 'Disable' : 'Enable'} {sk.name}"
                          aria-pressed={on}
                          disabled={savingSkills}
                          onclick={() => setProjectSkillEnabled(sk.name, !on)}
                        ></button>
                      </div>
                    {/each}
                  </div>
                </SettingsGroup>
              {/if}

              {#if skillsByKind.user.length}
                {@const userCount = kindOverrideCount('user')}
                <SettingsGroup
                  title="Your skills"
                  override={overrideUser}
                  overrideLabel="Override your skills"
                  overrideDisabled={savingSkills}
                  onOverrideChange={(on) => setKindOverride('user', on)}
                >
                  {#snippet blurbContent()}
                    {#if overrideUser}
                      Customize for this project. Turn off to use Settings defaults{userCount ? ` and clear ${userCount} override${userCount === 1 ? '' : 's'}` : ''}.
                    {:else}
                      Using Settings → Skills. Turn on to override for this project.
                    {/if}
                  {/snippet}
                  <div class="skill-list">
                    {#each skillsByKind.user as sk (sk.name + sk.path)}
                      {@const on = projectSkillOn(sk.name)}
                      {@const loc = skillLocation(sk.path, sk.scope)}
                      {@const hint = skillOverrideHint(sk.name, $settings?.disabledSkills, form.disabledSkills, form.enabledSkills)}
                      <div class="skill-row">
                        <div class="skill-copy">
                          <div class="skill-name">/{sk.name}</div>
                          {#if sk.description}
                            <div class="skill-desc">{sk.description}</div>
                          {/if}
                          {#if hint}
                            <div class="skill-desc">{hint}</div>
                          {/if}
                          {#if loc}
                            <div class="skill-path" title={sk.path}>{loc}</div>
                          {/if}
                        </div>
                        <button
                          type="button"
                          class="tog"
                          class:on={on}
                          aria-label="{on ? 'Disable' : 'Enable'} {sk.name}"
                          aria-pressed={on}
                          disabled={savingSkills}
                          onclick={() => setProjectSkillEnabled(sk.name, !on)}
                        ></button>
                      </div>
                    {/each}
                  </div>
                </SettingsGroup>
              {/if}
            {/if}
          </SettingsGroups>
        {/if}

        {#if section === 'mcp'}
          <SettingsGroups>
            <p class="set-lead">
              MCP tools for this project follow
              <button type="button" class="lnk" style="display:inline;padding:0;border:none;background:none;font:inherit" onclick={() => openSettings('mcp')}>Settings → MCP</button>
              unless you override them below.
            </p>
            <SettingsGroup title="Servers">
              <div class="skill-list">
                {#each mcpRows as row (row.id)}
                  {@const on = projectMCPOn(row.id)}
                  {@const hint = skillOverrideHint(row.id, mcpGlobalOff, form.disabledMCP, form.enabledMCP)}
                  <div class="skill-row">
                    <div class="skill-copy">
                      <div class="skill-name">{row.name}</div>
                      <div class="skill-desc">{row.detail}</div>
                      {#if hint}
                        <div class="skill-desc">{hint}</div>
                      {/if}
                    </div>
                    <button
                      type="button"
                      class="tog"
                      class:on={on}
                      aria-label="{on ? 'Disable' : 'Enable'} {row.name}"
                      aria-pressed={on}
                      disabled={savingSkills || isNew}
                      onclick={() => setProjectMCPEnabled(row.id, !on)}
                    ></button>
                  </div>
                {/each}
              </div>
            </SettingsGroup>
          </SettingsGroups>
        {/if}

        {#if section === 'danger' && !isNew}
          <div class="set-rows">
            <div class="set-row set-row-danger">
              <div class="set-copy">
                <div class="set-title">Delete this project</div>
                <div class="set-desc">Removes the project and all its workspaces, PRs and history. Your policy &amp; settings are kept. This can't be undone.</div>
              </div>
              <div class="set-ctrl">
                {#if !confirmingDelete}
                  <button type="button" class="tbtn set-danger-btn" onclick={() => (confirmingDelete = true)}>
                    <Icon name="trash" />
                    Delete this project
                  </button>
                {:else}
                  <div class="set-confirm">
                    <div class="set-confirm-msg">Delete “{form.name}” and everything under it?</div>
                    <div class="set-confirm-actions">
                      <button type="button" class="tbtn set-danger-btn solid" onclick={deleteProject} disabled={deleting}>{deleting ? 'Deleting…' : 'Delete project'}</button>
                      <button type="button" class="tbtn" onclick={() => (confirmingDelete = false)} disabled={deleting}>Cancel</button>
                    </div>
                  </div>
                {/if}
              </div>
            </div>
          </div>
        {/if}

      </div>
    </div>
  </div>
</section>
