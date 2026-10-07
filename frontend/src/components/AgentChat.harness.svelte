<script lang="ts">
  import AgentChat, { type AgentMsg } from './AgentChat.svelte'
  import SessionCard from './SessionCard.svelte'

  let { messages = [] }: { messages?: AgentMsg[] } = $props()
</script>

<AgentChat {messages} readonly emptyText="">
  {#snippet sessionCard(log)}
    <SessionCard code={log.code} name={log.name} messages={log.messages} live={log.live} />
  {/snippet}
</AgentChat>
