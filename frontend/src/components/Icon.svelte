<script lang="ts" module>
  export type { IconName } from '../lib/icons'
</script>

<script lang="ts">
  import { icons, type IconDef, type IconName } from '../lib/icons'

  let {
    name,
    svg = '',
    size,
    strokeWidth,
    class: className = '',
  }: {
    name: IconName
    /** Pack-authored SVG (complete-row icons). When set, `name` is unused. */
    svg?: string
    /** Optional explicit size (px number or CSS length). Omit to inherit from CSS. */
    size?: number | string
    strokeWidth?: number | string
    class?: string
  } = $props()

  const custom = $derived(!!svg.trim())
  const def: IconDef = $derived(icons[name])
  const filled = $derived(!!def.filled)
  const sw = $derived(strokeWidth ?? def.strokeWidth ?? 2)
  const vb = $derived(def.viewBox ?? '0 0 24 24')
  const box = $derived(
    size === undefined ? undefined : typeof size === 'number' ? `${size}px` : size,
  )
</script>

{#if custom}
  {@html svg}
{:else}
  <svg
    class={className}
    viewBox={vb}
    fill={filled ? 'currentColor' : 'none'}
    stroke={filled ? 'none' : 'currentColor'}
    stroke-width={filled ? undefined : sw}
    stroke-linecap={filled ? undefined : 'round'}
    stroke-linejoin={filled ? undefined : 'round'}
    aria-hidden="true"
    style:width={box}
    style:height={box}
  >
    {@html def.body}
  </svg>
{/if}
