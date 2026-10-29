<script lang="ts">
  // Floating tool-permission stack for requests the human is not already
  // answering in a chat transcript: unattended loop agents, and chat-surface
  // prompts when that tab is not focused. Chat prompts for the focused tab are
  // rendered inline by AgentChat instead.
  import {
    permissionRequests, workspaces, selectedWorkspaceId, focusedChat, go, agentChatOnScreen,
  } from '../lib/stores'
  import { answerPermission } from '../lib/api'
  import PermissionCard from './PermissionCard.svelte'

  // Prefer the oldest request that still needs the floating surface.
  const current = $derived.by(() => {
    const focus = $focusedChat
    const onDetail = $agentChatOnScreen
    return $permissionRequests.find((r) => {
      if (r.surface !== 'chat') return true
      if (!onDetail || !focus) return true
      return focus.workspaceId !== r.workspaceId || focus.code !== r.agentCode
    }) ?? null
  })
  const queued = $derived.by(() => {
    if (!current) return 0
    const focus = $focusedChat
    const onDetail = $agentChatOnScreen
    return $permissionRequests.filter((r) => {
      if (r.id === current.id) return false
      if (r.surface !== 'chat') return true
      if (!onDetail || !focus) return true
      return focus.workspaceId !== r.workspaceId || focus.code !== r.agentCode
    }).length
  })
  const workspaceName = $derived(
    $workspaces.find((w) => w.id === current?.workspaceId)?.short || current?.workspaceId || '',
  )

  function onKey(e: KeyboardEvent) {
    if (!current) return
    if (e.key === 'Escape') {
      e.preventDefault()
      void answerPermission(current.id, false, { reason: 'denied by the human' })
    }
  }

  function openWorkspace() {
    if (!current) return
    selectedWorkspaceId.set(current.workspaceId)
    go('detail')
  }
</script>

<svelte:window onkeydown={onKey} />

{#if current}
  <div class="perm-wrap" role="alertdialog" aria-label="Tool permission needed" aria-live="assertive">
    <PermissionCard
      request={current}
      queued={queued}
      whereLabel={workspaceName}
      onOpenWhere={openWorkspace}
    />
  </div>
{/if}
