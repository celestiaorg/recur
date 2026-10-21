<script lang="ts">
  import Kbd from '../../../../../frontend/src/components/Kbd.svelte'
  const keys = ['⌘K', '⌘⇧P', '⌥←', 'Ctrl+K', 'esc', '↵']
</script>

<div class="wrap">
  <div class="row hintrow">
    {#each keys as k}<Kbd text={k} />{/each}
  </div>
  <div class="row">
    {#each keys as k, i}<span class="pal-row" class:sel={i === 1}><Kbd class="pk" text={k} /></span>{/each}
  </div>
  <div class="row">
    <div class="pal-foot">
      <span><span class="k">↑↓</span>navigate</span><span><span class="k">↵</span>open</span>
    </div>
  </div>
  <div class="row">
    <div class="pal-in"><span class="esc">esc</span></div>
  </div>
</div>

<style>
  .wrap { padding: 12px; display: flex; flex-direction: column; gap: 10px; }
  .row { display: flex; align-items: center; gap: 10px; }
  .hintrow { font-size: 11px; color: var(--faint); }
</style>
