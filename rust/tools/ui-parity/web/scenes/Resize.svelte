<script lang="ts">
  import ResizeHandle from '../../../../../frontend/src/components/ResizeHandle.svelte'
</script>

<div class="side"><ResizeHandle panel="rail" /></div>
<div class="dock"><ResizeHandle panel="terminalDock" axis="y" edge="start" /></div>

<style>
  .side { position: absolute; left: 0; top: 0; bottom: 0; width: 160px; background: var(--pane); border-right: 1px solid var(--line); }
  .dock { position: absolute; left: 161px; right: 0; bottom: 0; height: 60px; background: var(--rail); border-top: 1px solid var(--line); }
</style>
