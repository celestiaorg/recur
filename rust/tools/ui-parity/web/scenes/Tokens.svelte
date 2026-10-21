<script lang="ts">
  // Every _tokens.scss custom property as a 36px swatch, painted over --bg.
  const names = [
    'bg', 'pane', 'pane-2', 'rail', 'titlebar', 'statusbar', 'line',
    'line-soft', 'line-bright', 'text', 'muted', 'faint', 'amber', 'amber-d',
    'cyan', 'emerald', 'coral', 'coral-d', 'indigo', 'warn', 'coral-line',
    'git-m', 'git-a', 'git-d', 'git-u', 'scrollbar-thumb', 'scrollbar-thumb-hover', 'on-amber',
  ]
</script>

<div class="grid">
  {#each names as n}<div class="sw" style:background="var(--{n})"></div>{/each}
</div>

<style>
  .grid { display: grid; grid-template-columns: repeat(7, 36px); gap: 4px; padding: 8px; }
  .sw { width: 36px; height: 36px; }
</style>
