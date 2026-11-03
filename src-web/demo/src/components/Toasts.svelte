<script lang="ts">
  import { toasts, dismissToast } from '../lib/stores'
</script>
<div class="toasts" aria-live="polite">
  {#each $toasts as t (t.id)}
    <div class="toast {t.kind}" role={t.kind === 'error' ? 'alert' : 'status'}>
      <span class="td"></span>
      <span class="tx">{t.text}</span>
      <button class="tclose" type="button" aria-label="Dismiss" onclick={() => dismissToast(t.id)}>✕</button>
    </div>
  {/each}
</div>
