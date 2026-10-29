<script lang="ts">
  import type { PR, PRComment } from '../lib/types'
  import { commentPermalink } from '../lib/comment-permalink'
  import { commentSource, threadReplies } from '../lib/comment-thread'
  import CommentReactions from './CommentReactions.svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'

  let { root, items, pr }: { root: PRComment; items: PRComment[]; pr?: PR } = $props()

  const replies = $derived(threadReplies(items, root.id))
</script>

{#snippet note(c: PRComment)}
  {@const href = commentPermalink(pr?.url, c)}
  <div class="cmt-kid-note">
    <div class="ca" style={c.avatarCss}>
      {c.code}
      {#if c.avatarUrl}<img src={c.avatarUrl} alt="" />{/if}
    </div>
    <div class="cmt-kid-cc">
      <div class="ch">
        <span class="cn">{c.author}</span>
        {#if href && c.time}
          <a class="ct" {href} title="Open on GitHub">{c.time}</a>
        {:else if c.time}
          <span class="ct">{c.time}</span>
        {/if}
      </div>
      {#if commentSource(c)}
        <MarkdownHtml text={commentSource(c)} class="chat-canvas cm-md" />
      {/if}
      <CommentReactions comment={c} />
    </div>
  </div>
{/snippet}

<div class="icmt-thread-inner">
  <div class="cmt-kid" class:resolved={!!root.resolved}>
    {#if root.resolved}
      <div class="cmt-resolved-bar"><span>Resolved</span></div>
    {/if}
    {@render note(root)}
    {#each replies as reply (reply.id)}
      {@render note(reply)}
    {/each}
  </div>
</div>
