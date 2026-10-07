<script lang="ts">
  import {
    prs, selectedPRId, view, prDetailTab, prReviewModal, prReviewDraft, prDiffJump,
    closePRReview, pushToast, type PRReviewKind,
  } from '../lib/stores'
  import { PRsService, call, patchPRPendingComments } from '../lib/api'
  import type { PendingLineComment } from '../../bindings/recur/internal/domain/models'
  import { commentHunkPathLabel } from '../lib/comment-hunk'
  import { get } from 'svelte/store'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import { cycleDialogTab, clearKeyboardFocus } from '../lib/dialog-focus'
  import { emojiShortcodes, emojiSuggest } from '../lib/emoji'
  import { userMentions, userMentionSuggest } from '../lib/user-mention'
  import Select, { type SelectOption } from './Select.svelte'
  import Icon from './Icon.svelte'

  const kindOptions: SelectOption[] = [
    { value: 'approve', label: 'Approve', icon: 'check' },
    { value: 'request', label: 'Request changes', icon: 'octagon-x' },
  ]

  let modalEl = $state<HTMLDivElement | null>(null)
  let bodyEl = $state<HTMLTextAreaElement | null>(null)
  let submitting = $state(false)

  const kind = $derived($prReviewModal)
  const pr = $derived($prs.find((p) => p.id === $selectedPRId))
  const pending = $derived((pr?.pendingComments ?? []) as PendingLineComment[])
  const pendingCount = $derived(pending.length)
  const isApprove = $derived(kind === 'approve')
  const title = $derived(isApprove ? 'Approve pull request' : 'Request changes')
  const submitLabel = $derived(isApprove ? 'Approve' : 'Request changes')
  const canSubmit = $derived(isApprove || !!$prReviewDraft.trim() || pendingCount > 0)

  function setKind(next: string) {
    if (next === 'approve' || next === 'request') prReviewModal.set(next as PRReviewKind)
  }

  function jumpToPending(pc: PendingLineComment) {
    closePRReview()
    prDetailTab.set('files')
    prDiffJump.set({ file: pc.file, line: pc.line })
  }

  async function removePending(pc: PendingLineComment) {
    if (!$selectedPRId) return
    const signed = (pc.side || 'RIGHT') === 'LEFT' ? -pc.line : pc.line
    const start = pc.startLine ?? 0
    const startSigned = start > 0 && start < pc.line
      ? ((pc.startSide || pc.side || 'RIGHT') === 'LEFT' ? -start : start)
      : 0
    patchPRPendingComments($selectedPRId, (prev) => prev.filter((x) => x.id !== pc.id))
    const ok = await call(PRsService.AddPRComment($selectedPRId, pc.file, signed, '', startSigned, pc.findingId || pc.id), 'Could not remove pending comment')
    if (ok === null) patchPRPendingComments($selectedPRId, (prev) => [...prev, pc])
  }

  async function submit() {
    if (!kind || submitting || !canSubmit || !$selectedPRId) return
    submitting = true
    const approved = kind === 'approve'
    const updated = await call(PRsService.SubmitReview($selectedPRId, kind, $prReviewDraft), approved ? 'Approve failed' : 'Request changes failed')
    submitting = false
    if (!updated) return
    prs.update((arr) => arr.map((p) => (p.id === updated.id ? updated : p)))
    prReviewDraft.set('')
    closePRReview()
    pushToast('success', approved ? 'Review approved' : 'Changes requested')
  }

  function onBodyKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && isCmdOrCtrl(e)) {
      e.preventDefault()
      void submit()
    }
  }

  $effect(() => {
    if (!kind) return
    if ($view !== 'prdetail') closePRReview()
  })

  $effect(() => {
    if (!kind) {
      submitting = false
      return
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Tab' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        // Emoji typeahead owns Tab. Don't cycle to Submit.
        if (e.defaultPrevented || get(emojiSuggest) || get(userMentionSuggest)) return
        const modal = modalEl
        if (!modal) return
        e.preventDefault()
        cycleDialogTab(modal, e.shiftKey)
        return
      }
      if (e.key !== 'Escape') return
      if (document.querySelector('#v-pr-review .sel.open')) return
      if (get(emojiSuggest) || get(userMentionSuggest)) return
      e.preventDefault()
      closePRReview()
    }
    window.addEventListener('keydown', onKey, true)
    const t = setTimeout(() => bodyEl?.focus(), 0)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      clearTimeout(t)
    }
  })
</script>

{#if kind}
  <div
    class="nt-bg"
    id="v-pr-review"
    onclick={(e) => { if (e.target === e.currentTarget) closePRReview() }}
    role="presentation"
  >
    <div
      class="nt-modal pr-rev-modal"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label={title}
      bind:this={modalEl}
      onpointerdown={() => { if (modalEl) clearKeyboardFocus(modalEl) }}
    >
      <div class="nt-head">
        <div class="nt-pickers">
          <Select value={kind} options={kindOptions} label="Review type" onchange={setKind} />
        </div>
        <button type="button" class="nt-x" tabindex="-1" onclick={closePRReview} aria-label="Close">
          <span class="esc">esc</span>
        </button>
      </div>

      <div class="nt-body">
        {#if pendingCount}
          <div class="pr-rev-pending" aria-label="Pending line comments">
            {#each pending as pc (pc.id)}
              <div class="pr-rev-chip">
                <div class="pr-rev-chip-main">
                  <button type="button" class="pr-rev-chip-loc" title="View in Files changed" onclick={() => jumpToPending(pc)}>{commentHunkPathLabel(pc.file, pc.line, pc.startLine)}</button>
                  <div class="pr-rev-chip-body">{pc.body}</div>
                </div>
                <button type="button" class="finding-note-del" title="Remove pending comment" aria-label="Remove pending comment" onclick={() => void removePending(pc)}>
                  <Icon name="trash" />
                </button>
              </div>
            {/each}
          </div>
        {/if}
        <textarea
          class="nt-in"
          bind:this={bodyEl}
          bind:value={$prReviewDraft}
          placeholder={isApprove ? 'Leave a comment (optional)…' : 'What should change?'}
          rows={5}
          {@attach emojiShortcodes((v) => prReviewDraft.set(v))}
          {@attach userMentions((v) => prReviewDraft.set(v))}
          onkeydown={onBodyKey}
        ></textarea>
      </div>

      <div class="nt-foot">
        {#if pendingCount}
          <span class="pr-rev-pending-hint">{pendingCount} line comment{pendingCount === 1 ? '' : 's'} will post with this review</span>
        {/if}
        <button type="button" class="nt-go" onclick={() => void submit()} disabled={submitting || !canSubmit}>
          {#if submitting}
            Submitting…
          {:else}
            {submitLabel}
            <span class="nt-ret">⌘↵</span>
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}
