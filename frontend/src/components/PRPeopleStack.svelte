<script lang="ts">
  import type { PR } from '../../bindings/recur/internal/domain/models'
  import {
    participantAvatarUrl,
    participantInitials,
    participantLabel,
    personKey,
    prFaceSlots,
    prPeopleWithReviews,
    reviewStateLabel,
    type PRPerson,
  } from '../lib/pr-participants'

  let { pr, people: given }: { pr?: PR; people?: PRPerson[] } = $props()

  const people = $derived(given ?? (pr ? prPeopleWithReviews(pr) : []))
  const faces = $derived(prFaceSlots(people))

  function hideBrokenAvatar(e: Event) {
    const el = e.currentTarget
    if (el instanceof HTMLImageElement) el.style.display = 'none'
  }
</script>

<div class="pr-people" data-ui-tip="off">
  {#if people.length}
    <div class="pr-faces" aria-label="Participants: {people.map((person) => [participantLabel(person), reviewStateLabel(person.reviewState)].filter(Boolean).join(' — ')).filter(Boolean).join(', ')}">
      {#each faces.shown as person (personKey(person))}
        {@const url = participantAvatarUrl(person)}
        {@const rev = person.reviewState}
        <span class="pr-face" class:rev-ok={rev === 'approved'} class:rev-ask={rev === 'changes_requested'} class:rev-wait={rev === 'pending'} style={person.avatarCss || undefined}>
          {participantInitials(person)}
          {#if url}<img src={url} alt="" onerror={hideBrokenAvatar} />{/if}
          {#if rev === 'approved' || rev === 'changes_requested' || rev === 'pending'}
            <span class="pr-face-mark" class:ok={rev === 'approved'} class:ask={rev === 'changes_requested'} class:wait={rev === 'pending'}>{rev === 'approved' ? '✓' : rev === 'changes_requested' ? '✕' : ''}</span>
          {/if}
        </span>
      {/each}
      {#if faces.extra}
        <span class="pr-face pr-face-more">+{faces.extra}</span>
      {/if}
    </div>
    <div class="pr-people-pop" role="tooltip">
      {#each people as person (personKey(person))}
        {@const url = participantAvatarUrl(person)}
        {@const state = person.reviewState}
        {@const rev = reviewStateLabel(state)}
        <div class="pr-people-row">
          <span class="pr-face" style={person.avatarCss || undefined}>
            {participantInitials(person)}
            {#if url}<img src={url} alt="" onerror={hideBrokenAvatar} />{/if}
            {#if state === 'approved' || state === 'changes_requested' || state === 'pending'}
              <span class="pr-face-mark" class:ok={state === 'approved'} class:ask={state === 'changes_requested'} class:wait={state === 'pending'}>{state === 'approved' ? '✓' : state === 'changes_requested' ? '✕' : ''}</span>
            {/if}
          </span>
          <span class="pr-people-meta">
            <span class="pr-people-name">{participantLabel(person)}</span>
            {#if rev}<span class="pr-people-rev" class:rev-ok={state === 'approved'} class:rev-ask={state === 'changes_requested'} class:rev-wait={state === 'pending'}>{rev}</span>{/if}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</div>
