<script lang="ts">
  /** T3-style model + flags pair: base model Select + ModelFlagsSelect.
   *  Shared by Settings role rows and the solo chat dock. */

  import Select from './Select.svelte'
  import ModelFlagsSelect from './ModelFlagsSelect.svelte'
  import {
    modelOptionsFor,
    flagsForModel,
    normalizeFlags,
    type CatalogModel,
    type FlagDimension,
  } from '../lib/models'

  let {
    models = [],
    model = '',
    flags = {},
    disabled = false,
    ghost = false,
    /** Open menus above the trigger (chat dock). */
    up = false,
    /** Settings chip chrome around the pair. */
    chip = false,
    modelLabel = 'Model',
    flagsLabel = 'Model flags',
    onModelChange,
    onFlagsChange,
  }: {
    models?: CatalogModel[]
    model?: string
    flags?: Record<string, string>
    disabled?: boolean
    ghost?: boolean
    up?: boolean
    chip?: boolean
    modelLabel?: string
    flagsLabel?: string
    onModelChange?: (ref: string, flags: Record<string, string>) => void
    onFlagsChange?: (flags: Record<string, string>) => void
  } = $props()

  const dims = $derived<FlagDimension[]>(flagsForModel(models, model))
  const options = $derived(modelOptionsFor(models, model))
  const resolvedFlags = $derived(normalizeFlags(dims, flags))

  function pickModel(ref: string) {
    const nextFlags = normalizeFlags(flagsForModel(models, ref), undefined)
    onModelChange?.(ref, nextFlags)
  }

  function pickFlags(next: Record<string, string>) {
    onFlagsChange?.(next)
  }
</script>

<div class="model-pickers" class:chip class:up>
  <Select
    {ghost}
    {up}
    {disabled}
    searchable
    label={modelLabel}
    {options}
    value={model}
    onchange={pickModel}
  />
  {#if dims.length}
    <ModelFlagsSelect
      {ghost}
      {up}
      {disabled}
      label={flagsLabel}
      dimensions={dims}
      value={resolvedFlags}
      onchange={pickFlags}
    />
  {/if}
</div>
