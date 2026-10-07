<script lang="ts">
  import {
    contextUsedTokens,
    contextUsageReady,
    formatTokens,
    type ContextUsage,
  } from '../lib/contextUsage'

  let {
    usage = null,
  }: {
    usage?: ContextUsage | null
  } = $props()

  let open = $state(false)
  let rootEl: HTMLDivElement | undefined = $state()

  const ready = $derived(contextUsageReady(usage))
  const used = $derived(contextUsedTokens(usage))
  const limit = $derived(usage?.contextWindow || 0)
  const pct = $derived(ready && limit > 0 ? Math.min(1, used / limit) : 0)
  const pctLabel = $derived(Math.round(pct * 100))
  const free = $derived(Math.max(0, limit - used))
  const cacheRead = $derived(usage?.cacheReadTokens || 0)
  const fresh = $derived((usage?.inputTokens || 0) + (usage?.cacheCreationTokens || 0))
  const showSplit = $derived(ready && (cacheRead > 0 || (usage?.cacheCreationTokens || 0) > 0))
  const compacting = $derived(!!usage?.compacting)
  const compacted = $derived(!!usage?.compacted)

  const size = 14
  const sw = 2.25
  const r = size / 2 - sw
  const c = size / 2
  const circ = 2 * Math.PI * r
  const dashOff = $derived(circ * (1 - pct))

  const ringColor = $derived(
    !ready ? 'var(--line)' : compacting ? 'var(--amber)' : pct >= 0.9 ? 'var(--coral)' : pct >= 0.7 ? 'var(--amber)' : 'var(--muted)',
  )

  function toggle(e: MouseEvent) {
    e.stopPropagation()
    if (!ready) return
    open = !open
  }

  function onDocPointer(e: PointerEvent) {
    if (!open || !rootEl) return
    if (e.target instanceof Node && rootEl.contains(e.target)) return
    open = false
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && open) {
      open = false
      e.stopPropagation()
    }
  }

  $effect(() => {
    if (!ready && open) open = false
  })

  $effect(() => {
    if (!open) return
    document.addEventListener('pointerdown', onDocPointer, true)
    document.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('pointerdown', onDocPointer, true)
      document.removeEventListener('keydown', onKey, true)
    }
  })
</script>

<div class="ctx-meter" class:open bind:this={rootEl}>
  <button
    type="button"
    class="ctx-btn"
    class:compacting
    disabled={!ready}
    aria-label={ready
      ? compacting
        ? `Compacting context, ${pctLabel}% full, ${formatTokens(used)} of ${formatTokens(limit)}`
        : `Context usage ${pctLabel}% full, ${formatTokens(used)} of ${formatTokens(limit)}`
      : 'Context usage unavailable until a turn finishes'}
    aria-expanded={ready ? open : undefined}
    aria-haspopup={ready ? 'dialog' : undefined}
    title={ready ? 'Context usage' : 'Context usage updates after a turn finishes'}
    onclick={toggle}
  >
    <svg class="ctx-ring" width={size} height={size} viewBox="0 0 {size} {size}" aria-hidden="true">
      <circle cx={c} cy={c} r={r} fill="none" stroke="var(--line)" stroke-width={sw} />
      <circle
        class="ctx-fill"
        class:off={!ready}
        cx={c}
        cy={c}
        r={r}
        fill="none"
        stroke={ringColor}
        stroke-width={sw}
        stroke-linecap="round"
        stroke-dasharray={circ}
        stroke-dashoffset={dashOff}
        transform="rotate(-90 {c} {c})"
      />
    </svg>
  </button>
  {#if open && ready}
    <div class="ctx-pop" role="dialog" aria-label="Context usage">
      <div class="ctx-head">
        <span class="ctx-title">{compacting ? 'Compacting' : 'Context'}</span>
        <span class="ctx-ratio">{formatTokens(used)}/{formatTokens(limit)}</span>
      </div>
      <div class="ctx-bar" role="progressbar" aria-valuenow={pctLabel} aria-valuemin="0" aria-valuemax="100">
        <div class="ctx-bar-fill" style="width:{pctLabel}%;background:{ringColor}"></div>
      </div>
      <ul class="ctx-rows">
        <li>
          <span>Used</span>
          <span>{pctLabel}%</span>
        </li>
        <li>
          <span>Free space</span>
          <span>{Math.round((free / limit) * 100)}%</span>
        </li>
        {#if compacting}
          <li>
            <span>Compacting</span>
            <span>in progress</span>
          </li>
        {:else if compacted}
          <li>
            <span>Last compact</span>
            <span>this session</span>
          </li>
        {/if}
        {#if showSplit}
          <li class="ctx-sub">
            <span>Cache read</span>
            <span>{formatTokens(cacheRead)}</span>
          </li>
          <li class="ctx-sub">
            <span>Fresh input</span>
            <span>{formatTokens(fresh)}</span>
          </li>
        {/if}
      </ul>
    </div>
  {/if}
</div>

<style>
  .ctx-meter {
    position: relative;
    flex: none;
    display: flex;
    align-items: center;
  }
  .ctx-btn {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }
  .ctx-btn:hover:not(:disabled),
  .ctx-meter.open .ctx-btn {
    background: var(--pane-2);
    color: var(--text);
  }
  .ctx-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .ctx-btn:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--amber-d);
  }
  .ctx-ring {
    display: block;
  }
  .ctx-fill {
    /* Color can ease. Do not transition stroke-dashoffset: the UA default is
       0 (full ring), and WKWebView replays dashoffset transitions when a
       parked shell is shown and when this circle is first inserted. */
    transition: stroke 0.2s ease;
  }
  .ctx-fill.off {
    opacity: 0;
  }
  .ctx-btn.compacting .ctx-fill {
    animation: ctx-pulse 1.1s ease-in-out infinite;
  }
  @keyframes ctx-pulse {
    50% { opacity: 0.45; }
  }
  @media (prefers-reduced-motion: reduce) {
    .ctx-fill {
      transition: none;
    }
    .ctx-btn.compacting .ctx-fill {
      animation: none;
    }
  }
  .ctx-pop {
    position: absolute;
    bottom: calc(100% + 8px);
    right: 0;
    width: 220px;
    padding: 11px 12px;
    background: var(--pane-2);
    border: 1px solid var(--line-bright);
    border-radius: 9px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
    z-index: 60;
  }
  .ctx-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    margin-bottom: 8px;
  }
  .ctx-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }
  .ctx-ratio {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--muted);
  }
  .ctx-bar {
    height: 4px;
    border-radius: 2px;
    background: var(--line);
    overflow: hidden;
    margin-bottom: 8px;
  }
  .ctx-bar-fill {
    height: 100%;
    border-radius: 2px;
    transition: width 0.2s ease;
  }
  .ctx-rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .ctx-rows li {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12px;
    color: var(--muted);
  }
  .ctx-rows li span:last-child {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text);
  }
  .ctx-rows li.ctx-sub {
    color: var(--faint);
  }
  .ctx-rows li.ctx-sub span:last-child {
    color: var(--muted);
  }
</style>
