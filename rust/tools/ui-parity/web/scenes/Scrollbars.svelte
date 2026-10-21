<script lang="ts">
  import { overlayScrollbar } from '../../../../../frontend/src/lib/overlay-scrollbar'
  const rows = Array.from({ length: 24 }, (_, i) => `Row ${i + 1}`)
</script>

<div class="wrap">
  <div class="box"><div class="list">{#each rows as r}<div class="r">{r}</div>{/each}</div></div>
  <div class="box"><div class="overlay-sb-shell"><div class="list" {@attach overlayScrollbar()}>{#each rows as r}<div class="r">{r}</div>{/each}</div></div></div>
</div>

<style>
  .wrap { padding: 16px; display: flex; gap: 16px; }
  .box { width: 160px; height: 160px; display: flex; flex-direction: column; background: var(--pane); }
  .list { flex: 1; min-height: 0; overflow-y: auto; }
  .r { padding: 4px 10px; font-size: 12px; }
</style>
