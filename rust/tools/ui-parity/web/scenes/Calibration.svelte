<script lang="ts">
  // Text metrics calibration: one nowrap line per sample in calibration.json. capture.spec.ts
  // records each line's box and baseline; the Rust side lays out the same samples.
  import cal from '../../calibration.json'
  const FAMILY: Record<string, string> = {
    ui: 'var(--ui)', mono: 'var(--mono)', disp: 'var(--disp)',
    system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  }
</script>

<div class="col">
  {#each cal.samples as s}
    <div
      data-m={s.id}
      style:font-family={FAMILY[s.family]}
      style:font-size="{s.size}px"
      style:font-weight={s.weight}
      style:line-height={String(s.lineHeight)}
      style:letter-spacing={s.tracking ? `${s.tracking}em` : undefined}
      style:text-transform={s.uppercase ? 'uppercase' : undefined}
    >{cal.text}<span class="bl"></span></div>
  {/each}
</div>

<style>
  .col { display: flex; flex-direction: column; align-items: flex-start; padding: 8px; color: var(--text); }
  .col > div { white-space: nowrap; }
  .bl { display: inline-block; width: 0; height: 0; }
</style>
