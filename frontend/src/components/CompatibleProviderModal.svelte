<script lang="ts">
  import { SettingsService } from '../../bindings/recur/internal/app'
  import type { CompatibleFlagDim, CompatibleProvider } from '../../bindings/recur/internal/domain/models'
  import { COMPATIBLE_PRESETS, compatibleSlugError } from '../lib/compatible-providers'
  import { cycleDialogTab, clearKeyboardFocus } from '../lib/dialog-focus'
  import { isCmdOrCtrl } from '../lib/shortcut'

  let {
    takenIds = [],
    onClose,
    onAdd,
  }: {
    takenIds?: string[]
    onClose: () => void
    onAdd: (p: CompatibleProvider) => void
  } = $props()

  let modalEl = $state<HTMLElement | undefined>()
  let slugEl = $state<HTMLInputElement | undefined>()
  let presetId = $state<(typeof COMPATIBLE_PRESETS)[number]['id']>('openai')
  let slug = $state('openai')
  let name = $state('OpenAI')
  let url = $state('https://api.openai.com')
  let modelId = $state('gpt-5')
  let modelLabel = $state('GPT-5')
  let error = $state('')
  let saving = $state(false)

  const canSave = $derived(!!slug.trim() && !!url.trim() && !!modelId.trim() && !saving)

  function applyPreset(id: (typeof COMPATIBLE_PRESETS)[number]['id']) {
    const p = COMPATIBLE_PRESETS.find((x) => x.id === id) ?? COMPATIBLE_PRESETS[3]
    presetId = p.id
    slug = p.id === 'custom' ? '' : p.id
    name = p.name
    url = p.baseUrl
    modelId = p.modelId
    modelLabel = p.modelLabel
    error = ''
  }

  $effect(() => {
    const t = setTimeout(() => slugEl?.focus(), 0)
    return () => clearTimeout(t)
  })

  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Tab' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const modal = modalEl
        if (!modal) return
        e.preventDefault()
        cycleDialogTab(modal, e.shiftKey)
        return
      }
      if (e.key === 'Enter' && isCmdOrCtrl(e)) {
        e.preventDefault()
        void submit()
        return
      }
      if (e.key !== 'Escape') return
      e.preventDefault()
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  })

  async function submit() {
    if (saving) return
    const id = slug.trim().toLowerCase()
    const slugErr = compatibleSlugError(id, takenIds)
    if (slugErr) {
      error = slugErr
      return
    }
    const mid = modelId.trim()
    if (!url.trim() || !mid) {
      error = 'Base URL and at least one model id are required.'
      return
    }
    saving = true
    error = ''
    try {
      const preset = COMPATIBLE_PRESETS.find((p) => p.id === presetId)
      let flags: CompatibleFlagDim[] | undefined
      if (preset?.flags) {
        try {
          flags = (await SettingsService.CompatibleFlagPreset(preset.flags)) ?? undefined
        } catch {
          flags = undefined
        }
      }
      onAdd({
        id,
        name: name.trim() || id,
        baseUrl: url.trim(),
        models: [{ id: mid, label: modelLabel.trim() || mid, flags }],
      })
    } finally {
      saving = false
    }
  }
</script>

<div
  class="nt-bg"
  id="v-compat-provider"
  onclick={(e) => { if (e.target === e.currentTarget) onClose() }}
  role="presentation"
>
  <div
    class="nt-modal np-modal"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label="Add provider"
    bind:this={modalEl}
    onpointerdown={() => { if (modalEl) clearKeyboardFocus(modalEl) }}
  >
    <div class="nt-head">
      <div class="np-title">Add provider</div>
      <button type="button" class="nt-x" tabindex="-1" onclick={onClose} aria-label="Close">
        <span class="esc">esc</span>
      </button>
    </div>
    <div class="np-body">
      <div class="np-row">
        <span class="np-lbl">Preset</span>
        <div class="compat-presets" role="group" aria-label="Provider preset">
          {#each COMPATIBLE_PRESETS as preset (preset.id)}
            <button
              class="tbtn"
              class:on={presetId === preset.id}
              type="button"
              onclick={() => applyPreset(preset.id)}
            >{preset.name}</button>
          {/each}
        </div>
      </div>
      <label class="np-row">
        <span class="np-lbl">Slug</span>
        <input
          class="inp"
          type="text"
          bind:this={slugEl}
          bind:value={slug}
          placeholder="openai"
          autocomplete="off"
          spellcheck="false"
          aria-label="Provider slug"
        />
      </label>
      <label class="np-row">
        <span class="np-lbl">Display name</span>
        <input
          class="inp"
          type="text"
          bind:value={name}
          placeholder="OpenAI"
          autocomplete="off"
          spellcheck="false"
          aria-label="Provider display name"
        />
      </label>
      <label class="np-row">
        <span class="np-lbl">Base URL</span>
        <input
          class="inp"
          type="url"
          bind:value={url}
          placeholder="https://api.openai.com"
          autocomplete="off"
          spellcheck="false"
          aria-label="Base URL"
        />
      </label>
      <div class="np-row-split">
        <label class="np-row">
          <span class="np-lbl">Model id</span>
          <input
            class="inp"
            type="text"
            bind:value={modelId}
            placeholder="model-id"
            autocomplete="off"
            spellcheck="false"
            aria-label="First model id"
          />
        </label>
        <label class="np-row">
          <span class="np-lbl">Label</span>
          <input
            class="inp"
            type="text"
            bind:value={modelLabel}
            placeholder="Label"
            autocomplete="off"
            spellcheck="false"
            aria-label="First model label"
          />
        </label>
      </div>
      {#if error}
        <div class="set-field-msg bad">{error}</div>
      {/if}
    </div>
    <div class="nt-foot">
      <button type="button" class="nt-go" onclick={() => void submit()} disabled={!canSave}>
        {saving ? 'Adding…' : 'Add provider'}
        {#if !saving}<span class="nt-ret">⌘↵</span>{/if}
      </button>
    </div>
  </div>
</div>
