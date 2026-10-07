<script lang="ts">
  // Scramble → resolve label animation for LLM renames (chat tabs, worktree titles).
  import { onDestroy, untrack } from 'svelte'

  let {
    text,
    /** When false, always show `text` immediately (no scramble). */
    animate = true,
    /** Stable identity for this label surface (workspace id, tab id, …).
     *  When it changes, snap without scramble — switching entities is not a rename.
     *  Text changes with the same key still animate (LLM renames). */
    sourceKey = '',
    class: className = '',
  }: {
    text: string
    animate?: boolean
    sourceKey?: string
    class?: string
  } = $props()

  // Glyph pool for the decode scramble — stay in the mono latin set UI labels use.
  const GLYPHS = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz0123456789'
  const TICK_MS = 28
  const HOLD_TICKS = 2 // ticks each column keeps scrambling before locking

  let display = $state(untrack(() => text))
  let decoding = $state(false)
  let seen = ''
  let seenKey = ''
  let timer: ReturnType<typeof setInterval> | undefined

  function preferReducedMotion() {
    return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
  }

  function stopDecode() {
    if (timer !== undefined) {
      clearInterval(timer)
      timer = undefined
    }
    decoding = false
  }

  function scrambleToward(target: string) {
    stopDecode()
    if (!target || preferReducedMotion()) {
      display = target
      return
    }
    decoding = true
    const len = target.length
    // Resolve left → right; column i locks after i*HOLD_TICKS + a small pad.
    let tick = 0
    const totalTicks = Math.max(8, len * HOLD_TICKS + 4)
    timer = setInterval(() => {
      tick += 1
      const locked = Math.min(len, Math.floor(tick / HOLD_TICKS))
      let out = ''
      for (let i = 0; i < len; i++) {
        const ch = target[i]
        if (i < locked || ch === ' ') {
          out += ch
        } else {
          out += GLYPHS[(Math.random() * GLYPHS.length) | 0]
        }
      }
      display = out
      if (tick >= totalTicks || locked >= len) {
        display = target
        stopDecode()
      }
    }, TICK_MS)
  }

  $effect(() => {
    const next = text
    const key = sourceKey
    if (!animate) {
      stopDecode()
      display = next
      seen = next
      seenKey = key
      return
    }
    // First paint, or a different entity reused this instance (e.g. workspace switch).
    if (!seen || (key !== '' && key !== seenKey)) {
      stopDecode()
      display = next
      seen = next
      seenKey = key
      return
    }
    if (seen === next) return
    seen = next
    scrambleToward(next)
  })

  onDestroy(stopDecode)
</script>

<span class={className} class:decoding aria-label={text}>{display}</span>

<style>
  .decoding {
    opacity: 0.85;
  }
</style>
