<script lang="ts" module>
  /** Question ids we've already toasted — module-level so remounts don't spam. */
  const toasted = new Set<string>()
</script>

<script lang="ts">
  // Off-tab ask_user: compact toast (auto-dismiss, click to open) instead of a
  // large floating QuestionCard. Inline AgentChat still renders the answer UI
  // when that chat tab is focused.
  import {
    userQuestions, workspaces, projects, focusedChat,
    pushAskToast, openWorkspaceChat, agentChatOnScreen,
  } from '../lib/stores'
  import type { UserQuestion } from '../../bindings/recur/internal/domain/models'

  function truncate(s: string, n: number): string {
    const t = s.replace(/\s+/g, ' ').trim()
    if (t.length <= n) return t
    return t.slice(0, Math.max(0, n - 1)).trimEnd() + '…'
  }

  function askPayload(q: UserQuestion) {
    const ws = $workspaces.find((w) => w.id === q.workspaceId)
    const project =
      $projects.find((p) => p.id === ws?.projectId)?.name ||
      ws?.projectId ||
      'Project'
    const workspace = ws?.short || ws?.branch || q.workspaceId
    return {
      project,
      workspace,
      question: truncate(q.question || '', 96),
    }
  }

  $effect(() => {
    const focus = $focusedChat
    const onDetail = $agentChatOnScreen
    const pending = $userQuestions

    for (const q of pending) {
      const focusedHere =
        onDetail &&
        !!focus &&
        focus.workspaceId === q.workspaceId &&
        focus.code === q.agentCode
      // Already looking at the inline card — remember it so leaving the tab
      // doesn't fire a redundant "you have a question" toast.
      if (focusedHere) {
        toasted.add(q.id)
        continue
      }
      if (toasted.has(q.id)) continue
      toasted.add(q.id)
      const workspaceId = q.workspaceId
      const agentCode = q.agentCode
      pushAskToast(askPayload(q), {
        label: 'Open',
        fn: () => openWorkspaceChat(workspaceId, agentCode),
      })
    }

    for (const id of [...toasted]) {
      if (!pending.some((q) => q.id === id)) toasted.delete(id)
    }
  })
</script>
