<script lang="ts">
  // One tab inside a TabStrip. Label + optional dirty marker / busy wash
  // (icon + title; close stays outside the wash) + close.
  // The chip itself is the tab (role=tab); close is a nested <button> so it isn't
  // a button-in-button — the old file-chip markup used a role=button span inside
  // a <button>, which is invalid.
  import DecodeText from './DecodeText.svelte'
  import Icon, { type IconName } from './Icon.svelte'

  let {
    label,
    title = label,
    active = false,
    /** Amber underline — panel that currently has keyboard focus. Active-but-unlit
     *  keeps the same thick rule in the standard border colour so you can still
     *  see which tab is selected in a background panel. */
    lit = false,
    dirty = false,
    /** Coral attention dot — finished background turn not yet viewed. */
    unseen = false,
    /** Count pill (review Changes/Commits tabs). Hidden when 0. */
    badge = 0,
    /** Horizontal wash on icon + title when busy and not selected (chat tabs). */
    busy = false,
    muted = false,
    /** Decode animation when the label string changes (e.g. LLM tab rename). */
    animateLabel = false,
    /** Identity for DecodeText — keep stable per tab so workspace switches don't scramble. */
    labelKey = '',
    /** Optional file-icons/atom class string (e.g. from fileIconClass). */
    iconClass = '',
    /** Optional app icon (SVG registry) — e.g. a chat tab's mode identity. */
    icon = undefined,
    /** Accent color for the app icon (chat-mode tint). */
    iconTint = '',
    closeLabel = `Close ${label}`,
    class: className = '',
    id = '',
    controls = '',
    tabKey = '',
    reorderable = false,
    floating = false,
    onclick,
    onclose,
    onkeydown,
    onpointerdown,
  }: {
    label: string
    title?: string
    active?: boolean
    lit?: boolean
    dirty?: boolean
    unseen?: boolean
    badge?: number
    busy?: boolean
    /** Dimmed / inactive look (e.g. an exited terminal). */
    muted?: boolean
    animateLabel?: boolean
    labelKey?: string
    iconClass?: string
    icon?: IconName
    iconTint?: string
    closeLabel?: string
    class?: string
    id?: string
    /** aria-controls for the tabpanel this chip selects. */
    controls?: string
    /** Stable id for drag-reorder (agent code, path, …). */
    tabKey?: string
    /** Grab cursor + touch-action none — drag-reorderable strips. */
    reorderable?: boolean
    /** Pointer-drag ghost; visually the chip, not in the tablist flow. */
    floating?: boolean
    onclick?: (e: MouseEvent) => void
    onclose?: (e: MouseEvent) => void
    onkeydown?: (e: KeyboardEvent) => void
    onpointerdown?: (e: PointerEvent) => void
  } = $props()

  function onChipKeydown(e: KeyboardEvent) {
    onkeydown?.(e)
    if (e.defaultPrevented) return
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      onclick?.(e as unknown as MouseEvent)
    }
  }
</script>

<div
  class="tab-chip {className}"
  id={id || undefined}
  aria-controls={controls || undefined}
  class:on={active}
  class:lit={active && lit}
  class:dirty
  class:busy
  class:exited={muted}
  class:reorderable
  class:is-float={floating}
  role="tab"
  tabindex={floating ? -1 : 0}
  aria-selected={active}
  aria-busy={busy || undefined}
  aria-hidden={floating || undefined}
  data-tab-key={tabKey || undefined}
  {title}
  onclick={onclick}
  onkeydown={onChipKeydown}
  onpointerdown={onpointerdown}
  draggable="false"
>
  <span class="tab-chip-body">
    {#if icon}
      <span class="tab-chip-ico" style:color={iconTint || undefined} aria-hidden="true"><Icon name={icon} /></span>
    {:else if iconClass}
      <span class="fico {iconClass}" aria-hidden="true"></span>
    {/if}
    <DecodeText class="name" text={label} animate={animateLabel} sourceKey={labelKey} />
    {#if badge}<span class="tab-chip-n">{badge}</span>{/if}
    {#if unseen && !busy && !active}<span class="unseen" aria-label="new response"></span>
    {:else if dirty && !busy}<span class="dot" aria-label="unsaved"></span>{/if}
  </span>
  {#if onclose}
    <button
      type="button"
      class="tab-chip-x"
      title="Close"
      aria-label={closeLabel}
      tabindex="-1"
      onpointerdown={(e) => e.stopPropagation()}
      onclick={(e) => {
        e.stopPropagation()
        onclose(e)
      }}
    >×</button>
  {/if}
</div>

<style>
  .tab-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 100%;
    padding: 0 10px;
    border: none;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--muted);
    font-family: var(--ui);
    font-size: 12px;
    font-weight: 500;
    flex: 0 0 auto;
    max-width: 160px;
    border-radius: 0;
    box-sizing: border-box;
    cursor: pointer;
  }
  .tab-chip:hover { color: var(--text); }
  .tab-chip.on { color: var(--text); border-bottom-color: var(--line); }
  .tab-chip.on.lit { border-bottom-color: var(--amber); }
  .tab-chip.exited { color: var(--faint); }
  .tab-chip.exited:hover { color: var(--text); }
  .tab-chip.reorderable {
    cursor: grab;
    touch-action: none;
    user-select: none;
  }
  .tab-chip.is-float {
    pointer-events: none;
    cursor: grabbing;
    background: var(--pane);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    max-width: none;
  }

  .tab-chip-body {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    max-width: 100%;
  }

  :global(.tab-chip .name) {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  /* Wash on icon + title for background busy tabs — close stays outside.
     Mask (not background-clip:text) so SVG icons shimmer with the label. */
  .tab-chip.busy:not(.on) .tab-chip-body {
    color: var(--text);
    -webkit-mask-image: linear-gradient(
      90deg,
      rgba(0, 0, 0, 0.45) 0%,
      rgba(0, 0, 0, 0.45) 35%,
      rgba(0, 0, 0, 1) 50%,
      rgba(0, 0, 0, 0.45) 65%,
      rgba(0, 0, 0, 0.45) 100%
    );
    mask-image: linear-gradient(
      90deg,
      rgba(0, 0, 0, 0.45) 0%,
      rgba(0, 0, 0, 0.45) 35%,
      rgba(0, 0, 0, 1) 50%,
      rgba(0, 0, 0, 0.45) 65%,
      rgba(0, 0, 0, 0.45) 100%
    );
    -webkit-mask-size: 200% 100%;
    mask-size: 200% 100%;
    animation: tab-busy-wash 1.8s linear infinite;
  }
  @keyframes tab-busy-wash {
    from {
      -webkit-mask-position: 100% 0;
      mask-position: 100% 0;
    }
    to {
      -webkit-mask-position: -100% 0;
      mask-position: -100% 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .tab-chip.busy:not(.on) .tab-chip-body {
      color: inherit;
      -webkit-mask-image: none;
      mask-image: none;
      animation: none;
      opacity: 0.7;
    }
  }

  .tab-chip-ico {
    display: inline-flex;
    align-items: center;
    flex: none;
  }
  .tab-chip-ico :global(svg) { width: 14px; height: 14px; }

  .tab-chip-n {
    font-family: var(--mono);
    font-size: 9.5px;
    color: var(--faint);
    background: var(--pane-2);
    border-radius: 10px;
    padding: 0 5px;
    flex: none;
  }
  .tab-chip.on .tab-chip-n { color: var(--muted); }

  .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--amber);
    flex: none;
  }
  .unseen {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--coral);
    flex: none;
  }

  .tab-chip-x {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    flex: none;
    padding: 0;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--faint);
    font-size: 13px;
    line-height: 1;
    opacity: 0;
    cursor: pointer;
  }
  .tab-chip:hover .tab-chip-x,
  .tab-chip.on .tab-chip-x { opacity: 1; }
  .tab-chip-x:hover { background: var(--pane-2); color: var(--text); }
</style>
