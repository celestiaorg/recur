<script lang="ts">
  // Titlebar issue control: opens the linked issue, and its dropdown searches,
  // links, changes, or unlinks it. Hidden when there is no link and nothing to link.
  import { linkWorkspaceIssue, unlinkWorkspaceIssue } from '../lib/api'
  import { issueKey, issueUrl, openIssueUrl, trackerIssues } from '../lib/issue-link'
  import type { Workspace } from '../../bindings/recur/internal/domain/models'
  import Select from './Select.svelte'
  import Icon from './Icon.svelte'

  let { workspace }: { workspace: Workspace } = $props()

  const issue = $derived(workspace.issue)
  const current = $derived(issue ? issueKey(issue) : '')
  const url = $derived(issueUrl(issue, $trackerIssues))
  const github = $derived(issue?.source === 'github')
  const label = $derived(github ? `#${(issue?.key || '').split('#').pop()}` : issue?.key || 'issue')
  const mine = $derived($trackerIssues.filter((i) => i.projectId === workspace.projectId))
  const options = $derived.by(() => {
    const out = mine.map((i) => ({ value: issueKey(i), label: `${i.ref} ${i.title}` }))
    if (issue && !out.some((o) => o.value === current)) out.unshift({ value: current, label: issue.key || issue.id })
    return issue ? [{ value: '', label: 'Unlink issue' }, ...out] : out
  })

  function onchange(next: string) {
    if (next === current) return
    if (!next) return void unlinkWorkspaceIssue(workspace.id)
    const i = mine.find((x) => issueKey(x) === next)
    if (i) void linkWorkspaceIssue(workspace.id, { source: i.source, id: i.id, key: i.ref })
  }
</script>

{#snippet picker()}
  <Select class="issue-pick" label={issue ? 'Change linked issue' : 'Link issue'} searchable placeholder="Link issue" value={current} {options} {onchange}>
    {#snippet icon()}<Icon name="circle-dot" />{/snippet}
  </Select>
{/snippet}

{#if issue && url}
  <div class="update-split issue-split">
    <button
      type="button"
      class="tbtn"
      title="Open {issue.key || 'this issue'} in {github ? 'GitHub' : 'Linear'}"
      onclick={() => void openIssueUrl(issue.source, url)}
    ><Icon name={github ? 'github' : 'linear'} />View {label}</button>
    {@render picker()}
  </div>
{:else if options.length}
  {@render picker()}
{/if}
