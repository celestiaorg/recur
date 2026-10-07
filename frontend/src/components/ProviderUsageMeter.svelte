<script lang="ts">
  import { onMount } from 'svelte'
  import { providerUsage, settings, openSettings } from '../lib/stores'
  import { prefetchProviderUsage, refreshProviderUsage } from '../lib/api'
  import { formatTokens } from '../lib/contextUsage'
  import {
    canDisconnectCursor,
    cursorUsageVisible,
    usageProviderOn,
    formatResetIn,
    formatUsagePct,
    modelBuckets,
    providerChips,
    ringColor,
    snapshotHasData,
    snapshotHasProviderError,
    type ChipRing,
  } from '../lib/providerUsage'
  import ProviderIcon from './ProviderIcon.svelte'

  let open = $state(false)
  let rootEl: HTMLDivElement | undefined = $state()
  let hoverTimer: ReturnType<typeof setTimeout> | undefined
  let popMax = $state(0)

  function fitPop() {
    if (!rootEl) return
    popMax = Math.max(160, Math.floor(rootEl.getBoundingClientRect().top - 12))
  }

  const snap = $derived($providerUsage)
  const disabled = $derived($settings?.disabledProviders)
  const chips = $derived(providerChips(snap, disabled))
  const has = $derived(snapshotHasData(snap, disabled))
  const rows = $derived((snap?.providers ?? []).filter((p) => usageProviderOn(p.id, disabled)))
  const stale = $derived(!!snap?.stale)

  const size = 16

  function ringGeom(count: number, i: number) {
    const n = Math.max(1, count)
    const sw = n === 1 ? 2.2 : 1.55
    const gap = 0.7
    const r = size / 2 - sw / 2 - (n - 1 - i) * (sw + gap)
    return { sw, r, circ: 2 * Math.PI * r, c: size / 2 }
  }

  function chipLabel(name: string, rings: ChipRing[]): string {
    const bits = rings.map((r) => `${r.label} ${Math.round(r.pct)}%`).join(', ')
    return bits ? `${name} usage: ${bits}` : `${name} usage`
  }

  function clearHover() {
    if (hoverTimer) {
      clearTimeout(hoverTimer)
      hoverTimer = undefined
    }
  }

  function onEnter() {
    clearHover()
    hoverTimer = setTimeout(() => {
      open = true
      fitPop()
    }, 160)
  }

  function onLeave() {
    clearHover()
    hoverTimer = setTimeout(() => { open = false }, 120)
  }

  function toggle(e: MouseEvent) {
    e.stopPropagation()
    // Hover already opens the popover without fetching. A click in that state
    // used to toggle it closed and skip the load. Force a collect only when a
    // cached error row might recover after Claude Code rotates the OAuth token.
    if (!open || snap == null) {
      open = true
      fitPop()
      if (snapshotHasProviderError(snap, disabled)) {
        void refreshProviderUsage()
      } else {
        void prefetchProviderUsage()
      }
      return
    }
    open = false
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && open) {
      open = false
      e.stopPropagation()
    }
  }

  function onDocPointer(e: PointerEvent) {
    if (!open || !rootEl) return
    if (e.target instanceof Node && rootEl.contains(e.target)) return
    open = false
  }

  $effect(() => {
    if (!open) return
    fitPop()
    document.addEventListener('pointerdown', onDocPointer, true)
    document.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('pointerdown', onDocPointer, true)
      document.removeEventListener('keydown', onKey, true)
    }
  })

  function openCursorSettings() {
    open = false
    openSettings('agent')
  }

  onMount(() => { void prefetchProviderUsage() })
</script>

<svelte:window onresize={fitPop} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="use-meter"
  class:open
  bind:this={rootEl}
  onpointerenter={onEnter}
  onpointerleave={onLeave}
>
  <button
    type="button"
    class="use-btn"
    aria-label={chips.length
      ? chips.map((c) => chipLabel(c.name, c.rings)).join('. ')
      : snap == null
        ? 'Provider usage — loading'
        : 'Provider usage — no plan data yet'}
    aria-expanded={open}
    aria-haspopup="dialog"
    onclick={toggle}
  >
    {#if chips.length}
      {#each chips as chip (chip.id)}
        <span class="use-chip">
          <span class="use-chip-ico"><ProviderIcon provider={chip.id} /></span>
          <svg class="use-ring" width={size} height={size} viewBox="0 0 {size} {size}" aria-hidden="true">
            {#each chip.rings as w, i (w.id)}
              {@const g = ringGeom(chip.rings.length, i)}
              <circle cx={g.c} cy={g.c} r={g.r} fill="none" stroke="var(--line)" stroke-width={g.sw} />
              <circle
                class="use-fill"
                cx={g.c}
                cy={g.c}
                r={g.r}
                fill="none"
                stroke={ringColor(w.pct, true)}
                stroke-width={g.sw}
                stroke-linecap="round"
                stroke-dasharray={g.circ}
                stroke-dashoffset={g.circ * (1 - Math.min(1, w.pct / 100))}
                transform="rotate(-90 {g.c} {g.c})"
              />
            {/each}
          </svg>
        </span>
      {/each}
    {:else}
      <span class="use-pct">—</span>
    {/if}
  </button>
  {#if open}
    <div class="use-pop" role="dialog" aria-label="Provider usage" style:--use-pop-max="{popMax}px">
      <div class="use-head">
        <span class="use-title">Usage</span>
        {#if stale}
          <span class="use-stale">Last known</span>
        {/if}
      </div>
      {#if snap == null}
        <p class="use-empty">Loading usage…</p>
      {:else if !has}
        <p class="use-empty">No plan quotas yet. Sign in to Claude Code, Codex (ChatGPT), or Connect Cursor.</p>
      {:else}
        <ul class="use-providers">
          {#each rows as p (p.id)}
            <li class="use-prov">
              <div class="use-prov-head">
                <span class="use-prov-title">
                  <span class="use-prov-ico"><ProviderIcon provider={p.id} /></span>
                  <span class="use-prov-name">{p.name}</span>
                </span>
                {#if p.email || p.plan}
                  <span class="use-prov-plan">{p.email || p.plan}</span>
                {/if}
              </div>
              {#if p.connecting}
                <div class="use-hint">Waiting for browser sign-in…</div>
              {:else if p.id === 'cursor' && !canDisconnectCursor(snap) && (p.error || !cursorUsageVisible(p))}
                <a class="lnk use-link" href="#settings-providers" onclick={(e) => { e.preventDefault(); openCursorSettings() }}>Connect Cursor in Settings</a>
              {:else if p.error}
                <div class="use-err">{p.error}</div>
                {#if p.hint}<div class="use-hint">{p.hint}</div>{/if}
              {:else if !(p.windows?.length) && p.hint}
                <div class="use-hint">{p.hint}</div>
              {:else}
                {#each p.windows ?? [] as w (w.id)}
                  <div class="use-win">
                    <div class="use-win-row">
                      <span>{w.label}</span>
                      <span>{w.id === 'extra' && w.detail ? w.detail : formatUsagePct(w.usedPct)}</span>
                    </div>
                    {#if !w.detail || w.id !== 'extra'}
                      <div class="use-bar" role="progressbar" aria-valuenow={Math.round(w.usedPct)} aria-valuemin="0" aria-valuemax="100">
                        <div class="use-bar-fill" style="width:{Math.min(100, w.usedPct)}%;background:{ringColor(w.usedPct, true)}"></div>
                      </div>
                    {/if}
                    {#if w.detail && w.id !== 'extra'}
                      <div class="use-reset">{w.detail}</div>
                    {/if}
                    {#if w.resetsAt}
                      <div class="use-reset">{formatResetIn(w.resetsAt)}</div>
                    {/if}
                  </div>
                {/each}
                {#if p.models?.length}
                  <ul class="use-models">
                    {#each modelBuckets(p.models) as b (b.id)}
                      <li class="use-bucket">
                        <span>{b.label}</span>
                        <span>{formatTokens(b.tokens)} · {formatUsagePct(b.usedPct)}</span>
                      </li>
                      {#each b.models as m (m.name)}
                        <li>
                          <span>{m.name}</span>
                          <span>{formatTokens(m.tokens)} · {formatUsagePct(m.usedPct ?? 0)}</span>
                        </li>
                      {/each}
                    {/each}
                  </ul>
                {/if}
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .use-meter {
    position: relative;
    flex: none;
    display: flex;
    align-items: center;
    height: 26px;
  }
  .use-btn {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 26px;
    padding: 0 5px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--faint);
    cursor: pointer;
    line-height: 1;
  }
  .use-chip {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .use-chip-ico {
    display: flex;
    width: 11px;
    height: 11px;
    flex: none;
    color: var(--faint);
  }
  .use-chip-ico :global(svg) {
    width: 11px;
    height: 11px;
  }
  .use-btn:hover,
  .use-meter.open .use-btn {
    background: var(--pane);
    color: var(--muted);
  }
  .use-btn:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--amber-d);
  }
  .use-ring { display: block; flex: none; }
  .use-fill { transition: stroke-dashoffset 0.25s ease, stroke 0.2s ease; }
  .use-pct {
    display: block;
    font-family: var(--mono);
    font-size: 10px;
    line-height: 1;
    min-width: 1.6em;
  }
  .use-pop {
    position: absolute;
    bottom: calc(100% + 8px);
    left: 0;
    width: 300px;
    max-height: var(--use-pop-max, calc(100dvh - 24px));
    overflow-x: hidden;
    overflow-y: auto;
    padding: 11px 12px;
    background: var(--pane-2);
    border: 1px solid var(--line-bright);
    border-radius: 9px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
    z-index: 60;
  }
  .use-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    margin-bottom: 8px;
  }
  .use-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }
  .use-stale {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--muted);
  }
  .use-empty, .use-err, .use-hint, .use-reset {
    font-size: 12px;
    color: var(--muted);
    line-height: 1.4;
  }
  .use-err { color: var(--coral); margin-top: 4px; }
  .use-hint, .use-reset { color: var(--faint); margin-top: 2px; }
  .use-empty { margin: 0; }
  .use-providers {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .use-prov + .use-prov {
    padding-top: 14px;
    border-top: 1px solid var(--line);
  }
  .use-prov-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .use-prov-title {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
  }
  .use-prov-ico {
    display: flex;
    width: 13px;
    height: 13px;
    flex: none;
    color: var(--text);
  }
  .use-prov-ico :global(svg) {
    width: 13px;
    height: 13px;
  }
  .use-prov-name {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }
  .use-prov-plan {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    max-width: 58%;
    text-align: right;
  }
  .use-win { margin-top: 6px; }
  .use-win-row, .use-models li {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    font-size: 12px;
    color: var(--muted);
  }
  .use-models li.use-bucket {
    margin-top: 6px;
    font-weight: 600;
    color: var(--text);
  }
  .use-models li.use-bucket:first-child { margin-top: 0; }
  .use-models li:not(.use-bucket) span:first-child {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .use-win-row span:last-child,
  .use-models li span:last-child {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text);
  }
  .use-bar {
    height: 4px;
    border-radius: 2px;
    background: var(--line);
    overflow: hidden;
    margin-top: 4px;
  }
  .use-bar-fill {
    height: 100%;
    border-radius: 2px;
    transition: width 0.2s ease;
  }
  .use-models {
    list-style: none;
    margin: 6px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .use-link {
    display: block;
    margin-top: 8px;
    text-decoration: none;
  }
  .use-link:hover { text-decoration: underline; }
  @media (prefers-reduced-motion: reduce) {
    .use-fill, .use-bar-fill { transition: none; }
  }
</style>
