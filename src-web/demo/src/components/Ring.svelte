<script lang="ts">
  // Progress ring with 5 iteration dots and a centered iteration count.
  let { size = 30, prog = 0, iter = 0, color = 'var(--amber)' }:
    { size?: number; prog?: number; iter?: number; color?: string } = $props()

  const svg = $derived.by(() => {
    const r = size / 2 - (size > 90 ? 7 : 5)
    const c = size / 2
    const circ = 2 * Math.PI * r
    const sw = size > 90 ? 7 : 4.5
    const off = circ * (1 - prog)
    let dots = ''
    for (let i = 0; i < 5; i++) {
      const a = ((-90 + i * 72) * Math.PI) / 180
      const fill = i / 5 <= prog ? color : '#2a3340'
      dots += `<circle cx="${c + r * Math.cos(a)}" cy="${c + r * Math.sin(a)}" r="${size > 90 ? 3.2 : 2.3}" fill="${fill}"/>`
    }
    return `<svg width="${size}" height="${size}" viewBox="0 0 ${size} ${size}"><circle cx="${c}" cy="${c}" r="${r}" fill="none" stroke="#1b212a" stroke-width="${sw}"/><circle cx="${c}" cy="${c}" r="${r}" fill="none" stroke="${color}" stroke-width="${sw}" stroke-linecap="round" stroke-dasharray="${circ}" stroke-dashoffset="${off}" transform="rotate(-90 ${c} ${c})"/>${dots}</svg>`
  })
</script>

<div class="ring" style="width:{size}px;height:{size}px">
  {@html svg}
  <div class="ctr">
    <div>
      <div class="it" style="font-size:{size > 90 ? '24px' : size < 42 ? '11px' : '13px'};color:{color}">{iter}</div>
      {#if size > 90}<div class="il" style="font-size:8px">iter</div>{/if}
    </div>
  </div>
</div>
