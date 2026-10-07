<script lang="ts" module>
  export type ImageCtxMenu = { x: number; y: number; src: string }

  /** Build a clamped menu position from a contextmenu event. */
  export function openImageCtx(e: MouseEvent, src: string): ImageCtxMenu {
    e.preventDefault()
    e.stopPropagation()
    const pad = 8
    const mw = 180
    const mh = 40
    let x = e.clientX
    let y = e.clientY
    if (x + mw > window.innerWidth - pad) x = window.innerWidth - mw - pad
    if (y + mh > window.innerHeight - pad) y = window.innerHeight - mh - pad
    return { x: Math.max(pad, x), y: Math.max(pad, y), src }
  }
</script>

<script lang="ts">
  import { copyImage } from '../lib/clipboard'
  import { portalToBody } from '../lib/menu-space'
  import { pushToast } from '../lib/stores'
  import Kbd from './Kbd.svelte'

  let {
    menu = $bindable<ImageCtxMenu | null>(null),
  }: {
    menu?: ImageCtxMenu | null
  } = $props()

  function close() {
    menu = null
  }

  async function onCopy() {
    const src = menu?.src
    close()
    if (!src) return
    try {
      await copyImage(src)
      pushToast('info', 'Copied image')
    } catch {
      pushToast('error', 'Could not copy image')
    }
  }

  $effect(() => {
    if (!menu) return
    const onDown = (ev: MouseEvent) => {
      const el = ev.target as HTMLElement | null
      if (el?.closest?.('.att-ctx')) return
      close()
    }
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== 'Escape') return
      ev.preventDefault()
      ev.stopPropagation()
      close()
    }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey, true)
    }
  })
</script>

{#if menu}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="att-ctx"
    style="left:{menu.x}px;top:{menu.y}px"
    role="menu"
    tabindex="-1"
    {@attach portalToBody}
    onmousedown={(e) => e.stopPropagation()}
  >
    <button type="button" role="menuitem" onclick={onCopy}>
      <span class="att-ctx-label">Copy Image</span>
      <Kbd class="att-ctx-kbd" text="⌘C" />
    </button>
  </div>
{/if}
