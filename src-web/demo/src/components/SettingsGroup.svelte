<script lang="ts">
  import type { Snippet } from 'svelte'

  let {
    title,
    blurb = '',
    /** When set, shows a toggle on the right of the header; body shows only while on. */
    override = undefined as boolean | undefined,
    onOverrideChange,
    overrideLabel = '',
    overrideDisabled = false,
    class: className = '',
    /** Custom blurb content (wins over `blurb` string when provided). */
    blurbContent,
    children,
  }: {
    title: string
    blurb?: string
    override?: boolean
    onOverrideChange?: (on: boolean) => void
    overrideLabel?: string
    overrideDisabled?: boolean
    class?: string
    blurbContent?: Snippet
    children?: Snippet
  } = $props()

  const hasOverride = $derived(override !== undefined)
  const showBody = $derived(!hasOverride || !!override)
  const aria = $derived(overrideLabel || `Override ${title}`)
</script>

<section
  class="set-subsec {className}"
  class:set-subsec-collapsed={hasOverride && !override}
>
  <header class="set-subhead" class:set-subhead-row={hasOverride}>
    <div class={hasOverride ? 'set-subhead-copy' : undefined}>
      <h3 class="set-subhead-title">{title}</h3>
      {#if blurbContent}
        <div class="set-subhead-blurb">{@render blurbContent()}</div>
      {:else if blurb}
        <p class="set-subhead-blurb">{blurb}</p>
      {/if}
    </div>
    {#if hasOverride}
      <div class="set-subhead-ctrl">
        <button
          type="button"
          class="tog"
          class:on={!!override}
          aria-label={aria}
          aria-pressed={!!override}
          disabled={overrideDisabled}
          onclick={() => onOverrideChange?.(!override)}
        ></button>
      </div>
    {/if}
  </header>
  {#if showBody && children}
    <div class="set-subsec-body">
      {@render children()}
    </div>
  {/if}
</section>
