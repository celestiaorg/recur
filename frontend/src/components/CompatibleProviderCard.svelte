<script lang="ts">
  import Icon from './Icon.svelte'
  import { SettingsService } from '../../bindings/recur/internal/app'
  import type { CompatibleFlagDim, CompatibleModel, CompatibleProvider } from '../../bindings/recur/internal/domain/models'
  import { persistableFlags, persistableHeaders, persistableModels, unusedKey, COMPATIBLE_WIRES } from '../lib/compatible-providers'

  let {
    provider,
    testing = false,
    testMsg = null as { ok: boolean; text: string } | null,
    onChange,
    onTest,
    onRemove,
  }: {
    provider: CompatibleProvider
    testing?: boolean
    testMsg?: { ok: boolean; text: string } | null
    onChange: (next: CompatibleProvider) => void
    onTest: () => void
    onRemove: () => void
  } = $props()

  let keyVisible = $state(false)

  function set(patch: Partial<CompatibleProvider>) {
    onChange({ ...provider, ...patch })
  }

  type HeaderRow = { key: string; value: string }

  let headerDrafts = $state<HeaderRow[]>([])
  const headerSaved = $derived(
    Object.entries(provider.extraHeaders ?? {})
      .filter((e): e is [string, string] => typeof e[1] === 'string')
      .map(([key, value]) => ({ key, value })),
  )
  const headerRows = $derived([...headerSaved, ...headerDrafts])

  function setHeader(i: number, patch: Partial<HeaderRow>) {
    const n = headerSaved.length
    if (i < n) {
      set({ extraHeaders: persistableHeaders(headerSaved.map((r, j) => (j === i ? { ...r, ...patch } : r))) })
      return
    }
    const next = headerDrafts.map((r, j) => (j === i - n ? { ...r, ...patch } : r))
    const ready = next.filter((r) => r.key.trim())
    headerDrafts = next.filter((r) => !r.key.trim())
    if (ready.length) set({ extraHeaders: persistableHeaders([...headerSaved, ...ready]) })
  }

  function removeHeader(i: number) {
    const n = headerSaved.length
    if (i < n) {
      set({ extraHeaders: persistableHeaders(headerSaved.filter((_, j) => j !== i)) })
      return
    }
    headerDrafts = headerDrafts.filter((_, j) => j !== i - n)
  }

  let drafts = $state<CompatibleModel[]>([])
  const saved = $derived(provider.models ?? [])
  const rows = $derived(saved.length || drafts.length ? [...saved, ...drafts] : [{ id: '', label: '' }])

  function setModel(i: number, patch: Partial<CompatibleModel>) {
    const n = saved.length
    if (i < n) {
      const next = persistableModels(saved.map((m, j) => (j === i ? { ...m, ...patch } : m)))
      if (next.length) set({ models: next })
      return
    }
    const next = drafts.map((m, j) => (j === i - n ? { ...m, ...patch } : m))
    const ready = persistableModels(next)
    drafts = next.filter((m) => !m.id?.trim())
    if (ready.length) set({ models: [...saved, ...ready] })
  }

  function flagKind(m: CompatibleModel): string {
    const flags = m.flags ?? []
    if (flags.length === 0) return ''
    const ids = flags.map((f) => f.id)
    if (ids.includes('effort') && ids.includes('verbosity') && flags.length === 2) return 'openai'
    if (ids.length === 1 && ids[0] === 'effort') return 'effort'
    return 'custom'
  }

  async function applyFlags(i: number, kind: string) {
    if (kind === 'custom') {
      const cur = rows[i]?.flags ?? []
      if (cur.length) return
      setModel(i, { flags: [{ id: 'effort', label: 'Effort', wire: 'reasoning.effort', options: [] }] })
      return
    }
    let flags: CompatibleFlagDim[] = []
    if (kind) {
      try {
        flags = (await SettingsService.CompatibleFlagPreset(kind)) ?? []
      } catch {
        flags = []
      }
    }
    setModel(i, { flags: flags.length ? flags : undefined })
  }

  function setFlag(modelIdx: number, dimIdx: number, patch: Partial<CompatibleFlagDim>) {
    const m = rows[modelIdx]
    const flags = [...(m.flags ?? [])]
    flags[dimIdx] = { ...flags[dimIdx], ...patch }
    setModel(modelIdx, { flags: persistableFlags(flags) })
  }

  function addFlag(modelIdx: number) {
    const m = rows[modelIdx]
    const flags = m.flags ?? []
    const id = unusedKey(flags.map((f) => f.id ?? ''), 'flag')
    setModel(modelIdx, { flags: [...flags, { id, label: id, wire: '', options: [] }] })
  }

  function removeFlag(modelIdx: number, dimIdx: number) {
    const m = rows[modelIdx]
    setModel(modelIdx, { flags: persistableFlags((m.flags ?? []).filter((_, j) => j !== dimIdx)) })
  }

  function setOption(modelIdx: number, dimIdx: number, optIdx: number, patch: { value?: string; label?: string; default?: boolean }) {
    const dim = rows[modelIdx]?.flags?.[dimIdx]
    if (!dim) return
    const options = [...(dim.options ?? [])]
    options[optIdx] = { ...options[optIdx], ...patch }
    if (patch.default) {
      for (let i = 0; i < options.length; i++) {
        if (i !== optIdx) options[i] = { ...options[i], default: false }
      }
    }
    setFlag(modelIdx, dimIdx, { options })
  }

  function addOption(modelIdx: number, dimIdx: number) {
    const dim = rows[modelIdx]?.flags?.[dimIdx]
    if (!dim) return
    const options = dim.options ?? []
    const value = unusedKey(options.map((o) => o.value ?? ''), 'value')
    setFlag(modelIdx, dimIdx, { options: [...options, { value, label: value }] })
  }

  function removeOption(modelIdx: number, dimIdx: number, optIdx: number) {
    const dim = rows[modelIdx]?.flags?.[dimIdx]
    if (!dim) return
    setFlag(modelIdx, dimIdx, { options: (dim.options ?? []).filter((_, j) => j !== optIdx) })
  }
</script>

<div class="set-rows code-panel-sides">
  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Display name</div>
      <div class="set-desc">Shown in model pickers. Slug <code>{provider.id}</code> prefixes model refs.</div>
    </div>
    <div class="path-pick">
      <input
        class="inp"
        type="text"
        autocomplete="off"
        spellcheck="false"
        aria-label="Provider display name"
        value={provider.name}
        onchange={(e) => set({ name: e.currentTarget.value })}
      />
    </div>
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Base URL</div>
      <div class="set-desc">Recur POSTs <code>{'{base}'}/v1/responses</code>. No trailing slash.</div>
    </div>
    <div class="path-pick">
      <input
        class="inp"
        type="url"
        placeholder="https://api.openai.com"
        autocomplete="off"
        spellcheck="false"
        aria-label="Base URL"
        value={provider.baseUrl}
        onchange={(e) => set({ baseUrl: e.currentTarget.value })}
      />
    </div>
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">API key</div>
      <div class="set-desc">Sent as <code>Authorization: Bearer</code>. Optional for local servers.</div>
    </div>
    <div class="path-pick">
      <input
        class="inp"
        type={keyVisible ? 'text' : 'password'}
        placeholder="sk-…"
        autocomplete="off"
        spellcheck="false"
        aria-label="API key"
        value={provider.apiKey ?? ''}
        onchange={(e) => set({ apiKey: e.currentTarget.value })}
      />
      <button
        class="tbtn"
        type="button"
        aria-label={keyVisible ? 'Hide API key' : 'Show API key'}
        aria-pressed={keyVisible}
        disabled={!provider.apiKey}
        onclick={() => { keyVisible = !keyVisible }}
      >
        <Icon name={keyVisible ? 'eye-off' : 'eye'} />
      </button>
      <button class="tbtn" type="button" disabled={testing || !provider.baseUrl} onclick={onTest}>Test</button>
    </div>
    {#if testing}
      <div class="set-field-msg muted">Testing…</div>
    {:else if testMsg}
      <div class="set-field-msg" class:ok={testMsg.ok} class:bad={!testMsg.ok}>{testMsg.text}</div>
    {/if}
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Extra headers</div>
      <div class="set-desc">Optional. Sent on every request to this endpoint.</div>
    </div>
    <div class="compat-models">
      {#each headerRows as h, i (i)}
        <div class="compat-header">
          <input
            class="inp"
            type="text"
            placeholder="Key"
            autocomplete="off"
            spellcheck="false"
            aria-label="Header key"
            value={h.key}
            onchange={(e) => setHeader(i, { key: e.currentTarget.value })}
          />
          <input
            class="inp"
            type="text"
            placeholder="Value"
            autocomplete="off"
            spellcheck="false"
            aria-label="Header value"
            value={h.value}
            onchange={(e) => setHeader(i, { value: e.currentTarget.value })}
          />
          <button type="button" class="exclude-rm" aria-label="Remove header" onclick={() => removeHeader(i)}>
            <Icon name="trash" />
          </button>
        </div>
      {/each}
      <div class="exclude-actions">
        <button type="button" class="tbtn" onclick={() => { headerDrafts = [...headerDrafts, { key: '', value: '' }] }}>
          <Icon name="plus" />Add header
        </button>
      </div>
    </div>
  </div>

  <div class="set-row set-row-stack">
    <div class="set-copy">
      <div class="set-title">Models</div>
      <div class="set-desc">Each id is stored as <code>{provider.id}/id</code>. Slash in the id is allowed (OpenRouter).</div>
    </div>
    <div class="compat-models">
      {#each rows as m, i (i)}
        <div class="compat-model-block">
          <div class="compat-model">
            <input
              class="inp"
              type="text"
              placeholder="model-id"
              autocomplete="off"
              spellcheck="false"
              aria-label="Model id"
              value={m.id}
              onchange={(e) => setModel(i, { id: e.currentTarget.value, label: m.label || e.currentTarget.value })}
            />
            <input
              class="inp"
              type="text"
              placeholder="Label"
              autocomplete="off"
              spellcheck="false"
              aria-label="Model label"
              value={m.label}
              onchange={(e) => setModel(i, { label: e.currentTarget.value })}
            />
            <input
              class="inp"
              type="number"
              min="0"
              step="1"
              placeholder="ctx"
              autocomplete="off"
              aria-label="Context window tokens"
              value={m.contextWindow || ''}
              onchange={(e) => {
                const n = Number(e.currentTarget.value)
                setModel(i, { contextWindow: Number.isFinite(n) && n > 0 ? Math.round(n) : 0 })
              }}
            />
            <select
              class="inp"
              aria-label="Flag preset"
              value={flagKind(m)}
              onchange={(e) => void applyFlags(i, e.currentTarget.value)}
            >
              <option value="">No flags</option>
              <option value="effort">Effort</option>
              <option value="openai">Effort + verbosity</option>
              <option value="custom">Custom</option>
            </select>
            <button
              type="button"
              class="exclude-rm"
              aria-label="Remove model"
              disabled={rows.length <= 1 || (i < saved.length && saved.length <= 1)}
              onclick={() => {
                const n = saved.length
                if (i < n) {
                  const next = saved.filter((_, j) => j !== i)
                  if (next.length) set({ models: next })
                  return
                }
                drafts = drafts.filter((_, j) => j !== i - n)
              }}
            >
              <Icon name="trash" />
            </button>
          </div>
          {#if (m.flags ?? []).length}
            <div class="compat-flags">
              {#each m.flags ?? [] as dim, di (dim.id || di)}
                <div class="compat-flag">
                  <input
                    class="inp"
                    type="text"
                    placeholder="id"
                    autocomplete="off"
                    spellcheck="false"
                    aria-label="Flag id"
                    value={dim.id}
                    onchange={(e) => setFlag(i, di, { id: e.currentTarget.value, label: dim.label || e.currentTarget.value })}
                  />
                  <input
                    class="inp"
                    type="text"
                    placeholder="Label"
                    autocomplete="off"
                    spellcheck="false"
                    aria-label="Flag label"
                    value={dim.label}
                    onchange={(e) => setFlag(i, di, { label: e.currentTarget.value })}
                  />
                  <select
                    class="inp"
                    aria-label="Flag wire"
                    value={dim.wire ?? ''}
                    onchange={(e) => setFlag(i, di, { wire: e.currentTarget.value || undefined })}
                  >
                    <option value="">(display only)</option>
                    {#each COMPATIBLE_WIRES as w (w.value)}
                      <option value={w.value}>{w.label}</option>
                    {/each}
                  </select>
                  <button type="button" class="exclude-rm" aria-label="Remove flag" onclick={() => removeFlag(i, di)}>
                    <Icon name="trash" />
                  </button>
                </div>
                <div class="compat-flag-opts">
                  {#each dim.options ?? [] as opt, oi (opt.value || oi)}
                    <input
                      class="inp"
                      type="text"
                      placeholder="value"
                      autocomplete="off"
                      spellcheck="false"
                      aria-label="Flag option value"
                      value={opt.value}
                      onchange={(e) => setOption(i, di, oi, { value: e.currentTarget.value, label: opt.label || e.currentTarget.value })}
                    />
                    <input
                      class="inp"
                      type="text"
                      placeholder="Label"
                      autocomplete="off"
                      spellcheck="false"
                      aria-label="Flag option label"
                      value={opt.label}
                      onchange={(e) => setOption(i, di, oi, { label: e.currentTarget.value })}
                    />
                    <label class="compat-default">
                      <input
                        type="checkbox"
                        checked={!!opt.default}
                        onchange={(e) => setOption(i, di, oi, { default: e.currentTarget.checked })}
                      />
                      default
                    </label>
                    <button type="button" class="exclude-rm" aria-label="Remove option" onclick={() => removeOption(i, di, oi)}>
                      <Icon name="trash" />
                    </button>
                  {/each}
                  <button type="button" class="tbtn" onclick={() => addOption(i, di)}>
                    <Icon name="plus" />Add option
                  </button>
                </div>
              {/each}
              <button type="button" class="tbtn" onclick={() => addFlag(i)}>
                <Icon name="plus" />Add dimension
              </button>
            </div>
          {/if}
        </div>
      {/each}
      <div class="exclude-actions">
        <button type="button" class="tbtn" onclick={() => { drafts = [...drafts, { id: '', label: '' }] }}>
          <Icon name="plus" />Add model
        </button>
      </div>
    </div>
  </div>

  <div class="set-row">
    <div class="set-copy">
      <div class="set-title">Remove provider</div>
      <div class="set-desc">Drops this endpoint and its models from pickers.</div>
    </div>
    <div class="set-ctrl">
      <button class="tbtn set-danger-btn" type="button" onclick={onRemove}>Remove</button>
    </div>
  </div>
</div>
