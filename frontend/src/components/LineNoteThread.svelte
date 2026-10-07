<script lang="ts">
  import { onDestroy, untrack } from 'svelte'
  import type { PR, PRComment } from '../../bindings/recur/internal/domain/models'
  import { selectedPRId, prs, pushToast, appOverlayOpen } from '../lib/stores'
  import { PRsService, call } from '../lib/api'
  import { commentPermalink } from '../lib/comment-permalink'
  import { copyText } from '../lib/clipboard'
  import { openExternalUrl } from '../lib/open-external'
  import { followMoreMenu, moreMenuFixedBox, portalToBody } from '../lib/menu-space'
  import { autosizeTextarea, PR_REPLY_AUTOSIZE } from '../lib/autosize-textarea'
  import { emojiShortcodes } from '../lib/emoji'
  import { userMentions } from '../lib/user-mention'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import { prConversationLocked } from '../lib/colors'
  import {
    PENDING_REPLY_PREFIX,
    canDeletePRComment,
    canEditPRComment,
    canReactPRComment,
    canResolveLineNote,
    commentSource,
    isDroppedComment,
    setThreadEditorDraft,
    settlePostedReply,
    threadEditorDraft,
    threadReplies,
  } from '../lib/comment-thread'
  import { setPRThreadResolved } from '../lib/pr-thread-resolved'
  import CommentReactions from './CommentReactions.svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'

  let { root, items, pr }: { root: PRComment; items: PRComment[]; pr?: PR } = $props()

  const replies = $derived(threadReplies(items, root.id))
  const canResolve = $derived(canResolveLineNote(pr, root))
  const canComment = $derived(!prConversationLocked(pr))
  const held = untrack(() => threadEditorDraft(root.id))
  let replyOpen = $state(held?.replyOpen ?? false)
  let replyDraft = $state(held?.reply ?? '')
  let resolving = $state(false)
  let moreMenuId = $state('')
  let moreMenuBox = $state<{ top: number; right: number } | null>(null)
  let moreTrigger: HTMLElement | null = $state(null)
  let moreHost: HTMLDivElement | undefined = $state()
  const pinMore = followMoreMenu(() => moreTrigger, (box) => { moreMenuBox = box }, () => closeMore())
  let editingId = $state(held?.editingId ?? '')
  let editDraft = $state(held?.edit ?? '')
  let saving = $state('')

  onDestroy(() => {
    setThreadEditorDraft(root.id, { replyOpen, reply: replyDraft, editingId, edit: editDraft })
  })

  function noteBody(c: PRComment): string {
    return commentSource(c)
  }

  function canEdit(c: PRComment) {
    return canEditPRComment(pr, c)
  }

  function canDelete(c: PRComment) {
    return canDeletePRComment(pr, c)
  }

  function hasMoreActions(c: PRComment) {
    if (canEdit(c) || canDelete(c)) return true
    if (commentSource(c)) return true
    return !!commentPermalink(pr?.url, c)
  }

  function openReply() {
    if (!canComment) return
    replyOpen = true
    queueMicrotask(() => document.getElementById(`diff-reply-${root.id}`)?.focus())
  }

  function patchComments(prId: string, next: (comments: PRComment[]) => PRComment[]) {
    prs.update((arr) => arr.map((p) => (p.id !== prId ? p : { ...p, comments: next(p.comments ?? []) })))
  }

  async function submitReply() {
    const body = replyDraft.trim()
    if (!body || !canComment) return
    const prId = $selectedPRId
    const login = pr?.viewerLogin || ''
    const author = login || 'You'
    const pending: PRComment = {
      id: `${PENDING_REPLY_PREFIX}${Date.now()}`,
      code: author.slice(0, 2).toUpperCase(),
      author,
      authorLogin: login,
      avatarCss: '',
      time: 'now',
      bodyHtml: '',
      bodyMd: body,
      verb: 'commented',
      kind: 'comment',
      inReplyToId: root.id,
      parentId: root.parentId,
      path: root.path,
      line: root.line,
      side: root.side,
      threadId: root.threadId || root.id,
      resolved: root.resolved,
    }
    patchComments(prId, (comments) => [...comments, pending])
    replyOpen = false
    replyDraft = ''
    const posted = await call(PRsService.ReplyPRComment(prId, root.id, body), 'Could not reply')
    if (posted?.id) {
      patchComments(prId, (rows) => settlePostedReply(rows, pending.id, posted))
      pushToast('success', 'Reply posted')
      return
    }
    patchComments(prId, (rows) => settlePostedReply(rows, pending.id, null))
  }

  function stamp(c: PRComment) {
    const href = commentPermalink(pr?.url, c)
    return { href, time: c.time }
  }

  async function setResolved(resolved: boolean) {
    if (resolving) return
    resolving = true
    await setPRThreadResolved(root, resolved)
    resolving = false
  }

  function closeMore() {
    moreMenuId = ''
    moreMenuBox = null
    moreTrigger = null
  }

  $effect(() => {
    if ($appOverlayOpen) closeMore()
  })

  function toggleMore(id: string, e: MouseEvent) {
    e.stopPropagation()
    if (moreMenuId === id) {
      closeMore()
      return
    }
    moreTrigger = e.currentTarget as HTMLElement
    moreMenuId = id
    moreMenuBox = moreMenuFixedBox(moreTrigger.getBoundingClientRect())
  }

  function moreTarget(): PRComment | undefined {
    if (!moreMenuId) return
    if (root.id === moreMenuId) return root
    return replies.find((c) => c.id === moreMenuId)
  }

  function quoteReply(c: PRComment) {
    if (!canComment) return
    const src = commentSource(c)
    if (!src) return
    const quoted = src.split('\n').map((line) => `> ${line}`).join('\n') + '\n\n'
    replyDraft = quoted + (replyOpen ? replyDraft : '')
    replyOpen = true
    queueMicrotask(() => document.getElementById(`diff-reply-${root.id}`)?.focus())
  }

  function startEdit(c: PRComment) {
    editingId = c.id
    editDraft = commentSource(c)
  }

  async function saveEdit(id: string) {
    const body = editDraft.trim()
    if (!body || saving) return
    saving = id
    const updated = await call(PRsService.UpdatePRComment($selectedPRId, id, body), 'Could not update comment')
    saving = ''
    if (!updated) return
    patchComments($selectedPRId, (rows) => rows.map((c) => (c.id === id ? { ...c, ...updated, bodyMd: body } : c)))
    editingId = ''
    pushToast('success', 'Comment updated')
  }

  async function deleteComment(id: string) {
    if (saving || !id) return
    const prId = $selectedPRId
    const fromPR = ($prs.find((p) => p.id === prId)?.comments ?? []).filter((c) => isDroppedComment(c, id))
    if (!fromPR.length) return
    saving = `del-${id}`
    patchComments(prId, (rows) => rows.filter((c) => !isDroppedComment(c, id)))
    if (editingId === id) editingId = ''
    const updated = await call(PRsService.DeletePRComment(prId, id), 'Could not delete comment')
    saving = ''
    if (!updated) {
      patchComments(prId, (rows) => [...rows, ...fromPR])
      return
    }
    prs.update((arr) => arr.map((p) => (p.id === updated.id ? updated : p)))
    pushToast('success', 'Comment deleted')
  }

  function moreEdit() {
    const c = moreTarget()
    closeMore()
    if (c) startEdit(c)
  }

  function moreQuote() {
    const c = moreTarget()
    closeMore()
    if (c) quoteReply(c)
  }

  function morePermalink() {
    return commentPermalink(pr?.url, moreTarget())
  }

  async function moreCopyLink() {
    const link = morePermalink()
    closeMore()
    if (!link) return
    await copyText(link)
    pushToast('info', 'Copied link')
  }

  async function moreOpenGitHub() {
    const link = morePermalink()
    closeMore()
    if (!link) return
    await openExternalUrl(link)
  }

  function moreDelete() {
    const c = moreTarget()
    closeMore()
    if (c) void deleteComment(c.id)
  }

  function onMoreDown(ev: MouseEvent) {
    if (!moreMenuId) return
    const el = ev.target as HTMLElement | null
    if (el?.closest?.('.cmt-more-menu')) return
    const more = el?.closest?.('.cmt-more')
    if (more && moreHost?.contains(more)) return
    closeMore()
  }

  function onMoreKey(ev: KeyboardEvent) {
    if (!moreMenuId || ev.key !== 'Escape') return
    ev.preventDefault()
    ev.stopPropagation()
    closeMore()
  }

  function composeKey(e: KeyboardEvent, save: () => void, cancel: () => void) {
    if (e.key === 'Enter' && isCmdOrCtrl(e)) {
      e.preventDefault()
      save()
      return
    }
    if (e.key === 'Escape') {
      e.preventDefault()
      cancel()
    }
  }
</script>

<svelte:window onmousedowncapture={onMoreDown} onkeydowncapture={onMoreKey} />

{#snippet note(c: PRComment)}
  {@const s = stamp(c)}
  <div class="cmt-kid-note">
    <div class="ca" style={c.avatarCss}>
      {c.code}
      {#if c.avatarUrl}<img src={c.avatarUrl} alt="" />{/if}
    </div>
    <div class="cmt-kid-cc">
      <div class="ch">
        <span class="cn">{c.author}</span>
        {#if s.href && s.time}
          <a class="ct" href={s.href} title="Open on GitHub">{s.time}</a>
        {:else if s.time}
          <span class="ct">{s.time}</span>
        {/if}
        {#if editingId !== c.id && hasMoreActions(c)}
          <div class="cmt-more" class:open={moreMenuId === c.id}>
            <button type="button" class="cmt-act" title="More" aria-label="More" aria-haspopup="menu" aria-expanded={moreMenuId === c.id} onclick={(e) => toggleMore(c.id, e)}><Icon name="ellipsis" /></button>
          </div>
        {/if}
      </div>
      {#if editingId === c.id}
        <div class="finding-note-compose cmt-edit">
          <textarea class="finding-note-input" bind:value={editDraft} placeholder="Edit comment" disabled={saving === c.id} {@attach autosizeTextarea(editDraft, PR_REPLY_AUTOSIZE)} {@attach emojiShortcodes((v) => { editDraft = v })} {@attach userMentions((v) => { editDraft = v })} onkeydown={(e) => composeKey(e, () => void saveEdit(c.id), () => { editingId = '' })}></textarea>
          <div class="finding-note-compose-foot">
            <span class="finding-note-hint"><Kbd text="⌘↵" /> to save</span>
            <div class="finding-note-compose-actions">
              <button type="button" class="finding-note-btn" disabled={saving === c.id} onclick={() => (editingId = '')}>Cancel</button>
              <button type="button" class="finding-note-btn primary" disabled={!editDraft.trim() || saving === c.id} onclick={() => void saveEdit(c.id)}>{saving === c.id ? 'Saving…' : 'Save'}</button>
            </div>
          </div>
        </div>
      {:else if noteBody(c)}
        <MarkdownHtml text={noteBody(c)} class="chat-canvas cm-md" />
      {/if}
      {#if editingId !== c.id}
        <CommentReactions comment={c} canReact={canReactPRComment(pr, c)} />
      {/if}
    </div>
  </div>
{/snippet}

<div class="icmt-thread-inner" bind:this={moreHost}>
  <div class="cmt-kid" class:resolved={!!root.resolved}>
    {#if root.resolved}
      <div class="cmt-resolved-bar">
        <span>Resolved</span>
        {#if canResolve}
          <button type="button" class="finding-note-btn cmt-resolve" disabled={resolving} onclick={() => void setResolved(false)}>Unresolve</button>
        {/if}
      </div>
    {/if}
    {@render note(root)}
    {#each replies as reply (reply.id)}
      {@render note(reply)}
    {/each}
    <div class="cmt-kid-foot">
      <div class="cmt-reply">
        {#if replyOpen && canComment}
          <div class="finding-note-compose cmt-edit">
            <textarea id={`diff-reply-${root.id}`} class="finding-note-input" bind:value={replyDraft} placeholder="Reply…" {@attach autosizeTextarea(replyDraft, PR_REPLY_AUTOSIZE)} {@attach emojiShortcodes((v) => { replyDraft = v })} {@attach userMentions((v) => { replyDraft = v })} onkeydown={(e) => {
              if (e.key === 'Enter' && isCmdOrCtrl(e)) { e.preventDefault(); void submitReply() }
              if (e.key === 'Escape') { e.preventDefault(); replyOpen = false }
            }}></textarea>
            <div class="finding-note-compose-foot">
              <span class="finding-note-hint"><Kbd text="⌘↵" /> to save</span>
              <div class="finding-note-compose-actions">
                <button type="button" class="finding-note-btn" onclick={() => (replyOpen = false)}>Cancel</button>
                <button type="button" class="finding-note-btn primary" disabled={!replyDraft.trim()} onclick={() => void submitReply()}>Reply</button>
              </div>
            </div>
          </div>
        {:else if canComment}
          <button type="button" class="cmt-reply-ph" onclick={openReply}>Reply…</button>
        {/if}
      </div>
      {#if canResolve && !root.resolved}
        <button type="button" class="finding-note-btn cmt-resolve" disabled={resolving} onclick={() => void setResolved(true)}>Resolve conversation</button>
      {/if}
    </div>
  </div>
</div>

{#if moreMenuId && moreMenuBox}
  {@const target = moreTarget()}
  {@const canQuote = canComment && !!target && !!commentSource(target)}
  {@const link = commentPermalink(pr?.url, target)}
  <div class="cmt-more-menu" style="top:{moreMenuBox.top}px;right:{moreMenuBox.right}px" role="menu" tabindex="-1" {@attach pinMore} {@attach portalToBody} onmousedown={(e) => e.stopPropagation()}>
    {#if link}
      <button type="button" class="cmt-more-item" role="menuitem" onclick={() => void moreOpenGitHub()}><Icon name="github" />Open in GitHub</button>
      <button type="button" class="cmt-more-item" role="menuitem" onclick={() => void moreCopyLink()}><Icon name="copy" />Copy link</button>
    {/if}
    {#if canQuote}
      <button type="button" class="cmt-more-item" role="menuitem" onclick={moreQuote}><Icon name="quote" />Quote reply</button>
    {/if}
    {#if target && canEdit(target)}
      <button type="button" class="cmt-more-item" role="menuitem" onclick={moreEdit}><Icon name="pencil" />Edit</button>
    {/if}
    {#if target && canDelete(target)}
      <button type="button" class="cmt-more-item danger" role="menuitem" onclick={moreDelete}><Icon name="trash" />Delete</button>
    {/if}
  </div>
{/if}
