<script lang="ts">
  import type { Snippet } from 'svelte'
  import { armedClick, armedPointerUp, createArmedConfirm, resetArmedConfirm } from '../lib/armed-confirm'
  let { label, confirmLabel, onconfirm, onarm, cls = 'tbtn', disabled = false, timeout = 3000, icon, title }:
    {
      label: string
      confirmLabel: string
      onconfirm: () => void
      /** Fired when the button arms (first click) — e.g. refresh confirm copy. */
      onarm?: () => void
      cls?: string
      disabled?: boolean
      timeout?: number
      icon?: Snippet
      title?: string
    } = $props()
  const gate = createArmedConfirm()
  let armed = $state(false)
  let timer: ReturnType<typeof setTimeout> | undefined
  $effect(() => {
    if (!disabled) return
    armed = false
    resetArmedConfirm(gate)
    clearTimeout(timer)
  })
  function arm() {
    armed = true
    onarm?.()
    timer = setTimeout(() => {
      resetArmedConfirm(gate)
      armed = false
    }, timeout)
  }
  function confirm() {
    clearTimeout(timer)
    armed = false
    onconfirm()
  }
  function onPointerUp(e: PointerEvent) {
    if (!armedPointerUp(gate, e.button, disabled)) return
    confirm()
  }
  function onClick() {
    const action = armedClick(gate, disabled)
    if (action === 'arm') arm()
    else if (action === 'confirm') confirm()
  }
</script>
<button class="{cls} danger" class:armed {disabled} {title} onpointerup={onPointerUp} onclick={onClick}>
  {#if icon}{@render icon()}{/if}{armed ? confirmLabel : label}
</button>
