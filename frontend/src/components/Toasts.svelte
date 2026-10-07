<script lang="ts">
  import { toasts, dismissToast, type Toast } from '../lib/stores'
  import Icon from './Icon.svelte'

  function activate(t: Toast) {
    if (!t.action) return
    dismissToast(t.id, { invokeOnDismiss: false })
    t.action.fn()
  }
</script>
<div class="toasts" aria-live="polite">
  {#each $toasts as t (t.id)}
    {#if t.ask}
      <div
        class="toast ask"
        role="status"
        style:--toast-ttl="{t.ttlMs ?? 30_000}ms"
      >
        <button type="button" class="toast-ask-go" onclick={() => activate(t)}>
          <div class="toast-ask-head">
            <Icon name="message-question" class="toast-ask-ico" />
            <span class="toast-ask-kicker">Question for you</span>
            {#if t.action}
              <span class="toast-ask-cta">{t.action.label}<span class="toast-ask-cta-chev" aria-hidden="true">›</span></span>
            {/if}
          </div>
          <div class="toast-ask-where" title="{t.ask.project} · {t.ask.workspace}">
            <span class="toast-ask-project">{t.ask.project}</span>
            <span class="toast-ask-sep" aria-hidden="true">/</span>
            <span class="toast-ask-ws">{t.ask.workspace}</span>
          </div>
          {#if t.ask.question}
            <p class="toast-ask-q">{t.ask.question}</p>
          {/if}
        </button>
        <button class="tclose" type="button" aria-label="Dismiss" onclick={() => dismissToast(t.id)}>✕</button>
        <span class="toast-ask-ttl" aria-hidden="true"></span>
      </div>
    {:else}
      <div class="toast {t.kind}" class:actionable={!!t.action} role={t.kind === 'error' ? 'alert' : 'status'}>
        {#if t.action}
          <button type="button" class="toast-go" onclick={() => activate(t)}>
            <span class="td"></span>
            <span class="tx">{t.text}</span>
            <span class="ta">{t.action.label}</span>
          </button>
        {:else}
          <span class="td"></span>
          <span class="tx">{t.text}</span>
        {/if}
        <button class="tclose" type="button" aria-label="Dismiss" onclick={() => dismissToast(t.id)}>✕</button>
      </div>
    {/if}
  {/each}
</div>
