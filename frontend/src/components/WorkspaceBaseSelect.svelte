<script lang="ts">
  // Target-branch picker for an existing workspace: the same choice as the
  // new-workspace "Base branch" select, changeable after creation. Changing it
  // retargets diffs / open PRs; it does not rebase the worktree.
  import { projects, workspaces, pushToast } from '../lib/stores'
  import { ProjectsService, WorkspacesService, call } from '../lib/api'
  import Select from './Select.svelte'
  import Icon from './Icon.svelte'
  import type { BranchChoice, Workspace } from '../../bindings/recur/internal/domain/models'

  let { workspace }: { workspace: Workspace } = $props()

  // Primitives — not the workspace object. File edits / agent ticks replace
  // that object via workspace.updated; depending on it refetched and flashed
  // the picker disabled. workspaceId still reloads when navigating to another
  // loop (App reuses this instance across non-main workspaces).
  const projectId = $derived(workspace.projectId)
  const workspaceId = $derived(workspace.id)
  const project = $derived($projects.find((p) => p.id === projectId))
  const current = $derived(
    (workspace.baseBranch || project?.baseBranch || 'main').trim(),
  )

  let branches = $state<BranchChoice[]>([])
  let loadedProjectId = $state<string | null>(null)
  let saving = $state(false)

  $effect(() => {
    const id = projectId
    void workspaceId
    if (!id) return
    let cancelled = false
    ProjectsService.ListBranches(id)
      .then((list) => {
        if (!cancelled) {
          branches = list ?? []
          loadedProjectId = id
        }
      })
      .catch(() => {
        if (!cancelled) {
          branches = []
          loadedProjectId = id
        }
      })
    return () => {
      cancelled = true
    }
  })

  const options = $derived.by(() => {
    const own = workspace.branch
    const seen = new Set<string>()
    const out: { value: string; label: string; group?: string }[] = []
    const add = (name: string, group?: string) => {
      if (!name || name === own || seen.has(name)) return
      seen.add(name)
      out.push({ value: name, label: name, group })
    }
    for (const b of branches) add(b.name, b.group)
    if (current && current !== own && !seen.has(current)) {
      out.unshift({ value: current, label: current, group: 'Local' })
    }
    return out
  })

  async function onchange(next: string) {
    if (!next || next === current || saving) return
    saving = true
    const updated = await call(
      WorkspacesService.SetWorkspaceBaseBranch(workspace.id, next),
      'Could not change the target branch',
    )
    saving = false
    if (!updated) return
    workspaces.update((arr) => arr.map((w) => (w.id === updated.id ? updated : w)))
    pushToast('success', `Target branch is now ${updated.baseBranch || next}`)
  }
</script>

<Select
  class="crumb-base"
  label="Target branch"
  searchable
  value={current}
  {options}
  disabled={saving || loadedProjectId !== projectId || options.length === 0}
  {onchange}
>
  {#snippet icon()}
    <Icon name="git-branch" />
  {/snippet}
</Select>
