<script lang="ts">
  import TitleTooltip from '../../../../../frontend/src/components/TitleTooltip.svelte'
</script>

<div class="top"><button class="tbtn" title="Open settings (⌘,)">Settings</button></div>
<div class="low"><button class="tbtn" title="Run the parity check against the WebKit reference">Check</button></div>
<TitleTooltip />

<style>
  .top { position: absolute; left: 16px; top: 16px; }
  .low { position: absolute; left: 120px; bottom: 16px; }
</style>
